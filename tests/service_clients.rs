#![cfg(unix)]
//! Synthetic socket peer: real CLI/MCP clients must not launch another browser.
use glaux::config::{Config, DEFAULT_URL};
use serde_json::{Value, json};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    process::{Child, ChildStdin, Command},
};

fn listener(path: &Path) -> UnixListener {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = path.join("service.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    listener
}

async fn receive(stream: &mut UnixStream) -> Value {
    let size = stream.read_u32().await.unwrap() as usize;
    assert!(size < 2 * 1024 * 1024);
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn respond(stream: &mut UnixStream, data: Value) {
    let bytes = serde_json::to_vec(&json!({"status":"success","data":data})).unwrap();
    stream.write_u32(bytes.len() as u32).await.unwrap();
    stream.write_all(&bytes).await.unwrap();
}

fn cli(path: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_glaux"));
    command
        .arg("--data-dir")
        .arg(path)
        .args(["--browser", "/bin/false"])
        .env_remove("GLAUX_PASSKEY_ACCOUNT")
        .env_remove("GLAUX_MAIL_WRITE_POLICY")
        .env_remove("GLAUX_OWA_URL");
    command
}

struct Mcp {
    child: Child,
    stdin: ChildStdin,
    lines: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
}

impl Mcp {
    async fn start(path: &Path) -> Self {
        let mut child = cli(path)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut mcp = Self {
            child,
            stdin,
            lines,
        };
        mcp.request(1,"initialize",json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"socket-fixture","version":"1"}})).await;
        mcp.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        mcp
    }
    async fn send(&mut self, value: Value) {
        self.stdin
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
    }
    async fn request(&mut self, id: u32, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        loop {
            let value: Value =
                serde_json::from_str(&self.lines.next_line().await.unwrap().unwrap()).unwrap();
            if value["id"] == id && value.get("method").is_none() {
                return value;
            }
        }
    }
    async fn stop(mut self) {
        self.stdin.shutdown().await.unwrap();
        drop(self.stdin);
        self.child.wait().await.unwrap();
    }
}

#[tokio::test]
async fn real_cli_and_mcp_forward_while_the_owner_holds_the_profile() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let directory = tempfile::tempdir().unwrap();
        let config =
            Config::new(Some(directory.path().into()), None, DEFAULT_URL, false, 1).unwrap();
        let _lock = config.lock().unwrap();
        let listener = listener(directory.path());
        let expected =
            json!({"id":"fixture","subject":"Synthetic","body":"Fixture body","body_type":"Text"});
        let reply = expected.clone();
        let owner = tokio::spawn(async move {
            let mut requests = tokio::task::JoinSet::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let reply = reply.clone();
                requests.spawn(async move {
                    let request = receive(&mut stream).await;
                    assert_eq!(request["version"], 1);
                    assert_eq!(request["url"], DEFAULT_URL);
                    assert_eq!(
                        request["operation"],
                        json!({"operation":"read","id":"fixture"})
                    );
                    respond(&mut stream, reply).await;
                });
            }
            while let Some(result) = requests.join_next().await {
                result.unwrap();
            }
        });
        let mut mcp = Mcp::start(directory.path()).await;
        let (output, response) = tokio::join!(
            cli(directory.path()).args(["read", "fixture"]).output(),
            mcp.request(
                2,
                "tools/call",
                json!({"name":"read_message","arguments":{"id":"fixture"}})
            ),
        );
        let output = output.unwrap();
        assert!(output.status.success());
        let cli: Value = serde_json::from_slice(&output.stdout).unwrap();
        let mcp_result: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(cli["data"], expected);
        assert_eq!(mcp_result["data"], expected);
        assert!(config.lock().is_err());
        mcp.stop().await;
        owner.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn submitted_cli_write_is_not_retried_after_response_loss() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let directory = tempfile::tempdir().unwrap();
        let listener = listener(directory.path());
        let input = directory.path().join("email.json");
        std::fs::write(&input, r#"{"subject":"Synthetic","html":"<p>Fixture</p>"}"#).unwrap();
        let owner = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(
                receive(&mut stream).await["operation"]["operation"],
                "create_draft"
            );
            drop(stream);
            assert!(
                tokio::time::timeout(Duration::from_millis(100), listener.accept())
                    .await
                    .is_err()
            );
        });
        let output = cli(directory.path())
            .args([
                "--mail-write-policy",
                "drafts",
                "create-draft",
                "--signature-draft-id",
                "fixture",
                "--input",
            ])
            .arg(input)
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("draft_save_unconfirmed"));
        assert!(!directory.path().join("browser-profile").exists());
        owner.await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn daemon_ownership_protects_logout_and_sync_even_without_a_browser() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::new(
        Some(directory.path().join("data")),
        None,
        DEFAULT_URL,
        false,
        1,
    )
    .unwrap();
    let _service = glaux::service::Service::bind(&config)
        .await
        .unwrap()
        .unwrap();
    std::fs::create_dir_all(config.profile()).unwrap();
    let retained = config.profile().join("synthetic-state");
    std::fs::write(&retained, b"fixture").unwrap();
    for args in [vec!["logout"], vec!["--standalone", "status"]] {
        let output = cli(&config.data_dir).args(args).output().await.unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("service_owns_profile"));
        assert_eq!(std::fs::read(&retained).unwrap(), b"fixture");
    }
    let archive = directory.path().join("archive");
    let settings = directory.path().join("sync.toml");
    std::fs::write(
        &settings,
        format!("archive_dir = '{}'\n", archive.display()),
    )
    .unwrap();
    let output = cli(&config.data_dir)
        .args(["sync", "--config"])
        .arg(settings)
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("service_owns_profile"));
    assert!(!archive.exists());
    assert_eq!(std::fs::read(&retained).unwrap(), b"fixture");
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn mcp_shutdown_disconnects_a_pending_request_without_stopping_the_owner() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let directory = tempfile::tempdir().unwrap(); let listener = listener(directory.path());
        let mut mcp = Mcp::start(directory.path()).await;
        mcp.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"read_message","arguments":{"id":"fixture"}}})).await;
        let (mut stream, _) = listener.accept().await.unwrap(); receive(&mut stream).await;
        assert!(Command::new("kill").args(["-TERM",&mcp.child.id().unwrap().to_string()]).status().await.unwrap().success());
        tokio::time::timeout(Duration::from_secs(5),mcp.child.wait()).await.unwrap().unwrap();
        assert_eq!(stream.read(&mut [0;1]).await.unwrap(), 0);
        assert!(!directory.path().join("browser-profile").exists());
        assert!(directory.path().join("service.sock").exists());
    }).await.unwrap();
}
