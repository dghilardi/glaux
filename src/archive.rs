//! Local, read-only mailbox replication. Only completed RFC 5322/MIME files are published.
use crate::{
    browser::Browser,
    config::{Config, private_dir},
    export, owa,
    retention::{self, Retention},
    shutdown::Shutdown,
};
use anyhow::{Context, Result, ensure};
use fs2::FileExt;
use mailparse::MailHeaderMap;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub archive_dir: PathBuf,
    pub folder: String,
    pub page_size: u32,
    pub max_pages: u32,
    pub rules: Vec<Rule>,
    pub retry_skipped: bool,
    /// Local copies of these categories are deleted after the period; off by default.
    pub retention: BTreeMap<String, Retention>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub category: String,
    pub header: String,
    pub contains: String,
    /// Optional exact mailbox, combined with the header condition. Not sender authentication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            archive_dir: PathBuf::new(),
            folder: "inbox".into(),
            page_size: 100,
            max_pages: 10,
            rules: Vec::new(),
            retry_skipped: false,
            retention: BTreeMap::new(),
        }
    }
}

impl Settings {
    pub fn load(path: Option<&Path>, config: &Config) -> Result<Self> {
        let mut settings: Self = if let Some(path) = path {
            toml::from_str(&std::fs::read_to_string(path)?)
                .map_err(|_| anyhow::anyhow!("Invalid sync configuration"))?
        } else {
            Self::default()
        };
        if settings.archive_dir.as_os_str().is_empty() {
            settings.archive_dir = config.data_dir.join("archive");
        }
        settings.validate()?;
        Ok(settings)
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.archive_dir.is_absolute(),
            "Archive directory must be absolute"
        );
        owa::list_request(&self.folder, self.page_size, 0)?;
        ensure!(
            (1..=1000).contains(&self.max_pages),
            "max_pages must be 1..1000"
        );
        for category in self.retention.keys() {
            retention::validate(category)?;
        }
        for rule in &self.rules {
            if let Some(sender) = &rule.sender {
                let mailbox = mailparse::addrparse(sender)
                    .ok()
                    .and_then(|addresses| addresses.extract_single_info());
                ensure!(
                    sender.len() <= 254
                        && sender.contains('@')
                        && mailbox.is_some_and(|mailbox| mailbox.addr == *sender),
                    "Rule sender must be a single mailbox address"
                );
            }
            ensure!(
                !rule.category.is_empty()
                    && rule.category.len() <= 64
                    && rule.category.bytes().all(|b| b.is_ascii_lowercase()
                        || b.is_ascii_digit()
                        || b == b'-'
                        || b == b'_'),
                "Invalid local category"
            );
            ensure!(
                [
                    "from",
                    "to",
                    "cc",
                    "subject",
                    "list-id",
                    "auto-submitted",
                    "content-type"
                ]
                .contains(&rule.header.to_ascii_lowercase().as_str()),
                "Unsupported classification header"
            );
            ensure!(
                !rule.contains.is_empty() && rule.contains.len() <= 1024,
                "Rule contains must be 1..1024 bytes"
            );
        }
        Ok(())
    }

    fn category(&self, bytes: &[u8]) -> Result<String> {
        let mail =
            mailparse::parse_mail(bytes).map_err(|_| anyhow::anyhow!("Invalid archived MIME"))?;
        let from = mail.headers.get_all_values("from");
        let sender = if from.len() == 1 {
            mailparse::addrparse(&from[0])
                .ok()
                .and_then(|addresses| addresses.extract_single_info())
        } else {
            None
        };
        for rule in &self.rules {
            if rule.sender.as_ref().is_some_and(|expected| {
                !sender
                    .as_ref()
                    .is_some_and(|actual| actual.addr.eq_ignore_ascii_case(expected))
            }) {
                continue;
            }
            if mail
                .headers
                .get_all_values(&rule.header)
                .iter()
                .any(|value| value.to_lowercase().contains(&rule.contains.to_lowercase()))
            {
                return Ok(rule.category.clone());
            }
        }
        Ok("other".into())
    }
}

/// Fallback age for rows archived before server times were recorded.
fn date_header(path: &Path) -> Option<i64> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > 10 * 1024 * 1024 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let date = mailparse::parse_headers(&bytes)
        .ok()
        .and_then(|(headers, _)| headers.get_first_value("Date"))
        .and_then(|date| mailparse::dateparse(&date).ok());
    let modified = || {
        let since = meta
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        i64::try_from(since.as_secs()).ok()
    };
    date.or_else(modified)
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn private_directory(path: &Path) -> Result<()> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        ensure!(
            !metadata.file_type().is_symlink() && metadata.is_dir(),
            "Archive directory must not be a symlink or file"
        );
    }
    private_dir(path)
}

fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(file) => Ok(file),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let meta = std::fs::symlink_metadata(path)?;
            ensure!(
                meta.is_file() && !meta.file_type().is_symlink(),
                "Archive state must be a regular file"
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                ensure!(
                    meta.permissions().mode() & 0o077 == 0,
                    "Archive state permissions must be private"
                );
            }
            Ok(OpenOptions::new().read(true).write(true).open(path)?)
        }
        Err(e) => Err(e.into()),
    }
}

pub struct Archive {
    root: PathBuf,
    database: Connection,
    _lock: File,
}

impl Archive {
    pub fn open(settings: &Settings) -> Result<Self> {
        settings.validate()?;
        private_directory(&settings.archive_dir)?;
        let lock = private_file(&settings.archive_dir.join("archive.lock"))?;
        lock.try_lock_exclusive()
            .context("Local archive is already in use")?;
        private_directory(&settings.archive_dir.join("messages"))?;
        let path = settings.archive_dir.join("state.sqlite3");
        private_file(&path)?;
        let database =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        database.busy_timeout(Duration::from_secs(5))?;
        let version: u32 = database.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        ensure!(version <= 2, "Unsupported archive schema");
        if version == 0 {
            let count: u32 = database.query_row("SELECT count(*) FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%'", [], |r| r.get(0))?;
            ensure!(count == 0, "Refusing unrelated archive database");
            database.execute_batch("BEGIN IMMEDIATE; CREATE TABLE glaux_messages(owa_id TEXT PRIMARY KEY, relative_path TEXT NOT NULL, sha256 TEXT NOT NULL); PRAGMA user_version=1; COMMIT;")?;
        }
        let own_schema: u32 = database.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE type='table' AND name='glaux_messages'",
            [],
            |r| r.get(0),
        )?;
        ensure!(own_schema == 1, "Refusing unrelated archive database");
        database.execute_batch("PRAGMA synchronous=FULL;")?;
        if version <= 1 {
            // Rows archived before v2 have no server time; expiry falls back to the Date header.
            database.execute_batch("BEGIN IMMEDIATE; ALTER TABLE glaux_messages ADD COLUMN received_unix INTEGER; ALTER TABLE glaux_messages ADD COLUMN expired_at INTEGER; PRAGMA user_version=2; COMMIT;")?;
        }
        database.execute_batch("CREATE TABLE IF NOT EXISTS glaux_scan(folder TEXT PRIMARY KEY,next_offset INTEGER NOT NULL);")?;
        database.execute_batch("CREATE TABLE IF NOT EXISTS glaux_skipped(owa_id TEXT PRIMARY KEY,reason TEXT NOT NULL);")?;
        if settings.retry_skipped {
            database.execute("DELETE FROM glaux_skipped", [])?;
        }
        Ok(Self {
            root: settings.archive_dir.clone(),
            database,
            _lock: lock,
        })
    }

    pub fn contains(&self, id: &str) -> Result<bool> {
        let row: Option<(String, String, Option<i64>)> = self
            .database
            .query_row(
                "SELECT relative_path,sha256,expired_at FROM glaux_messages WHERE owa_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((relative, checksum, expired)) = row else {
            return Ok(false);
        };
        // Expired by retention: keep it out of the archive instead of downloading it again.
        if expired.is_some() {
            return Ok(true);
        }
        ensure!(
            Path::new(&relative)
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
            "Invalid archive state path"
        );
        let path = self.root.join(relative);
        if !path.try_exists()? {
            return Ok(false);
        }
        let meta = std::fs::symlink_metadata(&path)?;
        ensure!(
            meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= 10 * 1024 * 1024,
            "Invalid archived message file"
        );
        ensure!(
            digest(&std::fs::read(path)?) == checksum,
            "Archived message checksum mismatch"
        );
        Ok(true)
    }

    pub fn store(
        &self,
        settings: &Settings,
        id: &str,
        bytes: &[u8],
        received_unix: Option<i64>,
    ) -> Result<()> {
        owa::validate_mime(bytes)?;
        let category = settings.category(bytes)?;
        let checksum = digest(bytes);
        let directory = self.root.join("messages").join(&category);
        private_directory(&directory)?;
        let relative = format!("messages/{category}/{checksum}.eml");
        let path = self.root.join(&relative);
        if path.try_exists()? {
            let meta = std::fs::symlink_metadata(&path)?;
            ensure!(
                meta.is_file()
                    && !meta.file_type().is_symlink()
                    && meta.len() == bytes.len() as u64,
                "Archive filename conflict"
            );
            ensure!(
                digest(&std::fs::read(&path)?) == checksum,
                "Archive checksum conflict"
            );
        } else {
            export::publish(&path, bytes)?;
        }
        // An identical export repairs a publication without a state row. A server-regenerated
        // MIME representation can leave an orphan with different bytes after a crash.
        self.database.execute("INSERT INTO glaux_messages(owa_id,relative_path,sha256,received_unix,expired_at) VALUES(?1,?2,?3,?4,NULL) ON CONFLICT(owa_id) DO UPDATE SET relative_path=excluded.relative_path,sha256=excluded.sha256,received_unix=excluded.received_unix,expired_at=NULL", params![id,relative,checksum,received_unix])?;
        Ok(())
    }

    /// Delete local copies older than their category's retention; returns how many expired.
    pub fn expire(&self, settings: &Settings, now_unix: i64) -> Result<u32> {
        let mut expired = 0;
        for (category, retention) in &settings.retention {
            let prefix = format!("messages/{category}/");
            let cutoff = retention.cutoff(now_unix);
            let rows: Vec<(String, String, Option<i64>)> = self
                .database
                .prepare("SELECT owa_id,relative_path,received_unix FROM glaux_messages WHERE expired_at IS NULL AND substr(relative_path,1,length(?1))=?1 AND (received_unix IS NULL OR received_unix<?2)")?
                .query_map(params![prefix, cutoff], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            for (id, relative, received) in rows {
                let path = self.root.join(&relative);
                let received = match received {
                    Some(received) => received,
                    None => match date_header(&path) {
                        Some(date) => {
                            self.database.execute(
                                "UPDATE glaux_messages SET received_unix=?2 WHERE owa_id=?1",
                                params![id, date],
                            )?;
                            date
                        }
                        None => continue,
                    },
                };
                if received >= cutoff {
                    continue;
                }
                self.database.execute(
                    "UPDATE glaux_messages SET expired_at=?2 WHERE owa_id=?1",
                    params![id, now_unix],
                )?;
                // Identical messages share one file: keep it while any copy is still live.
                let live: bool = self.database.query_row("SELECT EXISTS(SELECT 1 FROM glaux_messages WHERE relative_path=?1 AND expired_at IS NULL)", [&relative], |r| r.get(0))?;
                if !live
                    && Path::new(&relative)
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_)))
                {
                    match std::fs::symlink_metadata(&path) {
                        Ok(meta) if meta.is_file() => std::fs::remove_file(&path)?,
                        _ => {}
                    }
                }
                expired += 1;
            }
        }
        Ok(expired)
    }

    fn health(&self, value: &Value) -> Result<()> {
        let mut value = value.clone();
        value["updated_at_unix"] = json!(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs()
        );
        let mut temporary = tempfile::NamedTempFile::new_in(&self.root)?;
        serde_json::to_writer(&mut temporary, &value)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary.persist(self.root.join("status.json"))?;
        Ok(())
    }

    fn skipped(&self, id: &str) -> Result<bool> {
        Ok(self.database.query_row(
            "SELECT EXISTS(SELECT 1 FROM glaux_skipped WHERE owa_id=?1)",
            [id],
            |r| r.get(0),
        )?)
    }
    fn skip(&self, id: &str, reason: &owa::ItemExportError) -> Result<()> {
        self.database.execute(
            "INSERT OR REPLACE INTO glaux_skipped VALUES(?1,?2)",
            params![id, reason.to_string()],
        )?;
        Ok(())
    }
    fn skipped_count(&self) -> Result<u32> {
        Ok(self
            .database
            .query_row("SELECT count(*) FROM glaux_skipped", [], |r| r.get(0))?)
    }

    fn cursor(&self, folder: &str) -> Result<u32> {
        Ok(self
            .database
            .query_row(
                "SELECT next_offset FROM glaux_scan WHERE folder=?1",
                [folder],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0))
    }

    fn advance(&self, folder: &str, offset: u32) -> Result<()> {
        self.database.execute("INSERT INTO glaux_scan VALUES(?1,?2) ON CONFLICT(folder) DO UPDATE SET next_offset=excluded.next_offset", params![folder,offset])?;
        Ok(())
    }
}

