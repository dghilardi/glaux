//! Synthetic OWA write fixtures. Never connects to Microsoft or uses a saved account.
use super::navigate;
use base64::{Engine, engine::general_purpose::STANDARD};
use glaux::{
    browser::Browser,
    cdp::OwaContext,
    compose::EmailInput,
    config::{Config, DEFAULT_URL, MailWritePolicy},
    owa,
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
};

const TEMPLATE_MIME: &[u8] = b"From: owner@example.test\r\nDate: Mon, 2 Nov 2026 10:00:00 +0000\r\nContent-Type: multipart/related; boundary=fixture\r\n\r\n--fixture\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><head><style type=\"text/css\">P {margin-top:0;margin-bottom:0;}</style></head><body><p style=\"font-family:Aptos_EmbeddedFont\">Synthetic signature <a href=\"https://example.test/\" target=\"_blank\" rel=\"noopener noreferrer\">Link</a><img src=\"cid:logo@example.test\" alt=\"Logo\"></p></body></html>\r\n--fixture\r\nContent-Type: image/png\r\nContent-ID: <logo@example.test>\r\nContent-Transfer-Encoding: base64\r\n\r\nc3ludGhldGljLWltYWdl\r\n--fixture--\r\n";

struct Fixture {
    items: HashMap<String, Value>,
    writes: usize,
    write_status: u16,
    corrupt_readback: bool,
    conflict_on_update: bool,
    write_error_code: Option<&'static str>,
}

impl Fixture {
    fn new() -> Self {
        let item = json!({"ItemId":{"Id":"signature-template","ChangeKey":"template-version"},"IsDraft":true,"ToRecipients":[],"CcRecipients":[],"BccRecipients":[],"MimeContent":{"Value":STANDARD.encode(TEMPLATE_MIME)}});
        Self {
            items: HashMap::from([("signature-template".into(), item)]),
            writes: 0,
            write_status: 200,
            corrupt_readback: false,
            conflict_on_update: false,
            write_error_code: None,
        }
    }

