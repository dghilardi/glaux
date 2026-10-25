//! Private, durable requests consumed by the archive owner between polling cycles.
use crate::archive::{Archive, ReprocessReport, Rule, Settings};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{io::Write, path::Path};

const REQUEST: &str = "reprocess.request.json";
const STATUS: &str = "reprocess.status.json";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    request_id: String,
    dry_run: bool,
    rules: Vec<Rule>,
}

fn read_private<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= 1024 * 1024,
        "Invalid reprocess control file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            meta.permissions().mode() & 0o077 == 0,
            "Reprocess control file must be private"
        );
    }
    serde_json::from_slice(&std::fs::read(path)?)
        .map(Some)
        .map_err(|_| anyhow::anyhow!("Invalid reprocess control data"))
}

fn temporary_json(root: &Path, value: &impl Serialize) -> Result<tempfile::NamedTempFile> {
    let mut temporary = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer(&mut temporary, value)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    Ok(temporary)
}

fn write_status(root: &Path, value: &Value) -> Result<()> {
    temporary_json(root, value)?.persist(root.join(STATUS))?;
    Ok(())
}

fn validate_root(settings: &Settings) -> Result<()> {
    settings.validate()?;
    let meta = std::fs::symlink_metadata(&settings.archive_dir)?;
    ensure!(
        meta.is_dir() && !meta.file_type().is_symlink(),
        "Archive directory must be a regular directory"
    );
    Ok(())
}

/// Offline reclassification must not reset the sync-only skipped-message state.
pub fn local(settings: &Settings, dry_run: bool) -> Result<ReprocessReport> {
    let owner_settings = Settings {
        retry_skipped: false,
        ..settings.clone()
    };
    Archive::open(&owner_settings)?.reprocess(settings, dry_run)
}

/// Publish one request without acquiring the archive/profile lock or launching a browser.
pub fn request(settings: &Settings, dry_run: bool) -> Result<Value> {
    validate_root(settings)?;
    let mut temporary = tempfile::NamedTempFile::new_in(&settings.archive_dir)?;
    let request_id = temporary
        .path()
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let request = Request {
        request_id: request_id.clone(),
        dry_run,
        rules: settings.rules.clone(),
    };
    serde_json::to_writer(&mut temporary, &request)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    ensure!(
        temporary.as_file().metadata()?.len() <= 1024 * 1024,
        "Reprocess request is too large"
    );
    temporary
        .persist_noclobber(settings.archive_dir.join(REQUEST))
        .map_err(|_| {
            anyhow::anyhow!("Cannot queue reprocess request; check for a pending request")
        })?;
    #[cfg(unix)]
    std::fs::File::open(&settings.archive_dir)?.sync_all()?;
    Ok(json!({"state":"queued","request_id":request_id,"dry_run":dry_run}))
}

/// Only aggregate results are returned; the queued rule snapshot stays private.
pub fn status(settings: &Settings) -> Result<Value> {
    validate_root(settings)?;
    let pending = read_private::<Request>(&settings.archive_dir.join(REQUEST))?;
    let last_result = read_private::<Value>(&settings.archive_dir.join(STATUS))?;
    Ok(
        json!({"pending":pending.is_some(),"request_id":pending.map(|request| request.request_id),"last_result":last_result}),
    )
}

