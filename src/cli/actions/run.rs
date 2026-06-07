use crate::{
    cli::{actions::Action, globals::GlobalArgs},
    utils::{
        crypto::{encrypt, generate_file_key},
        db::get_public_key,
        format::format_duration,
        hash::blake3,
    },
};
use anyhow::{Result, anyhow};
use futures::stream::{FuturesUnordered, StreamExt};
use ignore::WalkBuilder;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, params};
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
use x25519_dalek::PublicKey;

struct ScannedFile {
    path: PathBuf,
    hash: String,
}

/// Handle the create action
/// # Errors
/// Returns an error if the configured backup cannot be scanned or the metadata database cannot be
/// updated.
#[instrument(skip(action, globals))]
pub async fn handle(action: Action, globals: GlobalArgs) -> Result<()> {
    // start a timer to measure the time taken to run the backup
    let timer = globals.timer.start();

    if let Action::Run {
        name,
        no_gitignore,
        no_compression: _,
        no_encryption: _,
        dry_run,
    } = action
    {
        let home_dir = globals.home;

        let skipped_files_log = home_dir.join(format!("{name}-skipped_files.log"));

        debug!("Skipped files log: {}", skipped_files_log.display());

        // Truncate the log file
        write(&skipped_files_log, "").await?;

        let db_file = home_dir.join(format!("{name}.db"));

        // Check if the database file exists
        if !db_file.exists() {
            return Err(anyhow!(
                "No backup named \"{name}\" found. Create a new backup first."
            ));
        }

        let pool = create_connection_pool(&db_file)?;

        // create backup version
        let backup_version = if dry_run {
            0
        } else {
            get_backup_version(&pool)?
        };

        println!(
            "Backup{} version: {}\n",
            if dry_run { " (dry-run)" } else { "" },
            backup_version
        );

        // get the directories to backup
        let directories = get_directories_to_backup(&pool)?;

        let mut tasks = FuturesUnordered::new();

        // Limit the number of concurrent tasks to the number of physical cores - 2, max 255
        let num_treads = cmp::min((num_cpus::get_physical() - 2).max(1), u8::MAX as usize);

        // Create a semaphore to limit the number of concurrent tasks
        let semaphore = Arc::new(Semaphore::new(num_treads));

        // Get public key from database
        let public_key = get_public_key(&db_file)?;

        debug!("Public Key: {:?}", hex::encode(public_key));

        for directory in directories {
            if !directory.exists() {
                return Err(anyhow!("Directory does not exist: {}", directory.display()));
            }

            let iterator = walk_directory(&directory, no_gitignore);

            for file_result in iterator {
                match file_result {
                    Ok(file_path) => {
                        if file_path.exists() {
                            let semaphore = semaphore.clone();
                            let log_file = skipped_files_log.clone();

                            tasks.push(async move {
                                let _permit = semaphore.acquire().await;
                                process_file(file_path, log_file).await
                            });
                        } else {
                            log_skipped_file(&skipped_files_log, &file_path).await?;
                        }
                    }
                    Err(err) => eprintln!("Failed to walk directory: {err}"),
                }
            }
        }

        let mut errors = Vec::new();
        let mut scanned_files = Vec::new();
        let mut skipped_files = false;

        // Await all tasks and handle errors
        while let Some(result) = tasks.next().await {
            match result {
                Ok(Some(scanned_file)) => scanned_files.push(scanned_file),
                Ok(None) => skipped_files = true,
                Err(err) => {
                    eprintln!("Task failed: {err}");
                    errors.push(err);
                }
            }
        }

        if !errors.is_empty() {
            return Err(anyhow!("Some tasks failed: {errors:?}"));
        }

        if !dry_run {
            update_backup_metadata(
                pool.clone(),
                public_key,
                backup_version,
                scanned_files,
                !skipped_files,
            )
            .await?;
        }

        println!();

        cleanup_skipped_log(&skipped_files_log).await?;

        // Get the elapsed time
        let elapsed = timer.elapsed();

        // Format the elapsed time
        let formatted_time = format_duration(elapsed);

        println!(
            "Backup{} completed successfully in: {}.",
            if dry_run { " (dry-run)" } else { "" },
            formatted_time
        );
    }

    Ok(())
}

