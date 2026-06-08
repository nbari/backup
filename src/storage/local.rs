//! Local filesystem blob store.
//!
//! A content-addressed object store under a root directory: objects are named by
//! their (keyed) content id and laid out in a sharded tree (`ab/cd/<id>`) to keep
//! directories small. Writes are atomic (temp file + rename) and **overwrite** any
//! existing object — the caller (the engine) holds the bytes that match the key it
//! is recording, so an earlier orphan blob (e.g. from an interrupted run) must be
//! replaced, not kept. Dedup happens a layer up (the engine skips content already
//! recorded in the catalog), so `put` is only called for content that must be
//! (re)written. This is the §6.5 filesystem backend (packs come later); because it
//! only needs a path, it also covers NFS, external drives, and FUSE mounts.

use anyhow::{Result, anyhow};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::{fs, io::AsyncWriteExt};

/// Per-process counter making temp filenames unique, so two writers targeting the
/// same object key never share a temp path (which would defeat the atomic rename).
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub struct LocalStore {
    root: PathBuf,
}

impl LocalStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Sharded path for an object id: `<root>/<id[0..2]>/<id[2..4]>/<id>`.
    ///
    /// Object keys are keyed-BLAKE3 content ids (lowercase hex). We reject any
    /// non-hex key so a crafted id can never escape the store root — `/`, `..`,
    /// and absolute paths are all non-hex, so this also blocks path traversal.
    fn object_path(&self, key: &str) -> Result<PathBuf> {
        if key.len() < 4 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(anyhow!("invalid object key: {key:?}"));
        }
        let shard_a = key
            .get(0..2)
            .ok_or_else(|| anyhow!("object key too short: {key:?}"))?;
        let shard_b = key
            .get(2..4)
            .ok_or_else(|| anyhow!("object key too short: {key:?}"))?;

        Ok(self.root.join(shard_a).join(shard_b).join(key))
    }

    /// Store `bytes` under `key`, atomically replacing any existing object.
    ///
    /// # Errors
    /// Returns an error if the object cannot be written.
    pub async fn put(&self, key: &str, bytes: &[u8]) -> Result<()> {
        let path = self.object_path(key)?;

        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("object path has no parent: {}", path.display()))?;
        fs::create_dir_all(parent).await?;

        // Write to a unique temp file in the same directory, fsync, then atomically
        // rename into place (replacing any existing object) so a reader never sees
        // a partial object. The temp name carries the pid + a per-process counter
        // so concurrent writers (or a second process) targeting the same key don't
        // clobber each other's temp file mid-write.
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp = parent.join(format!(".{key}.{}.{seq}.tmp", std::process::id()));

        if let Err(err) = write_then_rename(&tmp, &path, bytes).await {
            // Best-effort cleanup so a failed write doesn't leave a temp behind.
            let _ = fs::remove_file(&tmp).await;
            return Err(err);
        }

        Ok(())
    }

    /// Read the object stored under `key`.
    ///
    /// # Errors
    /// Returns an error if the object is missing or cannot be read.
    pub async fn get(&self, key: &str) -> Result<Vec<u8>> {
        let path = self.object_path(key)?;
        Ok(fs::read(&path).await?)
    }

    /// Whether an object exists under `key`.
    ///
    /// # Errors
    /// Returns an error if existence cannot be determined.
    pub async fn exists(&self, key: &str) -> Result<bool> {
        Ok(fs::try_exists(self.object_path(key)?).await?)
    }

    /// Remove the object stored under `key`, if present. Missing objects are not
    /// an error (removal is idempotent).
    ///
    /// # Errors
    /// Returns an error if an existing object cannot be removed.
    pub async fn remove(&self, key: &str) -> Result<()> {
        match fs::remove_file(self.object_path(key)?).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

/// Write `bytes` to `tmp`, fsync, then rename onto `path`. Split out so `put` can
/// clean up `tmp` if any step fails.
async fn write_then_rename(
    tmp: &std::path::Path,
    path: &std::path::Path,
    bytes: &[u8],
) -> Result<()> {
    let mut file = fs::File::create(tmp).await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    drop(file);

    fs::rename(tmp, path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Result<(tempfile::TempDir, LocalStore)> {
        let dir = tempfile::tempdir()?;
        let store = LocalStore::new(dir.path());
        Ok((dir, store))
    }

    #[tokio::test]
    async fn put_then_get_round_trips() -> Result<()> {
        let (_dir, store) = store()?;
        let key = "abcd1234deadbeef";

        assert!(!store.exists(key).await?);
        store.put(key, b"hello world").await?;
        assert!(store.exists(key).await?);
        assert_eq!(store.get(key).await?, b"hello world");

        Ok(())
    }

    #[tokio::test]
    async fn put_overwrites_existing_object() -> Result<()> {
        let (_dir, store) = store()?;
        let key = "abcd1234deadbeef";

        // An orphan/old blob under this key must be replaced, not kept — the
        // caller holds the bytes that match the key it records.
        store.put(key, b"old orphan bytes").await?;
        store.put(key, b"new authoritative bytes").await?;
        assert_eq!(store.get(key).await?, b"new authoritative bytes");

        Ok(())
    }

    #[tokio::test]
    async fn objects_are_sharded() -> Result<()> {
        let (dir, store) = store()?;
        let key = "abcd1234deadbeef";

        store.put(key, b"x").await?;

        let expected = dir.path().join("ab").join("cd").join(key);
        assert!(
            expected.is_file(),
            "expected sharded object at {expected:?}"
        );

        Ok(())
    }

    #[tokio::test]
    async fn short_key_is_rejected() -> Result<()> {
        let (_dir, store) = store()?;
        assert!(store.put("abc", b"x").await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn non_hex_and_traversal_keys_are_rejected() -> Result<()> {
        let (_dir, store) = store()?;
        // Anything that isn't lowercase/upper hex can't form a path that escapes
        // the root: slashes, dot-dot, absolute paths, and stray letters all fail.
        for key in ["../../etc/passwd", "/etc/passwd", "ab/cd", "zzzz", "abc!"] {
            assert!(store.put(key, b"x").await.is_err(), "should reject {key:?}");
            assert!(store.get(key).await.is_err(), "should reject {key:?}");
            assert!(store.exists(key).await.is_err(), "should reject {key:?}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn missing_object_get_errors() -> Result<()> {
        let (_dir, store) = store()?;
        assert!(store.get("abcd1234deadbeef").await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn remove_deletes_then_is_idempotent() -> Result<()> {
        let (_dir, store) = store()?;
        let key = "abcd1234deadbeef";

        store.put(key, b"x").await?;
        assert!(store.exists(key).await?);

        store.remove(key).await?;
        assert!(!store.exists(key).await?);

        // Removing a missing object is not an error (idempotent).
        store.remove(key).await?;
        Ok(())
    }
}
