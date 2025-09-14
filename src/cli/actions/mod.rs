pub mod new;
pub mod run;
pub mod show;

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose, Engine as _};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key,
};
use hkdf::Hkdf;
use rand::RngCore;
use rusqlite::Connection;
use sha2::Sha256;
use std::path::{Path, PathBuf};
use x25519_dalek::{EphemeralSecret, PublicKey};

#[derive(Debug)]
pub enum Action {
    New {
        name: String,
        directory: Option<Vec<PathBuf>>,
        file: Option<Vec<PathBuf>>,
        config: PathBuf,
    },
    Show,
    Run {
        name: String,
        no_gitignore: bool,
        no_compression: bool,
        no_encryption: bool,
        dry_run: bool,
    },
}

pub fn generate_file_key() -> [u8; 32] {
    let mut key = [0u8; 32];

    let mut rng = rand::rng();

    rng.fill_bytes(&mut key);

    key
}

fn get_public_key(db_path: &Path) -> Result<PublicKey> {
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

pub fn kek_wrap(
    file_key: &[u8; 32],
    hash: &str,
    public_key: &PublicKey,
) -> Result<(Vec<u8>, [u8; 32])> {
    // generate ephemeral key pair
    let e_secret = EphemeralSecret::random();
    let e_public = PublicKey::from(&e_secret);

    let shared_secret = e_secret.diffie_hellman(public_key);

    // Derive KEK (key encryption key) via HKDF
    let mut kek = [0; 32];
    Hkdf::<Sha256>::new(None, shared_secret.as_bytes())
        .expand(b"backup wrap", &mut kek)
        .map_err(|err| anyhow!("Error during KEK HKDF expansion: {}", err))?;

    let mut nonce = [0u8; 12];
    Hkdf::<Sha256>::new(None, hash.as_bytes())
        .expand(b"backup nonce", &mut nonce)
        .map_err(|err| anyhow!("Error during nonce HKDF expansion: {}", err))?;

    // Encrypt file key using ChaCha20Poly1305
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&kek));
    let wrapped = cipher
        .encrypt(&nonce.into(), file_key.as_ref())
        .map_err(|err| anyhow!("Error during file key encryption: {}", err))?;

    Ok((wrapped, e_public.to_bytes()))
}
