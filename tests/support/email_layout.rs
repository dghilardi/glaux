//! Layout checks are Chromium evidence only, not Outlook client certification.
use super::*;

#[tokio::test]
#[ignore = "requires Chromium; synthetic responsive email fixture only"]
async fn rendered_email_fits_mobile_and_desktop_without_remote_resources() {
    let input = serde_json::from_value(json!({
        "subject":"Synthetic update",
        "theme":{"accent":"#123456","text":"#292929","link":"#342156","border":"#aabbcc","decoration":"#bbccdd"},
        "html":format!("<h1>Project update</h1><p>Summary of completed work.</p><h2>Next steps</h2><ul><li>Review</li><li>Validation</li></ul><table><caption>Progress</caption><tr><th scope=\"col\">Task</th><th scope=\"col\">State</th></tr><tr><td>Implementation</td><td>Ready</td></tr></table><p><a href=\"https://example.test/report\">https://example.test/{}</a></p><pre>{}</pre>", "long".repeat(40), "code".repeat(40))
    })).unwrap();
    let prepared = glaux::compose::prepare(&input).unwrap();
    let body = prepared.html;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let body = body.clone();
            tokio::spawn(async move {
                let mut buffer = [0; 4096];
                let _ = stream.read(&mut buffer).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });
    let directory = tempfile::tempdir().unwrap();
    let config = Config::new(
        Some(directory.path().join("layout")),
        None,
        DEFAULT_URL,
        false,
        10,
    )
    .unwrap();
    let mut browser = Browser::launch(&config, false).await.unwrap();
    navigate(&mut browser, &url).await;
    for width in [375, 600, 1024] {
        browser
            .cdp
            .command(
                "Emulation.setDeviceMetricsOverride",
                json!({"width":width,"height":900,"deviceScaleFactor":1,"mobile":false}),
            )
            .await
            .unwrap();
        let result = browser.cdp.evaluate("({fits:document.documentElement.scrollWidth <= innerWidth, width:document.querySelector('div[style*=\"max-width:640px\"]').getBoundingClientRect().width, font:getComputedStyle(document.querySelector('div[style*=\"font-family:Arial\"]')).fontSize, link:getComputedStyle(document.querySelector('a')).color, tables:document.querySelectorAll('table:not([role=presentation])').length, remote:performance.getEntriesByType('resource').filter(r=>!r.name.startsWith(location.origin)).length})").await.unwrap();
        assert_eq!(result["fits"], true, "viewport {width}: {result}");
        assert!(result["width"].as_f64().unwrap() <= 640.0);
        assert_eq!(result["font"], "16px");
        assert_eq!(result["link"], "rgb(52, 33, 86)");
        assert_eq!(result["tables"], 1);
        assert_eq!(result["remote"], 0);
    }
    browser.shutdown().await;
    server.abort();
}
