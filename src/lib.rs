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
pub mod reprocess;
pub mod retention;
pub mod shutdown;

use anyhow::Result;
use serde_json::{Value, json};

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
    Export {
        id: String,
        output: std::path::PathBuf,
    },
    Status,
}

/// Every operation closes Chromium gracefully, including authentication/OWA errors.
pub async fn execute(config: &config::Config, operation: Operation) -> Result<Value> {
    // Validate before opening a browser or accessing an account.
    match &operation {
        Operation::PreviewEmail { email } => {
            return Ok(
                json!({"email":compose::prepare(email)?,"signature":"not_applied","warnings":["Local preview excludes the Outlook template signature and images; these are included when saving a draft."]}),
            );
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
        Operation::Read { id } => {
            owa::read_request(id)?;
        }
        Operation::Export { id, output } => {
            owa::mime_request(id)?;
            export::validate_output(output)?;
        }
        Operation::Status => {}
    }
    let mut shutdown = shutdown::Shutdown::new()?;
    let mut browser = browser::Browser::launch(config, config.headed).await?;
    let result = tokio::select! {
        biased;
        _ = shutdown.wait() => Err(anyhow::anyhow!("Operation interrupted")),
        result = async {
            browser.authenticate(config).await?;
            match operation {
                Operation::PreviewEmail { .. } => unreachable!(),
                Operation::CreateDraft { email, signature_draft_id } => {
                    owa::create_draft(&mut browser, config, &email, &signature_draft_id).await
                },
                Operation::UpdateDraft { email, signature_draft_id, id, change_key } => {
                    owa::update_draft(&mut browser, config, &email, &signature_draft_id, &id, &change_key).await
                },
                Operation::List { folder, limit, offset } => Ok(serde_json::to_value(owa::list(&mut browser, config, &folder, limit, offset).await?)?),
                Operation::Read { id } => Ok(serde_json::to_value(owa::read(&mut browser, config, &id).await?)?),
                Operation::Export { id, output } => {
                    let bytes = owa::read_mime(&mut browser, config, &id).await?;
                    export::publish(&output, &bytes)?;
                    Ok(json!({"exported":true,"bytes":bytes.len(),"format":"eml"}))
                },
                Operation::Status => {
                    // Observing a bearer alone isn't proof that mailbox reads work.
                    owa::list(&mut browser, config, "inbox", 1, 0).await?;
                    Ok(json!({"authenticated":true,"mailbox_readable":true}))
                },
            }
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
