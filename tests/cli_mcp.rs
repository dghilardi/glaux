use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

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
        drop(stdin);
        child.wait().await.unwrap();
    }).await.unwrap();
}
