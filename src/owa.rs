//! OWA's internal JSON protocol, not EWS SOAP or Graph. Payloads are isolated here.
use crate::{browser::Browser, config::Config};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailAddress {
    pub name: String,
    pub address: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mail {
    pub id: String,
    pub subject: String,
    pub from: Option<MailAddress>,
    pub received_at: Option<String>,
    pub is_read: bool,
    pub has_attachments: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body_type: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct MailPage {
    pub messages: Vec<Mail>,
    pub total: Option<u64>,
    pub next_offset: Option<u64>,
}

fn envelope(action: &str, body: Value) -> Value {
    json!({
        "__type":format!("{action}JsonRequest:#Exchange"),
        "Header":{"__type":"JsonRequestHeaders:#Exchange","RequestServerVersion":"Exchange2013",
            "TimeZoneContext":{"__type":"TimeZoneContext:#Exchange","TimeZoneDefinition":{"__type":"TimeZoneDefinitionType:#Exchange","Id":"UTC"}}},
        "Body":body
    })
}

pub fn list_request(folder: &str, limit: u32, offset: u32) -> Result<Value> {
    if !(1..=100).contains(&limit) {
        bail!("Limit must be between 1 and 100");
    }
    let parent = if let Some(id) = folder.strip_prefix("id:").filter(|s| !s.is_empty()) {
        json!({"__type":"FolderId:#Exchange","Id":id})
    } else {
        if ![
            "inbox",
            "sentitems",
            "drafts",
            "deleteditems",
            "junkemail",
            "archive",
        ]
        .contains(&folder)
        {
            bail!(
                "Unknown folder; use inbox, sentitems, drafts, deleteditems, junkemail, archive, or id:<OWA-folder-id>"
            );
        }
        json!({"__type":"DistinguishedFolderId:#Exchange","Id":folder})
    };
    Ok(envelope(
        "FindItem",
        json!({
            "__type":"FindItemRequest:#Exchange",
            "ItemShape":{"__type":"ItemResponseShape:#Exchange","BaseShape":"IdOnly"},
            "ParentFolderIds":[parent],"Traversal":"Shallow",
            "Paging":{"__type":"IndexedPageView:#Exchange","BasePoint":"Beginning","Offset":offset,"MaxEntriesReturned":limit},
            "ViewFilter":"All","ClutterFilter":"All","IsWarmUpSearch":0,"ShapeName":"MailListItem",
            "SortOrder":[{"__type":"SortResults:#Exchange","Order":"Descending","Path":{"__type":"PropertyUri:#Exchange","FieldURI":"DateTimeReceived"}}]
        }),
    ))
}

pub fn read_request(id: &str) -> Result<Value> {
    if id.trim().is_empty() || id.len() > 8192 {
        bail!("Invalid OWA message ID");
    }
    Ok(envelope(
        "GetItem",
        json!({
            "__type":"GetItemRequest:#Exchange",
            "ItemShape":{"__type":"ItemResponseShape:#Exchange","BaseShape":"AllProperties","BodyType":"Text"},
            "ItemIds":[{"__type":"ItemId:#Exchange","Id":id}]
        }),
    ))
}

pub fn mime_request(id: &str) -> Result<Value> {
    let mut payload = read_request(id)?;
    payload["Body"]["ItemShape"] = json!({
        "__type":"ItemResponseShape:#Exchange", "BaseShape":"IdOnly", "IncludeMimeContent":true
    });
    Ok(payload)
}

/// Export the server's MIME representation, never synthesize an incomplete .eml.
pub async fn read_mime(browser: &mut Browser, config: &Config, id: &str) -> Result<Vec<u8>> {
    let response = request(browser, config, "GetItem", mime_request(id)?).await?;
    parse_mime(&response, id)
}

fn parse_mime(response: &Value, id: &str) -> Result<Vec<u8>> {
    let item = response_messages(response)?
        .first()
        .and_then(|m| m["Items"].as_array())
        .and_then(|items| items.first())
        .context("OWA returned no MIME item")?;
    if item.pointer("/ItemId/Id").and_then(Value::as_str) != Some(id) {
        bail!("OWA returned a different MIME item");
    }
    let content = item
        .pointer("/MimeContent/Value")
        .or_else(|| item.get("MimeContent"))
        .and_then(Value::as_str)
        .context("OWA did not return MIME content; export compatibility needs checking")?;
    let bytes = STANDARD
        .decode(content)
        .map_err(|_| anyhow::anyhow!("Invalid OWA MIME encoding"))?;
    validate_mime(&bytes)?;
    Ok(bytes)
}

pub fn validate_mime(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || bytes.len() > 10 * 1024 * 1024 {
        bail!("MIME export must contain 1 byte to 10 MiB");
    }
    let mail = mailparse::parse_mail(bytes).map_err(|_| anyhow::anyhow!("Invalid MIME message"))?;
    use mailparse::MailHeaderMap;
    if mail.headers.get_first_value("From").is_none()
        || mail.headers.get_first_value("Date").is_none()
    {
        bail!("MIME export lacks required message headers");
    }
    Ok(())
}

/// Never exposes a general-purpose action, arbitrary URL or JavaScript through MCP.
async fn fetch(browser: &mut Browser, action: &str, mut body: Value) -> Result<Value> {
    if !["FindItem", "GetItem"].contains(&action) {
        bail!("Unsupported read-only action");
    }
    let context = browser
        .cdp
        .context
        .as_ref()
        .context("No authenticated OWA session")?
        .clone();
    if let Some(header) = context.request_header {
        // Keep only version/time-zone metadata, never replay app-specific actions or mailbox identities.
        for key in ["RequestServerVersion", "TimeZoneContext"] {
            if let Some(value) = header.get(key) {
                body["Header"][key] = value.clone();
            }
        }
    }
    let mut headers = context.headers;
    headers.insert(
        "content-type".into(),
        "application/json; charset=utf-8".into(),
    );
    headers.insert("action".into(), action.into());
    headers.insert("x-req-source".into(), "Mail".into());
    let args = json!({"endpoint":context.endpoint,"action":action,"headers":headers,"body":body});
    // JSON serialization is the only way user strings enter JS. Never interpolate IDs as code.
    let script = format!(
        r#"(async (a) => {{
        const url = new URL(a.endpoint);
        if (url.origin !== location.origin) return {{status:0,error:'origin'}};
        url.searchParams.set('action', a.action);
        try {{
            const response = await fetch(url, {{method:'POST', credentials:'include', redirect:'error',
                headers:a.headers, body:JSON.stringify(a.body), signal:AbortSignal.timeout(30000)}});
            const text = await response.text();
            if (text.length > 16 * 1024 * 1024) return {{status:response.status,error:'size'}};
            try {{ return {{status:response.status,data:JSON.parse(text)}}; }}
            catch {{ return {{status:response.status,error:'non-json'}}; }}
        }} catch {{ return {{status:0,error:'network'}}; }}
    }})({args})"#
    );
    browser.cdp.evaluate(&script).await
}

