use crate::{
    db::sqlite::{ScannedFile, SealedKeys, SqliteCatalog},
    storage::local::LocalStore,
    utils::{
        crypto::seal_content,
        hash::{blake3_keyed, blake3_keyed_bytes},
    },
};
use anyhow::{Result, anyhow};
use futures::stream::{FuturesUnordered, StreamExt};
use ignore::WalkBuilder;
use std::{
    cmp,
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};
use tokio::{
    fs::{OpenOptions, remove_file, write},
    io::{self, AsyncWriteExt},
    sync::Semaphore,
};
use tracing::{debug, instrument, warn};
use x25519_dalek::PublicKey;
use zeroize::Zeroizing;

/// Per-backup naming key shared across scan workers to key content identifiers.
pub type NamingKey = Arc<Zeroizing<[u8; 32]>>;

const BACKUP_IGNORE_FILE: &str = ".backupignore";

pub type ProgressCallback = Arc<dyn Fn(RunProgress) + Send + Sync>;

#[derive(Clone, Debug)]
pub enum RunProgress {
    FilesDiscovered(usize),
    FileFinished,
    MetadataFilesWritten(usize),
    MetadataWriteStarted(usize),
    ProcessingFile {
        worker_id: usize,
        path: PathBuf,
    },
    WorkerFinished(usize),
    /// Start of the compress/encrypt/store phase, with the number of new blobs.
    StorePhaseStarted(usize),
}

#[derive(Clone, Copy, Debug)]
pub struct IgnoreRules {
    pub backupignore: bool,
    pub gitignore: bool,
}

impl IgnoreRules {
    #[must_use]
    pub const fn backupignore_only() -> Self {
        Self {
            backupignore: true,
            gitignore: false,
        }
    }

    #[must_use]
    pub const fn none() -> Self {
        Self {
            backupignore: false,
            gitignore: false,
        }
    }
}

#[must_use]
pub fn scan_worker_count() -> usize {
    // Logical parallelism available to this process. `available_parallelism`
    // returns a `NonZeroUsize` (always ≥ 1) and respects the CPU affinity mask;
    // fall back to 1 if it can't be queried. Reserve 2 threads for the async
    // runtime and main thread, and cap the worker-id space at `u8::MAX`.
    let logical = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    cmp::min(logical.saturating_sub(2).max(1), u8::MAX as usize)
}

pub struct RunBackupRequest {
    pub name: String,
    pub config_dir: PathBuf,
    pub ignore_rules: IgnoreRules,
    pub dry_run: bool,
    pub progress: Option<ProgressCallback>,
    pub naming_key: NamingKey,
}

pub struct RunBackupResult {
    pub version: i64,
    pub scanned_files: usize,
    pub skipped_entries: usize,
    pub skipped_files_log: PathBuf,
    /// Number of new content blobs sealed and written this run.
    pub stored_blobs: usize,
    /// Number of usable destinations the blobs were written to.
    pub destination_count: usize,
}

/// Result of the upload phase.
struct UploadOutcome {
    sealed: SealedKeys,
    storable: Vec<ScannedFile>,
    stored_blobs: usize,
    skipped: usize,
}

struct QueuedScan {
    tasks: FuturesUnordered<tokio::task::JoinHandle<Result<Option<ScannedFile>>>>,
    queued_files: usize,
    skipped_entries: usize,
}

struct ScanResults {
    files: Vec<ScannedFile>,
    skipped_entries: usize,
}

/// Run a backup metadata scan.
///
/// # Errors
/// Returns an error if the configured backup cannot be scanned or the metadata database cannot be
/// updated.
#[instrument(skip(request))]
pub async fn run(request: RunBackupRequest) -> Result<RunBackupResult> {
    let skipped_files_log = request
        .config_dir
        .join(format!("{}-skipped_files.log", request.name));

    debug!("Skipped files log: {}", skipped_files_log.display());

    write(&skipped_files_log, "").await?;

    let db_file = request.config_dir.join(format!("{}.db", request.name));

    if !db_file.exists() {
        return Err(anyhow!(
            "No backup named \"{}\" found. Create a new backup first.",
            request.name
        ));
    }

    let catalog = SqliteCatalog::open(&db_file)?;
    let backup_version = if request.dry_run {
        0
    } else {
        catalog.create_version()?
    };
    let public_key = catalog.public_key()?;

    debug!("Public Key: {:?}", hex::encode(public_key));

    let queued_scan = queue_scan_tasks(
        &catalog.configured_directories()?,
        request.ignore_rules,
        request.progress.clone(),
        &skipped_files_log,
        &request.naming_key,
    )
    .await?;

    if let Some(progress) = &request.progress {
        progress(RunProgress::FilesDiscovered(queued_scan.queued_files));
    }

    let scan_results = collect_scan_results(
        queued_scan.tasks,
        request.progress.as_ref(),
        &skipped_files_log,
        queued_scan.skipped_entries,
    )
    .await?;
    let mut skipped_entries = scan_results.skipped_entries;
    let scanned_file_count = scan_results.files.len();

    let mut stored_blobs = 0;
    let mut destination_count = 0;

    if !request.dry_run {
        let (sb, dc, upload_skipped) = store_and_record(UploadCtx {
            catalog: &catalog,
            public_key,
            naming_key: &request.naming_key,
            files: &scan_results.files,
            version: backup_version,
            scan_skipped: skipped_entries,
            skipped_files_log: &skipped_files_log,
            progress: request.progress.as_ref(),
        })
        .await?;
        stored_blobs = sb;
        destination_count = dc;
        skipped_entries += upload_skipped;
    }

    if skipped_entries == 0 {
        cleanup_skipped_log(&skipped_files_log).await?;
    }

    Ok(RunBackupResult {
        version: backup_version,
        scanned_files: scanned_file_count,
        skipped_entries,
        skipped_files_log,
        stored_blobs,
        destination_count,
    })
}