#[derive(Default, Serialize)]
pub struct Report {
    pub scanned: u32,
    pub downloaded: u32,
    pub reused: u32,
    pub bytes: u64,
    pub scan_complete: bool,
    pub resume_offset: u32,
    pub skipped: u32,
    pub expired: u32,
}

trait Mailbox {
    async fn list(&mut self, folder: &str, limit: u32, offset: u32) -> Result<owa::MailPage>;
    async fn mime(&mut self, id: &str) -> Result<Vec<u8>>;
}

struct OwaMailbox<'a> {
    browser: &'a mut Browser,
    config: &'a Config,
}
impl Mailbox for OwaMailbox<'_> {
    async fn list(&mut self, folder: &str, limit: u32, offset: u32) -> Result<owa::MailPage> {
        owa::list(self.browser, self.config, folder, limit, offset).await
    }
    async fn mime(&mut self, id: &str) -> Result<Vec<u8>> {
        owa::read_mime(self.browser, self.config, id).await
    }
}

async fn cycle(
    mailbox: &mut impl Mailbox,
    settings: &Settings,
    archive: &Archive,
) -> Result<Report> {
    let mut report = Report::default();
    let mut pending_skips = Vec::new();
    let mut consecutive_errors = 0;
    let resume = archive.cursor(&settings.folder)?;
    let mut reconciling_head = true;
    let mut offset = 0;
    for _ in 0..settings.max_pages {
        let page = mailbox
            .list(&settings.folder, settings.page_size, offset)
            .await?;
        let mut unseen = false;
        for message in page.messages {
            report.scanned += 1;
            if archive.contains(&message.id)? {
                report.reused += 1;
                continue;
            }
            if archive.skipped(&message.id)? {
                report.skipped += 1;
                continue;
            }
            unseen = true;
            let bytes = match mailbox.mime(&message.id).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    if let Some(reason) = error.downcast_ref::<owa::ItemExportError>() {
                        // A locally measured bound is an item property, even if every
                        // new message is oversized. Protocol-shaped errors stay gated.
                        if matches!(reason, owa::ItemExportError::TooLarge) {
                            archive.skip(&message.id, reason)?;
                            report.skipped += 1;
                            continue;
                        }
                        consecutive_errors += 1;
                        ensure!(
                            consecutive_errors < 5,
                            "Repeated MIME export failures; check protocol compatibility"
                        );
                        pending_skips.push((message.id, *reason));
                        report.skipped += 1;
                        continue;
                    }
                    return Err(error);
                }
            };
            let received = message
                .received_at
                .as_deref()
                .and_then(retention::parse_received);
            archive.store(settings, &message.id, &bytes, received)?;
            consecutive_errors = 0;
            report.downloaded += 1;
            report.bytes += bytes.len() as u64;
        }
        match page.next_offset {
            Some(next) => {
                let next =
                    u32::try_from(next).context("OWA paging offset exceeds supported range")?;
                ensure!(next > offset, "OWA paging did not advance");
                // Every cycle reconciles the head, then resumes the historical sweep.
                offset = if reconciling_head && !unseen {
                    reconciling_head = false;
                    next.max(resume)
                } else {
                    next
                };
            }
            None => {
                report.scan_complete = true;
                offset = 0;
                if pending_skips.is_empty() || report.downloaded > 0 {
                    archive.advance(&settings.folder, offset)?;
                }
                break;
            }
        }
        if pending_skips.is_empty() || report.downloaded > 0 {
            archive.advance(&settings.folder, offset)?;
        }
    }
    report.resume_offset = offset;
    ensure!(
        pending_skips.is_empty() || report.downloaded > 0,
        "No MIME export succeeded; check protocol compatibility before skipping items"
    );
    for (id, reason) in pending_skips {
        archive.skip(&id, &reason)?;
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;
    report.expired = archive.expire(settings, i64::try_from(now.as_secs())?)?;
    Ok(report)
}