async fn request(
    browser: &mut Browser,
    config: &Config,
    action: &str,
    body: Value,
) -> Result<Value> {
    let mut response = fetch(browser, action, body.clone()).await?;
    if matches!(response["status"].as_u64(), Some(401 | 440)) {
        // One bounded renewal via the real Outlook login/SSO flow, never an MFA bypass.
        browser.authenticate(config).await?;
        response = fetch(browser, action, body).await?;
    }
    match response["status"].as_u64().unwrap_or_default() {
        200..=299 => {}
        401 | 440 => bail!("OWA session expired; run `glaux login`"),
        403 => bail!(
            "OWA denied access (HTTP 403). Run `glaux login`; tenant policy or canary validation may require attention"
        ),
        429 => bail!("OWA throttled the request (HTTP 429). Wait before retrying"),
        0 => bail!("OWA browser request failed (origin mismatch, network failure or timeout)"),
        status => bail!("OWA returned HTTP {status}"),
    }
    if response.get("error").is_some() {
        bail!(
            "OWA returned a non-JSON or oversized response; protocol compatibility needs checking"
        );
    }
    response
        .get("data")
        .cloned()
        .context("OWA returned no data")
}

pub async fn list(
    browser: &mut Browser,
    config: &Config,
    folder: &str,
    limit: u32,
    offset: u32,
) -> Result<MailPage> {
    let payload = list_request(folder, limit, offset)?;
    let response = request(browser, config, "FindItem", payload).await?;
    parse_page(&response)
}

pub async fn read(browser: &mut Browser, config: &Config, id: &str) -> Result<Mail> {
    let response = request(browser, config, "GetItem", read_request(id)?).await?;
    let messages = response_messages(&response)?;
    let item = messages
        .first()
        .and_then(|m| m["Items"].as_array())
        .and_then(|a| a.first())
        .context("OWA returned no message")?;
    parse_mail(item)
}

fn response_messages(value: &Value) -> Result<&Vec<Value>> {
    if value.pointer("/Body/FaultMessage").is_some() || value.pointer("/Body/ErrorCode").is_some() {
        bail!("OWA returned a protocol fault; no response contents were logged");
    }
    let messages = value
        .pointer("/Body/ResponseMessages/Items")
        .and_then(Value::as_array)
        .context("Unsupported OWA response shape: expected Body.ResponseMessages.Items")?;
    if messages.is_empty() {
        bail!("OWA returned no response messages");
    }
    for message in messages {
        if message["ResponseClass"]
            .as_str()
            .is_some_and(|s| s != "Success")
            || message["ResponseCode"]
                .as_str()
                .is_some_and(|s| s != "NoError")
        {
            // Error codes are identifiers; don't print MessageText, which can contain mail/identities.
            let code = message["ResponseCode"].as_str().unwrap_or("UnknownError");
            let code: String = code
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(100)
                .collect();
            bail!("OWA operation failed: {code}");
        }
    }
    Ok(messages)
}