/// Inputs to [`store_and_record`].
struct UploadCtx<'a> {
    catalog: &'a SqliteCatalog,
    public_key: PublicKey,
    naming_key: &'a NamingKey,
    files: &'a [ScannedFile],
    version: i64,
    scan_skipped: usize,
    skipped_files_log: &'a Path,
    progress: Option<&'a ProgressCallback>,
}

/// Seal + store new content to all destinations, then record the scan metadata.
/// Returns `(stored_blobs, destination_count, upload_skipped)`.
async fn store_and_record(ctx: UploadCtx<'_>) -> Result<(usize, usize, usize)> {
    let UploadCtx {
        catalog,
        public_key,
        naming_key,
        files,
        version,
        scan_skipped,
        skipped_files_log,
        progress,
    } = ctx;

    // Build a store per usable (filesystem) destination; S3 is not wired yet.
    let mut stores = Vec::new();
    for dest in catalog.configured_destinations()? {
        if dest.starts_with("s3://") {
            warn!("S3 destination not yet supported, skipping: {dest}");
        } else {
            stores.push(LocalStore::new(dest));
        }
    }
    let destination_count = stores.len();

    // Upload phase: seal + store new content; metadata-only if no destinations.
    let upload = if stores.is_empty() {
        UploadOutcome {
            sealed: SealedKeys::new(),
            storable: files.to_vec(),
            stored_blobs: 0,
            skipped: 0,
        }
    } else {
        upload_new_content(
            catalog,
            &stores,
            public_key,
            naming_key,
            files,
            skipped_files_log,
            progress,
        )
        .await?
    };
    let stored_blobs = upload.stored_blobs;
    let upload_skipped = upload.skipped;

    let catalog = catalog.clone();
    let progress = progress.cloned();
    if let Some(progress) = &progress {
        progress(RunProgress::MetadataWriteStarted(upload.storable.len()));
    }

    let sealed = upload.sealed;
    let storable = upload.storable;
    let close_missing = scan_skipped == 0 && upload_skipped == 0;
    tokio::task::spawn_blocking(move || {
        let progress_callback = progress.as_ref().map(|progress| -> Box<dyn Fn(usize)> {
            let progress = progress.clone();
            Box::new(move |written| progress(RunProgress::MetadataFilesWritten(written)))
        });

        catalog.record_scan(
            public_key,
            &sealed,
            version,
            &storable,
            close_missing,
            progress_callback.as_deref(),
        )
    })
    .await??;

    Ok((stored_blobs, destination_count, upload_skipped))
}

/// Seal + store every new content id (one not already in the catalog) to all
/// destinations, in parallel with a bounded worker pool (same bound as scanning,
/// so memory/CPU stay in check). Returns the wrapped keys to record and the files
/// safe to record (those whose content didn't change since the scan).
async fn upload_new_content(
    catalog: &SqliteCatalog,
    stores: &[LocalStore],
    public_key: PublicKey,
    naming_key: &NamingKey,
    files: &[ScannedFile],
    skipped_files_log: &Path,
    progress: Option<&ProgressCallback>,
) -> Result<UploadOutcome> {
    // Distinct content ids not yet stored, with a source path to read. Load the
    // set of already-stored ids in one query rather than a blocking catalog hit
    // per scanned file (which would stall the async runtime on large backups).
    let stored_ids: HashSet<String> = catalog.all_content_ids()?.into_iter().collect();
    let mut seen = HashSet::new();
    let mut new_content: Vec<(String, PathBuf)> = Vec::new();
    for file in files {
        if seen.insert(file.hash.clone()) && !stored_ids.contains(&file.hash) {
            new_content.push((file.hash.clone(), file.path.clone()));
        }
    }

    if let Some(progress) = progress {
        progress(RunProgress::StorePhaseStarted(new_content.len()));
    }

    let worker_count = scan_worker_count();
    let semaphore = Arc::new(Semaphore::new(worker_count));
    let available_workers = new_worker_pool(worker_count);
    let stores = Arc::new(stores.to_vec());

    let tasks = FuturesUnordered::new();
    for (hash, path) in new_content {
        let semaphore = semaphore.clone();
        let available_workers = available_workers.clone();
        let stores = stores.clone();
        let naming_key = naming_key.clone();
        let log = skipped_files_log.to_path_buf();
        let progress = progress.cloned();

        tasks.push(tokio::spawn(async move {
            let _permit = semaphore.acquire_owned().await?;
            // `worker` releases its id to the pool on drop (incl. on panic/error).
            let worker = acquire_worker_id(&available_workers)?;
            if let Some(progress) = &progress {
                progress(RunProgress::ProcessingFile {
                    worker_id: worker.id(),
                    path: path.clone(),
                });
            }

            let result = seal_one(&stores, public_key, &naming_key, &hash, &path, &log).await;

            if let Some(progress) = &progress {
                progress(RunProgress::WorkerFinished(worker.id()));
            }

            result.map(|wrapped| (hash, wrapped))
        }));
    }

    let mut sealed = SealedKeys::new();
    let mut skipped_hashes = HashSet::new();
    let mut tasks = tasks;
    while let Some(joined) = tasks.next().await {
        let (hash, wrapped) = joined??;
        match wrapped {
            Some(wrapped) => {
                sealed.insert(hash, wrapped);
            }
            None => {
                skipped_hashes.insert(hash);
            }
        }
        if let Some(progress) = progress {
            progress(RunProgress::FileFinished);
        }
    }

    let stored_blobs = sealed.len();
    let storable = files
        .iter()
        .filter(|file| !skipped_hashes.contains(&file.hash))
        .cloned()
        .collect();

    Ok(UploadOutcome {
        sealed,
        storable,
        stored_blobs,
        skipped: skipped_hashes.len(),
    })
}

