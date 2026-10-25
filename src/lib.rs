pub mod browser;
pub mod cdp;
pub mod config;
pub mod export;
pub mod mcp;
pub mod owa;

use anyhow::Result;
use serde_json::{Value, json};

pub enum Operation {
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
    let mut browser = browser::Browser::launch(config, config.headed).await?;
    let result = tokio::select! {
        result = async {
            browser.authenticate(config).await?;
            match operation {
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
        _ = tokio::signal::ctrl_c() => Err(anyhow::anyhow!("Operation interrupted")),
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
