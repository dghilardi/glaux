//! Explicitly run with `cargo test --test browser_smoke -- --ignored`.
//! Tests Glaux's own browser integration, with synthetic data on loopback only.
use glaux::{
    browser::Browser,
    config::{Config, DEFAULT_URL},
};
use serde_json::json;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn navigate(browser: &mut Browser, url: &str) {
    browser
        .cdp
        .command("Page.navigate", json!({"url":url}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let script = format!(
                "location.href === {} && document.readyState === 'complete'",
                serde_json::to_string(url).unwrap()
            );
            if browser.cdp.evaluate(&script).await.ok() == Some(json!(true)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires Chromium; uses the project-native Rust browser launcher"]
async fn browser_profile_persists_cookies_and_storage() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer).await;
                let body = "<!doctype html><title>Glaux local fixture</title>";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let config = Config::new(
        Some(directory.path().join("profile-test")),
        None,
        DEFAULT_URL,
        false,
        10,
    )
    .unwrap();
    let mut browser = Browser::launch(&config, false).await.unwrap();
    navigate(&mut browser, &url).await;
    browser.cdp.evaluate("localStorage.setItem('glaux-fixture','persisted'); sessionStorage.setItem('glaux-tab-fixture','resumed'); document.cookie='glaux_fixture=retained; Max-Age=3600; SameSite=Strict; Path=/'; document.cookie='glaux_session=resumed; SameSite=Strict; Path=/'; true").await.unwrap();
    browser
        .cdp
        .command("Target.createTarget", json!({"url":"about:blank"}))
        .await
        .unwrap();
    browser.shutdown().await;

    let preferences: serde_json::Value = serde_json::from_slice(
        &std::fs::read(config.profile().join("Default/Preferences")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        preferences["profile"]["exit_type"], "Normal",
        "browser shutdown must flush a clean session"
    );

    let mut browser = Browser::launch(&config, false).await.unwrap();
    let targets = browser
        .cdp
        .command("Target.getTargets", json!({}))
        .await
        .unwrap();
    assert_eq!(
        targets["targetInfos"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t["type"] == "page")
            .count(),
        1
    );
    navigate(&mut browser, &url).await;
    let result = browser
        .cdp
        .evaluate("({storage:localStorage.getItem('glaux-fixture'),tab:sessionStorage.getItem('glaux-tab-fixture'),cookies:document.cookie})")
        .await
        .unwrap();
    browser.shutdown().await;
    assert_eq!(result["storage"], "persisted");
    assert_eq!(result["tab"], "resumed");
    assert!(
        result["cookies"]
            .as_str()
            .unwrap()
            .contains("glaux_fixture=retained")
    );
    assert!(
        result["cookies"]
            .as_str()
            .unwrap()
            .contains("glaux_session=resumed"),
        "session cookies must survive browser restart with --restore-last-session"
    );
    // A third acquisition verifies that shutdown released the application lock.
    assert!(config.lock().is_ok());
    server.abort();
}

#[tokio::test]
#[ignore = "requires Chromium; browser cleanup must not depend on a live page target"]
async fn detached_page_still_closes_browser_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let config = Config::new(
        Some(dir.path().join("detached")),
        None,
        DEFAULT_URL,
        false,
        10,
    )
    .unwrap();
    let mut browser = Browser::launch(&config, false).await.unwrap();
    let target = browser
        .cdp
        .command("Target.getTargetInfo", json!({}))
        .await
        .unwrap();
    browser
        .cdp
        .command("Target.createTarget", json!({"url":"about:blank"}))
        .await
        .unwrap();
    let _ = browser
        .cdp
        .command(
            "Target.closeTarget",
            json!({"targetId":target["targetInfo"]["targetId"]}),
        )
        .await;
    browser.shutdown().await;
    let preferences: serde_json::Value = serde_json::from_slice(
        &std::fs::read(config.profile().join("Default/Preferences")).unwrap(),
    )
    .unwrap();
    assert_eq!(preferences["profile"]["exit_type"], "Normal");
    assert!(config.lock().is_ok());
}

#[tokio::test]
#[ignore = "requires a graphical desktop and Chromium; tests the Glaux login launcher"]
async fn headed_browser_starts_with_desktop_backend() {
    let directory = tempfile::tempdir().unwrap();
    let config = Config::new(
        Some(directory.path().join("headed-test")),
        None,
        DEFAULT_URL,
        true,
        10,
    )
    .unwrap();
    let mut browser = Browser::launch(&config, true).await.unwrap();
    let value = browser.cdp.evaluate("1 + 1").await.unwrap();
    browser.shutdown().await;
    assert_eq!(value, json!(2));
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires Chromium; checks signal handling during delayed browser startup"]
async fn interrupt_during_startup_closes_the_owned_browser() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let wrapper = dir.path().join("delayed-browser");
    let ready = dir.path().join("ready");
    std::fs::write(
        &wrapper,
        "#!/bin/sh\ntouch \"$GLAUX_LAUNCH_READY\"\nsleep 1\nexec \"$GLAUX_REAL_BROWSER\" \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let data = dir.path().join("data");
    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_glaux"))
        .arg("--data-dir")
        .arg(&data)
        .arg("--browser")
        .arg(&wrapper)
        .arg("status")
        .env(
            "GLAUX_REAL_BROWSER",
            glaux::config::find_browser(None).unwrap(),
        )
        .env("GLAUX_LAUNCH_READY", &ready)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !ready.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let sent = tokio::process::Command::new("kill")
        .args(["-INT", &child.id().unwrap().to_string()])
        .status()
        .await
        .unwrap();
    assert!(sent.success());
    let output = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Operation interrupted"));
    let config = Config::new(Some(data), None, DEFAULT_URL, false, 10).unwrap();
    assert!(config.lock().is_ok());
    let port = std::fs::read_to_string(config.profile().join("DevToolsActivePort")).unwrap();
    let address = format!(
        "http://127.0.0.1:{}/json/list",
        port.lines().next().unwrap()
    );
    assert!(
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(address)
            .send()
            .await
            .is_err()
    );
}
