//! Verify (and optionally repair) that every content blob the catalog references
//! actually exists in each destination.
//!
//! `run` trusts the catalog when deciding what to upload, so if a destination
//! loses blobs the catalog won't notice. `verify` re-checks the destinations:
//! - **existence check** (default): is each content id present in each destination?
//! - **`--repair`**: restore missing blobs. If another destination still has the
//!   blob, **copy** it over (preserves the recorded key). If it's gone from every
//!   destination, **re-seal** it from the source file (fresh key → overwrite all
//!   destinations → update the catalog key). If no copy and no source, it's
//!   reported as unrecoverable.

use crate::{
    db::sqlite::SqliteCatalog,
    engine::run::{NamingKey, scan_worker_count},
    storage::local::LocalStore,
    utils::{crypto::seal_content, hash::blake3_keyed_bytes},
};
use anyhow::{Result, anyhow};
use futures::stream::{self, StreamExt, TryStreamExt};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use x25519_dalek::PublicKey;

pub struct VerifyReport {
    pub destinations: usize,
    pub content_ids: usize,
    /// (content, destination) pairs found missing.
    pub missing: usize,
    pub repaired_by_copy: usize,
    pub repaired_by_reseal: usize,
    /// Content ids missing everywhere with no usable source to re-seal from.
    pub unrecoverable: Vec<String>,
}

/// Verify a backup's destinations, optionally repairing missing blobs.
///
/// `naming_key` is required only to repair by re-sealing from source (it verifies
/// the source still matches the content id); pass `None` for an existence-only check.
///
/// # Errors
/// Returns an error if the backup is missing or a destination/catalog op fails.
pub async fn verify(
    config_dir: &Path,
    name: &str,
    repair: bool,
    naming_key: Option<NamingKey>,
) -> Result<VerifyReport> {
    let db_file = config_dir.join(format!("{name}.db"));
    if !db_file.exists() {
        return Err(anyhow!(
            "No backup named \"{name}\" found. Create a new backup first."
        ));
    }

    let catalog = SqliteCatalog::open(&db_file)?;

    let mut stores = Vec::new();
    for dest in catalog.configured_destinations()? {
        if dest.starts_with("s3://") {
            tracing::warn!("S3 destination not yet supported, skipping: {dest}");
        } else {
            stores.push(LocalStore::new(dest));
        }
    }
    if stores.is_empty() {
        return Err(anyhow!("no usable (filesystem) destinations configured"));
    }

    let public_key = catalog.public_key()?;
    // `Files.hash` is UNIQUE, so this is one entry per distinct content blob.
    let content_ids = catalog.all_content_ids()?;

    // For re-seal repair we need a live source file to regenerate a lost blob.
    // Build the map only when repairing (it reads the catalog) — an existence-only
    // verify needs no sources.
    let source_by_id = if repair {
        latest_source_paths(&catalog)?
    } else {
        HashMap::new()
    };

    // Each content id is independent, so check (and repair) them concurrently with
    // a bounded pool — the same bound the upload phase uses. The work is dominated
    // by `exists()` stat calls (one per destination per blob), which are I/O-bound;
    // overlapping them is an order of magnitude faster than awaiting one at a time,
    // especially on networked destinations. Repairs touch only their own blob file
    // and catalog row, so distinct ids never race.
    let outcomes: Vec<ContentOutcome> = stream::iter(content_ids.iter())
        .map(|id| {
            check_content(
                id,
                &stores,
                repair,
                &catalog,
                public_key,
                naming_key.as_ref(),
                source_by_id.get(id).map_or(&[][..], Vec::as_slice),
            )
        })
        .buffer_unordered(scan_worker_count())
        .try_collect()
        .await?;

    let mut report = VerifyReport {
        destinations: stores.len(),
        content_ids: content_ids.len(),
        missing: 0,
        repaired_by_copy: 0,
        repaired_by_reseal: 0,
        unrecoverable: Vec::new(),
    };
    for outcome in outcomes {
        report.missing += outcome.missing;
        report.repaired_by_copy += outcome.repaired_by_copy;
        report.repaired_by_reseal += outcome.repaired_by_reseal;
        if let Some(id) = outcome.unrecoverable {
            report.unrecoverable.push(id);
        }
    }
    // buffer_unordered completes out of order; sort so the report is deterministic.
    report.unrecoverable.sort();

    Ok(report)
}

/// What checking a single content id produced — folded into the [`VerifyReport`]
/// after all ids finish. Returning a value (rather than mutating shared state)
/// keeps the concurrent fan-out race-free.
#[derive(Default)]
struct ContentOutcome {
    missing: usize,
    repaired_by_copy: usize,
    repaired_by_reseal: usize,
    unrecoverable: Option<String>,
}