    fn respond(&mut self, action: &str, payload: &Value) -> (u16, Value) {
        let body = &payload["Body"];
        let item = match action {
            "GetItem" => {
                let id = body["ItemIds"][0]["Id"].as_str().unwrap();
                self.items.get(id).unwrap().clone()
            }
            "CreateItem" | "UpdateItem" => {
                self.writes += 1;
                assert_eq!(body["MessageDisposition"], "SaveOnly");
                if self.write_status != 200 {
                    return (self.write_status, json!({}));
                }
                if let Some(code) = self.write_error_code {
                    return (
                        200,
                        json!({"Body":{"ResponseMessages":{"Items":[{"ResponseClass":"Error","ResponseCode":code}]}}}),
                    );
                }
                let mut item = if action == "CreateItem" {
                    assert_eq!(body["SavedItemFolderId"]["BaseFolderId"]["Id"], "drafts");
                    body["Items"][0].clone()
                } else {
                    assert_eq!(body["ConflictResolution"], "NeverOverwrite");
                    let change = &body["ItemChanges"][0];
                    assert_eq!(change["ItemId"]["Id"], "created-draft");
                    if self.conflict_on_update {
                        self.items.get_mut("created-draft").unwrap()["ItemId"]["ChangeKey"] =
                            json!("concurrent-version");
                    }
                    if change["ItemId"]["ChangeKey"]
                        != self.items["created-draft"]["ItemId"]["ChangeKey"]
                    {
                        return (
                            200,
                            json!({"Body":{"ResponseMessages":{"Items":[{"ResponseClass":"Error","ResponseCode":"ErrorIrresolvableConflict"}]}}}),
                        );
                    }
                    let mut item = self.items["created-draft"].clone();
                    for update in change["Updates"].as_array().unwrap() {
                        let field = update["Path"]["FieldURI"].as_str().unwrap();
                        assert!(
                            [
                                "MimeContent",
                                "Subject",
                                "ToRecipients",
                                "CcRecipients",
                                "BccRecipients"
                            ]
                            .contains(&field)
                        );
                        item[field] = update["Item"][field].clone();
                    }
                    item
                };
                assert!(item.get("IsRead").is_none());
                item["ItemId"] =
                    json!({"Id":"created-draft","ChangeKey":format!("version-{}", self.writes)});
                item["IsDraft"] = json!(true);
                let response = item.clone();
                // Exchange adds its paragraph reset to stored HTML. Keep this
                // synthetic rewriting separate from the bytes sent by the client.
                let mime = STANDARD
                    .decode(item["MimeContent"]["Value"].as_str().unwrap())
                    .unwrap();
                let parsed = mailparse::parse_mail(&mime).unwrap();
                let html = parsed
                    .parts()
                    .find(|part| part.ctype.mimetype == "text/html")
                    .unwrap()
                    .get_body()
                    .unwrap();
                let encoded = |body: &str| {
                    STANDARD
                        .encode(body)
                        .as_bytes()
                        .chunks(76)
                        .map(|line| format!("{}\r\n", std::str::from_utf8(line).unwrap()))
                        .collect::<String>()
                };
                let rewritten = html.replace("<html>", "<html><head><style type=\"text/css\">P {margin-top:0;margin-bottom:0;}</style></head>");
                let original = String::from_utf8(mime).unwrap();
                let rewritten = original.replace(&encoded(&html), &encoded(&rewritten));
                assert_ne!(
                    rewritten, original,
                    "fixture must exercise Exchange MIME rewriting"
                );
                item["MimeContent"]["Value"] = json!(STANDARD.encode(rewritten));
                if self.corrupt_readback {
                    item["MimeContent"]["Value"] = json!(STANDARD.encode(TEMPLATE_MIME));
                }
                self.items.insert("created-draft".into(), item);
                response
            }
            _ => panic!("Unexpected action in loopback fixture"),
        };
        (
            200,
            json!({"Body":{"ResponseMessages":{"Items":[{"ResponseClass":"Success","ResponseCode":"NoError","Items":[item]}]}}}),
        )
    }
}

async fn serve(stream: &mut tokio::net::TcpStream, state: Arc<Mutex<Fixture>>) {
    let mut request = Vec::new();
    let (end, length) = loop {
        let mut chunk = [0u8; 8192];
        let size = stream.read(&mut chunk).await.unwrap();
        if size == 0 {
            return;
        }
        request.extend_from_slice(&chunk[..size]);
        assert!(request.len() < 1024 * 1024);
        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            let header = std::str::from_utf8(&request[..end]).unwrap();
            let length = header
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                })
                .unwrap_or(0);
            break (end + 4, length);
        }
    };
    while request.len() < end + length {
        let mut chunk = [0u8; 8192];
        let size = stream.read(&mut chunk).await.unwrap();
        if size == 0 {
            return;
        }
        request.extend_from_slice(&chunk[..size]);
        assert!(request.len() < 1024 * 1024);
    }
    let header = std::str::from_utf8(&request[..end]).unwrap();
    let (status, body, content_type) = if header.starts_with("POST ") {
        let payload: Value = serde_json::from_slice(&request[end..end + length]).unwrap();
        let action = header
            .lines()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(name, _)| name.eq_ignore_ascii_case("action"))
                    .map(|(_, value)| value.trim())
            })
            .unwrap();
        let (status, value) = state.lock().await.respond(action, &payload);
        (status, value.to_string(), "application/json")
    } else {
        (
            200,
            "<!doctype html><title>Glaux synthetic OWA</title>".to_owned(),
            "text/html",
        )
    };
    if status == 0 {
        return;
    } // Simulate a lost write response.
    let response = format!(
        "HTTP/1.1 {status} Fixture\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
}

