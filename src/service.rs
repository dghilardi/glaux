//! Private local transport to the daemon's single browser owner. No request is retried.
use crate::{Operation, browser::Browser, config::Config};
use anyhow::Result;
use serde_json::Value;
use std::time::Duration;
use tokio::{
    sync::{mpsc, oneshot},
    time::Instant,
};

const QUEUE_SIZE: usize = 8;
const CALL_TIMEOUT: Duration = Duration::from_secs(180);

pub(crate) struct Job {
    operation: Operation,
    reply: oneshot::Sender<Result<Value>>,
    deadline: Instant,
}

impl Job {
    fn abandoned(&self) -> bool {
        self.reply.is_closed() || Instant::now() >= self.deadline
    }

    pub(crate) async fn run(self, config: &Config, browser: &mut Browser) {
        if self.abandoned() {
            let _ = self.reply.send(Err(anyhow::anyhow!(
                "service_request_expired: no operation started"
            )));
            return;
        }
        // Once started, a disconnect does not cancel a potentially applied write.
        let result = crate::execute_in_browser(config, browser, self.operation).await;
        let _ = self.reply.send(result);
    }

    pub(crate) fn reject(self, error: anyhow::Error) {
        let _ = self.reply.send(Err(error));
    }
}

/// Queue consumed only by the daemon's browser owner, between archive operations.
pub struct Service {
    requests: mpsc::Receiver<Job>,
    #[cfg(unix)]
    _endpoint: local::Endpoint,
}

impl Service {
    pub async fn bind(config: &Config) -> Result<Option<Self>> {
        #[cfg(unix)]
        {
            local::bind(config).await.map(Some)
        }
        #[cfg(not(unix))]
        {
            let _ = config;
            Ok(None)
        }
    }

    pub(crate) async fn next(&mut self) -> Option<Job> {
        self.requests.recv().await
    }

    /// Serve at most one live request, so continuous clients cannot starve the archive.
    pub async fn process_one(&mut self, config: &Config, browser: &mut Browser) {
        for _ in 0..QUEUE_SIZE {
            let Ok(job) = self.requests.try_recv() else {
                break;
            };
            if !job.abandoned() {
                job.run(config, browser).await;
                break;
            }
        }
    }

    pub async fn process_next(&mut self, config: &Config, browser: &mut Browser) {
        if let Some(job) = self.next().await {
            job.run(config, browser).await;
        }
    }
}

/// Run a queued operation during daemon idle/backoff, restoring its browser if necessary.
pub(crate) async fn run_idle(
    config: &Config,
    browser: &mut Option<Browser>,
    job: Job,
    shutdown: &mut crate::shutdown::Shutdown,
) -> bool {
    if job.abandoned() {
        return false;
    }
    if let Err(error) = crate::validate_operation(config, &job.operation) {
        job.reject(error);
        return false;
    }
    // The caller must retain ownership through launch, even if shutdown is pending.
    let authenticate = browser.is_none();
    if authenticate {
        match Browser::launch(config, config.headed).await {
            Ok(active) => *browser = Some(active),
            Err(error) => {
                job.reject(error);
                return false;
            }
        }
    }
    if authenticate {
        let result = tokio::select! {
            biased;
            _ = shutdown.wait() => return true,
            result = browser.as_mut().unwrap().authenticate(config) => result,
        };
        if let Err(error) = result {
            browser.take().unwrap().shutdown().await;
            job.reject(error);
            return false;
        }
    }
    tokio::select! {
        biased;
        _ = shutdown.wait() => true,
        _ = job.run(config, browser.as_mut().unwrap()) => false,
    }
}

pub async fn forward(config: &Config, operation: &Operation) -> Result<Option<Value>> {
    if let Some(local) = crate::validate_operation(config, operation)? {
        return Ok(Some(local));
    }
    #[cfg(unix)]
    {
        local::forward(config, operation).await
    }
    #[cfg(not(unix))]
    {
        let _ = (config, operation);
        Ok(None)
    }
}

pub(crate) fn owner_running(config: &Config) -> Result<bool> {
    #[cfg(unix)]
    {
        local::owner_running(config)
    }
    #[cfg(not(unix))]
    {
        let _ = config;
        Ok(false)
    }
}