// Returns an iterator over files in a directory, respecting `.gitignore` rules unless `no_gitignore` is true.
fn walk_directory(
    base_dir: &Path,
    no_gitignore: bool,
) -> impl Iterator<Item = Result<PathBuf, ignore::Error>> {
    WalkBuilder::new(base_dir)
        .follow_links(true)
        .hidden(false)
        .git_exclude(false)
        .git_global(false)
        .git_ignore(!no_gitignore)
        .require_git(false)
        .parents(true)
        .build()
        .filter_map(|entry| match entry {
            Ok(e) if e.path().is_file() => Some(Ok(e.into_path())),
            Ok(_) => None,
            Err(err) => Some(Err(err)),
        })
}

fn create_connection_pool(db_file: &Path) -> Result<Arc<Pool<SqliteConnectionManager>>> {
    let manager = SqliteConnectionManager::file(db_file).with_init(|conn| {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )
    });

    // use the number of physical cores as the pool size, but limit it to 32
    let pool_size = u32::try_from(cmp::min(num_cpus::get_physical(), 32))?;

    Ok(Arc::new(
        Pool::builder().max_size(pool_size).build(manager)?,
    ))
}

fn get_directories_to_backup(pool: &Pool<SqliteConnectionManager>) -> Result<Vec<PathBuf>> {
    let conn = pool.get()?;

    let directories: Vec<String> = conn
        .prepare("SELECT path FROM config_directories")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;

    Ok(directories.iter().map(PathBuf::from).collect())
}

fn get_backup_version(pool: &Pool<SqliteConnectionManager>) -> Result<i64> {
    let conn = pool.get()?;

    conn.execute(
        "INSERT INTO BackupVersions (timestamp) VALUES (strftime('%s', 'now'))",
        [],
    )?;

    let version_id = conn.last_insert_rowid();

    Ok(version_id)
}

async fn process_file(
    file_path: PathBuf,
    skipped_files_log: PathBuf,
) -> Result<Option<ScannedFile>> {
    println!("Processing file: {}", file_path.display());

    let hash = match calculate_hash(file_path.clone()).await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Skipping file: {e}");
            log_skipped_file(&skipped_files_log, &file_path).await?;
            return Ok(None);
        }
    };

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

async fn update_backup_metadata(
    pool: Arc<Pool<SqliteConnectionManager>>,
    public_key: PublicKey,
    version: i64,
    scanned_files: Vec<ScannedFile>,
    close_missing_files: bool,
) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let mut conn = pool.get()?;
        let tx = conn.transaction()?;

        tx.execute(
            "CREATE TEMP TABLE IF NOT EXISTS seen_files (
                path_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                PRIMARY KEY (path_id, name)
            )",
            [],
        )?;
        tx.execute("DELETE FROM seen_files", [])?;

        for scanned_file in &scanned_files {
            upsert_scanned_file(&tx, public_key, version, scanned_file)?;
        }

        if close_missing_files {
            close_deleted_files(&tx, version)?;
        }

        tx.commit()?;

        Ok::<_, anyhow::Error>(())
    })
    .await?
}

fn upsert_scanned_file(
    conn: &Connection,
    public_key: PublicKey,
    version: i64,
    scanned_file: &ScannedFile,
) -> Result<()> {
    let path = scanned_file
        .path
        .parent()
        .ok_or_else(|| anyhow!("Invalid file path"))?
        .to_string_lossy()
        .to_string();

    let file_name = scanned_file
        .path
        .file_name()
        .ok_or_else(|| anyhow!("Invalid file name"))?
        .to_string_lossy()
        .to_string();

    let path_id = get_or_insert_path(conn, &path)?;
    let file_id = get_or_insert_file(conn, &scanned_file.hash, public_key)?;

    conn.execute(
        "INSERT OR IGNORE INTO seen_files (path_id, name)
         VALUES (?1, ?2)",
        params![path_id, file_name],
    )?;

    let active_file_id = get_active_file_id(conn, path_id, &file_name)?;

    match active_file_id {
        Some(active_file_id) if active_file_id == file_id => {}
        Some(_) => {
            conn.execute(
                "UPDATE FileNames
                 SET last_version = ?1 - 1
                 WHERE path_id = ?2
                   AND name = ?3
                   AND last_version IS NULL",
                params![version, path_id, file_name],
            )?;

            insert_file_name(conn, path_id, &file_name, file_id, version)?;
        }
        None => insert_file_name(conn, path_id, &file_name, file_id, version)?,
    }

    Ok(())
}

