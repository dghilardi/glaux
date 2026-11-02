pub mod archive;
pub mod browser;
pub mod cdp;
pub mod compose;
pub mod config;
mod draft;
pub mod export;
pub mod guard;
pub mod login;
pub mod mcp;
pub mod owa;
mod render;
pub mod reprocess;
pub mod retention;
pub mod service;
pub mod shutdown;

use anyhow::Result;
use serde_json::{Value, json};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    PreviewEmail {
        email: compose::EmailInput,
    },
    CreateDraft {
        email: compose::EmailInput,
        signature_draft_id: String,
    },
    UpdateDraft {
        email: compose::EmailInput,
        signature_draft_id: String,
        id: String,
        change_key: String,
    },
    List {
        folder: String,
        limit: u32,
        offset: u32,
    },
    Read {
        id: String,
    },
    #[serde(skip)]
    Export {
        id: String,
        output: std::path::PathBuf,
    },
    #[doc(hidden)]
    ReadMime {
        id: String,
    },
    Status,
}

/// Validate locally, before profile access or forwarding to the session owner.
pub(crate) fn validate_operation(
    config: &config::Config,
    operation: &Operation,
) -> Result<Option<Value>> {
    match operation {
        Operation::PreviewEmail { email } => {
            return Ok(Some(
                json!({"email":compose::prepare(email)?,"signature":"not_applied","warnings":["Local preview excludes the Outlook template signature and images; these are included when saving a draft.","Inline presentation does not guarantee identical rendering or dark-mode colors across mail clients. Wide tables retain their structure; prefer sections or lists on small screens."]}),
            ));
        }
        Operation::CreateDraft {
            email,
            signature_draft_id,
        } => {
            config.mail_write_policy.require_drafts()?;
            compose::prepare(email)?;
            owa::read_request(signature_draft_id)?;
        }
        Operation::UpdateDraft {
            email,
            signature_draft_id,
            id,
            change_key,
        } => {
            config.mail_write_policy.require_drafts()?;
            compose::prepare(email)?;
            owa::read_request(signature_draft_id)?;
            owa::read_request(id)?;
            owa::validate_change_key(change_key)?;
            anyhow::ensure!(
                id != signature_draft_id,
                "signature_template_is_read_only: update a copy, not the signature template"
            );
        }
        Operation::List {
            folder,
            limit,
            offset,
        } => {
            owa::list_request(folder, *limit, *offset)?;
        }
        Operation::Read { id } | Operation::ReadMime { id } => {
            owa::read_request(id)?;
        }
        Operation::Export { id, output } => {
            owa::mime_request(id)?;
            export::validate_output(output)?;
        }
        Operation::Status => {}
    }
    Ok(None)
}

/// Execute against the caller's authenticated browser without taking ownership of it.
pub(crate) async fn execute_in_browser(
    config: &config::Config,
    browser: &mut browser::Browser,
    operation: Operation,
) -> Result<Value> {
    if let Some(local) = validate_operation(config, &operation)? {
        return Ok(local);
    }
    match operation {
        Operation::PreviewEmail { .. } => unreachable!(),
        Operation::CreateDraft {
            email,
            signature_draft_id,
        } => owa::create_draft(browser, config, &email, &signature_draft_id).await,
        Operation::UpdateDraft {
            email,
            signature_draft_id,
            id,
            change_key,
        } => {
            owa::update_draft(
                browser,
                config,
                &email,
                &signature_draft_id,
                &id,
                &change_key,
            )
            .await
        }
        Operation::List {
            folder,
            limit,
            offset,
        } => Ok(serde_json::to_value(
            owa::list(browser, config, &folder, limit, offset).await?,
        )?),
        Operation::Read { id } => Ok(serde_json::to_value(
            owa::read(browser, config, &id).await?,
        )?),
        Operation::ReadMime { id } => {
            use base64::Engine;
            Ok(
                json!({"mime":base64::engine::general_purpose::STANDARD.encode(owa::read_mime(browser, config, &id).await?)}),
            )
        }
        Operation::Export { id, output } => {
            let bytes = owa::read_mime(browser, config, &id).await?;
            export::publish(&output, &bytes)?;
            Ok(json!({"exported":true,"bytes":bytes.len(),"format":"eml"}))
        }
        Operation::Status => {
            // Observing a bearer alone isn't proof that mailbox reads work.
            owa::list(browser, config, "inbox", 1, 0).await?;
            Ok(json!({"authenticated":true,"mailbox_readable":true}))
        }
    }
}

/// Prefer the daemon for online operations; never retry a submitted service call.
pub async fn execute(config: &config::Config, operation: Operation) -> Result<Value> {
    if let Some(local) = validate_operation(config, &operation)? {
        return Ok(local);
    }
    if !config.standalone {
        if let Operation::Export { id, output } = &operation {
            if let Some(result) =
                service::forward(config, &Operation::ReadMime { id: id.clone() }).await?
            {
                use base64::Engine;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(
                        result["mime"]
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("invalid_service_mime"))?,
                    )
                    .map_err(|_| anyhow::anyhow!("invalid_service_mime"))?;
                owa::validate_mime(&bytes)?;
                export::publish(output, &bytes)?;
                return Ok(json!({"exported":true,"bytes":bytes.len(),"format":"eml"}));
            }
        } else if let Some(result) = service::forward(config, &operation).await? {
            return Ok(result);
        }
    }
    execute_standalone(config, operation).await
}

/// Every standalone operation closes Chromium on success, errors and signals.
pub async fn execute_standalone(config: &config::Config, operation: Operation) -> Result<Value> {
    if let Some(local) = validate_operation(config, &operation)? {
        return Ok(local);
    }
    service::require_idle_profile(config)?;
    let mut shutdown = shutdown::Shutdown::new()?;
    let mut browser = browser::Browser::launch(config, config.headed).await?;
    let result = tokio::select! {
        biased;
        _ = shutdown.wait() => Err(anyhow::anyhow!("Operation interrupted")),
        result = async {
            browser.authenticate(config).await?;
            execute_in_browser(config, &mut browser, operation).await
        } => result,
    };
    browser.shutdown().await;
    result
}

/// Mail content is untrusted: never emit terminal escape sequences or bidi controls.
pub fn terminal_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| {
            (!c.is_control() || *c == '\n' || *c == '\t')
                && !matches!(*c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        .collect()
}