#[tokio::test]
#[ignore = "requires Chromium; tests draft writes and failures through loopback OWA fixtures"]
async fn draft_writes_preserve_templates_reject_conflicts_and_never_retry() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(Fixture::new()));
        let fixture = state.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let state = fixture.clone();
                tokio::spawn(async move {
                    serve(&mut stream, state).await;
                });
            }
        });
        let directory = tempfile::tempdir().unwrap();
        let mut config = Config::new(
            Some(directory.path().join("synthetic-profile")),
            None,
            DEFAULT_URL,
            false,
            1,
        )
        .unwrap();
        // Even a regression in auth renewal must remain on the loopback fixture.
        config.url = url::Url::parse(&url).unwrap();
        let mut browser = Browser::launch(&config, false).await.unwrap();
        navigate(&mut browser, &url).await;
        browser.cdp.context = Some(OwaContext {
            endpoint: format!("{url}owa/service.svc"),
            headers: HashMap::new(),
        });
        let email = EmailInput {
            theme: Default::default(),
            to: vec!["reader@example.test".into()],
            cc: vec!["copy@example.test".into()],
            bcc: vec!["blind@example.test".into()],
            subject: "Synthetic subject".into(),
            html: "<p>Caffè &amp; attività</p>".into(),
        };
        assert!(
            owa::create_draft(&mut browser, &config, &email, "signature-template")
                .await
                .is_err()
        );
        assert_eq!(state.lock().await.writes, 0);
        config.mail_write_policy = MailWritePolicy::Drafts;
        let result = owa::create_draft(&mut browser, &config, &email, "signature-template")
            .await
            .unwrap();
        assert_eq!(result["saved"], true);
        assert_eq!(result["verified"], true);
        assert_eq!(state.lock().await.writes, 1);
        let template = state.lock().await.items["signature-template"].clone();
        assert_eq!(template["ItemId"]["ChangeKey"], "template-version");
        assert_eq!(
            template["MimeContent"]["Value"],
            STANDARD.encode(TEMPLATE_MIME)
        );
        assert!(
            owa::update_draft(
                &mut browser,
                &config,
                &email,
                "signature-template",
                "signature-template",
                "template-version"
            )
            .await
            .is_err()
        );
        assert!(
            owa::update_draft(
                &mut browser,
                &config,
                &email,
                "signature-template",
                "created-draft",
                "stale-version"
            )
            .await
            .is_err()
        );
        assert_eq!(state.lock().await.writes, 1);
        let result = owa::update_draft(
            &mut browser,
            &config,
            &email,
            "signature-template",
            "created-draft",
            "version-1",
        )
        .await
        .unwrap();
        assert_eq!(result["verified"], true);
        assert_eq!(result["change_key"], "version-2");
        let before_conflict = state.lock().await.items["created-draft"]["MimeContent"].clone();
        state.lock().await.conflict_on_update = true;
        let error = owa::update_draft(
            &mut browser,
            &config,
            &email,
            "signature-template",
            "created-draft",
            "version-2",
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("draft_conflict"));
        assert!(!error.to_string().contains("draft_save_unconfirmed"));
        assert_eq!(state.lock().await.writes, 3);
        assert_eq!(
            state.lock().await.items["created-draft"]["MimeContent"],
            before_conflict
        );
        state.lock().await.conflict_on_update = false;
        for status in [401, 440, 429, 0] {
            let before = state.lock().await.writes;
            state.lock().await.write_status = status;
            let error = owa::create_draft(&mut browser, &config, &email, "signature-template")
                .await
                .unwrap_err();
            assert!(error.to_string().contains("draft_save_unconfirmed"));
            assert_eq!(state.lock().await.writes, before + 1);
        }
        state.lock().await.write_status = 200;
        for (code, outcome) in [
            ("ErrorAccessDenied", "draft_save_rejected"),
            ("ErrorTimeoutExpired", "draft_save_unconfirmed"),
            ("ErrorInternalServerError", "draft_save_unconfirmed"),
            (
                "ErrorInternalServerTransientError",
                "draft_save_unconfirmed",
            ),
            ("ErrorServerBusy", "draft_save_unconfirmed"),
            ("ErrorUnknownFixture", "draft_save_unconfirmed"),
        ] {
            let before = state.lock().await.writes;
            state.lock().await.write_error_code = Some(code);
            let error = owa::create_draft(&mut browser, &config, &email, "signature-template")
                .await
                .unwrap_err();
            assert!(error.to_string().contains(outcome));
            assert_eq!(state.lock().await.writes, before + 1);
        }
        state.lock().await.write_error_code = None;
        {
            let mut fixture = state.lock().await;
            fixture.write_status = 200;
            fixture.corrupt_readback = true;
        }
        let result = owa::create_draft(&mut browser, &config, &email, "signature-template")
            .await
            .unwrap();
        assert_eq!(result["saved"], true);
        assert_eq!(result["verified"], false);
        assert!(result["warning"].is_string());
        browser.shutdown().await;
        assert!(config.lock().is_ok());
        server.abort();
    })
    .await
    .unwrap();
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires Chromium; shared socket clients and archive reads use one locked loopback browser"]
async fn socket_clients_and_archive_work_share_the_owned_browser() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(Fixture::new()));
        let fixture = state.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let state = fixture.clone();
                tokio::spawn(async move {
                    serve(&mut stream, state).await;
                });
            }
        });
        let directory = tempfile::tempdir().unwrap();
        let mut config = Config::new(
            Some(directory.path().join("data")),
            None,
            DEFAULT_URL,
            false,
            1,
        )
        .unwrap();
        config.url = url::Url::parse(&url).unwrap(); // Every possible auth renewal stays on loopback.
        config.mail_write_policy = MailWritePolicy::Drafts;
        let mut service = glaux::service::Service::bind(&config)
            .await
            .unwrap()
            .unwrap();
        let mut browser = Browser::launch(&config, false).await.unwrap();
        navigate(&mut browser, &url).await;
        browser.cdp.context = Some(OwaContext {
            endpoint: format!("{url}owa/service.svc"),
            headers: HashMap::new(),
        });
        assert!(config.lock().is_err());
        let mut client = config.clone();
        client.browser = Some("/bin/false".into()); // A second browser could not start.
        let (finished, finish) = tokio::sync::oneshot::channel();
        let owner_config = config.clone();
        let owner = tokio::spawn(async move {
            for _ in 0..3 {
                service.process_next(&owner_config, &mut browser).await;
                // Model the archive's MIME fetch between interactive requests.
                assert_eq!(
                    owa::read_mime(&mut browser, &owner_config, "signature-template")
                        .await
                        .unwrap(),
                    TEMPLATE_MIME
                );
                assert!(owner_config.lock().is_err());
            }
            let _ = finish.await;
            browser.shutdown().await;
        });
        let output = directory.path().join("client-export.eml");
        let email = EmailInput {
            theme: glaux::compose::EmailTheme {
                accent: "#123456".into(),
            },
            to: vec![],
            cc: vec![],
            bcc: vec![],
            subject: "Synthetic socket draft".into(),
            html:
                "<h2>Shared session</h2><p><a href=\"https://example.test/report\">Report</a></p>"
                    .into(),
        };
        let (read, draft, export) = tokio::join!(
            glaux::execute(
                &client,
                glaux::Operation::Read {
                    id: "signature-template".into()
                }
            ),
            glaux::execute(
                &client,
                glaux::Operation::CreateDraft {
                    email,
                    signature_draft_id: "signature-template".into()
                }
            ),
            glaux::execute(
                &client,
                glaux::Operation::Export {
                    id: "signature-template".into(),
                    output: output.clone()
                }
            ),
        );
        assert_eq!(read.unwrap()["is_draft"], true);
        assert_eq!(draft.unwrap()["verified"], true);
        assert_eq!(export.unwrap()["exported"], true);
        assert_eq!(std::fs::read(output).unwrap(), TEMPLATE_MIME);
        assert_eq!(state.lock().await.writes, 1);
        finished.send(()).unwrap();
        owner.await.unwrap();
        assert!(config.lock().is_ok());
        server.abort();
    })
    .await
    .unwrap();
}
