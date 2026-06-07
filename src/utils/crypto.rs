use anyhow::{Result, anyhow};
use chacha20poly1305::{
    ChaCha20Poly1305, Key,
    aead::{Aead, AeadCore, KeyInit, OsRng},
};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;
use x25519_dalek::{EphemeralSecret, PublicKey, StaticSecret};
use zeroize::Zeroizing;

/// HKDF label for the X25519 content keypair derived from the mnemonic seed.
const CONTENT_KEY_INFO: &[u8] = b"backup x25519 content v1";
/// HKDF label for the key-wrapping KEK derived from an ECDH shared secret.
const WRAP_KEK_INFO: &[u8] = b"backup wrap";

#[must_use]
pub fn generate_file_key() -> Zeroizing<[u8; 32]> {
    let mut key = Zeroizing::new([0u8; 32]);

    // `rand::rng()` is an OS-seeded CSPRNG; adequate for one-shot key material.
    rand::rng().fill_bytes(key.as_mut());

    key
}

/// Generate a random 32-byte naming key used to key content identifiers.
#[must_use]
pub fn generate_naming_key() -> Zeroizing<[u8; 32]> {
    generate_file_key()
}

/// Derive the X25519 content keypair from a BIP-39 mnemonic.
///
/// The full 64-byte seed is run through HKDF-SHA256 with a domain-separation
/// label, so the entire seed entropy is used (rather than truncating it) and
/// other subkeys can be derived from the same seed without collision.
///
/// # Errors
/// Returns an error if key derivation fails.
pub fn content_keypair(mnemonic: &bip39::Mnemonic) -> Result<(StaticSecret, PublicKey)> {
    let seed = Zeroizing::new(mnemonic.to_seed(""));

    let mut sk_bytes = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(None, seed.as_slice())
        .expand(CONTENT_KEY_INFO, sk_bytes.as_mut())
        .map_err(|err| anyhow!("Error during content key HKDF expansion: {err}"))?;

    let secret = StaticSecret::from(*sk_bytes);
    let public = PublicKey::from(&secret);

    Ok((secret, public))
}

/// Encrypt (wrap) a 32-byte key for the provided public key.
///
/// Returns the wrapped payload (`nonce || ciphertext`) and the ephemeral public
/// key needed to unwrap it.
///
/// # Errors
/// Returns an error if key derivation or encryption fails.
pub fn encrypt(key: &[u8; 32], public_key: &PublicKey) -> Result<(Vec<u8>, [u8; 32])> {
    // 1. generate ephemeral key pair
    let e_secret = EphemeralSecret::random();
    let e_public = PublicKey::from(&e_secret);

    // 2. shared secret
    let shared_secret = e_secret.diffie_hellman(public_key);

    // 3. Derive KEK (key encryption key) via HKDF
    let mut kek = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(None, shared_secret.as_bytes())
        .expand(WRAP_KEK_INFO, kek.as_mut())
        .map_err(|err| anyhow!("Error during KEK HKDF expansion: {err}"))?;

    // Encrypt the key using ChaCha20Poly1305
    let cipher = ChaCha20Poly1305::new(Key::from_slice(kek.as_ref()));

    // Generate a random nonce and use it as part of the ciphertext (prefix)
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);

    cipher.encrypt(&nonce, key.as_ref()).map_or_else(
        |_| Err(anyhow!("Failed to encrypt data")),
        |ciphertext| {
            let mut encrypted_data = nonce.to_vec();
            encrypted_data.extend_from_slice(&ciphertext);
            Ok((encrypted_data, e_public.to_bytes()))
        },
    )
}

/// Decrypt (unwrap) a wrapped key using the recovery mnemonic.
///
/// # Errors
/// Returns an error if the encrypted payload is malformed or key unwrapping fails.
pub fn decrypt(
    encrypted_data: &[u8],
    eph_pub_bytes: &[u8; 32],
    mnemonic: &bip39::Mnemonic,
) -> Result<Zeroizing<Vec<u8>>> {
    if encrypted_data.len() < 12 {
        return Err(anyhow!("Encrypted data too short to contain nonce"));
    }

    // Derive the private key from the mnemonic (same derivation as content_keypair).
    let (private_key, _) = content_keypair(mnemonic)?;

    let eph_pub = PublicKey::from(*eph_pub_bytes);

    // shared secret
    let shared = private_key.diffie_hellman(&eph_pub);

    // derive KEK
    let mut kek = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(None, shared.as_bytes())
        .expand(WRAP_KEK_INFO, kek.as_mut())
        .map_err(|err| anyhow!("Error during KEK HKDF expansion: {err}"))?;

    let cipher = ChaCha20Poly1305::new(Key::from_slice(kek.as_ref()));
    let (nonce_bytes, ciphertext) = encrypted_data.split_at(12);
    let nonce = chacha20poly1305::Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|_| anyhow!("Failed to decrypt data"))?;

    Ok(Zeroizing::new(plaintext))
}

