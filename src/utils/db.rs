use anyhow::{anyhow, Result};
use base64::{engine::general_purpose, Engine as _};
use rusqlite::Connection;
use std::path::Path;
use x25519_dalek::PublicKey;

pub fn get_public_key(db_path: &Path) -> Result<PublicKey> {
    let conn = Connection::open(db_path)?;

    let mut stmt = conn.prepare("SELECT value FROM Config WHERE name='public_key'")?;

    let key: String = stmt.query_row([], |row| row.get(0))?;

    let key = general_purpose::STANDARD.decode(key)?;

    if key.len() != 32 {
        return Err(anyhow!("Invalid private key length"));
    }

    // Convert Vec<u8> to [u8; 32]
    let key_array: [u8; 32] = key
        .try_into()
        .map_err(|_| anyhow!("Failed to convert key to 32-byte array"))?;

    Ok(PublicKey::from(key_array))
}
