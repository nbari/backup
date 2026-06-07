use anyhow::Result;
use std::{io::Read, path::Path};

/// Get the plain BLAKE3 hash of a file.
///
/// Use this for non-identity integrity checks. Content identifiers stored in the
/// catalog use [`blake3_keyed`] so they reveal nothing without the naming key.
///
/// # Errors
/// Returns an error if the file cannot be opened or read.
pub fn blake3(file_path: &Path) -> Result<String> {
    hash_file(file_path, &mut blake3::Hasher::new())
}

/// Get the keyed BLAKE3 content identifier of a file.
///
/// Keyed with the per-backup naming key so the identifier is opaque to anyone
/// without the key, while staying deterministic for deduplication. The hash
/// covers file content only (never name or path), so identical content under
/// different names still deduplicates.
///
/// # Errors
/// Returns an error if the file cannot be opened or read.
pub fn blake3_keyed(file_path: &Path, key: &[u8; 32]) -> Result<String> {
    hash_file(file_path, &mut blake3::Hasher::new_keyed(key))
}

fn hash_file(file_path: &Path, hasher: &mut blake3::Hasher) -> Result<String> {
    let mut file = std::fs::File::open(file_path)?;
    let mut buf = vec![0_u8; 65_536].into_boxed_slice();
    loop {
        let size = file.read(&mut buf)?;
        if size == 0 {
            break;
        }
        let chunk = buf
            .get(..size)
            .ok_or_else(|| anyhow::anyhow!("Invalid read buffer size"))?;
        hasher.update(chunk);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp(content: &[u8]) -> Result<(tempfile::TempDir, std::path::PathBuf)> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("file.bin");
        std::fs::write(&path, content)?;
        Ok((dir, path))
    }

    #[test]
    fn keyed_hash_is_deterministic_for_same_key() -> Result<()> {
        let (_dir, path) = write_temp(b"identical content")?;
        let key = [9u8; 32];

        assert_eq!(blake3_keyed(&path, &key)?, blake3_keyed(&path, &key)?);

        Ok(())
    }

    #[test]
    fn keyed_hash_differs_across_keys_and_from_plain() -> Result<()> {
        let (_dir, path) = write_temp(b"identical content")?;

        let id_a = blake3_keyed(&path, &[1u8; 32])?;
        let id_b = blake3_keyed(&path, &[2u8; 32])?;
        let plain = blake3(&path)?;

        // Same content, different naming keys -> uncorrelatable identifiers.
        assert_ne!(id_a, id_b);
        // Keyed identifier never matches the bare BLAKE3 hash.
        assert_ne!(id_a, plain);

        Ok(())
    }
}
