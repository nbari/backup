use anyhow::Result;
use std::{io::Read, path::Path};

/// Get blake3 hash of a file
pub fn blake3(file_path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(file_path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0_u8; 65536];
    while let Ok(size) = file.read(&mut buf[..]) {
        if size == 0 {
            break;
        }
        hasher.update(&buf[0..size]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}
