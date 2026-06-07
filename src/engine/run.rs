use crate::{
    db::sqlite::{ScannedFile, SqliteCatalog},
    utils::hash::blake3,
};
use anyhow::{Result, anyhow};
use futures::stream::{FuturesUnordered, StreamExt};
use ignore::WalkBuilder;
use std::{
    cmp,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    fs::{OpenOptions, remove_file, write},
    io::{self, AsyncWriteExt},
    sync::Semaphore,
};
use tracing::{debug, instrument};

const BACKUP_IGNORE_FILE: &str = ".backupignore";

pub type ProgressCallback = Arc<dyn Fn(RunProgress) + Send + Sync>;

#[derive(Clone, Debug)]
pub enum RunProgress {
    FilesDiscovered(usize),
    FileFinished,
    MetadataFilesWritten(usize),
    MetadataWriteStarted(usize),
    ProcessingFile { worker_id: usize, path: PathBuf },
    WorkerFinished(usize),
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
    cmp::min((num_cpus::get_physical() - 2).max(1), u8::MAX as usize)
}

pub struct RunBackupRequest {
    pub name: String,
    pub config_dir: PathBuf,
    pub ignore_rules: IgnoreRules,
    pub dry_run: bool,
    pub progress: Option<ProgressCallback>,
}

pub struct RunBackupResult {
    pub version: i64,
    pub scanned_files: usize,
    pub skipped_entries: usize,
    pub skipped_files_log: PathBuf,
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
    let skipped_entries = scan_results.skipped_entries;
    let scanned_file_count = scan_results.files.len();

    if !request.dry_run {
        let catalog = catalog.clone();
        let progress = request.progress.clone();
        if let Some(progress) = &progress {
            progress(RunProgress::MetadataWriteStarted(scanned_file_count));
        }

        tokio::task::spawn_blocking(move || {
            let progress_callback = progress.as_ref().map(|progress| -> Box<dyn Fn(usize)> {
                let progress = progress.clone();
                Box::new(move |written| progress(RunProgress::MetadataFilesWritten(written)))
            });

            catalog.record_scan(
                public_key,
                backup_version,
                &scan_results.files,
                skipped_entries == 0,
                progress_callback.as_deref(),
            )
        })
        .await??;
    }

    if skipped_entries == 0 {
        cleanup_skipped_log(&skipped_files_log).await?;
    }

    Ok(RunBackupResult {
        version: backup_version,
        scanned_files: scanned_file_count,
        skipped_entries,
        skipped_files_log,
    })
}