/// The daemon owns the profile even while recovering without a running browser.
/// Standalone callers must still acquire the browser/profile lock after this check.
pub fn require_idle_profile(config: &Config) -> Result<()> {
    anyhow::ensure!(
        !owner_running(config)?,
        "service_owns_profile: use the shared daemon or stop it before standalone login/diagnostics"
    );
    Ok(())
}

#[cfg(unix)]
mod local {
    use super::*;
    use anyhow::{Context, bail, ensure};
    use fs2::FileExt;
    use serde::{Deserialize, Serialize};
    use std::{
        fs::{self, File, OpenOptions},
        io::ErrorKind,
        os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        path::{Path, PathBuf},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{UnixListener, UnixStream},
        task::{JoinHandle, JoinSet},
    };

    const VERSION: u32 = 1;
    const MAX_REQUEST: usize = 2 * 1024 * 1024;
    const MAX_RESPONSE: usize = 16 * 1024 * 1024;
    const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Request {
        version: u32,
        url: String,
        operation: Operation,
    }

    #[derive(Serialize, Deserialize)]
    #[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
    enum Response {
        Success { data: Value },
        Error { message: String },
    }

    pub(super) struct Endpoint {
        task: JoinHandle<()>,
        path: PathBuf,
        inode: u64,
        _lock: File,
    }

    impl Drop for Endpoint {
        fn drop(&mut self) {
            self.task.abort();
            // Do not unlink a replacement endpoint installed by another process.
            if fs::symlink_metadata(&self.path)
                .is_ok_and(|m| m.ino() == self.inode && m.file_type().is_socket())
            {
                let _ = fs::remove_file(&self.path);
            }
        }
    }

    fn private_metadata(path: &Path, directory: bool) -> Result<fs::Metadata> {
        let metadata = fs::symlink_metadata(path).context("service_endpoint_unavailable")?;
        ensure!(
            metadata.uid() == rustix::process::geteuid().as_raw()
                && metadata.mode() & 0o077 == 0
                && if directory {
                    metadata.is_dir()
                } else {
                    metadata.file_type().is_socket()
                },
            "unsafe_service_endpoint: require an owned private directory and socket without symlinks"
        );
        Ok(metadata)
    }

