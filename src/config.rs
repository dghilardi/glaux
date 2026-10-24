use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    path::PathBuf,
};
use url::Url;

pub const DEFAULT_URL: &str = "https://outlook.office.com/mail/";
pub const HOSTS: &[&str] = &[
    "outlook.office.com",
    "outlook.office365.com",
    "outlook.cloud.microsoft",
];

#[derive(Clone, Copy, Default, clap::ValueEnum)]
pub enum BrowserPlatform {
    #[default]
    Auto,
    X11,
    Wayland,
}

pub fn trusted_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).context("Invalid Outlook URL")?;
    if url.scheme() != "https"
        || !HOSTS.contains(&url.host_str().unwrap_or_default())
        || url.port().is_some_and(|p| p != 443)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!(
            "Only HTTPS Microsoft 365 Outlook hosts are supported (office.com, office365.com, cloud.microsoft)"
        );
    }
    Ok(url)
}

#[derive(Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub browser: Option<PathBuf>,
    pub url: Url,
    pub headed: bool,
    pub timeout_secs: u64,
    pub browser_platform: BrowserPlatform,
}

impl Config {
    pub fn new(
        data_dir: Option<PathBuf>,
        browser: Option<PathBuf>,
        url: &str,
        headed: bool,
        timeout_secs: u64,
    ) -> Result<Self> {
        let data_dir = data_dir
            .or_else(|| {
                ProjectDirs::from("dev", "glaux", "glaux").map(|d| d.data_local_dir().to_owned())
            })
            .context("Cannot determine data directory; use --data-dir")?;
        Ok(Self {
            data_dir,
            browser,
            url: trusted_url(url)?,
            headed,
            timeout_secs,
            browser_platform: BrowserPlatform::Auto,
        })
    }
    pub fn profile(&self) -> PathBuf {
        self.data_dir.join("browser-profile")
    }
    pub fn lock(&self) -> Result<File> {
        private_dir(&self.data_dir)?;
        private_dir(&self.profile())?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(self.data_dir.join("profile.lock"))?;
        file.try_lock_exclusive()
            .context("Glaux profile is already in use; close the other CLI/MCP process")?;
        Ok(file)
    }
}

pub fn private_dir(path: &std::path::Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn find_browser(explicit: Option<&PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Ok(path.clone());
        }
        bail!("Browser executable not found; pass an absolute path with --browser");
    }
    for name in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
        "chrome",
        "msedge",
    ] {
        if let Some(paths) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&paths) {
                let path = dir.join(name);
                if path.is_file() {
                    return Ok(path);
                }
            }
        }
    }
    for path in [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    ] {
        if std::path::Path::new(path).is_file() {
            return Ok(path.into());
        }
    }
    bail!("Chromium/Chrome not found. Install a browser or set GLAUX_BROWSER to its absolute path")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_untrusted_targets() {
        for raw in [
            "http://outlook.office.com",
            "https://outlook.office.com.evil.test",
            "https://evil.test",
            "https://user@outlook.office.com",
            "https://outlook.office.com:1234",
        ] {
            assert!(trusted_url(raw).is_err(), "{raw}");
        }
        assert!(trusted_url(DEFAULT_URL).is_ok());
    }
    #[test]
    fn excludes_concurrent_profile_access() {
        let dir = tempfile::tempdir().unwrap();
        let cfg =
            Config::new(Some(dir.path().join("glaux")), None, DEFAULT_URL, false, 10).unwrap();
        let lock = cfg.lock().unwrap();
        assert!(cfg.lock().is_err());
        drop(lock);
        assert!(cfg.lock().is_ok());
    }
}