/// One owner per archive/profile; browser survives successful daemon polling cycles.
pub async fn run(config: &Config, settings: &Settings, interval: Option<u64>) -> Result<Value> {
    settings.validate()?;
    if let Some(seconds) = interval {
        ensure!(
            (30..=86400).contains(&seconds),
            "Interval must be 30..86400 seconds"
        );
    }
    let mut shutdown = Shutdown::new()?;
    let archive = Archive::open(settings)?;
    let mut browser = None;
    let mut failures = 0u32;
    let outcome = async { loop {
        // Signals are already registered. Finish startup before consuming a pending stop,
        // retaining ownership so the launched browser can always be shut down gracefully.
        let launch = if browser.is_none() {
            Browser::launch(config, config.headed).await.map(|active| { browser = Some(active); true })
        } else { Ok(false) };
        let result = match launch {
            Err(error) => Some(Err(error)),
            Ok(authenticate) => tokio::select! {
            biased;
            _ = shutdown.wait() => None,
            result = async {
                if authenticate {
                    browser.as_mut().unwrap().authenticate(config).await?;
                }
                cycle(&mut OwaMailbox { browser:browser.as_mut().unwrap(),config }, settings, &archive).await
            } => Some(result),
        }};
        let Some(result) = result else {
            if let Some(active) = browser.take() {
                active.shutdown().await;
            }
            archive.health(&json!({"state":"stopped"}))?;
            return Ok(json!({"stopped":true}));
        };
        match result {
            Ok(report) => {
                failures = 0;
                let skipped_total = archive.skipped_count()?;
                let value = json!({"state":if skipped_total > 0 {"partial"} else if report.scan_complete {"ready"} else {"backfilling"},"report":report,"skipped_total":skipped_total});
                archive.health(&value)?;
                if interval.is_none() {
                    if let Some(active) = browser.take() {
                        active.shutdown().await;
                    }
                    return Ok(value);
                }
                eprintln!("{}", value);
            }
            Err(error) => {
                if let Some(active) = browser.take() {
                    active.shutdown().await;
                }
                let reason = failure_reason(&error);
                archive.health(&json!({"state":"attention_required","reason":reason,"hint":"Stop daemon and run glaux --headed status or login; also check network and protocol compatibility"}))?;
                if interval.is_none() {
                    return Err(error);
                }
                failures = failures.saturating_add(1);
                if failures == 1 && reason == "authentication_or_protocol_required" {
                    notify_attention().await;
                }
                eprintln!(
                    "{}",
                    json!({"event":"sync_failed","state":"attention_required","reason":reason,"consecutive_failures":failures})
                );
            }
        }
        let seconds = interval
            .unwrap()
            .saturating_mul(1u64 << failures.min(6))
            .min(86400);
        tokio::select! {
            _ = shutdown.wait() => {
                if let Some(active) = browser.take() { active.shutdown().await; }
                archive.health(&json!({"state":"stopped"}))?;
                return Ok(json!({"stopped":true}));
            }
            _ = tokio::time::sleep(Duration::from_secs(seconds)) => {}
        }
    }}.await;
    if let Some(active) = browser.take() {
        active.shutdown().await;
    }
    outcome
}