pub fn parse_page(value: &Value) -> Result<MailPage> {
    let messages = response_messages(value)?;
    let root = messages
        .first()
        .and_then(|m| m.get("RootFolder"))
        .context("OWA returned no RootFolder")?;
    let items = root["Items"]
        .as_array()
        .context("OWA returned no Items array")?;
    Ok(MailPage {
        messages: items.iter().map(parse_mail).collect::<Result<_>>()?,
        total: root["TotalItemsInView"].as_u64(),
        next_offset: if root["IncludesLastItemInRange"] == true {
            None
        } else {
            root["IndexedPagingOffset"].as_u64()
        },
    })
}

fn parse_mail(item: &Value) -> Result<Mail> {
    let sender = item.get("From").or_else(|| item.get("Sender"));
    let address = sender
        .and_then(|v| v.get("Mailbox").or(Some(v)))
        .map(|v| MailAddress {
            name: v["Name"].as_str().unwrap_or_default().to_owned(),
            address: v["EmailAddress"].as_str().unwrap_or_default().to_owned(),
        });
    Ok(Mail {
        id: item
            .pointer("/ItemId/Id")
            .and_then(Value::as_str)
            .context("Message has no OWA ItemId")?
            .to_owned(),
        subject: item["Subject"]
            .as_str()
            .unwrap_or("(no subject)")
            .to_owned(),
        from: address,
        received_at: item["DateTimeReceived"].as_str().map(str::to_owned),
        is_read: item["IsRead"].as_bool().unwrap_or(false),
        has_attachments: item["HasAttachments"].as_bool().unwrap_or(false),
        body: item
            .pointer("/Body/Value")
            .and_then(Value::as_str)
            .map(str::to_owned),
        body_type: item
            .pointer("/Body/BodyType")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mime_export_preserves_bytes_and_does_not_change_read_state() {
        let bytes = b"From: sender@example.test\r\nDate: Sun, 25 Oct 2026 10:00:00 +0000\r\nSubject: synthetic\r\n\r\nBody\r\n";
        let response = json!({"Body":{"ResponseMessages":{"Items":[{"ResponseClass":"Success","Items":[{"ItemId":{"Id":"fixture"},"MimeContent":{"Value":STANDARD.encode(bytes)}}]}]}}});
        assert_eq!(parse_mime(&response, "fixture").unwrap(), bytes);
        assert!(parse_mime(&response, "different").is_err());
        let request = mime_request("fixture").unwrap();
        assert_eq!(request["Body"]["ItemShape"]["IncludeMimeContent"], true);
        assert!(request["Body"].get("IsRead").is_none());
        assert!(validate_mime(b"not a message").is_err());
    }
    #[test]
    fn parses_paging_sender_and_opaque_ids() {
        let fixture = json!({"Body":{"ResponseMessages":{"Items":[{"ResponseClass":"Success","ResponseCode":"NoError","RootFolder":{
            "TotalItemsInView":3,"IncludesLastItemInRange":false,"IndexedPagingOffset":1,
            "Items":[{"ItemId":{"Id":"opaque+/="},"Subject":"Test","From":{"Mailbox":{"Name":"Alice","EmailAddress":"alice@example.test"}},"IsRead":true}]
        }}]}}});
        let page = parse_page(&fixture).unwrap();
        assert_eq!(page.next_offset, Some(1));
        assert_eq!(page.messages[0].id, "opaque+/=");
        assert_eq!(
            page.messages[0].from.as_ref().unwrap().address,
            "alice@example.test"
        );
    }
    #[test]
    fn rejects_http_200_with_application_error() {
        assert!(parse_page(&json!({"Body":{"ResponseMessages":{"Items":[{"ResponseClass":"Error","ResponseCode":"ErrorAccessDenied"}]}}})).is_err());
        assert!(parse_page(&json!({"Body":{"ResponseMessages":{"Items":[]}}})).is_err());
    }
    #[test]
    fn bounded_paging_and_ids_are_data() {
        assert!(list_request("inbox", 101, 0).is_err());
        assert!(list_request("inbox", 0, 0).is_err());
        assert!(list_request("not-a-folder", 20, 0).is_err());
        let id = "opaque\" ); throw Error('bad'); //";
        assert_eq!(read_request(id).unwrap()["Body"]["ItemIds"][0]["Id"], id);
    }
    #[test]
    fn text_read_does_not_mark_messages_read() {
        let request = read_request("fixture").unwrap();
        assert_eq!(request["Body"]["ItemShape"]["BodyType"], "Text");
        assert!(request["Body"].get("IsRead").is_none());
    }
}
