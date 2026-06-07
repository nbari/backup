use anyhow::Result;
use std::{io::Read, path::Path};

/// Get blake3 hash of a file
/// # Errors
/// Returns an error if the file cannot be opened or read.
pub fn blake3(file_path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(file_path)?;
    let mut hasher = blake3::Hasher::new();
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