/// Best effort and content-free: unattended sign-in could not restore the session.
async fn notify_attention() {
    let run = tokio::process::Command::new("notify-send")
        .args([
            "--app-name=Glaux",
            "Glaux needs Microsoft sign-in",
            "Stop glaux.service, run `glaux login`, then start the service again.",
        ])
        .kill_on_drop(true)
        .status();
    let _ = tokio::time::timeout(Duration::from_secs(5), run).await;
}

fn failure_reason(error: &anyhow::Error) -> &'static str {
    let description = error.to_string();
    if description.contains("No authenticated OWA")
        || description.contains("session expired")
        || description.contains("HTTP 403")
        || description.contains("sign-in")
    {
        "authentication_or_protocol_required"
    } else if description.contains("HTTP 429") {
        "throttled"
    } else if description.contains("checksum") {
        "archive_integrity"
    } else if description.contains("Chromium") {
        "browser_startup"
    } else {
        "transport_or_protocol"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn passkey_sign_in_failures_require_attention() {
        for message in [
            "Microsoft asked for a password; passkey sign-in needs attention, run `glaux login`",
            "Microsoft sign-in reported an error; run `glaux login`",
            "Passkey sign-in did not progress; run `glaux login`",
        ] {
            assert_eq!(
                failure_reason(&anyhow::anyhow!(message)),
                "authentication_or_protocol_required"
            );
        }
    }
    const MIME:&[u8] = b"From: robot@example.test\r\nDate: Sun, 25 Oct 2026 10:00:00 +0000\r\nSubject: Alert synthetic\r\n\r\nBody\r\n";
    struct FakeMailbox {
        offsets: Vec<u32>,
        reads: Vec<String>,
        fail: bool,
    }
    impl Mailbox for FakeMailbox {
        async fn list(&mut self, _: &str, _: u32, offset: u32) -> Result<owa::MailPage> {
            self.offsets.push(offset);
            Ok(owa::MailPage { messages:vec![serde_json::from_value(json!({"id":format!("item-{offset}"),"subject":"synthetic","from":null,"received_at":null,"is_read":false,"has_attachments":false})).unwrap()], total:Some(4), next_offset:if offset<3 {Some((offset+1) as u64)} else {None} })
        }
        async fn mime(&mut self, id: &str) -> Result<Vec<u8>> {
            ensure!(!self.fail, "synthetic failure");
            self.reads.push(id.into());
            Ok(MIME.to_vec())
        }
    }
    #[tokio::test]
    async fn new_mail_burst_is_reconciled_before_old_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            archive_dir: dir.path().join("archive"),
            page_size: 1,
            max_pages: 4,
            ..Default::default()
        };
        let archive = Archive::open(&settings).unwrap();
        archive.store(&settings, "item-2", MIME, None).unwrap();
        archive.advance("inbox", 5).unwrap();
        let mut fake = FakeMailbox {
            offsets: vec![],
            reads: vec![],
            fail: false,
        };
        cycle(&mut fake, &settings, &archive).await.unwrap();
        assert_eq!(fake.offsets, vec![0, 1, 2, 5]);
    }

    #[tokio::test]
    async fn protocol_wide_mime_failures_are_not_durable_skips() {
        struct BrokenMailbox(FakeMailbox);
        impl Mailbox for BrokenMailbox {
            async fn list(&mut self, f: &str, l: u32, o: u32) -> Result<owa::MailPage> {
                let mut page = self.0.list(f, l, o).await?;
                page.next_offset = Some((o + 1).into());
                Ok(page)
            }
            async fn mime(&mut self, _: &str) -> Result<Vec<u8>> {
                Err(owa::ItemExportError::InvalidMime.into())
            }
        }
        for max_pages in [2, 10] {
            let dir = tempfile::tempdir().unwrap();
            let settings = Settings {
                archive_dir: dir.path().join("archive"),
                max_pages,
                ..Default::default()
            };
            let archive = Archive::open(&settings).unwrap();
            let mut fake = BrokenMailbox(FakeMailbox {
                offsets: vec![],
                reads: vec![],
                fail: false,
            });
            assert!(cycle(&mut fake, &settings, &archive).await.is_err());
            assert_eq!(archive.skipped_count().unwrap(), 0);
            assert_eq!(archive.cursor("inbox").unwrap(), 0);
            assert!(fake.0.offsets.len() <= 5);
        }
    }

    #[tokio::test]
    async fn oversized_new_messages_do_not_stall_a_synced_archive() {
        struct LargeMailbox(FakeMailbox);
        impl Mailbox for LargeMailbox {
            async fn list(&mut self, f: &str, l: u32, o: u32) -> Result<owa::MailPage> {
                let mut page = self.0.list(f, l, o).await?;
                page.next_offset = if o < 5 { Some((o + 1).into()) } else { None };
                Ok(page)
            }
            async fn mime(&mut self, _: &str) -> Result<Vec<u8>> {
                Err(owa::ItemExportError::TooLarge.into())
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            archive_dir: dir.path().join("archive"),
            max_pages: 10,
            ..Default::default()
        };
        let archive = Archive::open(&settings).unwrap();
        let mut fake = LargeMailbox(FakeMailbox {
            offsets: vec![],
            reads: vec![],
            fail: false,
        });
        let report = cycle(&mut fake, &settings, &archive).await.unwrap();
        assert_eq!(report.downloaded, 0);
        assert_eq!(report.skipped, 6);
        assert_eq!(archive.skipped_count().unwrap(), 6);
        assert_eq!(archive.cursor("inbox").unwrap(), 0);
        assert!(report.scan_complete);
    }

    #[tokio::test]
    async fn oversized_message_does_not_block_other_messages_or_restarts() {
        struct PoisonMailbox(FakeMailbox);
        impl Mailbox for PoisonMailbox {
            async fn list(&mut self, f: &str, l: u32, o: u32) -> Result<owa::MailPage> {
                self.0.list(f, l, o).await
            }
            async fn mime(&mut self, id: &str) -> Result<Vec<u8>> {
                if id == "item-0" {
                    return Err(owa::ItemExportError::TooLarge.into());
                }
                self.0.mime(id).await
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings {
            archive_dir: dir.path().join("archive"),
            page_size: 1,
            max_pages: 4,
            ..Default::default()
        };
        let archive = Archive::open(&settings).unwrap();
        let mut fake = PoisonMailbox(FakeMailbox {
            offsets: vec![],
            reads: vec![],
            fail: false,
        });
        let report = cycle(&mut fake, &settings, &archive).await.unwrap();
        assert_eq!(report.downloaded, 3);
        assert_eq!(report.skipped, 1);
        assert!(report.scan_complete);
        assert_eq!(archive.skipped_count().unwrap(), 1);
        drop(archive);
        let archive = Archive::open(&settings).unwrap();
        assert!(archive.skipped("item-0").unwrap());
        drop(archive);
        settings.retry_skipped = true;
        assert_eq!(
            Archive::open(&settings).unwrap().skipped_count().unwrap(),
            0
        );
    }
    #[tokio::test]
    async fn sync_resumes_after_restart_and_failed_page_without_skipping_head() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            archive_dir: dir.path().join("archive"),
            page_size: 1,
            max_pages: 2,
            ..Default::default()
        };
        let archive = Archive::open(&settings).unwrap();
        let mut fake = FakeMailbox {
            offsets: Vec::new(),
            reads: Vec::new(),
            fail: false,
        };
        let first = cycle(&mut fake, &settings, &archive).await.unwrap();
        assert_eq!(fake.offsets, vec![0, 1]);
        assert_eq!(first.downloaded, 2);
        assert!(!first.scan_complete);
        drop(archive);
        let archive = Archive::open(&settings).unwrap();
        fake.offsets.clear();
        fake.fail = true;
        assert!(cycle(&mut fake, &settings, &archive).await.is_err());
        assert_eq!(archive.cursor("inbox").unwrap(), 2);
        fake.fail = false;
        fake.offsets.clear();
        let second = cycle(&mut fake, &settings, &archive).await.unwrap();
        assert_eq!(fake.offsets, vec![0, 2]);
        assert_eq!(second.reused, 1);
        assert_eq!(second.downloaded, 1);
        fake.offsets.clear();
        assert!(
            cycle(&mut fake, &settings, &archive)
                .await
                .unwrap()
                .scan_complete
        );
        assert_eq!(fake.offsets, vec![0, 3]);
        assert_eq!(archive.cursor("inbox").unwrap(), 0);
        assert_eq!(fake.reads.len(), 4);
    }
    #[test]
    fn archive_recovers_publication_and_deduplicates_without_using_message_id() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            archive_dir: dir.path().join("archive"),
            rules: vec![Rule {
                category: "alerts".into(),
                header: "subject".into(),
                contains: "alert".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let archive = Archive::open(&settings).unwrap();
        assert!(Archive::open(&settings).is_err());
        archive.store(&settings, "owa-1", MIME, None).unwrap();
        archive.store(&settings, "owa-2", MIME, None).unwrap();
        assert!(archive.contains("owa-1").unwrap());
        let path = settings
            .archive_dir
            .join(format!("messages/alerts/{}.eml", digest(MIME)));
        assert_eq!(std::fs::read(&path).unwrap(), MIME);
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        archive
            .database
            .execute("DELETE FROM glaux_messages", [])
            .unwrap();
        archive.store(&settings, "owa-1", MIME, None).unwrap();
        assert!(archive.contains("owa-1").unwrap());
        std::fs::remove_file(&path).unwrap();
        assert!(!archive.contains("owa-1").unwrap());
        archive.store(&settings, "owa-1", MIME, None).unwrap();
        std::fs::write(&path, b"tampered").unwrap();
        assert!(archive.contains("owa-1").is_err());
    }
    #[test]
    fn retention_expires_local_copies_and_keeps_tombstones() {
        const NEW: &[u8] = b"From: robot@example.test\r\nDate: Sun, 25 Oct 2026 11:00:00 +0000\r\nSubject: Alert newer\r\n\r\nBody\r\n";
        const NOTE: &[u8] = b"From: colleague@example.test\r\nDate: Sun, 25 Oct 2026 12:00:00 +0000\r\nSubject: Note\r\n\r\nBody\r\n";
        let dir = tempfile::tempdir().unwrap();
        let settings: Settings = toml::from_str(&format!(
            "archive_dir = {:?}\n[[rules]]\ncategory = \"alerts\"\nheader = \"subject\"\ncontains = \"alert\"\n[retention]\nalerts = \"10d\"\n",
            dir.path().join("archive")
        ))
        .unwrap();
        settings.validate().unwrap();
        let archive = Archive::open(&settings).unwrap();
        let now = 2_000_000_000;
        let old = now - 11 * 86_400;
        archive.store(&settings, "old", MIME, Some(old)).unwrap();
        archive
            .store(&settings, "old-duplicate", MIME, Some(now))
            .unwrap();
        archive
            .store(&settings, "new", NEW, Some(now - 86_400))
            .unwrap();
        archive.store(&settings, "note", NOTE, Some(old)).unwrap();
        let shared = settings
            .archive_dir
            .join(format!("messages/alerts/{}.eml", digest(MIME)));

        assert_eq!(archive.expire(&settings, now).unwrap(), 1);
        assert!(shared.exists(), "a live duplicate keeps the shared file");
        archive
            .database
            .execute(
                "UPDATE glaux_messages SET received_unix=?1 WHERE owa_id='old-duplicate'",
                [old],
            )
            .unwrap();
        assert_eq!(archive.expire(&settings, now).unwrap(), 1);
        assert!(!shared.exists());
        assert!(
            archive.contains("old").unwrap(),
            "expired messages are not downloaded again"
        );
        assert!(archive.contains("new").unwrap());
        assert!(
            archive.contains("note").unwrap(),
            "categories without retention are kept"
        );
        assert_eq!(archive.expire(&settings, now).unwrap(), 0);
    }
    #[test]
    fn legacy_archive_is_migrated_and_aged_by_date_header() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings {
            archive_dir: dir.path().join("archive"),
            rules: vec![Rule {
                category: "alerts".into(),
                header: "subject".into(),
                contains: "alert".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        std::fs::create_dir_all(settings.archive_dir.join("messages/alerts")).unwrap();
        let relative = format!("messages/alerts/{}.eml", digest(MIME));
        std::fs::write(settings.archive_dir.join(&relative), MIME).unwrap();
        let legacy = Connection::open(settings.archive_dir.join("state.sqlite3")).unwrap();
        legacy.execute_batch("CREATE TABLE glaux_messages(owa_id TEXT PRIMARY KEY, relative_path TEXT NOT NULL, sha256 TEXT NOT NULL); PRAGMA user_version=1;").unwrap();
        legacy
            .execute(
                "INSERT INTO glaux_messages VALUES('legacy',?1,?2)",
                params![relative, digest(MIME)],
            )
            .unwrap();
        drop(legacy);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let state = settings.archive_dir.join("state.sqlite3");
            std::fs::set_permissions(state, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        settings
            .retention
            .insert("alerts".into(), Retention { days: 10 });
        let archive = Archive::open(&settings).unwrap();
        let version: u32 = archive
            .database
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 2);
        // The fixture Date header is 2026-10-25 10:00 UTC.
        assert_eq!(
            archive
                .expire(&settings, 1_792_922_400 + 9 * 86_400)
                .unwrap(),
            0
        );
        assert_eq!(
            archive
                .expire(&settings, 1_792_922_400 + 11 * 86_400)
                .unwrap(),
            1
        );
        assert!(archive.contains("legacy").unwrap());
        assert!(!settings.archive_dir.join(relative).exists());
    }
    #[test]
    fn retention_configuration_is_validated() {
        for raw in [
            "[retention]\nalerts = \"10\"\n",
            "[retention]\nalerts = 10\n",
            "[retention]\n\"../x\" = \"10d\"\n",
        ] {
            let parsed = toml::from_str::<Settings>(&format!("archive_dir = \"/tmp/a\"\n{raw}"));
            assert!(
                parsed.is_err() || parsed.unwrap().validate().is_err(),
                "{raw:?}"
            );
        }
    }
    #[test]
    fn exact_sender_and_subject_are_combined_before_generic_rules() {
        let settings: Settings = toml::from_str(
            r#"
archive_dir = "/tmp/synthetic-archive"
[[rules]]
category = "monitoring"
sender = "robot@example.test"
header = "subject"
contains = "alert"
[[rules]]
category = "review"
header = "subject"
contains = "alert"
"#,
        )
        .unwrap();
        settings.validate().unwrap();
        assert_eq!(settings.category(MIME).unwrap(), "monitoring");
        for from in [
            "robot@example.test <other@example.test>",
            "robot@example.test.invalid",
            "robot@example.test, other@example.test",
        ] {
            let bytes = format!("From: {from}\r\nSubject: Alert synthetic\r\n\r\nBody\r\n");
            assert_eq!(settings.category(bytes.as_bytes()).unwrap(), "review");
        }
        let bytes = b"From: ROBOT@example.test\r\nSubject: Note\r\n\r\nBody\r\n";
        assert_eq!(settings.category(bytes).unwrap(), "other");
        let bytes = b"From: ROBOT@example.test\r\nSubject: ALERT synthetic\r\n\r\nBody\r\n";
        assert_eq!(settings.category(bytes).unwrap(), "monitoring");
        let mut invalid = settings;
        invalid.rules[0].sender = Some("Robot <robot@example.test>".into());
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn example_rules_separate_monitoring_administration_and_development() {
        let settings: Settings = toml::from_str(include_str!("../sync.example.toml")).unwrap();
        let settings = Settings {
            archive_dir: "/tmp/synthetic-archive".into(),
            ..settings
        };
        settings.validate().unwrap();
        for (from, subject, category) in [
            (
                "robot@example.test",
                "[FIRING] Prometheus architecture warning",
                "monitoring",
            ),
            (
                "Grafana <robot@example.test>",
                "Grafana resolved",
                "monitoring",
            ),
            (
                "robot@example.test",
                "Promemoria timbrature",
                "administrative",
            ),
            (
                "Bitbucket <robot@example.test>",
                "Pull request updated",
                "development-notifications",
            ),
            (
                "Jira <robot@example.test>",
                "Issue updated",
                "development-notifications",
            ),
            ("colleague@example.test", "Architecture decision", "other"),
        ] {
            let bytes = format!("From: {from}\r\nSubject: {subject}\r\n\r\nBody\r\n");
            assert_eq!(settings.category(bytes.as_bytes()).unwrap(), category);
        }
    }

    #[test]
    fn configuration_and_payload_are_validated_before_publication() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings {
            archive_dir: dir.path().join("archive"),
            ..Default::default()
        };
        settings.rules.push(Rule {
            category: "../escape".into(),
            header: "subject".into(),
            contains: "test".into(),
            ..Default::default()
        });
        assert!(settings.validate().is_err());
        settings.rules.clear();
        let archive = Archive::open(&settings).unwrap();
        assert!(archive.store(&settings, "id", b"not mail", None).is_err());
        assert!(!archive.contains("id").unwrap());
    }
}