/// Check one content id across all destinations and, when `repair` is set, restore
/// any missing copies (copy from a healthy destination, else re-seal from source).
async fn check_content(
    id: &str,
    stores: &[LocalStore],
    repair: bool,
    catalog: &SqliteCatalog,
    public_key: PublicKey,
    naming_key: Option<&NamingKey>,
    sources: &[PathBuf],
) -> Result<ContentOutcome> {
    // Which destinations are missing this blob, and is any still healthy?
    let mut missing_idx = Vec::new();
    let mut healthy_idx = None;
    for (idx, store) in stores.iter().enumerate() {
        if store.exists(id).await? {
            // Remember the first destination that still has the blob; it becomes
            // the source for copy-repair.
            healthy_idx.get_or_insert(idx);
        } else {
            missing_idx.push(idx);
        }
    }

    let mut outcome = ContentOutcome::default();
    if missing_idx.is_empty() {
        return Ok(outcome);
    }
    outcome.missing = missing_idx.len();

    if !repair {
        return Ok(outcome);
    }

    if let Some(healthy) = healthy_idx {
        // At least one destination still has the blob: copy it to the others. The
        // content key is unchanged, so every copy stays byte-identical and the
        // catalog key keeps decrypting all of them.
        let blob = store_at(stores, healthy)?.get(id).await?;
        for idx in &missing_idx {
            store_at(stores, *idx)?.put(id, &blob).await?;
            outcome.repaired_by_copy += 1;
        }
    } else if reseal_and_store(catalog, stores, public_key, naming_key, id, sources).await? {
        // Gone from every destination — re-sealed from the source file.
        outcome.repaired_by_reseal = 1;
    } else {
        outcome.unrecoverable = Some(id.to_string());
    }

    Ok(outcome)
}

fn store_at(stores: &[LocalStore], idx: usize) -> Result<&LocalStore> {
    stores
        .get(idx)
        .ok_or_else(|| anyhow!("store index {idx} out of range"))
}

/// Map each content id to the source paths of the latest completed snapshot.
///
/// A single content id can have several paths (deduplicated identical files), so
/// values are vectors: if one path was deleted, [`reseal_and_store`] can still
/// regenerate the blob from a surviving duplicate. Only the latest *completed*
/// version is considered — that is the state a fresh `run` would reproduce.
fn latest_source_paths(catalog: &SqliteCatalog) -> Result<HashMap<String, Vec<PathBuf>>> {
    let mut map: HashMap<String, Vec<PathBuf>> = HashMap::new();
    if let Some(version) = catalog.latest_version()? {
        for entry in catalog.restore_entries(version)? {
            map.entry(entry.hash).or_default().push(entry.path);
        }
    }
    Ok(map)
}

