//! Minimal sequential CDP transport. Network credentials stay in memory and are never logged.
use crate::config::trusted_url;
use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{collections::HashMap, time::Duration};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

#[derive(Clone, Default)]
pub struct OwaContext {
    pub endpoint: String,
    pub headers: HashMap<String, String>,
    pub request_header: Option<Value>,
}

pub struct Cdp {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    pub context: Option<OwaContext>,
    requests: HashMap<String, Value>,
    extra_headers: HashMap<String, Value>,
}

impl Cdp {
    pub async fn connect(url: &str) -> Result<Self> {
        let (socket, _) = tokio_tungstenite::connect_async(url)
            .await
            .context("Cannot connect to browser debugger")?;
        Ok(Self {
            socket,
            next_id: 0,
            context: None,
            requests: HashMap::new(),
            extra_headers: HashMap::new(),
        })
    }
    pub async fn command(&mut self, method: &str, params: Value) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.socket
            .send(Message::Text(
                json!({"id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .await?;
        tokio::time::timeout(Duration::from_secs(45), async {
            loop {
                let value = self.receive().await?;
                if value["id"].as_u64() == Some(id) {
                    // CDP exception text can include evaluated JS and credentials: do not print it.
                    if value.get("error").is_some() {
                        bail!("Browser command failed ({method})");
                    }
                    return Ok(value["result"].clone());
                }
            }
        })
        .await
        .context("Browser command timed out")?
    }
    pub async fn evaluate(&mut self, expression: &str) -> Result<Value> {
        let result = self
            .command(
                "Runtime.evaluate",
                json!({"expression":expression,"awaitPromise":true,"returnByValue":true}),
            )
            .await?;
        if result.get("exceptionDetails").is_some() {
            bail!("Browser script failed; no sensitive diagnostic was logged");
        }
        result["result"]
            .get("value")
            .cloned()
            .context("Browser returned no value")
    }
    pub async fn receive(&mut self) -> Result<Value> {
        loop {
            let message = self
                .socket
                .next()
                .await
                .context("Browser closed before completing the operation")??;
            match message {
                Message::Text(text) => {
                    let value: Value =
                        serde_json::from_str(&text).context("Invalid debugger response")?;
                    self.observe(&value);
                    return Ok(value);
                }
                Message::Ping(data) => self.socket.send(Message::Pong(data)).await?,
                Message::Close(_) => bail!("Browser was closed"),
                _ => {}
            }
        }
    }
    fn observe(&mut self, event: &Value) {
        let params = &event["params"];
        let id = params["requestId"].as_str().unwrap_or_default().to_owned();
        match event["method"].as_str() {
            Some("Network.requestWillBeSent") => {
                let request = &params["request"];
                if self.requests.len() > 512 {
                    self.requests.clear();
                    self.extra_headers.clear();
                }
                self.capture(request, &request["headers"]);
                if let Some(headers) = self.extra_headers.remove(&id) {
                    self.capture(request, &headers);
                }
                self.requests.insert(id, request.clone());
            }
            Some("Network.requestWillBeSentExtraInfo") => {
                if let Some(request) = self.requests.get(&id).cloned() {
                    self.capture(&request, &params["headers"]);
                } else {
                    if self.extra_headers.len() > 512 {
                        self.extra_headers.clear();
                    }
                    self.extra_headers.insert(id, params["headers"].clone());
                }
            }
            _ => {}
        }
    }
    fn capture(&mut self, request: &Value, headers: &Value) {
        let Some(raw) = request["url"].as_str() else {
            return;
        };
        let Ok(mut url) = trusted_url(raw) else {
            return;
        };
        if !(url.path().starts_with("/owa/") || url.path().starts_with("/ows/"))
            || !url.path().ends_with("/service.svc")
        {
            return;
        }
        let mut captured = HashMap::new();
        // ExtraInfo may contain Authorization while the main event contains the canary.
        for source in [&request["headers"], headers] {
            let Some(source) = source.as_object() else {
                continue;
            };
            for (name, value) in source {
                let lower = name.to_ascii_lowercase();
                if ["authorization", "x-owa-canary", "x-owa-sessionid"].contains(&lower.as_str())
                    && let Some(value) = value.as_str()
                {
                    captured.insert(lower, value.to_owned());
                }
            }
        }
        if !captured
            .get("authorization")
            .is_some_and(|s| s.starts_with("Bearer "))
        {
            return;
        }
        url.set_query(None);
        url.set_fragment(None);
        let request_header = request["postData"]
            .as_str()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|v| v.get("Header").cloned());
        if let Some(current) = self.context.as_mut().filter(|c| c.endpoint == url.as_str()) {
            current.headers.extend(captured);
            if request_header.is_some() {
                current.request_header = request_header;
            }
        } else {
            self.context = Some(OwaContext {
                endpoint: url.to_string(),
                headers: captured,
                request_header,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Exercise event reordering with an actual loopback websocket, without corporate credentials.
    #[tokio::test]
    async fn captures_extra_info_only_for_trusted_owa() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            for value in [
                json!({"method":"Network.requestWillBeSentExtraInfo","params":{"requestId":"1","headers":{"Authorization":"Bearer fixture"}}}),
                json!({"method":"Network.requestWillBeSent","params":{"requestId":"1","request":{"url":"https://outlook.office.com/owa/service.svc?action=FindItem","headers":{}}}}),
                json!({"method":"Network.requestWillBeSent","params":{"requestId":"2","request":{"url":"https://evil.test/owa/service.svc","headers":{"Authorization":"Bearer wrong"}}}}),
            ] {
                ws.send(Message::Text(value.to_string().into()))
                    .await
                    .unwrap();
            }
        });
        let mut cdp = Cdp::connect(&format!("ws://{addr}")).await.unwrap();
        for _ in 0..3 {
            cdp.receive().await.unwrap();
        }
        let ctx = cdp.context.unwrap();
        assert_eq!(ctx.endpoint, "https://outlook.office.com/owa/service.svc");
        assert_eq!(ctx.headers["authorization"], "Bearer fixture");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn waits_for_bearer_and_merges_canary_from_main_event() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            for value in [
                json!({"method":"Network.requestWillBeSent","params":{"requestId":"1","request":{"url":"https://outlook.office.com/owa/service.svc?action=FindItem","headers":{"X-OWA-CANARY":"fixture-canary"}}}}),
                json!({"method":"Network.requestWillBeSentExtraInfo","params":{"requestId":"1","headers":{"Authorization":"Bearer fixture"}}}),
            ] {
                ws.send(Message::Text(value.to_string().into()))
                    .await
                    .unwrap();
            }
        });
        let mut cdp = Cdp::connect(&format!("ws://{addr}")).await.unwrap();
        cdp.receive().await.unwrap();
        assert!(
            cdp.context.is_none(),
            "a canary alone must not race ahead of the bearer event"
        );
        cdp.receive().await.unwrap();
        let ctx = cdp.context.unwrap();
        assert_eq!(ctx.headers["authorization"], "Bearer fixture");
        assert_eq!(ctx.headers["x-owa-canary"], "fixture-canary");
        server.await.unwrap();
    }
}
