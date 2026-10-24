use crate::{
    cdp::Cdp,
    config::{Config, find_browser},
};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{fs::File, process::Stdio, time::Duration};
use tokio::{
    process::{Child, Command},
    time::{Instant, sleep},
};

/// Owns only the browser started for this operation. The profile outlives this process.
pub struct Browser {
    pub cdp: Cdp,
    child: Child,
    _lock: File,
}

impl Browser {
    pub async fn launch(config: &Config, headed: bool) -> Result<Self> {
        let lock = config.lock()?;
        let browser_path = find_browser(config.browser.as_ref())?;
        let active_port = config.profile().join("DevToolsActivePort");
        if active_port.exists() {
            std::fs::remove_file(&active_port)?;
        }
        let mut command = Command::new(browser_path);
        command
            .arg(format!("--user-data-dir={}", config.profile().display()))
            .args([
                "--remote-debugging-address=127.0.0.1",
                "--remote-debugging-port=0",
                "--no-first-run",
                "--no-default-browser-check",
                "--restore-last-session",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if !headed {
            command.arg("--headless=new");
        }
        #[cfg(target_os = "linux")]
        if headed {
            use crate::config::BrowserPlatform;
            // XWayland also presents a normal desktop window. Native Wayland startup can
            // stall in some Chromium/KDE combinations before the debugger is available.
            let platform = match config.browser_platform {
                BrowserPlatform::X11 => Some("x11"),
                BrowserPlatform::Wayland => Some("wayland"),
                BrowserPlatform::Auto if std::env::var_os("DISPLAY").is_some() => Some("x11"),
                BrowserPlatform::Auto => None,
            };
            if let Some(platform) = platform {
                command.arg(format!("--ozone-platform={platform}"));
            }
        }
        command.arg("about:blank");
        let mut child = command.spawn().context("Cannot start Chromium")?;
        // kill_on_drop also covers every startup error, so no orphan keeps the profile locked.
        let deadline = Instant::now() + Duration::from_secs(20);
        let port = loop {
            if child.try_wait()?.is_some() {
                bail!(
                    "Chromium exited at startup. Check --browser, graphical session for login, and profile ownership"
                );
            }
            if let Ok(content) = std::fs::read_to_string(&active_port)
                && let Some(port) = content
                    .lines()
                    .next()
                    .and_then(|v| v.parse::<u16>().ok())
                    .filter(|p| *p != 0)
            {
                break port;
            }
            if Instant::now() > deadline {
                bail!("Chromium debugger did not become available within 20 seconds");
            }
            sleep(Duration::from_millis(100)).await;
        };
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(5))
            .build()?;
        let targets: Vec<Value> = http
            .get(format!("http://127.0.0.1:{port}/json/list"))
            .send()
            .await?
            .json()
            .await?;
        let target = targets
            .iter()
            .find(|t| t["type"] == "page")
            .context("Chromium has no page target")?;
        let socket = target["webSocketDebuggerUrl"]
            .as_str()
            .context("Chromium debugger URL missing")?;
        let socket_url = url::Url::parse(socket)?;
        if socket_url.scheme() != "ws"
            || socket_url.host_str() != Some("127.0.0.1")
            || socket_url.port() != Some(port)
        {
            bail!("Browser returned a non-loopback debugger address");
        }
        let mut cdp = Cdp::connect(socket).await?;
        cdp.command("Network.enable", json!({})).await?;
        cdp.command("Page.enable", json!({})).await?;
        Ok(Self {
            cdp,
            child,
            _lock: lock,
        })
    }

    pub async fn authenticate(&mut self, config: &Config) -> Result<()> {
        self.cdp.context = None;
        self.cdp
            .command("Page.navigate", json!({"url":config.url.as_str()}))
            .await?;
        let timeout = Duration::from_secs(config.timeout_secs);
        tokio::time::timeout(timeout, async {
            while self.cdp.context.is_none() { self.cdp.receive().await?; }
            Ok::<_, anyhow::Error>(())
        }).await.context("No authenticated OWA request observed. Run `glaux login` to complete sign-in/MFA; if already signed in, OWA may have changed its protocol")??;
        Ok(())
    }

    pub async fn shutdown(mut self) {
        // Ask Chromium to flush cookies/storage before releasing the profile lock.
        let _ = tokio::time::timeout(
            Duration::from_secs(3),
            self.cdp.command("Browser.close", json!({})),
        )
        .await;
        if tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.kill().await;
        }
    }
}