/// Called only while the archive lock is held. Requests survive daemon downtime/crashes.
pub(crate) fn process_pending(archive: &Archive, active: &mut Settings) -> Result<()> {
    let path = active.archive_dir.join(REQUEST);
    let request = match read_private::<Request>(&path) {
        Ok(None) => return Ok(()),
        Ok(Some(request)) => request,
        Err(_) => {
            write_status(
                &active.archive_dir,
                &json!({"state":"failed","reason":"invalid_request"}),
            )?;
            std::fs::remove_file(path)?;
            return Ok(());
        }
    };
    let valid_id = request.request_id.len() <= 64
        && !request.request_id.is_empty()
        && request
            .request_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-');
    if !valid_id {
        write_status(
            &active.archive_dir,
            &json!({"state":"failed","reason":"invalid_request"}),
        )?;
        std::fs::remove_file(path)?;
        return Ok(());
    }
    let mut settings = active.clone();
    settings.rules = request.rules;
    write_status(
        &active.archive_dir,
        &json!({"state":"running","request_id":request.request_id,"dry_run":request.dry_run}),
    )?;
    let result = match archive.reprocess(&settings, request.dry_run) {
        Ok(report) => {
            if !request.dry_run {
                active.rules = settings.rules;
            }
            json!({"state":"completed","request_id":request.request_id,"report":report})
        }
        Err(_) => {
            json!({"state":"failed","request_id":request.request_id,"reason":"reprocess_failed"})
        }
    };
    write_status(&active.archive_dir, &result)?;
    std::fs::remove_file(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const MIME: &[u8] =
        b"From: robot@example.test\r\nDate: Sun, 25 Oct 2026 10:00:00 +0000\r\nSubject: Prometheus synthetic\r\n\r\nBody\r\n";

    fn setup(root: &Path) -> (Archive, Settings, Settings) {
        let active = Settings {
            archive_dir: root.join("archive"),
            ..Default::default()
        };
        let archive = Archive::open(&active).unwrap();
        archive
            .store(&active, "synthetic", MIME, Some(100))
            .unwrap();
        let proposed = Settings {
            rules: vec![Rule {
                category: "monitoring".into(),
                header: "subject".into(),
                contains: "prometheus".into(),
                ..Default::default()
            }],
            ..active.clone()
        };
        (archive, active, proposed)
    }

    #[test]
    fn owner_consumes_durable_snapshot_and_uses_rules_for_new_messages() {
        let dir = tempfile::tempdir().unwrap();
        let (archive, mut active, mut proposed) = setup(dir.path());
        let queued = request(&proposed, false).unwrap();
        assert!(
            request(&proposed, false).is_err(),
            "a pending request cannot be overwritten"
        );
        assert_eq!(status(&active).unwrap()["pending"], true);
        // Changing the caller's settings cannot change an already queued snapshot.
        proposed.rules.clear();
        drop(archive);
        let archive = Archive::open(&active).unwrap();
        process_pending(&archive, &mut active).unwrap();
        let result = status(&active).unwrap();
        assert_eq!(result["pending"], false);
        assert_eq!(result["last_result"]["request_id"], queued["request_id"]);
        assert_eq!(result["last_result"]["state"], "completed");
        assert_eq!(result["last_result"]["report"]["moved"], 1);
        assert_eq!(active.rules[0].category, "monitoring");
        let next = b"From: robot@example.test\r\nDate: Sun, 25 Oct 2026 10:00:00 +0000\r\nSubject: Prometheus next\r\n\r\nBody\r\n";
        archive.store(&active, "next", next, None).unwrap();
        assert_eq!(
            std::fs::read_dir(active.archive_dir.join("messages/monitoring"))
                .unwrap()
                .count(),
            2
        );
        assert!(archive.contains("synthetic").unwrap());
        assert!(archive.contains("next").unwrap());
    }

    #[test]
    fn dry_run_does_not_move_files_or_replace_active_rules() {
        let dir = tempfile::tempdir().unwrap();
        let (archive, mut active, proposed) = setup(dir.path());
        request(&proposed, true).unwrap();
        process_pending(&archive, &mut active).unwrap();
        let result = status(&active).unwrap();
        assert_eq!(result["last_result"]["state"], "completed");
        assert_eq!(result["last_result"]["report"]["dry_run"], true);
        assert_eq!(result["last_result"]["report"]["moved"], 1);
        assert!(active.rules.is_empty());
        assert!(!active.archive_dir.join("messages/monitoring").exists());
        assert!(archive.contains("synthetic").unwrap());
    }

    #[test]
    fn failed_requests_report_no_private_content_and_allow_retry() {
        let dir = tempfile::tempdir().unwrap();
        let (archive, mut active, mut proposed) = setup(dir.path());
        proposed.rules[0].category = "../synthetic-private-value".into();
        assert!(request(&proposed, false).is_err());
        let path = active.archive_dir.join(REQUEST);
        temporary_json(
            &active.archive_dir,
            &json!({"synthetic-private-value":"not a request"}),
        )
        .unwrap()
        .persist(&path)
        .unwrap();
        process_pending(&archive, &mut active).unwrap();
        let result = status(&active).unwrap();
        assert_eq!(result["last_result"]["state"], "failed");
        assert!(!result.to_string().contains("synthetic-private-value"));
        assert!(!path.exists());
        assert!(archive.contains("synthetic").unwrap());
    }

    #[test]
    fn integrity_failure_leaves_rules_unchanged_and_can_be_retried() {
        let dir = tempfile::tempdir().unwrap();
        let (archive, mut active, proposed) = setup(dir.path());
        let path = std::fs::read_dir(active.archive_dir.join("messages/other"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(&path, b"synthetic-private-corruption").unwrap();
        request(&proposed, false).unwrap();
        process_pending(&archive, &mut active).unwrap();
        let result = status(&active).unwrap();
        assert_eq!(result["pending"], false);
        assert_eq!(result["last_result"]["state"], "failed");
        assert!(!result.to_string().contains("synthetic-private-corruption"));
        assert!(active.rules.is_empty());
        std::fs::write(path, MIME).unwrap();
        request(&proposed, false).unwrap();
        process_pending(&archive, &mut active).unwrap();
        assert_eq!(
            status(&active).unwrap()["last_result"]["state"],
            "completed"
        );
        assert_eq!(active.rules[0].category, "monitoring");
    }
}