fn get_or_insert_path(conn: &Connection, path: &str) -> Result<i64> {
    conn.execute(
        "INSERT OR IGNORE INTO Paths (path) VALUES (?1)",
        params![path],
    )?;

    let mut stmt = conn.prepare("SELECT path_id FROM Paths WHERE path = ?1")?;

    let id: i64 = stmt.query_row(params![path], |row| row.get(0))?;

    Ok(id)
}

fn get_or_insert_file(conn: &Connection, hash: &str, public_key: PublicKey) -> Result<i64> {
    if let Some(file_id) = get_file_id(conn, hash)? {
        return Ok(file_id);
    }

    let (wrapped, e_public) = encrypted_file_key(public_key)?;

    conn.execute(
        "INSERT INTO Files (hash, encrypted_key, ephemeral_public_key)
         VALUES (?1, ?2, ?3)",
        params![hash, wrapped, e_public],
    )?;

    get_file_id(conn, hash)?.ok_or_else(|| anyhow!("Failed to get inserted file id"))
}

fn get_file_id(conn: &Connection, hash: &str) -> Result<Option<i64>> {
    let mut stmt = conn.prepare("SELECT file_id FROM Files WHERE hash = ?1")?;
    let mut rows = stmt.query(params![hash])?;

    rows.next()?.map_or(Ok(None), |row| Ok(Some(row.get(0)?)))
}

fn encrypted_file_key(public_key: PublicKey) -> Result<(Vec<u8>, [u8; 32])> {
    let file_key = generate_file_key();
    encrypt(&file_key, &public_key)
}

fn get_active_file_id(conn: &Connection, path_id: i64, file_name: &str) -> Result<Option<i64>> {
    let mut stmt = conn.prepare(
        "SELECT file_id
         FROM FileNames
         WHERE path_id = ?1
           AND name = ?2
           AND last_version IS NULL",
    )?;

    let mut rows = stmt.query(params![path_id, file_name])?;

    rows.next()?.map_or(Ok(None), |row| Ok(Some(row.get(0)?)))
}

fn insert_file_name(
    conn: &Connection,
    path_id: i64,
    file_name: &str,
    file_id: i64,
    version: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO FileNames (path_id, name, file_id, first_version)
         VALUES (?1, ?2, ?3, ?4)",
        params![path_id, file_name, file_id, version],
    )?;

    Ok(())
}

fn close_deleted_files(conn: &Connection, version: i64) -> Result<()> {
    conn.execute(
        "UPDATE FileNames
         SET last_version = ?1 - 1
         WHERE last_version IS NULL
           AND first_version < ?1
           AND NOT EXISTS (
               SELECT 1
               FROM seen_files
               WHERE seen_files.path_id = FileNames.path_id
                 AND seen_files.name = FileNames.name
           )",
        params![version],
    )?;

    Ok(())
}

// Check if the log file is empty
async fn is_log_file_empty(log_file: &Path) -> io::Result<bool> {
    let metadata = tokio::fs::metadata(log_file).await?;
    Ok(metadata.len() == 0)
}

async fn cleanup_skipped_log(skipped_files_log: &Path) -> Result<()> {
    if is_log_file_empty(skipped_files_log).await? {
        remove_file(skipped_files_log).await?;
    } else {
        println!(
            "Some files were skipped. Check the log file: {}",
            skipped_files_log.display()
        );
    }

    Ok(())
}

