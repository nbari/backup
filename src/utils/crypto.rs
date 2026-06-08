use anyhow::{Result, anyhow};
use chacha20poly1305::{
    ChaCha20Poly1305, Key, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
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
/// AAD bound when wrapping the naming key (domain-separated from content keys).
const NAMING_KEY_AAD: &[u8] = b"backup:naming-key:v1";

/// Associated data binding a wrapped content/file key to its content id, so a
/// wrapped-key row in the catalog can't be reused under a different content id.
/// Domain-separated from [`NAMING_KEY_AAD`] by the label prefix.
#[must_use]
pub fn content_key_aad(content_id: &str) -> Vec<u8> {
    let label = b"backup:content-key:v1:";
    let mut aad = Vec::with_capacity(label.len() + content_id.len());
    aad.extend_from_slice(label);
    aad.extend_from_slice(content_id.as_bytes());
    aad
}

/// Content blob format version (header byte 0).
const BLOB_VERSION: u8 = 1;
/// Codec tag (header byte 1): stored uncompressed.
const CODEC_RAW: u8 = 0;
/// Codec tag (header byte 1): zstd-compressed.
const CODEC_ZSTD: u8 = 1;
/// zstd compression level.
const ZSTD_LEVEL: i32 = 3;
/// ChaCha20-Poly1305 nonce length.
const NONCE_LEN: usize = 12;
/// Blob header length: `version || codec`.
const BLOB_HEADER_LEN: usize = 2;

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
/// `aad` is bound as associated data so the wrapped key is cryptographically tied
/// to its context (e.g. its content id), and must be supplied identically to
/// [`decrypt`]. Returns the wrapped payload (`nonce || ciphertext`) and the
/// ephemeral public key needed to unwrap it.
///
/// # Errors
/// Returns an error if key derivation or encryption fails.
pub fn encrypt(key: &[u8; 32], public_key: &PublicKey, aad: &[u8]) -> Result<(Vec<u8>, [u8; 32])> {
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

    cipher
        .encrypt(
            &nonce,
            Payload {
                msg: key.as_ref(),
                aad,
            },
        )
        .map_or_else(
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
/// `aad` must match the associated data passed to [`encrypt`] when the key was
/// wrapped, otherwise authentication fails.
///
/// # Errors
/// Returns an error if the encrypted payload is malformed or key unwrapping fails.
pub fn decrypt(
    encrypted_data: &[u8],
    eph_pub_bytes: &[u8; 32],
    mnemonic: &bip39::Mnemonic,
    aad: &[u8],
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
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext,
                aad,
            },
        )
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
    let (wrapped, eph_pub) = encrypt(naming_key, public_key, NAMING_KEY_AAD)?;

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

    let plaintext = decrypt(wrapped, &eph_pub, mnemonic, NAMING_KEY_AAD)?;

    let key: [u8; 32] = plaintext
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("Unsealed naming key has unexpected length"))?;

    Ok(Zeroizing::new(key))
}

/// A compressed + encrypted content blob plus the wrapped key to record.
pub struct SealedContent {
    /// `version || codec || nonce || ciphertext` — stored opaque in the blob store.
    pub blob: Vec<u8>,
    /// The content key wrapped to the backup public key (`nonce || ciphertext`).
    pub wrapped_key: Vec<u8>,
    /// Ephemeral public key needed to unwrap `wrapped_key`.
    pub ephemeral_public_key: [u8; 32],
}

/// Associated data binding the blob to its content id, format version and codec
/// (prevents a tampering store from swapping a blob or forcing a codec downgrade).
fn content_aad(content_id: &str, codec: u8) -> Vec<u8> {
    let mut aad = Vec::with_capacity(content_id.len() + 2);
    aad.extend_from_slice(content_id.as_bytes());
    aad.push(BLOB_VERSION);
    aad.push(codec);
    aad
}

/// Compress then encrypt `plaintext` into a storable blob, and wrap a fresh
/// per-content key to `public_key`. The content key is single-use, so a random
/// nonce stored in the blob is safe; `content_id` is bound as associated data.
///
/// # Errors
/// Returns an error if compression, encryption, or key wrapping fails.
pub fn seal_content(
    plaintext: &[u8],
    public_key: &PublicKey,
    content_id: &str,
) -> Result<SealedContent> {
    let content_key = generate_file_key();

    // Compress; keep the raw bytes if zstd doesn't actually shrink them.
    let compressed = zstd::encode_all(plaintext, ZSTD_LEVEL)
        .map_err(|err| anyhow!("compression failed: {err}"))?;
    let (codec, payload): (u8, &[u8]) = if compressed.len() < plaintext.len() {
        (CODEC_ZSTD, &compressed)
    } else {
        (CODEC_RAW, plaintext)
    };

    let cipher = ChaCha20Poly1305::new(Key::from_slice(content_key.as_ref()));
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let aad = content_aad(content_id, codec);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: payload,
                aad: &aad,
            },
        )
        .map_err(|_| anyhow!("content encryption failed"))?;

    let mut blob = Vec::with_capacity(BLOB_HEADER_LEN + NONCE_LEN + ciphertext.len());
    blob.push(BLOB_VERSION);
    blob.push(codec);
    blob.extend_from_slice(nonce.as_slice());
    blob.extend_from_slice(&ciphertext);

    let (wrapped_key, ephemeral_public_key) =
        encrypt(&content_key, public_key, &content_key_aad(content_id))?;

    Ok(SealedContent {
        blob,
        wrapped_key,
        ephemeral_public_key,
    })
}

