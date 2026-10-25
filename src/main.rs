use anyhow::Result;
use clap::{Parser, Subcommand};
use glaux::{
    Operation,
    config::{BrowserPlatform, Config, DEFAULT_URL, find_browser},
    execute, terminal_text,
};
use serde_json::{Value, json};
use std::{io::IsTerminal, path::PathBuf};

#[derive(Parser)]
#[command(
    name = "glaux",
    version,
    about = "Your Microsoft 365 mailbox, from the terminal.",
    long_about = "Read your Microsoft 365 mailbox through an isolated, persistent Outlook Web session. Login opens Chromium for Microsoft SSO/MFA. OWA is an undocumented protocol: live tenant compatibility must be verified."
)]
struct Cli {
    #[arg(
        long,
        global = true,
        env = "GLAUX_BROWSER_PLATFORM",
        value_enum,
        default_value = "auto",
        help = "Linux graphical backend; auto prefers X11/XWayland when DISPLAY exists"
    )]
    browser_platform: BrowserPlatform,
    #[arg(
        long,
        global = true,
        env = "GLAUX_DATA_DIR",
        help = "Dedicated private directory for browser/session state"
    )]
    data_dir: Option<PathBuf>,
    #[arg(
        long,
        global = true,
        env = "GLAUX_BROWSER",
        help = "Absolute path to Chromium/Chrome"
    )]
    browser: Option<PathBuf>,
    #[arg(long, global=true, env="GLAUX_OWA_URL", default_value=DEFAULT_URL)]
    url: String,
    #[arg(
        long,
        global = true,
        help = "Show the browser for mailbox commands too"
    )]
    headed: bool,
    #[arg(long, global=true, value_parser=clap::value_parser!(u64).range(1..=900), help="Authentication wait in seconds (login: 300, other commands: 60)")]
    timeout: Option<u64>,
    #[arg(
        long,
        global = true,
        help = "Machine-readable JSON (also automatic when stdout is piped)"
    )]
    json: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Open Microsoft login/MFA and verify that the inbox is readable.
    Login,
    /// Check the saved session against OWA (headless unless --headed).
    Status,
    /// List messages, newest first. Does not mark messages as read.
    List {
        #[arg(long, default_value = "inbox")]
        folder: String,
        #[arg(long, default_value_t=20, value_parser=clap::value_parser!(u32).range(1..=100))]
        limit: u32,
        #[arg(long, default_value_t = 0)]
        offset: u32,
    },
    /// Read a message by its full OWA ItemId; requests plain text.
    Read { id: String },
    /// Export one server-provided MIME message, including MIME attachments when available.
    Export {
        id: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Remove the local Glaux browser profile. Does not revoke server-side sessions.
    Logout,
    /// Show local configuration diagnostics without signing in.
    Doctor,
    /// Run a read-only MCP server over stdin/stdout. Login separately first.
    Mcp,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let json_output = cli.json || !std::io::stdout().is_terminal();
    if let Err(error) = run(cli, json_output).await {
        // Only controlled errors are emitted; no HTTP bodies, cookies, CDP events or tokens.
        if json_output {
            eprintln!("{}", json!({"ok":false,"error":format!("{error:#}")}));
        } else {
            eprintln!("glaux: {}", terminal_text(&format!("{error:#}")));
        }
        std::process::exit(1);
    }
}

async fn run(cli: Cli, json_output: bool) -> Result<()> {
    let login = matches!(cli.command, Commands::Login);
    let mut config = Config::new(
        cli.data_dir,
        cli.browser,
        &cli.url,
        cli.headed || login,
        cli.timeout.unwrap_or(if login { 300 } else { 60 }),
    )?;
    config.browser_platform = cli.browser_platform;
    match cli.command {
        Commands::Mcp => glaux::mcp::serve(config).await,
        Commands::Doctor => {
            let browser = find_browser(config.browser.as_ref())?;
            output(
                &json!({"browser":browser,"data_dir":config.data_dir,"profile_exists":config.profile().exists(),"owa_url":config.url.as_str(),"live_compatibility":"not checked by doctor"}),
                json_output,
            );
            Ok(())
        }
        Commands::Logout => {
            let _lock = config.lock()?;
            std::fs::remove_dir_all(config.profile())?;
            output(
                &json!({"local_session_removed":true,"server_session_revoked":false}),
                json_output,
            );
            Ok(())
        }
        command => {
            let operation = match command {
                Commands::Login => {
                    eprintln!(
                        "Complete Microsoft sign-in/MFA in the Glaux browser window. Session state stays in the dedicated profile."
                    );
                    Operation::Status
                }
                Commands::Status => Operation::Status,
                Commands::List {
                    folder,
                    limit,
                    offset,
                } => Operation::List {
                    folder,
                    limit,
                    offset,
                },
                Commands::Read { id } => Operation::Read { id },
                Commands::Export { id, output } => Operation::Export { id, output },
                _ => unreachable!(),
            };
            let value = execute(&config, operation).await?;
            output(&value, json_output);
            Ok(())
        }
    }
}

fn output(value: &Value, json_output: bool) {
    if json_output {
        println!("{}", json!({"ok":true,"schema_version":1,"data":value}));
    } else if let Some(messages) = value["messages"].as_array() {
        for mail in messages {
            print_mail(mail, false);
        }
        if let Some(offset) = value["next_offset"].as_u64() {
            println!("Next page: --offset {offset}");
        }
        if messages.is_empty() {
            println!("No messages.");
        }
    } else if value.get("id").is_some() {
        print_mail(value, true);
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_default()
        );
    }
}

fn print_mail(mail: &Value, with_body: bool) {
    let string = |key: &str| terminal_text(mail[key].as_str().unwrap_or_default());
    let from = terminal_text(
        mail.pointer("/from/address")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    println!(
        "{}  {}\n{}\nID: {}\n",
        string("received_at"),
        from,
        string("subject"),
        string("id")
    );
    if with_body {
        println!("{}", string("body"));
    }
}