async fn log_skipped_file(log_file: &Path, file_path: &Path) -> Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_file)
        .await?;

    file.write_all(format!("{}\n", &file_path.display()).as_bytes())
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use x25519_dalek::StaticSecret;

    fn public_key() -> PublicKey {
        let private_key = StaticSecret::from([7_u8; 32]);
        PublicKey::from(&private_key)
    }

    fn create_test_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE Files (
                 file_id INTEGER PRIMARY KEY,
                 hash TEXT NOT NULL UNIQUE,
                 encrypted_key BLOB NOT NULL,
                 ephemeral_public_key BLOB NOT NULL
             );
             CREATE TABLE Paths (
                 path_id INTEGER PRIMARY KEY,
                 path TEXT NOT NULL UNIQUE
             );
             CREATE TABLE FileNames (
                 name_id INTEGER PRIMARY KEY,
                 path_id INTEGER NOT NULL,
                 name TEXT NOT NULL,
                 file_id INTEGER NOT NULL,
                 first_version INTEGER NOT NULL,
                 last_version INTEGER,
                 FOREIGN KEY (path_id) REFERENCES Paths(path_id),
                 FOREIGN KEY (file_id) REFERENCES Files(file_id),
                 CHECK(last_version IS NULL OR last_version >= first_version),
                 UNIQUE(path_id, name, first_version)
             );
             CREATE UNIQUE INDEX idx_filenames_one_active
                 ON FileNames(path_id, name)
                 WHERE last_version IS NULL;",
        )?;

        Ok(())
    }

    fn start_seen_files(conn: &Connection) -> Result<()> {
        conn.execute(
            "CREATE TEMP TABLE IF NOT EXISTS seen_files (
                path_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                PRIMARY KEY (path_id, name)
            )",
            [],
        )?;
        conn.execute("DELETE FROM seen_files", [])?;

        Ok(())
    }

    fn scan_file(conn: &Connection, version: i64, name: &str, hash: &str) -> Result<()> {
        upsert_scanned_file(
            conn,
            public_key(),
            version,
            &ScannedFile {
                path: PathBuf::from(format!("/backup/{name}")),
                hash: hash.to_string(),
            },
        )
    }

    fn complete_version(conn: &Connection, version: i64) -> Result<()> {
        close_deleted_files(conn, version)
    }

    fn restore_hashes(conn: &Connection, version: i64) -> Result<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT Files.hash
             FROM FileNames
             JOIN Files ON Files.file_id = FileNames.file_id
             WHERE FileNames.first_version <= ?1
               AND (
                   FileNames.last_version IS NULL
                   OR FileNames.last_version >= ?1
               )
             ORDER BY Files.hash",
        )?;

        stmt.query_map(params![version], |row| row.get(0))?
            .collect::<Result<_, _>>()
            .map_err(Into::into)
    }

    #[test]
    fn unchanged_file_does_not_create_new_history_row() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_test_schema(&conn)?;

        start_seen_files(&conn)?;
        scan_file(&conn, 1, "file.txt", "hash-a")?;
        complete_version(&conn, 1)?;

        start_seen_files(&conn)?;
        scan_file(&conn, 2, "file.txt", "hash-a")?;
        complete_version(&conn, 2)?;

        let row_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM FileNames", [], |row| row.get(0))?;
        let active_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM FileNames WHERE last_version IS NULL",
            [],
            |row| row.get(0),
        )?;

        assert_eq!(row_count, 1);
        assert_eq!(active_count, 1);
        assert_eq!(restore_hashes(&conn, 2)?, vec!["hash-a".to_string()]);

        Ok(())
    }

    #[test]
    fn modified_file_restores_only_current_content() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_test_schema(&conn)?;

        start_seen_files(&conn)?;
        scan_file(&conn, 1, "file.txt", "hash-a")?;
        complete_version(&conn, 1)?;

        start_seen_files(&conn)?;
        scan_file(&conn, 2, "file.txt", "hash-b")?;
        complete_version(&conn, 2)?;

        assert_eq!(restore_hashes(&conn, 1)?, vec!["hash-a".to_string()]);
        assert_eq!(restore_hashes(&conn, 2)?, vec!["hash-b".to_string()]);

        Ok(())
    }

    #[test]
    fn missing_file_is_closed_as_deleted() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_test_schema(&conn)?;

        start_seen_files(&conn)?;
        scan_file(&conn, 1, "file.txt", "hash-a")?;
        complete_version(&conn, 1)?;

        start_seen_files(&conn)?;
        complete_version(&conn, 2)?;

        assert_eq!(restore_hashes(&conn, 1)?, vec!["hash-a".to_string()]);
        assert!(restore_hashes(&conn, 2)?.is_empty());

        Ok(())
    }

    #[test]
    fn reverted_content_keeps_separate_intervals() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        create_test_schema(&conn)?;

        start_seen_files(&conn)?;
        scan_file(&conn, 1, "file.txt", "hash-a")?;
        complete_version(&conn, 1)?;

        start_seen_files(&conn)?;
        scan_file(&conn, 2, "file.txt", "hash-b")?;
        complete_version(&conn, 2)?;

        start_seen_files(&conn)?;
        scan_file(&conn, 3, "file.txt", "hash-a")?;
        complete_version(&conn, 3)?;

        let row_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM FileNames", [], |row| row.get(0))?;

        assert_eq!(row_count, 3);
        assert_eq!(restore_hashes(&conn, 1)?, vec!["hash-a".to_string()]);
        assert_eq!(restore_hashes(&conn, 2)?, vec!["hash-b".to_string()]);
        assert_eq!(restore_hashes(&conn, 3)?, vec!["hash-a".to_string()]);

        Ok(())
    }
}
