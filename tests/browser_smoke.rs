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
    browser.cdp.evaluate("localStorage.setItem('glaux-fixture','persisted'); document.cookie='glaux_fixture=retained; Max-Age=3600; SameSite=Strict; Path=/'; document.cookie='glaux_session=resumed; SameSite=Strict; Path=/'; true").await.unwrap();
    browser.shutdown().await;

    let mut browser = Browser::launch(&config, false).await.unwrap();
    navigate(&mut browser, &url).await;
    let result = browser
        .cdp
        .evaluate("({storage:localStorage.getItem('glaux-fixture'),cookies:document.cookie})")
        .await
        .unwrap();
    browser.shutdown().await;
    assert_eq!(result["storage"], "persisted");
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
