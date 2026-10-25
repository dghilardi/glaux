//! Register before browser startup so systemd stop/Ctrl-C cannot orphan Chromium.
use anyhow::Result;

pub struct Shutdown {
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
}

impl Shutdown {
    pub fn new() -> Result<Self> {
        Ok(Self {
            #[cfg(unix)]
            interrupt: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?,
            #[cfg(unix)]
            terminate: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?,
        })
    }

    pub async fn wait(&mut self) {
        #[cfg(unix)]
        tokio::select! { _ = self.interrupt.recv() => {}, _ = self.terminate.recv() => {} }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
    }
}
