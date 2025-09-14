use anyhow::{anyhow, Result};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key,
};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;
use x25519_dalek::{EphemeralSecret, PublicKey};

pub fn generate_file_key() -> [u8; 32] {
    let mut key = [0u8; 32];

    let mut rng = rand::rng();

    rng.fill_bytes(&mut key);

    key
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