/// Read, verify, compress+encrypt, and store one content id to every store.
/// `Ok(None)` means the file was skipped (unreadable or changed since the scan).
async fn seal_one(
    stores: &[LocalStore],
    public_key: PublicKey,
    naming_key: &NamingKey,
    hash: &str,
    path: &Path,
    skipped_files_log: &Path,
) -> Result<Option<(Vec<u8>, [u8; 32])>> {
    let bytes = match tokio::fs::read(path).await {
        Ok(bytes) => bytes,
        Err(err) => {
            log_skipped_entry(
                skipped_files_log,
                &format!("Read error for {}: {err}", path.display()),
            )
            .await?;
            return Ok(None);
        }
    };

    // Verify the bytes still match the scanned id (changed-during-backup).
    if blake3_keyed_bytes(&bytes, naming_key) != hash {
        log_skipped_entry(
            skipped_files_log,
            &format!("Changed during backup, skipped: {}", path.display()),
        )
        .await?;
        return Ok(None);
    }

    let seal_id = hash.to_string();
    let sealed =
        tokio::task::spawn_blocking(move || seal_content(&bytes, &public_key, &seal_id)).await??;

    // Write to every destination. If a later destination fails, earlier ones keep
    // the blob: that's a tolerated orphan, not corruption — the run aborts before
    // `record_scan`, so the version is never completed, a re-run overwrites the
    // blob (LocalStore replaces), and a future `prune` reclaims any leftovers.
    for store in stores {
        store.put(hash, &sealed.blob).await?;
    }

    Ok(Some((sealed.wrapped_key, sealed.ephemeral_public_key)))
}

async fn queue_scan_tasks(
    directories: &[PathBuf],
    ignore_rules: IgnoreRules,
    progress: Option<ProgressCallback>,
    skipped_files_log: &Path,
    naming_key: &NamingKey,
) -> Result<QueuedScan> {
    let tasks = FuturesUnordered::new();
    let worker_count = scan_worker_count();
    let semaphore = Arc::new(Semaphore::new(worker_count));
    let available_workers = new_worker_pool(worker_count);
    let mut queued_files = 0_usize;
    let mut skipped_entries = 0_usize;

    for directory in directories {
        if !directory.exists() {
            return Err(anyhow!("Directory does not exist: {}", directory.display()));
        }

        let iterator = walk_directory(directory, ignore_rules);

        for file_result in iterator {
            match file_result {
                Ok(file_path) => {
                    if file_path.exists() {
                        queued_files += 1;
                        let semaphore = semaphore.clone();
                        let log_file = skipped_files_log.to_path_buf();
                        let progress = progress.clone();
                        let available_workers = available_workers.clone();
                        let naming_key = naming_key.clone();

                        tasks.push(tokio::spawn(async move {
                            let _permit = semaphore.acquire_owned().await?;
                            // `worker` releases its id on drop (incl. on panic/error).
                            let worker = acquire_worker_id(&available_workers)?;
                            process_file(file_path, log_file, progress, worker.id(), &naming_key)
                                .await
                        }));
                    } else {
                        log_skipped_entry(
                            skipped_files_log,
                            &format!("Missing file: {}", file_path.display()),
                        )
                        .await?;
                        skipped_entries += 1;
                    }
                }
                Err(err) => {
                    log_skipped_entry(skipped_files_log, &format!("Walk error: {err}")).await?;
                    skipped_entries += 1;
                }
            }
        }
    }

    Ok(QueuedScan {
        tasks,
        queued_files,
        skipped_entries,
    })
}

/// Pool of free worker ids (used only for labelling progress rows). A plain
/// `std::sync::Mutex` is fine: the critical section is just a pop/push, never held
/// across an `.await`.
type WorkerPool = Arc<Mutex<Vec<usize>>>;

#[must_use]
fn new_worker_pool(worker_count: usize) -> WorkerPool {
    Arc::new(Mutex::new((1..=worker_count).rev().collect()))
}

/// A borrowed worker id that returns itself to the pool on drop, so a panicking
/// task can't leak its id (which would shrink the labelled-worker space on a retry
/// within the same process).
struct WorkerId {
    id: usize,
    pool: WorkerPool,
}

impl WorkerId {
    fn id(&self) -> usize {
        self.id
    }
}

impl Drop for WorkerId {
    fn drop(&mut self) {
        // Recover from poisoning: the data (a Vec of ids) is still valid even if a
        // previous holder panicked, and we never hold this lock across a panic point.
        self.pool
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(self.id);
    }
}

fn acquire_worker_id(pool: &WorkerPool) -> Result<WorkerId> {
    let id = pool
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .pop()
        .ok_or_else(|| anyhow!("No scan worker id available"))?;
    Ok(WorkerId {
        id,
        pool: pool.clone(),
    })
}

