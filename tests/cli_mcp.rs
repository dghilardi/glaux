use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

#[tokio::test]
async fn invalid_sync_settings_do_not_open_a_browser() {
    let directory = tempfile::tempdir().unwrap();
    let settings = directory.path().join("sync.toml");
    std::fs::write(&settings, "archive_dir = 'relative/path'\n").unwrap();
    let data = directory.path().join("data");
    let output = Command::new(env!("CARGO_BIN_EXE_glaux"))
        .arg("--data-dir")
        .arg(&data)
        .args(["--browser", "/bin/false", "sync", "--config"])
        .arg(settings)
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!data.exists());
}

#[tokio::test]
async fn doctor_is_local_and_cli_rejects_untrusted_hosts() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_glaux"))
        .args([
            "--data-dir",
            dir.path().to_str().unwrap(),
            "--browser",
            "/bin/false",
            "doctor",
        ])
        .output()
        .await
        .unwrap();
    assert!(output.status.success());
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["data"]["profile_exists"], false);
    // Doctor must not have launched /bin/false or created browser state.
    assert!(!dir.path().join("browser-profile").exists());

    let output = Command::new(env!("CARGO_BIN_EXE_glaux"))
        .args(["--url", "https://evil.test", "status"])
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Only HTTPS"));
}

#[tokio::test]
async fn mcp_stdio_discovery_and_errors_without_opening_browser() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let dir = tempfile::tempdir().unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_glaux"))
            .args(["--data-dir",dir.path().to_str().unwrap(),"--browser","/bin/false","mcp"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
        let initialize = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"glaux-fixture","version":"1"}}});
        stdin.write_all(format!("{initialize}\n").as_bytes()).await.unwrap();
        let response: Value = serde_json::from_str(&stdout.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(response["id"], 1);
        assert!(response.get("result").is_some(), "{response}");
        stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n").await.unwrap();
        stdin.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n").await.unwrap();
        let response: Value = serde_json::from_str(&stdout.next_line().await.unwrap().unwrap()).unwrap();
        let tools = response["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert!(tools.iter().all(|tool| tool["annotations"]["readOnlyHint"] == true));
        assert!(!dir.path().join("browser-profile").exists());

        let invalid_call = json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_messages","arguments":{"limit":1000}}});
        stdin.write_all(format!("{invalid_call}\n").as_bytes()).await.unwrap();
        let response: Value = serde_json::from_str(&stdout.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(response["result"]["isError"], true);
        assert!(!dir.path().join("browser-profile").exists());
        #[cfg(unix)] {
            let call = json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_messages","arguments":{"limit":1}}});
            stdin.write_all(format!("{call}\n").as_bytes()).await.unwrap();
            let response: Value=serde_json::from_str(&stdout.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(response["result"]["isError"],true);
            let signal = Command::new("kill").args(["-TERM", &child.id().unwrap().to_string()]).status().await.unwrap();
            assert!(signal.success());
        }
        drop(stdin);
        child.wait().await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn reprocess_local_and_queue_commands_do_not_launch_a_browser() {
    let directory = tempfile::tempdir().unwrap();
    let settings = glaux::archive::Settings {
        archive_dir: directory.path().join("archive"),
        ..Default::default()
    };
    let archive = glaux::archive::Archive::open(&settings).unwrap();
    archive.store(&settings, "synthetic", b"From: robot@example.test\r\nDate: Sun, 25 Oct 2026 10:00:00 +0000\r\nSubject: Prometheus synthetic\r\n\r\nBody\r\n", None).unwrap();
    drop(archive);
    let path = directory.path().join("sync.toml");
    std::fs::write(&path, format!("archive_dir = {:?}\n[[rules]]\ncategory = 'monitoring'\nheader = 'subject'\ncontains = 'prometheus'\n", settings.archive_dir)).unwrap();
    let data = directory.path().join("data");
    for flags in [
        vec!["--local", "--dry-run"],
        vec!["--local"],
        vec!["--dry-run"],
        vec!["--status"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_glaux"))
            .arg("--data-dir")
            .arg(&data)
            .args(["--browser", "/bin/false", "reprocess", "--config"])
            .arg(&path)
            .args(&flags)
            .output()
            .await
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let response: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(response["ok"], true);
        if flags.contains(&"--local") {
            assert_eq!(response["data"]["moved"], 1);
        }
        assert!(
            !data.exists(),
            "local commands must not create browser state"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn daemon_handles_reprocess_requests_during_browser_failure_backoff() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let directory = tempfile::tempdir().unwrap();
        let settings = glaux::archive::Settings { archive_dir: directory.path().join("archive"), ..Default::default() };
        let archive = glaux::archive::Archive::open(&settings).unwrap();
        archive.store(&settings, "synthetic", b"From: robot@example.test\r\nDate: Sun, 25 Oct 2026 10:00:00 +0000\r\nSubject: Prometheus synthetic\r\n\r\nBody\r\n", None).unwrap();
        drop(archive);
        let path = directory.path().join("sync.toml");
        std::fs::write(&path, format!("archive_dir = {:?}\n", settings.archive_dir)).unwrap();
        let mut daemon = Command::new(env!("CARGO_BIN_EXE_glaux"))
            .arg("--data-dir").arg(directory.path().join("data"))
            .args(["--browser", "/bin/false", "daemon", "--interval", "30", "--config"])
            .arg(&path).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true).spawn().unwrap();
        loop {
            if settings.archive_dir.join("status.json").exists() { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let proposed = glaux::archive::Settings {
            rules: vec![glaux::archive::Rule { category: "monitoring".into(), header: "subject".into(), contains: "prometheus".into(), ..Default::default() }],
            ..settings.clone()
        };
        assert!(glaux::archive::Archive::open(&settings).is_err(), "daemon owns the archive");
        glaux::reprocess::request(&proposed, false).unwrap();
        loop {
            let result = glaux::reprocess::status(&proposed).unwrap();
            if result["last_result"]["state"] == "completed" && result["pending"] == false {
                assert_eq!(result["last_result"]["report"]["moved"], 1);
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let signal = Command::new("kill").args(["-TERM", &daemon.id().unwrap().to_string()]).status().await.unwrap();
        assert!(signal.success());
        assert!(daemon.wait().await.unwrap().success());
        let archive = glaux::archive::Archive::open(&proposed).unwrap();
        assert!(archive.contains("synthetic").unwrap());
        assert_eq!(std::fs::read_dir(settings.archive_dir.join("messages/monitoring")).unwrap().count(), 1);
    }).await.unwrap();
}
