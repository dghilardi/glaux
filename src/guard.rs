//! Keeps an automatic passkey authenticator available only while the user's local
//! graphical session is active and unlocked. Fails closed when the state is unknown.
use crate::shutdown::Shutdown;
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::process::Command;

pub fn validate_unit(unit: &str) -> Result<()> {
    ensure!(
        unit.len() <= 128
            && unit.strip_suffix(".service").is_some_and(|name| {
                !name.is_empty()
                    && !name.starts_with('-')
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"@._-".contains(&b))
            }),
        "Authenticator unit must be a systemd .service name"
    );
    Ok(())
}

/// `loginctl show-session -p Active -p LockedHint -p Remote` output.
pub fn session_unlocked(properties: &str) -> bool {
    let value = |key: &str| {
        properties
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
    };
    value("Active") == Some("yes")
        && value("LockedHint") == Some("no")
        && value("Remote") == Some("no")
}

async fn output(program: &str, args: &[&str]) -> Option<String> {
    let run = Command::new(program).args(args).kill_on_drop(true).output();
    let out = tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .ok()?
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn identifier(raw: &str) -> Option<&str> {
    let value = raw.trim();
    (!value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric())).then_some(value)
}

async fn unlocked() -> bool {
    // A user service is outside any login session: name the user explicitly.
    let Some(uid) = output("id", &["-u"]).await else {
        return false;
    };
    let Some(uid) = identifier(&uid) else {
        return false;
    };
    let display = ["show-user", uid, "-p", "Display", "--value"];
    let Some(session) = output("loginctl", &display).await else {
        return false;
    };
    let Some(session) = identifier(&session) else {
        return false;
    };
    let properties = ["-p", "Active", "-p", "LockedHint", "-p", "Remote"];
    let mut args = vec!["show-session", session];
    args.extend(properties);
    output("loginctl", &args)
        .await
        .is_some_and(|p| session_unlocked(&p))
}

async fn set_unit(unit: &str, running: bool) -> bool {
    let action = if running { "start" } else { "stop" };
    output("systemctl", &["--user", action, unit])
        .await
        .is_some()
}

pub async fn run(unit: &str, poll: Duration) -> Result<Value> {
    validate_unit(unit)?;
    let mut shutdown = Shutdown::new()?;
    // Unknown initial state: apply the first decision unconditionally.
    let mut applied: Option<bool> = None;
    let mut reported = None;
    loop {
        let wanted = unlocked().await;
        if applied != Some(wanted) {
            let ok = set_unit(unit, wanted).await;
            if reported != Some((wanted, ok)) {
                eprintln!(
                    "{}",
                    json!({"event":"authenticator","unit":unit,"running":wanted,"ok":ok})
                );
                reported = Some((wanted, ok));
            }
            applied = ok.then_some(wanted);
        }
        tokio::select! {
            _ = shutdown.wait() => break,
            _ = tokio::time::sleep(poll) => {}
        }
    }
    let stopped = set_unit(unit, false).await;
    Ok(json!({"stopped":true,"authenticator_stopped":stopped}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_active_unlocked_local_session() {
        assert!(session_unlocked("Active=yes\nLockedHint=no\nRemote=no\n"));
        for properties in [
            "Active=yes\nLockedHint=yes\nRemote=no\n",
            "Active=no\nLockedHint=no\nRemote=no\n",
            "Active=yes\nLockedHint=no\nRemote=yes\n",
            "Active=yes\nRemote=no\n",
            "",
            "XActive=yes\nLockedHint=no\nRemote=no\n",
        ] {
            assert!(!session_unlocked(properties), "{properties:?}");
        }
    }

    #[test]
    fn accepts_only_service_names() {
        assert!(validate_unit("passless.service").is_ok());
        assert!(validate_unit("passless@glaux.service").is_ok());
        for unit in [
            "",
            ".service",
            "passless",
            "passless.timer",
            "-x.service",
            "../passless.service",
            "pass less.service",
        ] {
            assert!(validate_unit(unit).is_err(), "{unit:?}");
        }
    }
}