async fn collect_scan_results(
    mut tasks: FuturesUnordered<tokio::task::JoinHandle<Result<Option<ScannedFile>>>>,
    progress: Option<&ProgressCallback>,
    skipped_files_log: &Path,
    initial_skipped_entries: usize,
) -> Result<ScanResults> {
    let mut errors = Vec::new();
    let mut scanned_files = Vec::new();
    let mut skipped_entries = initial_skipped_entries;

    while let Some(result) = tasks.next().await {
        match result? {
            Ok(Some(scanned_file)) => {
                scanned_files.push(scanned_file);
                if let Some(progress) = progress {
                    progress(RunProgress::FileFinished);
                }
            }
            Ok(None) => {
                skipped_entries += 1;
                if let Some(progress) = progress {
                    progress(RunProgress::FileFinished);
                }
            }
            Err(err) => {
                log_skipped_entry(skipped_files_log, &format!("Task error: {err}")).await?;
                skipped_entries += 1;
                errors.push(err);
            }
        }
    }

    if !errors.is_empty() {
        return Err(anyhow!("Some tasks failed: {errors:?}"));
    }

    Ok(ScanResults {
        files: scanned_files,
        skipped_entries,
    })
}

// Returns an iterator over files in a directory, using backup-specific ignore rules by default.
fn walk_directory(
    base_dir: &Path,
    ignore_rules: IgnoreRules,
) -> impl Iterator<Item = Result<PathBuf, ignore::Error>> {
    let mut builder = WalkBuilder::new(base_dir);

    builder
        .follow_links(true)
        .hidden(false)
        .git_exclude(false)
        .git_global(false)
        .git_ignore(ignore_rules.gitignore)
        .require_git(false)
        .parents(true);

    if ignore_rules.backupignore {
        builder.add_custom_ignore_filename(BACKUP_IGNORE_FILE);
    }

    builder.build().filter_map(|entry| match entry {
        Ok(e) if e.path().is_file() => Some(Ok(e.into_path())),
        Ok(_) => None,
        Err(err) => Some(Err(err)),
    })
}

async fn process_file(
    file_path: PathBuf,
    skipped_files_log: PathBuf,
    progress: Option<ProgressCallback>,
    worker_id: usize,
    naming_key: &NamingKey,
) -> Result<Option<ScannedFile>> {
    if let Some(progress) = &progress {
        progress(RunProgress::ProcessingFile {
            worker_id,
            path: file_path.clone(),
        });
    }

    let hash = match calculate_hash(file_path.clone(), naming_key).await {
        Ok(h) => h,
        Err(e) => {
            log_skipped_entry(
                &skipped_files_log,
                &format!("Hash error for {}: {e}", file_path.display()),
            )
            .await?;
            if let Some(progress) = &progress {
                progress(RunProgress::WorkerFinished(worker_id));
            }
            return Ok(None);
        }
    };

    if let Some(progress) = &progress {
        progress(RunProgress::WorkerFinished(worker_id));
    }

    Ok(Some(ScannedFile {
        path: file_path,
        hash,
    }))
}

async fn calculate_hash(file_path: PathBuf, naming_key: &NamingKey) -> Result<String> {
    let file_path_clone = file_path.clone();
    let naming_key = naming_key.clone();
    tokio::task::spawn_blocking(move || blake3_keyed(&file_path, &naming_key))
        .await?
        .map_err(|err| {
            let path = file_path_clone.display();
            anyhow!("Hash computation for: {path} failed: {err}")
        })
}

// Check if the log file is empty
async fn is_log_file_empty(log_file: &Path) -> io::Result<bool> {
    let metadata = tokio::fs::metadata(log_file).await?;
    Ok(metadata.len() == 0)
}

async fn cleanup_skipped_log(skipped_files_log: &Path) -> Result<()> {
    if is_log_file_empty(skipped_files_log).await? {
        remove_file(skipped_files_log).await?;
    }

    Ok(())
}

