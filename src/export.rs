use anyhow::{Result, ensure};
use std::{io::Write, path::Path};

pub fn validate_output(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "Use an absolute export path");
    ensure!(
        path.extension().is_some_and(|e| e == "eml"),
        "Export path must end in .eml"
    );
    ensure!(!path.try_exists()?, "Export destination already exists");
    ensure!(
        path.parent().is_some_and(Path::is_dir),
        "Export parent directory must exist"
    );
    Ok(())
}

/// Atomic, private publication; an existing destination is never overwritten.
pub fn publish(path: &Path, bytes: &[u8]) -> Result<()> {
    validate_output(path)?;
    let mut temporary = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist_noclobber(path)?;
    #[cfg(unix)]
    std::fs::File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn publication_is_private_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mail.eml");
        publish(&path, b"synthetic").unwrap();
        assert!(publish(&path, b"replacement").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"synthetic");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