async fn queue_scan_tasks(
    directories: &[PathBuf],
    ignore_rules: IgnoreRules,
    progress: Option<ProgressCallback>,
    skipped_files_log: &Path,
) -> Result<QueuedScan> {
    let tasks = FuturesUnordered::new();
    let worker_count = scan_worker_count();
    let semaphore = Arc::new(Semaphore::new(worker_count));
    let available_workers = Arc::new(tokio::sync::Mutex::new(
        (1..=worker_count).rev().collect::<Vec<_>>(),
    ));
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

                        tasks.push(tokio::spawn(async move {
                            let _permit = semaphore.acquire_owned().await?;
                            let worker_id = acquire_worker_id(&available_workers).await?;
                            let result =
                                process_file(file_path, log_file, progress, worker_id).await;
                            release_worker_id(&available_workers, worker_id).await;

                            result
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

async fn acquire_worker_id(available_workers: &tokio::sync::Mutex<Vec<usize>>) -> Result<usize> {
    available_workers
        .lock()
        .await
        .pop()
        .ok_or_else(|| anyhow!("No scan worker id available"))
}

async fn release_worker_id(available_workers: &tokio::sync::Mutex<Vec<usize>>, worker_id: usize) {
    available_workers.lock().await.push(worker_id);
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
) -> Result<Option<ScannedFile>> {
    if let Some(progress) = &progress {
        progress(RunProgress::ProcessingFile {
            worker_id,
            path: file_path.clone(),
        });
    }

    let hash = match calculate_hash(file_path.clone()).await {
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

async fn calculate_hash(file_path: PathBuf) -> Result<String> {
    let file_path_clone = file_path.clone();
    tokio::task::spawn_blocking(move || blake3(&file_path))
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
        utils::crypto::decrypt,
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

    fn recovery_keypair() -> Result<(Mnemonic, PublicKey)> {
        let mnemonic = Mnemonic::generate_in(Language::English, 12)?;
        let seed = mnemonic.to_seed("");

        let mut seed_bytes = [0_u8; 32];
        let seed_prefix = seed
            .get(..32)
            .ok_or_else(|| anyhow!("Mnemonic seed is too short"))?;
        seed_bytes.copy_from_slice(seed_prefix);

        let private_key = StaticSecret::from(seed_bytes);

        Ok((mnemonic, PublicKey::from(&private_key)))
    }

    fn hash_content(content: &str) -> String {
        ::blake3::hash(content.as_bytes()).to_hex().to_string()
    }

    fn write_test_file(
        expected: &mut BTreeMap<PathBuf, String>,
        path: &Path,
        content: &str,
    ) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("Test file has no parent directory"))?;
        fs::create_dir_all(parent)?;
        fs::write(path, content)?;
        expected.insert(path.to_path_buf(), hash_content(content));

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
    ) -> Result<()> {
        let result = run(RunBackupRequest {
            name: name.to_string(),
            config_dir: config_dir.to_path_buf(),
            ignore_rules: IgnoreRules::backupignore_only(),
            dry_run: false,
            progress: None,
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
        let (encrypted_key, ephemeral_public_key) = catalog.first_wrapped_file_key()?;
        let file_key = decrypt(&encrypted_key, &ephemeral_public_key, mnemonic)?;

        assert_eq!(file_key.len(), 32);

        Ok(())
    }

    async fn record_initial_versions(
        root: &Path,
        name: &str,
        paths: &TestPaths,
        expected: &mut BTreeMap<PathBuf, String>,
        expected_versions: &mut Vec<ExpectedVersion>,
    ) -> Result<()> {
        write_test_file(expected, &paths.alpha, "alpha original\n")?;
        write_test_file(expected, &paths.duplicate_a, "same duplicate content\n")?;
        write_test_file(expected, &paths.duplicate_b, "same duplicate content\n")?;
        write_test_file(expected, &paths.stable, "stable content\n")?;

        run_and_record_expected(
            root,
            name,
            "initial files with duplicate content",
            expected,
            expected_versions,
        )
        .await?;

        run_and_record_expected(root, name, "unchanged scan", expected, expected_versions).await
    }

    async fn record_changed_versions(
        root: &Path,
        name: &str,
        paths: &TestPaths,
        expected: &mut BTreeMap<PathBuf, String>,
        expected_versions: &mut Vec<ExpectedVersion>,
    ) -> Result<()> {
        write_test_file(expected, &paths.alpha, "alpha modified\n")?;
        run_and_record_expected(root, name, "alpha modified", expected, expected_versions).await?;

        remove_test_file(expected, &paths.duplicate_b)?;
        run_and_record_expected(
            root,
            name,
            "duplicate removed from source-b",
            expected,
            expected_versions,
        )
        .await?;

        write_test_file(expected, &paths.new_file, "new file content\n")?;
        run_and_record_expected(root, name, "new file added", expected, expected_versions).await?;

        write_test_file(expected, &paths.duplicate_b, "same duplicate content\n")?;
        run_and_record_expected(
            root,
            name,
            "duplicate restored with original content",
            expected,
            expected_versions,
        )
        .await?;

        write_test_file(expected, &paths.duplicate_a, "updated duplicate content\n")?;
        write_test_file(expected, &paths.duplicate_b, "updated duplicate content\n")?;
        run_and_record_expected(
            root,
            name,
            "both duplicates modified to matching content",
            expected,
            expected_versions,
        )
        .await
    }

    async fn record_deleted_versions(
        root: &Path,
        name: &str,
        paths: &TestPaths,
        expected: &mut BTreeMap<PathBuf, String>,
        expected_versions: &mut Vec<ExpectedVersion>,
    ) -> Result<()> {
        remove_test_file(expected, &paths.alpha)?;
        run_and_record_expected(root, name, "alpha deleted", expected, expected_versions).await?;

        write_test_file(expected, &paths.alpha, "alpha original\n")?;
        run_and_record_expected(
            root,
            name,
            "alpha restored with original content",
            expected,
            expected_versions,
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
        let mut expected = BTreeMap::new();
        let mut expected_versions = Vec::new();

        catalog.save_public_key(&public_key)?;
        catalog.save_directories(&paths.sources())?;

        record_initial_versions(
            temp_dir.path(),
            name,
            &paths,
            &mut expected,
            &mut expected_versions,
        )
        .await?;
        record_changed_versions(
            temp_dir.path(),
            name,
            &paths,
            &mut expected,
            &mut expected_versions,
        )
        .await?;
        record_deleted_versions(
            temp_dir.path(),
            name,
            &paths,
            &mut expected,
            &mut expected_versions,
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

        catalog.record_scan(public_key(), version, &scanned_files, true, None)
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
}
