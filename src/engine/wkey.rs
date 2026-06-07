//! Persistent cache for the per-backup naming key (`{name}.wkey`).
//!
//! The naming key keys the BLAKE3 content identifiers. It is cached here so
//! `run` (e.g. from `cron`) does not need the mnemonic on every invocation. The
//! file is created with owner-only permissions and holds **only** the naming
//! key — never the content private key or mnemonic. It is a convenience cache:
//! the authoritative copy is sealed inside the `.db` and recoverable from the
//! mnemonic, so deleting it just forces a one-time mnemonic prompt on the next
//! run.

use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

#[must_use]
pub fn wkey_path(config_dir: &Path, name: &str) -> PathBuf {
    config_dir.join(format!("{name}.wkey"))
}

/// Load the cached naming key for a backup, if the cache file exists.
///
/// # Errors
/// Returns an error if the file exists but cannot be read or decoded.
pub fn load_naming_key(config_dir: &Path, name: &str) -> Result<Option<Zeroizing<[u8; 32]>>> {
    let path = wkey_path(config_dir, name);

    if !path.exists() {
        return Ok(None);
    }

    let encoded = Zeroizing::new(std::fs::read_to_string(&path)?);
    let decoded = Zeroizing::new(general_purpose::STANDARD.decode(encoded.trim())?);

    let key: [u8; 32] = decoded
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("Invalid naming key length in {}", path.display()))?;

    Ok(Some(Zeroizing::new(key)))
}

/// Write the naming key cache for a backup with owner-only permissions.
///
/// # Errors
/// Returns an error if the cache file cannot be written.
pub fn write_naming_key(config_dir: &Path, name: &str, naming_key: &[u8; 32]) -> Result<()> {
    let path = wkey_path(config_dir, name);
    let encoded = Zeroizing::new(general_purpose::STANDARD.encode(naming_key));

    write_owner_only(&path, encoded.as_bytes())
}

#[cfg(unix)]
fn write_owner_only(path: &Path, data: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;

    file.write_all(data)?;

    Ok(())
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, data: &[u8]) -> Result<()> {
    std::fs::write(path, data)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_naming_key() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let name = "demo";

        assert!(load_naming_key(dir.path(), name)?.is_none());

        let key = [7u8; 32];
        write_naming_key(dir.path(), name, &key)?;

        let loaded = load_naming_key(dir.path(), name)?.ok_or_else(|| anyhow!("missing cache"))?;
        assert_eq!(*loaded, key);

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn cache_is_owner_only() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir()?;
        write_naming_key(dir.path(), "demo", &[1u8; 32])?;

        let mode = std::fs::metadata(wkey_path(dir.path(), "demo"))?
            .permissions()
            .mode();

        assert_eq!(mode & 0o777, 0o600);

        Ok(())
    }
}