/// Decrypt and decompress a blob produced by [`seal_content`], given the
/// already-unwrapped content key (see [`decrypt`]) and the blob's `content_id`.
///
/// # Errors
/// Returns an error if the blob is malformed or authentication/decompression fails.
pub fn open_content(
    blob: &[u8],
    content_id: &str,
    content_key: &[u8; 32],
) -> Result<Zeroizing<Vec<u8>>> {
    let version = *blob.first().ok_or_else(|| anyhow!("blob too short"))?;
    if version != BLOB_VERSION {
        return Err(anyhow!("unsupported blob version {version}"));
    }
    let codec = *blob.get(1).ok_or_else(|| anyhow!("blob too short"))?;

    let nonce_bytes = blob
        .get(BLOB_HEADER_LEN..BLOB_HEADER_LEN + NONCE_LEN)
        .ok_or_else(|| anyhow!("blob missing nonce"))?;
    let ciphertext = blob
        .get(BLOB_HEADER_LEN + NONCE_LEN..)
        .ok_or_else(|| anyhow!("blob missing ciphertext"))?;

    let cipher = ChaCha20Poly1305::new(Key::from_slice(content_key));
    let nonce = Nonce::from_slice(nonce_bytes);
    let aad = content_aad(content_id, codec);
    let payload = cipher
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| anyhow!("content decryption failed"))?;

    let plaintext = match codec {
        CODEC_RAW => payload,
        CODEC_ZSTD => {
            zstd::decode_all(&payload[..]).map_err(|err| anyhow!("decompression failed: {err}"))?
        }
        other => return Err(anyhow!("unknown codec tag {other}")),
    };

    Ok(Zeroizing::new(plaintext))
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
        let aad = b"backup:content-key:v1:abcd";

        let (encrypted_data, eph_pub_bytes) = encrypt(&message, &public_key, aad)?;

        let decrypted_message = decrypt(&encrypted_data, &eph_pub_bytes, &mnemonic, aad)?;

        assert_eq!(
            hex::encode(*message),
            hex::encode(decrypted_message.as_slice())
        );

        Ok(())
    }

    #[test]
    fn test_decrypt_with_wrong_aad_fails() -> Result<()> {
        let mnemonic = Mnemonic::generate_in(Language::English, 24)?;
        let (_, public_key) = content_keypair(&mnemonic)?;

        let message = generate_file_key();
        let (encrypted_data, eph_pub_bytes) =
            encrypt(&message, &public_key, &content_key_aad(TEST_ID))?;

        // A different content id -> different AAD -> the wrapped key won't unwrap.
        let wrong = content_key_aad("0000000000000000");
        assert!(decrypt(&encrypted_data, &eph_pub_bytes, &mnemonic, &wrong).is_err());

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

    const TEST_ID: &str = "abcd1234deadbeefabcd1234deadbeefabcd1234deadbeefabcd1234deadbeef";

    /// Unwrap the content key from a sealed blob and open it (mirrors restore).
    fn open_sealed(
        sealed: &SealedContent,
        content_id: &str,
        mnemonic: &Mnemonic,
    ) -> Result<Zeroizing<Vec<u8>>> {
        let key_vec = decrypt(
            &sealed.wrapped_key,
            &sealed.ephemeral_public_key,
            mnemonic,
            &content_key_aad(content_id),
        )?;
        let key: [u8; 32] = key_vec
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("bad content key length"))?;
        open_content(&sealed.blob, content_id, &key)
    }

    #[test]
    fn test_seal_open_roundtrip_compressible() -> Result<()> {
        let mnemonic = Mnemonic::generate_in(Language::English, 12)?;
        let (_, public_key) = content_keypair(&mnemonic)?;

        let plaintext = b"hello world ".repeat(1000); // very compressible
        let sealed = seal_content(&plaintext, &public_key, TEST_ID)?;
        assert_eq!(sealed.blob.get(1).copied(), Some(CODEC_ZSTD));

        let opened = open_sealed(&sealed, TEST_ID, &mnemonic)?;
        assert_eq!(opened.as_slice(), plaintext.as_slice());

        Ok(())
    }

    #[test]
    fn test_seal_open_roundtrip_incompressible() -> Result<()> {
        let mnemonic = Mnemonic::generate_in(Language::English, 12)?;
        let (_, public_key) = content_keypair(&mnemonic)?;

        let mut plaintext = vec![0u8; 4096];
        rand::rng().fill_bytes(&mut plaintext); // random -> won't compress
        let sealed = seal_content(&plaintext, &public_key, TEST_ID)?;
        assert_eq!(sealed.blob.get(1).copied(), Some(CODEC_RAW));

        let opened = open_sealed(&sealed, TEST_ID, &mnemonic)?;
        assert_eq!(opened.as_slice(), plaintext.as_slice());

        Ok(())
    }

    #[test]
    fn test_open_with_wrong_content_id_fails() -> Result<()> {
        let mnemonic = Mnemonic::generate_in(Language::English, 12)?;
        let (_, public_key) = content_keypair(&mnemonic)?;

        let sealed = seal_content(b"payload", &public_key, TEST_ID)?;
        // Same key, different content id -> AAD mismatch -> auth failure.
        assert!(open_sealed(&sealed, "0000000000000000", &mnemonic).is_err());

        Ok(())
    }

    #[test]
    fn test_open_with_wrong_key_fails() -> Result<()> {
        let mnemonic = Mnemonic::generate_in(Language::English, 12)?;
        let (_, public_key) = content_keypair(&mnemonic)?;

        let sealed = seal_content(b"payload", &public_key, TEST_ID)?;
        let wrong_key = [9u8; 32];
        assert!(open_content(&sealed.blob, TEST_ID, &wrong_key).is_err());

        Ok(())
    }
}