async fn log_skipped_entry(log_file: &Path, message: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
        .await?;

    file.write_all(format!("{message}\n").as_bytes()).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::sqlite::{RestoreEntry, SqliteCatalog},
        utils::crypto::{
            content_key_aad, content_keypair, decrypt, generate_naming_key, seal_naming_key,
        },
    };
    use anyhow::Context;
    use bip39::{Language, Mnemonic};
    use std::{collections::BTreeMap, fs};
    use x25519_dalek::{PublicKey, StaticSecret};

    struct ExpectedVersion {
        version: i64,
        label: &'static str,
        entries: Vec<RestoreEntry>,
    }

    struct TestPaths {
        source_a: PathBuf,
        source_b: PathBuf,
        alpha: PathBuf,
        duplicate_a: PathBuf,
        duplicate_b: PathBuf,
        stable: PathBuf,
        new_file: PathBuf,
    }

    impl TestPaths {
        fn new(root: &Path) -> Self {
            let source_a = root.join("source-a");
            let source_b = root.join("source-b");

            Self {
                alpha: source_a.join("alpha.txt"),
                duplicate_a: source_a.join("duplicate.txt"),
                duplicate_b: source_b.join("duplicate.txt"),
                stable: source_b.join("stable.txt"),
                new_file: source_a.join("new.txt"),
                source_a,
                source_b,
            }
        }

        fn sources(&self) -> Vec<PathBuf> {
            vec![self.source_a.clone(), self.source_b.clone()]
        }
    }

    fn public_key() -> PublicKey {
        let private_key = StaticSecret::from([7_u8; 32]);
        PublicKey::from(&private_key)
    }

    #[test]
    fn scan_worker_count_is_in_range() {
        // Always at least one worker, never more than the worker-id space (u8).
        let count = scan_worker_count();
        assert!((1..=u8::MAX as usize).contains(&count));
    }

    #[test]
    fn worker_id_guard_returns_id_to_pool_on_drop() -> Result<()> {
        let pool = new_worker_pool(2);

        let a = acquire_worker_id(&pool)?;
        let b = acquire_worker_id(&pool)?;
        let id_a = a.id();

        // Pool exhausted while both guards are held.
        assert!(acquire_worker_id(&pool).is_err());

        // Dropping a guard returns its id; the next acquire reuses it.
        drop(a);
        let c = acquire_worker_id(&pool)?;
        assert_eq!(c.id(), id_a);

        drop(b);
        drop(c);
        Ok(())
    }

    fn recovery_keypair() -> Result<(Mnemonic, PublicKey)> {
        let mnemonic = Mnemonic::generate_in(Language::English, 12)?;
        let (_, public_key) = content_keypair(&mnemonic)?;

        Ok((mnemonic, public_key))
    }

    fn hash_content(naming_key: &NamingKey, content: &str) -> String {
        let mut hasher = ::blake3::Hasher::new_keyed(naming_key);
        hasher.update(content.as_bytes());
        hasher.finalize().to_hex().to_string()
    }

    fn write_test_file(
        expected: &mut BTreeMap<PathBuf, String>,
        path: &Path,
        content: &str,
        naming_key: &NamingKey,
    ) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("Test file has no parent directory"))?;
        fs::create_dir_all(parent)?;
        fs::write(path, content)?;
        expected.insert(path.to_path_buf(), hash_content(naming_key, content));

        Ok(())
    }

    fn remove_test_file(expected: &mut BTreeMap<PathBuf, String>, path: &Path) -> Result<()> {
        fs::remove_file(path)?;
        expected.remove(path);

        Ok(())
    }

    fn snapshot_from_expected(expected: &BTreeMap<PathBuf, String>) -> Vec<RestoreEntry> {
        expected
            .iter()
            .map(|(path, hash)| RestoreEntry {
                path: path.clone(),
                hash: hash.clone(),
            })
            .collect()
    }

    async fn run_and_record_expected(
        config_dir: &Path,
        name: &str,
        label: &'static str,
        expected: &BTreeMap<PathBuf, String>,
        expected_versions: &mut Vec<ExpectedVersion>,
        naming_key: &NamingKey,
    ) -> Result<()> {
        let result = run(RunBackupRequest {
            name: name.to_string(),
            config_dir: config_dir.to_path_buf(),
            ignore_rules: IgnoreRules::backupignore_only(),
            dry_run: false,
            progress: None,
            naming_key: naming_key.clone(),
        })
        .await?;
        let expected_version = i64::try_from(expected_versions.len() + 1)?;

        assert_eq!(
            result.version, expected_version,
            "version mismatch for {label}"
        );
        assert_eq!(
            result.scanned_files,
            expected.len(),
            "scanned file count mismatch for version {} ({label})",
            result.version
        );

        expected_versions.push(ExpectedVersion {
            version: result.version,
            label,
            entries: snapshot_from_expected(expected),
        });

        Ok(())
    }

    fn format_entries(root: &Path, entries: &[RestoreEntry]) -> String {
        let mut output = String::new();

        for entry in entries {
            if !output.is_empty() {
                output.push('\n');
            }

            let path = entry.path.strip_prefix(root).unwrap_or(&entry.path);
            output.push_str(&path.display().to_string());
            output.push(' ');
            output.push_str(&entry.hash);
        }

        output
    }

    fn assert_restore_entries(
        catalog: &SqliteCatalog,
        root: &Path,
        expected: &ExpectedVersion,
    ) -> Result<()> {
        let actual = catalog.restore_entries(expected.version)?;

        assert_eq!(
            format_entries(root, &actual),
            format_entries(root, &expected.entries),
            "restore snapshot mismatch for version {} ({})",
            expected.version,
            expected.label
        );
        assert_eq!(
            actual, expected.entries,
            "restore entries differ for version {} ({})",
            expected.version, expected.label
        );

        Ok(())
    }

    fn assert_recovery_material_is_not_stored(catalog: &SqliteCatalog) -> Result<()> {
        assert_eq!(catalog.recovery_secret_count()?, 0);

        Ok(())
    }

    fn assert_file_key_can_be_unwrapped(
        catalog: &SqliteCatalog,
        mnemonic: &Mnemonic,
    ) -> Result<()> {
        let (hash, encrypted_key, ephemeral_public_key) = catalog.first_wrapped_file_key()?;
        let file_key = decrypt(
            &encrypted_key,
            &ephemeral_public_key,
            mnemonic,
            &content_key_aad(&hash),
        )?;

        assert_eq!(file_key.len(), 32);

        Ok(())
    }

    async fn record_initial_versions(
        root: &Path,
        name: &str,
        paths: &TestPaths,
        expected: &mut BTreeMap<PathBuf, String>,
        expected_versions: &mut Vec<ExpectedVersion>,
        naming_key: &NamingKey,
    ) -> Result<()> {
        write_test_file(expected, &paths.alpha, "alpha original\n", naming_key)?;
        write_test_file(
            expected,
            &paths.duplicate_a,
            "same duplicate content\n",
            naming_key,
        )?;
        write_test_file(
            expected,
            &paths.duplicate_b,
            "same duplicate content\n",
            naming_key,
        )?;
        write_test_file(expected, &paths.stable, "stable content\n", naming_key)?;

        run_and_record_expected(
            root,
            name,
            "initial files with duplicate content",
            expected,
            expected_versions,
            naming_key,
        )
        .await?;

        run_and_record_expected(
            root,
            name,
            "unchanged scan",
            expected,
            expected_versions,
            naming_key,
        )
        .await
    }

    async fn record_changed_versions(
        root: &Path,
        name: &str,
        paths: &TestPaths,
        expected: &mut BTreeMap<PathBuf, String>,
        expected_versions: &mut Vec<ExpectedVersion>,
        naming_key: &NamingKey,
    ) -> Result<()> {
        write_test_file(expected, &paths.alpha, "alpha modified\n", naming_key)?;
        run_and_record_expected(
            root,
            name,
            "alpha modified",
            expected,
            expected_versions,
            naming_key,
        )
        .await?;

        remove_test_file(expected, &paths.duplicate_b)?;
        run_and_record_expected(
            root,
            name,
            "duplicate removed from source-b",
            expected,
            expected_versions,
            naming_key,
        )
        .await?;

        write_test_file(expected, &paths.new_file, "new file content\n", naming_key)?;
        run_and_record_expected(
            root,
            name,
            "new file added",
            expected,
            expected_versions,
            naming_key,
        )
        .await?;

        write_test_file(
            expected,
            &paths.duplicate_b,
            "same duplicate content\n",
            naming_key,
        )?;
        run_and_record_expected(
            root,
            name,
            "duplicate restored with original content",
            expected,
            expected_versions,
            naming_key,
        )
        .await?;

        write_test_file(
            expected,
            &paths.duplicate_a,
            "updated duplicate content\n",
            naming_key,
        )?;
        write_test_file(
            expected,
            &paths.duplicate_b,
            "updated duplicate content\n",
            naming_key,
        )?;
        run_and_record_expected(
            root,
            name,
            "both duplicates modified to matching content",
            expected,
            expected_versions,
            naming_key,
        )
        .await
    }

    async fn record_deleted_versions(
        root: &Path,
        name: &str,
        paths: &TestPaths,
        expected: &mut BTreeMap<PathBuf, String>,
        expected_versions: &mut Vec<ExpectedVersion>,
        naming_key: &NamingKey,
    ) -> Result<()> {
        remove_test_file(expected, &paths.alpha)?;
        run_and_record_expected(
            root,
            name,
            "alpha deleted",
            expected,
            expected_versions,
            naming_key,
        )
        .await?;

        write_test_file(expected, &paths.alpha, "alpha original\n", naming_key)?;
        run_and_record_expected(
            root,
            name,
            "alpha restored with original content",
            expected,
            expected_versions,
            naming_key,
        )
        .await?;

        remove_test_file(expected, &paths.duplicate_a)?;
        remove_test_file(expected, &paths.duplicate_b)?;
        run_and_record_expected(
            root,
            name,
            "both duplicates deleted",
            expected,
            expected_versions,
            naming_key,
        )
        .await
    }

    #[tokio::test]
    async fn ten_filesystem_versions_match_restore_snapshots() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let name = "metadata";
        let catalog = SqliteCatalog::initialize(&temp_dir.path().join(format!("{name}.db")))?;
        let paths = TestPaths::new(temp_dir.path());
        let (mnemonic, public_key) = recovery_keypair()?;
        let naming_key: NamingKey = Arc::new(generate_naming_key());
        let mut expected = BTreeMap::new();
        let mut expected_versions = Vec::new();

        catalog.save_public_key(&public_key)?;
        catalog.save_sealed_naming_key(&seal_naming_key(&naming_key, &public_key)?)?;
        catalog.save_directories(&paths.sources())?;

        record_initial_versions(
            temp_dir.path(),
            name,
            &paths,
            &mut expected,
            &mut expected_versions,
            &naming_key,
        )
        .await?;
        record_changed_versions(
            temp_dir.path(),
            name,
            &paths,
            &mut expected,
            &mut expected_versions,
            &naming_key,
        )
        .await?;
        record_deleted_versions(
            temp_dir.path(),
            name,
            &paths,
            &mut expected,
            &mut expected_versions,
            &naming_key,
        )
        .await?;

        for expected_version in &expected_versions {
            assert_restore_entries(&catalog, temp_dir.path(), expected_version)?;
        }

        assert_eq!(catalog.count_rows("Files")?, 6);
        assert_eq!(catalog.count_unique_hashes()?, 6);
        assert_eq!(catalog.count_rows("FileNames")?, 10);
        assert_eq!(catalog.count_active_file_names()?, 3);
        assert_eq!(
            catalog.restore_entries(10)?,
            snapshot_from_expected(&expected)
        );

        assert_recovery_material_is_not_stored(&catalog)?;
        assert_file_key_can_be_unwrapped(&catalog, &mnemonic)
            .context("stored file key should unwrap with recovery mnemonic")?;

        Ok(())
    }

    fn test_catalog() -> Result<(tempfile::TempDir, SqliteCatalog)> {
        let temp_dir = tempfile::tempdir()?;
        let catalog = SqliteCatalog::initialize(&temp_dir.path().join("test.db"))?;

        Ok((temp_dir, catalog))
    }

    fn scan_files(catalog: &SqliteCatalog, version: i64, files: &[(&str, &str)]) -> Result<()> {
        let scanned_files = files
            .iter()
            .map(|(name, hash)| ScannedFile {
                path: PathBuf::from(format!("/backup/{name}")),
                hash: (*hash).to_string(),
            })
            .collect::<Vec<_>>();

        catalog.record_scan(
            public_key(),
            &SealedKeys::new(),
            version,
            &scanned_files,
            true,
            None,
        )
    }

    fn restore_hashes(catalog: &SqliteCatalog, version: i64) -> Result<Vec<String>> {
        Ok(catalog
            .restore_entries(version)?
            .into_iter()
            .map(|entry| entry.hash)
            .collect())
    }

    fn relative_walked_files(root: &Path, ignore_rules: IgnoreRules) -> Result<Vec<PathBuf>> {
        let mut files = walk_directory(root, ignore_rules)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|path| {
                path.strip_prefix(root)
                    .map(Path::to_path_buf)
                    .with_context(|| format!("{} is not under {}", path.display(), root.display()))
            })
            .collect::<Result<Vec<_>>>()?;

        files.sort();

        Ok(files)
    }

    #[test]
    fn backupignore_is_default_ignore_file() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let root = temp_dir.path();

        fs::write(root.join(BACKUP_IGNORE_FILE), "ignored.txt\n")?;
        fs::write(root.join(".gitignore"), "git-ignored.txt\n")?;
        fs::write(root.join("ignored.txt"), "ignored by backup rules")?;
        fs::write(root.join("git-ignored.txt"), "not ignored by default")?;
        fs::write(root.join("kept.txt"), "kept")?;

        assert_eq!(
            relative_walked_files(root, IgnoreRules::backupignore_only())?,
            vec![
                PathBuf::from(BACKUP_IGNORE_FILE),
                PathBuf::from(".gitignore"),
                PathBuf::from("git-ignored.txt"),
                PathBuf::from("kept.txt"),
            ]
        );

        Ok(())
    }

    #[test]
    fn gitignore_is_only_used_when_enabled() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let root = temp_dir.path();

        fs::write(root.join(BACKUP_IGNORE_FILE), "backup-ignored.txt\n")?;
        fs::write(root.join(".gitignore"), "git-ignored.txt\n")?;
        fs::write(root.join("backup-ignored.txt"), "ignored by backup rules")?;
        fs::write(root.join("git-ignored.txt"), "ignored by git rules")?;
        fs::write(root.join("kept.txt"), "kept")?;

        assert_eq!(
            relative_walked_files(
                root,
                IgnoreRules {
                    backupignore: true,
                    gitignore: true,
                },
            )?,
            vec![
                PathBuf::from(BACKUP_IGNORE_FILE),
                PathBuf::from(".gitignore"),
                PathBuf::from("kept.txt"),
            ]
        );

        Ok(())
    }

    #[test]
    fn no_ignore_disables_backupignore_and_gitignore() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let root = temp_dir.path();

        fs::write(root.join(BACKUP_IGNORE_FILE), "backup-ignored.txt\n")?;
        fs::write(root.join(".gitignore"), "git-ignored.txt\n")?;
        fs::write(root.join("backup-ignored.txt"), "included")?;
        fs::write(root.join("git-ignored.txt"), "included")?;
        fs::write(root.join("kept.txt"), "kept")?;

        assert_eq!(
            relative_walked_files(root, IgnoreRules::none())?,
            vec![
                PathBuf::from(BACKUP_IGNORE_FILE),
                PathBuf::from(".gitignore"),
                PathBuf::from("backup-ignored.txt"),
                PathBuf::from("git-ignored.txt"),
                PathBuf::from("kept.txt"),
            ]
        );

        Ok(())
    }

    #[test]
    fn unchanged_file_does_not_create_new_history_row() -> Result<()> {
        let (_temp_dir, catalog) = test_catalog()?;

        scan_files(&catalog, 1, &[("file.txt", "hash-a")])?;
        scan_files(&catalog, 2, &[("file.txt", "hash-a")])?;

        assert_eq!(catalog.count_rows("FileNames")?, 1);
        assert_eq!(catalog.count_active_file_names()?, 1);
        assert_eq!(restore_hashes(&catalog, 2)?, vec!["hash-a".to_string()]);

        Ok(())
    }

    #[test]
    fn modified_file_restores_only_current_content() -> Result<()> {
        let (_temp_dir, catalog) = test_catalog()?;

        scan_files(&catalog, 1, &[("file.txt", "hash-a")])?;
        scan_files(&catalog, 2, &[("file.txt", "hash-b")])?;

        assert_eq!(restore_hashes(&catalog, 1)?, vec!["hash-a".to_string()]);
        assert_eq!(restore_hashes(&catalog, 2)?, vec!["hash-b".to_string()]);

        Ok(())
    }

    #[test]
    fn missing_file_is_closed_as_deleted() -> Result<()> {
        let (_temp_dir, catalog) = test_catalog()?;

        scan_files(&catalog, 1, &[("file.txt", "hash-a")])?;
        scan_files(&catalog, 2, &[])?;

        assert_eq!(restore_hashes(&catalog, 1)?, vec!["hash-a".to_string()]);
        assert!(restore_hashes(&catalog, 2)?.is_empty());

        Ok(())
    }

    #[test]
    fn reverted_content_keeps_separate_intervals() -> Result<()> {
        let (_temp_dir, catalog) = test_catalog()?;

        scan_files(&catalog, 1, &[("file.txt", "hash-a")])?;
        scan_files(&catalog, 2, &[("file.txt", "hash-b")])?;
        scan_files(&catalog, 3, &[("file.txt", "hash-a")])?;

        assert_eq!(catalog.count_rows("FileNames")?, 3);
        assert_eq!(restore_hashes(&catalog, 1)?, vec!["hash-a".to_string()]);
        assert_eq!(restore_hashes(&catalog, 2)?, vec!["hash-b".to_string()]);
        assert_eq!(restore_hashes(&catalog, 3)?, vec!["hash-a".to_string()]);

        Ok(())
    }

    #[tokio::test]
    async fn run_stores_blobs_that_decrypt_and_dedup() -> Result<()> {
        use crate::{
            engine::{
                create::{CreateBackupRequest, create},
                wkey,
            },
            storage::local::LocalStore,
            utils::crypto::open_content,
        };

        let tmp = tempfile::tempdir()?;
        let cfg = tmp.path().join("cfg");
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");
        fs::create_dir_all(&cfg)?;
        fs::create_dir_all(&src)?;

        // Two files with identical content (dedup) + one distinct.
        fs::write(src.join("a.txt"), b"hello world")?;
        fs::write(src.join("b.txt"), b"hello world")?;
        fs::write(src.join("c.txt"), b"different")?;

        let created = create(CreateBackupRequest {
            name: "t".to_string(),
            config_dir: cfg.clone(),
            directories: vec![src.clone()],
            files: Vec::new(),
            destinations: vec![dest.to_string_lossy().into_owned()],
        })?;
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, &created.recovery_phrase)?;

        let naming_key: NamingKey =
            Arc::new(wkey::load_naming_key(&cfg, "t")?.ok_or_else(|| anyhow!("missing wkey"))?);

        let result = run(RunBackupRequest {
            name: "t".to_string(),
            config_dir: cfg.clone(),
            ignore_rules: IgnoreRules::backupignore_only(),
            dry_run: false,
            progress: None,
            naming_key: naming_key.clone(),
        })
        .await?;

        // Three files, two unique contents -> two stored blobs to one destination.
        assert_eq!(result.scanned_files, 3);
        assert_eq!(result.stored_blobs, 2);
        assert_eq!(result.destination_count, 1);

        // The stored blob for "hello world" decrypts byte-for-byte.
        let id = blake3_keyed_bytes(b"hello world", &naming_key);
        let store = LocalStore::new(&dest);
        assert!(store.exists(&id).await?);

        let catalog = SqliteCatalog::open(&cfg.join("t.db"))?;
        let (wrapped, eph) = catalog
            .wrapped_content_key(&id)?
            .ok_or_else(|| anyhow!("no wrapped key for content"))?;
        let key_vec = decrypt(&wrapped, &eph, &mnemonic, &content_key_aad(&id))?;
        let key: [u8; 32] = key_vec
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("bad key length"))?;
        let blob = store.get(&id).await?;
        let plaintext = open_content(&blob, &id, &key)?;
        assert_eq!(plaintext.as_slice(), b"hello world");

        // Re-run with no changes stores nothing new (dedup / idempotent).
        let again = run(RunBackupRequest {
            name: "t".to_string(),
            config_dir: cfg.clone(),
            ignore_rules: IgnoreRules::backupignore_only(),
            dry_run: false,
            progress: None,
            naming_key,
        })
        .await?;
        assert_eq!(again.stored_blobs, 0);

        Ok(())
    }

    #[tokio::test]
    async fn run_overwrites_orphan_blob_from_interrupted_run() -> Result<()> {
        use crate::{
            engine::{
                create::{CreateBackupRequest, create},
                wkey,
            },
            storage::local::LocalStore,
            utils::crypto::open_content,
        };

        let tmp = tempfile::tempdir()?;
        let cfg = tmp.path().join("cfg");
        let src = tmp.path().join("src");
        let dest = tmp.path().join("dest");
        fs::create_dir_all(&cfg)?;
        fs::create_dir_all(&src)?;
        fs::write(src.join("a.txt"), b"hello world")?;

        let created = create(CreateBackupRequest {
            name: "t".to_string(),
            config_dir: cfg.clone(),
            directories: vec![src.clone()],
            files: Vec::new(),
            destinations: vec![dest.to_string_lossy().into_owned()],
        })?;
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, &created.recovery_phrase)?;
        let naming_key: NamingKey =
            Arc::new(wkey::load_naming_key(&cfg, "t")?.ok_or_else(|| anyhow!("missing wkey"))?);

        // Simulate an interrupted run: an orphan blob exists at the content id
        // (bytes from a lost key), but there is no Files row for it.
        let id = blake3_keyed_bytes(b"hello world", &naming_key);
        let store = LocalStore::new(&dest);
        store.put(&id, b"garbage from a cancelled run").await?;

        run(RunBackupRequest {
            name: "t".to_string(),
            config_dir: cfg.clone(),
            ignore_rules: IgnoreRules::backupignore_only(),
            dry_run: false,
            progress: None,
            naming_key,
        })
        .await?;

        // The run must overwrite the orphan so the stored blob matches the key
        // it recorded — i.e. it decrypts to the real content.
        let catalog = SqliteCatalog::open(&cfg.join("t.db"))?;
        let (wrapped, eph) = catalog
            .wrapped_content_key(&id)?
            .ok_or_else(|| anyhow!("no wrapped key"))?;
        let key_vec = decrypt(&wrapped, &eph, &mnemonic, &content_key_aad(&id))?;
        let key: [u8; 32] = key_vec
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("bad key length"))?;
        let blob = store.get(&id).await?;
        let plaintext = open_content(&blob, &id, &key)?;
        assert_eq!(plaintext.as_slice(), b"hello world");

        Ok(())
    }

    #[test]
    fn latest_version_only_returns_completed() -> Result<()> {
        let (_temp_dir, catalog) = test_catalog()?;

        // A created-but-not-recorded version (interrupted run) is not "latest".
        let version = catalog.create_version()?;
        assert_eq!(catalog.latest_version()?, None);

        // Recording the scan marks it complete.
        catalog.record_scan(
            public_key(),
            &SealedKeys::new(),
            version,
            &[ScannedFile {
                path: PathBuf::from("/backup/a.txt"),
                hash: "hash-a".to_string(),
            }],
            true,
            None,
        )?;
        assert_eq!(catalog.latest_version()?, Some(version));

        Ok(())
    }
}