/// Re-seal a content id from one of its source files and write it to every
/// destination, updating the catalog key so all copies agree.
///
/// Returns `Ok(false)` (repairing nothing) when there is no naming key, no source
/// path, or none of the candidate paths still reads back with matching content —
/// i.e. the blob is genuinely unrecoverable.
///
/// Ordering note: the catalog key is updated *before* the blobs are written. The
/// catalog is the source of truth, and the two effects (catalog row + N store
/// writes) cannot be made atomic. Updating the key first means a crash mid-repair
/// leaves the blob *missing* — which a later `verify` detects and repairs — rather
/// than *present but encrypted with a stale key*, which would look healthy to an
/// existence check yet fail to decrypt on restore.
async fn reseal_and_store(
    catalog: &SqliteCatalog,
    stores: &[LocalStore],
    public_key: PublicKey,
    naming_key: Option<&NamingKey>,
    id: &str,
    sources: &[PathBuf],
) -> Result<bool> {
    let Some(naming_key) = naming_key else {
        return Ok(false);
    };

    // Find a source file that still exists and whose content matches the id.
    let mut bytes = None;
    for path in sources {
        if let Ok(data) = tokio::fs::read(path).await
            && blake3_keyed_bytes(&data, naming_key) == id
        {
            bytes = Some(data);
            break;
        }
    }
    let Some(bytes) = bytes else {
        return Ok(false);
    };

    let seal_id = id.to_string();
    let sealed =
        tokio::task::spawn_blocking(move || seal_content(&bytes, &public_key, &seal_id)).await??;

    catalog.update_content_key(id, &sealed.wrapped_key, &sealed.ephemeral_public_key)?;
    for store in stores {
        store.put(id, &sealed.blob).await?;
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        engine::{
            create::{CreateBackupRequest, create},
            run::{IgnoreRules, RunBackupRequest, run},
            wkey,
        },
        utils::crypto::{content_key_aad, decrypt, open_content},
    };
    use anyhow::anyhow;
    use bip39::{Language, Mnemonic};
    use std::{fs, sync::Arc};

    struct Fixture {
        _tmp: tempfile::TempDir,
        cfg: PathBuf,
        src: PathBuf,
        dests: Vec<PathBuf>,
        mnemonic: Mnemonic,
        naming_key: NamingKey,
    }

    /// Build a backup, write `files` into the source dir, and run it so every
    /// filesystem destination holds the blobs.
    ///
    /// `dest_count` filesystem destinations are created (`dest0`, `dest1`, …).
    /// When `with_s3` is set, an extra `s3://…` destination is also *configured*
    /// (to exercise the "skip unsupported backend" path) but is not returned in
    /// `Fixture::dests`, since there is no local store to inspect for it.
    async fn build(files: &[(&str, &[u8])], dest_count: usize, with_s3: bool) -> Result<Fixture> {
        let tmp = tempfile::tempdir()?;
        let cfg = tmp.path().join("cfg");
        let src = tmp.path().join("src");
        fs::create_dir_all(&cfg)?;
        fs::create_dir_all(&src)?;
        for (name, contents) in files {
            fs::write(src.join(name), contents)?;
        }

        let dests: Vec<PathBuf> = (0..dest_count)
            .map(|i| tmp.path().join(format!("dest{i}")))
            .collect();

        let mut configured: Vec<String> = dests
            .iter()
            .map(|d| d.to_string_lossy().into_owned())
            .collect();
        if with_s3 {
            configured.push("s3://example-bucket/prefix".to_string());
        }

        let created = create(CreateBackupRequest {
            name: "t".to_string(),
            config_dir: cfg.clone(),
            directories: vec![src.clone()],
            files: Vec::new(),
            destinations: configured,
        })?;
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, &created.recovery_phrase)?;
        let naming_key: NamingKey =
            Arc::new(wkey::load_naming_key(&cfg, "t")?.ok_or_else(|| anyhow!("missing wkey"))?);

        run(RunBackupRequest {
            name: "t".to_string(),
            config_dir: cfg.clone(),
            ignore_rules: IgnoreRules::backupignore_only(),
            dry_run: false,
            progress: None,
            naming_key: naming_key.clone(),
        })
        .await?;

        Ok(Fixture {
            _tmp: tmp,
            cfg,
            src,
            dests,
            mnemonic,
            naming_key,
        })
    }

    /// Convenience: one "hello world" file across `dest_count` filesystem
    /// destinations.
    async fn setup(dest_count: usize) -> Result<Fixture> {
        build(&[("a.txt", b"hello world")], dest_count, false).await
    }

    impl Fixture {
        fn store(&self, i: usize) -> Result<LocalStore> {
            self.dests
                .get(i)
                .map(LocalStore::new)
                .ok_or_else(|| anyhow!("no destination {i}"))
        }
    }

    /// Decrypt the stored blob for `id` from `store` using the catalog's current
    /// wrapped key and the recovery mnemonic.
    async fn decrypt_blob(
        catalog: &SqliteCatalog,
        store: &LocalStore,
        mnemonic: &Mnemonic,
        id: &str,
    ) -> Result<Vec<u8>> {
        let (wrapped, eph) = catalog
            .wrapped_content_key(id)?
            .ok_or_else(|| anyhow!("no wrapped key"))?;
        let key_vec = decrypt(&wrapped, &eph, mnemonic, &content_key_aad(id))?;
        let key: [u8; 32] = key_vec
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("bad key length"))?;
        let blob = store.get(id).await?;
        Ok(open_content(&blob, id, &key)?.to_vec())
    }

    #[tokio::test]
    async fn verify_detects_missing_blob_without_repairing() -> Result<()> {
        let fx = setup(1).await?;
        let id = blake3_keyed_bytes(b"hello world", &fx.naming_key);
        let store = fx.store(0)?;
        store.remove(&id).await?;

        let report = verify(&fx.cfg, "t", false, None).await?;

        assert_eq!(report.missing, 1);
        assert_eq!(report.repaired_by_copy, 0);
        assert_eq!(report.repaired_by_reseal, 0);
        assert!(report.unrecoverable.is_empty());
        // No repair requested -> still missing.
        assert!(!store.exists(&id).await?);

        Ok(())
    }

    #[tokio::test]
    async fn repair_copies_from_healthy_destination() -> Result<()> {
        let fx = setup(2).await?;
        let id = blake3_keyed_bytes(b"hello world", &fx.naming_key);
        let broken = fx.store(0)?;
        let healthy = fx.store(1)?;
        broken.remove(&id).await?;

        let report = verify(&fx.cfg, "t", true, Some(fx.naming_key.clone())).await?;

        assert_eq!(report.missing, 1);
        assert_eq!(report.repaired_by_copy, 1);
        assert_eq!(report.repaired_by_reseal, 0);
        assert!(report.unrecoverable.is_empty());

        // Copied back, and both copies are byte-identical (same key preserved).
        assert!(broken.exists(&id).await?);
        assert_eq!(broken.get(&id).await?, healthy.get(&id).await?);

        // Still decrypts with the original (unchanged) catalog key.
        let catalog = SqliteCatalog::open(&fx.cfg.join("t.db"))?;
        let plaintext = decrypt_blob(&catalog, &broken, &fx.mnemonic, &id).await?;
        assert_eq!(plaintext, b"hello world");

        Ok(())
    }

    #[tokio::test]
    async fn repair_reseals_when_gone_everywhere() -> Result<()> {
        let fx = setup(2).await?;
        let id = blake3_keyed_bytes(b"hello world", &fx.naming_key);
        for dest in &fx.dests {
            LocalStore::new(dest).remove(&id).await?;
        }

        let report = verify(&fx.cfg, "t", true, Some(fx.naming_key.clone())).await?;

        assert_eq!(report.repaired_by_copy, 0);
        assert_eq!(report.repaired_by_reseal, 1);
        assert!(report.unrecoverable.is_empty());

        // Re-sealed into every destination; the new (updated) key decrypts it.
        let catalog = SqliteCatalog::open(&fx.cfg.join("t.db"))?;
        for dest in &fx.dests {
            let store = LocalStore::new(dest);
            assert!(store.exists(&id).await?);
            let plaintext = decrypt_blob(&catalog, &store, &fx.mnemonic, &id).await?;
            assert_eq!(plaintext, b"hello world");
        }

        Ok(())
    }

    #[tokio::test]
    async fn unrecoverable_when_source_and_all_copies_gone() -> Result<()> {
        let fx = setup(1).await?;
        let id = blake3_keyed_bytes(b"hello world", &fx.naming_key);
        fx.store(0)?.remove(&id).await?;
        fs::remove_file(fx.src.join("a.txt"))?;

        let report = verify(&fx.cfg, "t", true, Some(fx.naming_key.clone())).await?;

        assert_eq!(report.repaired_by_copy, 0);
        assert_eq!(report.repaired_by_reseal, 0);
        assert_eq!(report.unrecoverable, vec![id]);

        Ok(())
    }

    #[tokio::test]
    async fn verify_passes_when_all_blobs_present() -> Result<()> {
        let fx = setup(2).await?;
        let report = verify(&fx.cfg, "t", false, None).await?;
        assert_eq!(report.missing, 0);
        assert_eq!(report.content_ids, 1);
        assert_eq!(report.destinations, 2);
        Ok(())
    }

    /// Re-sealing must replace the catalog's wrapped key (it cannot reuse the old
    /// one — the host never had the plaintext content key). Guards against a
    /// regression where the blob is rewritten but the key is left stale.
    #[tokio::test]
    async fn reseal_replaces_the_catalog_key() -> Result<()> {
        let fx = setup(1).await?;
        let id = blake3_keyed_bytes(b"hello world", &fx.naming_key);

        let catalog = SqliteCatalog::open(&fx.cfg.join("t.db"))?;
        let before = catalog
            .wrapped_content_key(&id)?
            .ok_or_else(|| anyhow!("no key before"))?;

        fx.store(0)?.remove(&id).await?;
        let report = verify(&fx.cfg, "t", true, Some(fx.naming_key.clone())).await?;
        assert_eq!(report.repaired_by_reseal, 1);

        let after = catalog
            .wrapped_content_key(&id)?
            .ok_or_else(|| anyhow!("no key after"))?;
        assert_ne!(before, after, "reseal should rewrap with a fresh key");

        // And it still decrypts to the original bytes under the new key.
        let plaintext = decrypt_blob(&catalog, &fx.store(0)?, &fx.mnemonic, &id).await?;
        assert_eq!(plaintext, b"hello world");
        Ok(())
    }

    /// Deduplicated content has several source paths; losing one must not make the
    /// blob unrecoverable as long as a duplicate source survives.
    #[tokio::test]
    async fn reseal_uses_a_surviving_duplicate_source() -> Result<()> {
        let fx = build(&[("dup1.txt", b"same"), ("dup2.txt", b"same")], 1, false).await?;
        let id = blake3_keyed_bytes(b"same", &fx.naming_key);

        // One blob for both files; remove it and delete only the first source.
        fx.store(0)?.remove(&id).await?;
        fs::remove_file(fx.src.join("dup1.txt"))?;

        let report = verify(&fx.cfg, "t", true, Some(fx.naming_key.clone())).await?;
        assert_eq!(report.content_ids, 1);
        assert_eq!(report.repaired_by_reseal, 1);
        assert!(report.unrecoverable.is_empty());
        assert!(fx.store(0)?.exists(&id).await?);
        Ok(())
    }

    /// Verify must flag only the blobs that are actually gone, not every blob.
    #[tokio::test]
    async fn verify_reports_only_the_missing_blob() -> Result<()> {
        let fx = build(&[("a.txt", b"alpha"), ("b.txt", b"beta")], 1, false).await?;
        let gone = blake3_keyed_bytes(b"alpha", &fx.naming_key);
        fx.store(0)?.remove(&gone).await?;

        let report = verify(&fx.cfg, "t", false, None).await?;
        assert_eq!(report.content_ids, 2);
        assert_eq!(report.missing, 1);
        Ok(())
    }

    /// `s3://` destinations are not wired yet; verify must skip them rather than
    /// fail, and count only the filesystem destinations it can actually check.
    #[tokio::test]
    async fn s3_destinations_are_skipped() -> Result<()> {
        let fx = build(&[("a.txt", b"hello world")], 1, true).await?;
        let report = verify(&fx.cfg, "t", false, None).await?;
        assert_eq!(
            report.destinations, 1,
            "only the filesystem dest is checked"
        );
        assert_eq!(report.missing, 0);
        Ok(())
    }

    /// Without a naming key, re-seal cannot verify a source against its content id,
    /// so a blob missing everywhere is reported unrecoverable rather than risk
    /// writing unverified bytes. (The CLI only omits the key for `verify` without
    /// `--repair`; this guards the engine contract directly.)
    #[tokio::test]
    async fn repair_without_naming_key_cannot_reseal() -> Result<()> {
        let fx = setup(1).await?;
        let id = blake3_keyed_bytes(b"hello world", &fx.naming_key);
        fx.store(0)?.remove(&id).await?;

        let report = verify(&fx.cfg, "t", true, None).await?;
        assert_eq!(report.repaired_by_reseal, 0);
        assert_eq!(report.unrecoverable, vec![id]);
        Ok(())
    }

    /// Build a backup with `count` distinct files (`f0.txt`…), one blob each.
    async fn build_many(count: usize, dest_count: usize) -> Result<(Fixture, Vec<String>)> {
        let owned: Vec<(String, Vec<u8>)> = (0..count)
            .map(|i| (format!("f{i}.txt"), format!("content-{i}").into_bytes()))
            .collect();
        let files: Vec<(&str, &[u8])> = owned
            .iter()
            .map(|(n, c)| (n.as_str(), c.as_slice()))
            .collect();
        let fx = build(&files, dest_count, false).await?;
        let ids = owned
            .iter()
            .map(|(_, c)| blake3_keyed_bytes(c, &fx.naming_key))
            .collect();
        Ok((fx, ids))
    }

    /// The concurrent fan-out must count every missing blob exactly once across
    /// many content ids (guards the per-id outcome fold).
    #[tokio::test]
    async fn verify_aggregates_missing_across_many_contents() -> Result<()> {
        let (fx, ids) = build_many(12, 1).await?;
        let store = fx.store(0)?;

        let mut removed = 0;
        for (i, id) in ids.iter().enumerate() {
            if i % 3 == 0 {
                store.remove(id).await?;
                removed += 1;
            }
        }

        let report = verify(&fx.cfg, "t", false, None).await?;
        assert_eq!(report.content_ids, 12);
        assert_eq!(report.missing, removed);
        Ok(())
    }

    /// All blobs and all sources gone: every id is unrecoverable, collected from
    /// the out-of-order fan-out and returned sorted (deterministic output).
    #[tokio::test]
    async fn repair_collects_all_unrecoverable_sorted() -> Result<()> {
        let (fx, mut ids) = build_many(8, 1).await?;
        let store = fx.store(0)?;

        for id in &ids {
            store.remove(id).await?;
        }
        for entry in fs::read_dir(&fx.src)? {
            fs::remove_file(entry?.path())?;
        }

        let report = verify(&fx.cfg, "t", true, Some(fx.naming_key.clone())).await?;
        assert_eq!(report.repaired_by_reseal, 0);

        ids.sort();
        assert_eq!(report.unrecoverable, ids);
        Ok(())
    }
}
