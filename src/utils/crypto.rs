use anyhow::{anyhow, Result};
use bip39::Mnemonic;
use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    ChaCha20Poly1305, Key, Nonce,
};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};

pub fn generate_file_key() -> [u8; 32] {
    let mut key = [0u8; 32];

    let mut rng = rand::rng();

    rng.fill_bytes(&mut key);

    key
}

pub fn encrypt(file_key: &[u8; 32], public_key: &PublicKey) -> Result<(Vec<u8>, [u8; 32])> {
    // 1. generate ephemeral key pair
    let e_secret = EphemeralSecret::random();
    let e_public = PublicKey::from(&e_secret);

    // 2. shared secret
    let shared_secret = e_secret.diffie_hellman(public_key);

    // 3. Derive KEK (key encryption key) via HKDF
    let mut kek = [0; 32];
    Hkdf::<Sha256>::new(None, shared_secret.as_bytes())
        .expand(b"backup wrap", &mut kek)
        .map_err(|err| anyhow!("Error during KEK HKDF expansion: {}", err))?;

    // Encrypt file key using ChaCha20Poly1305
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&kek));

    // Generate a random nonce and use it as part of the ciphertext (prefix)
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);

    cipher.encrypt(&nonce, file_key.as_ref()).map_or_else(
        |_| Err(anyhow!("Failed to encrypt data")),
        |ciphertext| {
            let mut encrypted_data = nonce.to_vec();
            encrypted_data.extend_from_slice(&ciphertext);
            Ok((encrypted_data, e_public.to_bytes()))
        },
    )
}

// decrypt using mnemonic/seed (private key)
fn _decrypt(
    encrypted_data: &[u8],
    eph_pub_bytes: &[u8; 32],
    mnemonic: &Mnemonic,
) -> Result<Vec<u8>> {
    if encrypted_data.len() < 12 {
        return Err(anyhow::anyhow!("Encrypted data too short to contain nonce"));
    }

    println!("encrypted data length: {}", encrypted_data.len());

    // derive private key again from mnemonic
    let seed = mnemonic.to_seed("");

    let mut seed_bytes = [0u8; 32];

    seed_bytes.copy_from_slice(&seed[0..32]);

    let private_key = StaticSecret::from(seed_bytes);

    let eph_pub = PublicKey::from(*eph_pub_bytes);

    // shared secret
    let shared = private_key.diffie_hellman(&eph_pub);

    // derive KEK
    let mut kek = [0u8; 32];
    Hkdf::<Sha256>::new(None, shared.as_bytes())
        .expand(b"backup wrap", &mut kek)
        .unwrap();

    let cipher = ChaCha20Poly1305::new(Key::from_slice(&kek));
    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher.decrypt(nonce, ciphertext.as_ref()).unwrap();

    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bip39::Language;

    #[test]
    fn test_encrypt_decrypt() {
        let mnemonic = Mnemonic::generate_in(Language::English, 24).unwrap();

        let seed = mnemonic.to_seed("");

        let mut seed_bytes = [0u8; 32];

        seed_bytes.copy_from_slice(&seed[0..32]);

        let private_key = StaticSecret::from(seed_bytes);

        let public_key = PublicKey::from(&private_key);

        let message = generate_file_key();

        let (encrypted_data, eph_pub_bytes) = encrypt(&message, &public_key).unwrap();

        let decrypted_message = _decrypt(&encrypted_data, &eph_pub_bytes, &mnemonic).unwrap();

        assert_eq!(hex::encode(message), hex::encode(decrypted_message));
    }
}