    fn lock(config: &Config) -> Result<File> {
        let path = config.data_dir.join("service.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
            )
            .open(path)
            .context("Cannot open service ownership lock")?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file()
                && metadata.uid() == rustix::process::geteuid().as_raw()
                && metadata.mode() & 0o077 == 0,
            "unsafe_service_lock"
        );
        file.try_lock_exclusive()
            .context("Glaux service already owns this data directory")?;
        Ok(file)
    }

    pub(super) fn owner_running(config: &Config) -> Result<bool> {
        let path = config.data_dir.join("service.lock");
        let file = match OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(
                (rustix::fs::OFlags::NOFOLLOW | rustix::fs::OFlags::NONBLOCK).bits() as i32,
            )
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
            Err(_) => bail!("unsafe_service_lock"),
        };
        private_metadata(&config.data_dir, true)?;
        let metadata = file.metadata()?;
        ensure!(
            metadata.is_file()
                && metadata.uid() == rustix::process::geteuid().as_raw()
                && metadata.mode() & 0o077 == 0,
            "unsafe_service_lock"
        );
        match file.try_lock_exclusive() {
            Ok(()) => Ok(false),
            Err(error) if error.kind() == ErrorKind::WouldBlock => Ok(true),
            Err(_) => bail!("service_lock_unavailable"),
        }
    }

    pub(super) async fn bind(config: &Config) -> Result<Service> {
        if !config.data_dir.exists() {
            crate::config::private_dir(&config.data_dir)?;
        }
        private_metadata(&config.data_dir, true)?;
        let lock = lock(config)?;
        let path = config.data_dir.join("service.sock");
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                private_metadata(&path, false)?;
                match tokio::time::timeout(HANDSHAKE_TIMEOUT, UnixStream::connect(&path)).await {
                    Ok(Err(error)) if error.kind() == ErrorKind::ConnectionRefused => {
                        fs::remove_file(&path)?
                    }
                    _ => bail!(
                        "service_endpoint_in_use: refusing to replace a live or inaccessible socket"
                    ),
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => bail!("service_endpoint_unavailable"),
        }
        let listener =
            UnixListener::bind(&path).context("Cannot bind local Glaux service socket")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let inode = fs::symlink_metadata(&path)?.ino();
        let (sender, requests) = mpsc::channel(QUEUE_SIZE);
        let config = config.clone();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break };
                        if connections.len() >= 16 { drop(stream); continue; }
                        let config = config.clone(); let sender = sender.clone();
                        connections.spawn(async move { let _ = connection(stream, &config, sender).await; });
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {},
                }
            }
        });
        Ok(Service {
            requests,
            _endpoint: Endpoint {
                task,
                path,
                inode,
                _lock: lock,
            },
        })
    }

    async fn read_frame(stream: &mut UnixStream, limit: usize) -> Result<Vec<u8>> {
        let size = stream
            .read_u32()
            .await
            .context("service_connection_closed")? as usize;
        ensure!(size > 0 && size <= limit, "service_frame_size_limit");
        let mut bytes = vec![0; size];
        stream
            .read_exact(&mut bytes)
            .await
            .context("service_connection_closed")?;
        Ok(bytes)
    }

    async fn write_frame(stream: &mut UnixStream, bytes: &[u8], limit: usize) -> Result<()> {
        ensure!(
            !bytes.is_empty() && bytes.len() <= limit,
            "service_frame_size_limit"
        );
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(bytes).await?;
        Ok(())
    }

    async fn connection(
        mut stream: UnixStream,
        config: &Config,
        sender: mpsc::Sender<Job>,
    ) -> Result<()> {
        ensure!(
            stream.peer_cred()?.uid() == rustix::process::geteuid().as_raw(),
            "service_peer_denied"
        );
        let request =
            tokio::time::timeout(HANDSHAKE_TIMEOUT, read_frame(&mut stream, MAX_REQUEST)).await??;
        let result = async {
            let request: Request = serde_json::from_slice(&request).map_err(|_| anyhow::anyhow!("invalid_service_request"))?;
            ensure!(request.version == VERSION, "service_version_mismatch");
            ensure!(request.url == config.url.as_str(), "service_configuration_mismatch: daemon owns a different Outlook URL");
            if let Some(local) = crate::validate_operation(config, &request.operation)? { return Ok(local); }
            let (reply, result) = oneshot::channel();
            sender.try_send(Job { operation: request.operation, reply, deadline: Instant::now() + CALL_TIMEOUT })
                .map_err(|_| anyhow::anyhow!("service_busy: no operation queued"))?;
            tokio::select! {
                result = result => result.map_err(|_| anyhow::anyhow!("service_operation_interrupted: inspect Drafts before retrying a write"))?,
                _ = stream.read_u8() => bail!("service_client_disconnected"),
                _ = tokio::time::sleep(CALL_TIMEOUT) => bail!("service_timeout: inspect Drafts before retrying a write"),
            }
        }.await;
        let response = match result {
            Ok(data) => Response::Success { data },
            Err(error) => Response::Error {
                message: format!("{error:#}"),
            },
        };
        let mut bytes = serde_json::to_vec(&response)?;
        if bytes.len() > MAX_RESPONSE {
            bytes = serde_json::to_vec(&Response::Error {
                message: "service_response_too_large".into(),
            })?;
        }
        tokio::time::timeout(
            HANDSHAKE_TIMEOUT,
            write_frame(&mut stream, &bytes, MAX_RESPONSE),
        )
        .await??;
        Ok(())
    }

    pub(super) async fn forward(config: &Config, operation: &Operation) -> Result<Option<Value>> {
        let path = config.data_dir.join("service.sock");
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == ErrorKind::NotFound => {
                ensure!(
                    !owner_running(config)?,
                    "service_starting: daemon owns this profile; no standalone fallback"
                );
                return Ok(None);
            }
            Err(_) => bail!("service_endpoint_unavailable"),
            Ok(_) => {}
        }
        private_metadata(&config.data_dir, true)?;
        private_metadata(&path, false)?;
        let mut shutdown = crate::shutdown::Shutdown::new()?;
        let mut stream = tokio::select! {
            biased;
            _ = shutdown.wait() => bail!("service_request_interrupted: no operation submitted"),
            result = tokio::time::timeout(HANDSHAKE_TIMEOUT, UnixStream::connect(&path)) => result
                .context("service_connection_timeout")?
                .context("service_unavailable: restart the daemon or use --standalone with an idle profile")?,
        };
        ensure!(
            stream.peer_cred()?.uid() == rustix::process::geteuid().as_raw(),
            "service_peer_denied"
        );
        #[derive(Serialize)]
        struct BorrowedRequest<'a> {
            version: u32,
            url: &'a str,
            operation: &'a Operation,
        }
        let request = serde_json::to_vec(&BorrowedRequest {
            version: VERSION,
            url: config.url.as_str(),
            operation,
        })
        .map_err(|_| anyhow::anyhow!("invalid_service_operation"))?;
        ensure!(
            request.len() <= MAX_REQUEST,
            "service_request_too_large: no operation submitted"
        );
        let call = tokio::time::timeout(CALL_TIMEOUT, async {
            write_frame(&mut stream, &request, MAX_REQUEST).await?;
            let bytes = read_frame(&mut stream, MAX_RESPONSE).await?;
            serde_json::from_slice::<Response>(&bytes)
                .map_err(|_| anyhow::anyhow!("invalid_service_response"))
        });
        let result = tokio::select! {
            biased;
            _ = shutdown.wait() => None,
            result = call => Some(result),
        };
        let response = match result {
            Some(Ok(Ok(response))) => response,
            _ if matches!(
                operation,
                Operation::CreateDraft { .. } | Operation::UpdateDraft { .. }
            ) =>
            {
                bail!(
                    "draft_save_unconfirmed: service connection lost or timed out; inspect Outlook Drafts before retrying"
                )
            }
            _ => bail!("service_request_failed: connection lost or timed out; no automatic retry"),
        };
        match response {
            Response::Success { data } => Ok(Some(data)),
            Response::Error { message }
                if matches!(
                    operation,
                    Operation::CreateDraft { .. } | Operation::UpdateDraft { .. }
                ) && (message.starts_with("service_operation_interrupted")
                    || message.starts_with("service_timeout")) =>
            {
                bail!(
                    "draft_save_unconfirmed: service interrupted the request; inspect Outlook Drafts before retrying"
                )
            }
            Response::Error { message } => bail!("{message}"),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::json;

        fn config(directory: &Path) -> Config {
            Config::new(
                Some(directory.join("data")),
                Some("/bin/false".into()),
                crate::config::DEFAULT_URL,
                false,
                1,
            )
            .unwrap()
        }

        fn draft() -> Operation {
            Operation::CreateDraft {
                email: crate::compose::EmailInput {
                    theme: Default::default(),
                    to: vec![],
                    cc: vec![],
                    bcc: vec![],
                    subject: "Synthetic".into(),
                    html: "<p>Fixture</p>".into(),
                },
                signature_draft_id: "fixture-template".into(),
            }
        }

        async fn raw(config: &Config, value: Value) -> Response {
            let mut stream = UnixStream::connect(config.data_dir.join("service.sock"))
                .await
                .unwrap();
            write_frame(
                &mut stream,
                &serde_json::to_vec(&value).unwrap(),
                MAX_REQUEST,
            )
            .await
            .unwrap();
            serde_json::from_slice(&read_frame(&mut stream, MAX_RESPONSE).await.unwrap()).unwrap()
        }

        #[tokio::test]
        async fn absent_service_is_local_and_endpoint_ownership_is_exclusive() {
            let directory = tempfile::tempdir().unwrap();
            let config = config(directory.path());
            assert!(
                forward(&config, &Operation::Status)
                    .await
                    .unwrap()
                    .is_none()
            );
            assert!(!config.data_dir.exists());
            let service = bind(&config).await.unwrap();
            assert_eq!(
                fs::metadata(config.data_dir.join("service.sock"))
                    .unwrap()
                    .mode()
                    & 0o777,
                0o600
            );
            assert!(owner_running(&config).unwrap());
            assert!(bind(&config).await.is_err());
            assert!(
                crate::execute_standalone(&config, Operation::Status)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("service_owns_profile")
            );
            assert!(!config.profile().exists());
            fs::remove_file(config.data_dir.join("service.sock")).unwrap();
            assert!(
                crate::execute(&config, Operation::Status)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("service_starting")
            );
            assert!(!config.profile().exists());
            drop(service);
            assert!(!owner_running(&config).unwrap());
            assert!(!config.data_dir.join("service.sock").exists());
            assert!(bind(&config).await.is_ok());
        }

        #[tokio::test]
        async fn refuses_unsafe_paths_and_recovers_only_stale_sockets() {
            let directory = tempfile::tempdir().unwrap();
            let config = config(directory.path());
            crate::config::private_dir(&config.data_dir).unwrap();
            let path = config.data_dir.join("service.sock");
            fs::write(&path, b"synthetic").unwrap();
            assert!(bind(&config).await.is_err());
            assert_eq!(fs::read(&path).unwrap(), b"synthetic");
            fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink(directory.path().join("absent"), &path).unwrap();
            assert!(bind(&config).await.is_err());
            assert!(forward(&config, &Operation::Status).await.is_err());
            fs::remove_file(&path).unwrap();
            let stale = UnixListener::bind(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            drop(stale);
            let service = bind(&config).await.unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
            assert!(forward(&config, &Operation::Status).await.is_err());
            drop(service);
            fs::set_permissions(&config.data_dir, fs::Permissions::from_mode(0o755)).unwrap();
            assert!(bind(&config).await.is_err());
        }

        #[tokio::test]
        async fn daemon_policy_and_protocol_validation_precede_queueing() {
            let directory = tempfile::tempdir().unwrap();
            let config = config(directory.path());
            let mut service = bind(&config).await.unwrap();
            let mut client = config.clone();
            client.mail_write_policy = crate::config::MailWritePolicy::Drafts;
            let error = crate::execute(&client, draft()).await.unwrap_err();
            assert!(error.to_string().contains("mail_writes_disabled"));
            for value in [
                json!({"version":2,"url":config.url.as_str(),"operation":{"operation":"status"}}),
                json!({"version":1,"url":"https://outlook.office365.com/mail/","operation":{"operation":"status"}}),
                json!({"version":1,"url":config.url.as_str(),"operation":{"operation":"list","folder":"inbox","limit":101,"offset":0}}),
                json!({"version":1,"url":config.url.as_str(),"operation":{"operation":"export","id":"fixture","output":"/synthetic"}}),
                json!({"version":1,"url":config.url.as_str(),"operation":{"operation":"read","id":"fixture","arbitrary":"secret"}}),
            ] {
                let Response::Error { message } = raw(&config, value).await else {
                    panic!("invalid request accepted")
                };
                assert!(!message.contains("secret"));
            }
            assert!(service.requests.try_recv().is_err());
            assert!(!config.profile().exists());
        }

        #[tokio::test]
        async fn concurrent_clients_share_queue_and_disconnects_do_not_retry() {
            tokio::time::timeout(Duration::from_secs(10), async {
                let directory = tempfile::tempdir().unwrap();
                let mut config = config(directory.path());
                config.mail_write_policy = crate::config::MailWritePolicy::Drafts;
                let mut service = bind(&config).await.unwrap();
                let mut clients = JoinSet::new();
                for index in 0..4 {
                    let client = config.clone();
                    clients.spawn(async move {
                        let id = format!("fixture-{index}");
                        let data = crate::execute(&client, Operation::Read { id: id.clone() })
                            .await
                            .unwrap();
                        assert_eq!(data["id"], id);
                    });
                }
                for _ in 0..4 {
                    let job = service.next().await.unwrap();
                    let Operation::Read { id } = job.operation else {
                        panic!("unexpected operation")
                    };
                    job.reply.send(Ok(json!({"id":id}))).unwrap();
                }
                while let Some(result) = clients.join_next().await {
                    result.unwrap();
                }
                let client = config.clone();
                let write = tokio::spawn(async move { crate::execute(&client, draft()).await });
                let job = service.next().await.unwrap();
                drop(job); // Lose the response after the owner accepted the write.
                let error = write.await.unwrap().unwrap_err();
                assert!(error.to_string().contains("draft_save_unconfirmed"));
                assert!(error.to_string().contains("inspect Outlook Drafts"));
                assert!(service.requests.try_recv().is_err());

                let mut stream = UnixStream::connect(config.data_dir.join("service.sock"))
                    .await
                    .unwrap();
                let request = Request {
                    version: VERSION,
                    url: config.url.to_string(),
                    operation: Operation::Status,
                };
                write_frame(
                    &mut stream,
                    &serde_json::to_vec(&request).unwrap(),
                    MAX_REQUEST,
                )
                .await
                .unwrap();
                let job = service.next().await.unwrap();
                drop(stream);
                for _ in 0..100 {
                    if job.abandoned() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                assert!(job.abandoned());
                assert!(!config.profile().exists());
            })
            .await
            .unwrap();
        }

        #[tokio::test]
        async fn malformed_and_oversized_frames_do_not_break_the_listener() {
            let directory = tempfile::tempdir().unwrap();
            let config = config(directory.path());
            let _service = bind(&config).await.unwrap();
            for size in [0, (MAX_REQUEST + 1) as u32] {
                let mut stream = UnixStream::connect(config.data_dir.join("service.sock"))
                    .await
                    .unwrap();
                stream.write_u32(size).await.unwrap();
                assert!(read_frame(&mut stream, MAX_RESPONSE).await.is_err());
            }
            let mut stream = UnixStream::connect(config.data_dir.join("service.sock"))
                .await
                .unwrap();
            write_frame(&mut stream, b"not-json-secret", MAX_REQUEST)
                .await
                .unwrap();
            let bytes = read_frame(&mut stream, MAX_RESPONSE).await.unwrap();
            let Response::Error { message } = serde_json::from_slice(&bytes).unwrap() else {
                panic!("malformed frame accepted")
            };
            assert!(!message.contains("secret"));
            let Response::Success { .. } = raw(&config, json!({"version":1,"url":config.url.as_str(),"operation":{"operation":"preview_email","email":{"subject":"Synthetic","html":"<p>Local</p>"}}})).await else { panic!("listener failed after invalid frames") };
        }

        #[tokio::test]
        async fn full_queue_refuses_work_and_timeout_never_resubmits_a_write() {
            tokio::time::timeout(CALL_TIMEOUT.saturating_mul(2), async {
                let directory = tempfile::tempdir().unwrap(); let mut config = config(directory.path());
                config.mail_write_policy = crate::config::MailWritePolicy::Drafts;
                let mut service = bind(&config).await.unwrap();
                let mut streams = Vec::new();
                for index in 0..QUEUE_SIZE {
                    let mut stream = UnixStream::connect(config.data_dir.join("service.sock")).await.unwrap();
                    let request = Request { version:VERSION, url:config.url.to_string(), operation:Operation::Status };
                    write_frame(&mut stream, &serde_json::to_vec(&request).unwrap(), MAX_REQUEST).await.unwrap();
                    streams.push(stream);
                    while service.requests.len() <= index { tokio::task::yield_now().await; }
                }
                let Response::Error { message } = raw(&config, json!({"version":1,"url":config.url.as_str(),"operation":{"operation":"status"}})).await else { panic!("full queue accepted work") };
                assert!(message.contains("service_busy"));
                drop(streams);
                for _ in 0..QUEUE_SIZE { drop(service.next().await.unwrap()); }
                let client = config.clone();
                let write = tokio::spawn(async move { crate::execute(&client, draft()).await });
                let job = service.next().await.unwrap();
                tokio::time::pause();
                tokio::time::advance(CALL_TIMEOUT + Duration::from_secs(1)).await;
                assert!(write.await.unwrap().unwrap_err().to_string().contains("inspect"));
                assert!(job.abandoned());
                assert!(service.requests.try_recv().is_err());
                tokio::time::resume();
            }).await.unwrap();
        }
    }
}