/// Seal a naming key to the backup public key.
///
/// The returned blob is `ephemeral_public_key (32 bytes) || wrapped_key`, ready
/// to be stored in the catalog.
///
/// # Errors
/// Returns an error if sealing fails.
pub fn seal_naming_key(naming_key: &[u8; 32], public_key: &PublicKey) -> Result<Vec<u8>> {
    let (wrapped, eph_pub) = encrypt(naming_key, public_key)?;

    let mut sealed = Vec::with_capacity(eph_pub.len() + wrapped.len());
    sealed.extend_from_slice(&eph_pub);
    sealed.extend_from_slice(&wrapped);

    Ok(sealed)
}

/// Recover a naming key sealed with [`seal_naming_key`] using the recovery mnemonic.
///
/// # Errors
/// Returns an error if the sealed blob is malformed or unsealing fails.
pub fn unseal_naming_key(sealed: &[u8], mnemonic: &bip39::Mnemonic) -> Result<Zeroizing<[u8; 32]>> {
    let eph_pub: [u8; 32] = sealed
        .get(..32)
        .ok_or_else(|| anyhow!("Sealed naming key too short"))?
        .try_into()
        .map_err(|_| anyhow!("Invalid ephemeral public key length"))?;

    let wrapped = sealed
        .get(32..)
        .ok_or_else(|| anyhow!("Sealed naming key missing payload"))?;

    let plaintext = decrypt(wrapped, &eph_pub, mnemonic)?;

    let key: [u8; 32] = plaintext
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("Unsealed naming key has unexpected length"))?;

    Ok(Zeroizing::new(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bip39::{Language, Mnemonic};

    #[test]
    fn test_encrypt_decrypt() -> Result<()> {
        let mnemonic = Mnemonic::generate_in(Language::English, 24)?;
        let (_, public_key) = content_keypair(&mnemonic)?;

        let message = generate_file_key();

        let (encrypted_data, eph_pub_bytes) = encrypt(&message, &public_key)?;

        let decrypted_message = decrypt(&encrypted_data, &eph_pub_bytes, &mnemonic)?;

        assert_eq!(
            hex::encode(*message),
            hex::encode(decrypted_message.as_slice())
        );

        Ok(())
    }

    #[test]
    fn test_seal_unseal_naming_key() -> Result<()> {
        let mnemonic = Mnemonic::generate_in(Language::English, 12)?;
        let (_, public_key) = content_keypair(&mnemonic)?;

        let naming_key = generate_naming_key();
        let sealed = seal_naming_key(&naming_key, &public_key)?;

        let recovered = unseal_naming_key(&sealed, &mnemonic)?;

        assert_eq!(*naming_key, *recovered);

        Ok(())
    }

    #[test]
    fn test_unseal_with_wrong_mnemonic_fails() -> Result<()> {
        let mnemonic = Mnemonic::generate_in(Language::English, 12)?;
        let (_, public_key) = content_keypair(&mnemonic)?;

        let naming_key = generate_naming_key();
        let sealed = seal_naming_key(&naming_key, &public_key)?;

        let wrong = Mnemonic::generate_in(Language::English, 12)?;
        assert!(unseal_naming_key(&sealed, &wrong).is_err());

        Ok(())
    }

    #[test]
    fn test_content_keypair_is_deterministic() -> Result<()> {
        let mnemonic = Mnemonic::generate_in(Language::English, 12)?;

        let (_, pub_a) = content_keypair(&mnemonic)?;
        let (_, pub_b) = content_keypair(&mnemonic)?;

        assert_eq!(pub_a.to_bytes(), pub_b.to_bytes());

        Ok(())
    }
}
