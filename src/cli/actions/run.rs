use crate::{
    cli::{actions::Action, globals::GlobalArgs},
    utils::hash::blake3,
};
use anyhow::{anyhow, Result};
use futures::stream::{FuturesUnordered, StreamExt};
use ignore::{WalkBuilder, WalkParallel, WalkState};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{params, Connection};
use std::{
    cmp,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::Semaphore;
use tracing::instrument;

fn log_error(message: &str) {
    eprintln!("{}", message);
}

/// Handle the create action
#[instrument]
pub async fn handle(action: Action, globals: GlobalArgs) -> Result<()> {
    if let Action::Run {
        name,
        no_gitignore,
        no_compression: _,
        no_encryption: _,
        dry_run: _,
    } = action
    {
        let home_dir = globals.home;
        let db_file = home_dir.join(format!("{}.db", name));

        // Check if the database file exists
        if !db_file.exists() {
            let error_message = format!(
                "No backup named \"{}\" found. Create a new backup first.",
                name
            );

            log_error(&error_message);

            return Err(anyhow!(error_message));
        }

        let manager = SqliteConnectionManager::file(&db_file);

        let pool_size = cmp::min(num_cpus::get_physical(), 32) as u32;

        // for the pool_size use number of physical cores or a max of 32
        let pool = Arc::new(Pool::builder().max_size(pool_size).build(manager)?);

        let directories = get_directories_to_backup(pool.clone())?;

        let tasks = Arc::new(tokio::sync::Mutex::new(FuturesUnordered::new()));

        // Limit the number of concurrent tasks to the number of physical cores - 2, max 255
        let num_treads = cmp::min((num_cpus::get_physical() - 2).max(1), u8::MAX as usize);

        // Create a semaphore to limit the number of concurrent tasks
        let semaphore = Arc::new(Semaphore::new(num_treads));

        for directory in directories {
            if !directory.exists() {
                let error_message = format!("Directory does not exist: {}", directory.display());
                log_error(&error_message);
                return Err(anyhow!(error_message));
            }

            println!("Starting directory walk: {}", directory.display());

            let walker = walk_directory(&directory, no_gitignore);
            let pool = pool.clone();
            let semaphore = semaphore.clone();
            let tasks = tasks.clone();

            walker.run(|| {
                let semaphore = semaphore.clone();
                let pool = pool.clone();
                let tasks = tasks.clone();
                Box::new(move |entry| match entry {
                    Ok(dir_entry) if dir_entry.path().is_file() => {
                        let file_path = dir_entry.path().to_path_buf();
                        let pool = pool.clone();
                        let semaphore = semaphore.clone();
                        let tasks = tasks.clone();

                        tokio::task::block_in_place(move || {
                            let _permit = semaphore.acquire();
                            let tasks = tasks.blocking_lock();
                            tasks.push(async move { process_file(pool, file_path).await });
                        });

                        WalkState::Continue
                    }
                    Ok(_) => WalkState::Continue,
                    Err(err) => {
                        let error_message = format!("Failed to walk directory: {}", err);
                        log_error(&error_message);
                        WalkState::Continue
                    }
                })
            });

            println!("Finished directory walk: {}", directory.display());
        }

        let mut errors = Vec::new();

        // Await all tasks and handle errors
        while let Some(result) = {
            let mut guard = tasks.lock().await;
            guard.next().await
        } {
            if let Err(err) = result {
                eprintln!("Task failed: {}", err);
                errors.push(err);
            }
        }

        if !errors.is_empty() {
            return Err(anyhow!("Some tasks failed: {:?}", errors));
        }
    }

    println!("Backup completed successfully.");

    Ok(())
}

fn walk_directory(base_dir: &Path, no_gitignore: bool) -> WalkParallel {
    WalkBuilder::new(base_dir)
        .follow_links(true)
        .hidden(false)
        .git_exclude(false)
        .git_global(false)
        .git_ignore(!no_gitignore)
        .require_git(false)
        .parents(true)
        .build_parallel()
}

fn get_directories_to_backup(pool: Arc<Pool<SqliteConnectionManager>>) -> Result<Vec<PathBuf>> {
    let conn = pool.get()?;

    let directories: Vec<String> = conn
        .prepare("SELECT path FROM config_directories")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;

    Ok(directories.iter().map(PathBuf::from).collect())
}

async fn process_file(pool: Arc<Pool<SqliteConnectionManager>>, file_path: PathBuf) -> Result<()> {
    println!("Processing file: {}", file_path.display());

    let hash = calculate_hash(file_path.clone()).await?;

    insert_file_into_db(pool, file_path, hash).await?;

    Ok(())
}

async fn calculate_hash(file_path: PathBuf) -> Result<String> {
    tokio::task::spawn_blocking(move || blake3(&file_path))
        .await?
        .map_err(|e| anyhow!("Hash computation failed: {}", e))
}

async fn insert_file_into_db(
    pool: Arc<Pool<SqliteConnectionManager>>,
    file_path: PathBuf,
    hash: String,
) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;

        let path = file_path
            .parent()
            .ok_or_else(|| anyhow!("Invalid file path"))?
            .to_string_lossy()
            .to_string();

        let file_name = file_path
            .file_name()
            .ok_or_else(|| anyhow!("Invalid file name"))?
            .to_string_lossy()
            .to_string();

        let path_id = get_or_insert(&conn, "Paths", "path", "path_id", &path)?;
        let file_id = get_or_insert(&conn, "Files", "hash", "file_id", &hash)?;

        conn.execute(
            "INSERT OR IGNORE INTO FileNames (path_id, name, file_id, first_version, last_version, is_deleted)
             VALUES (?1, ?2, ?3, ?4, NULL, 0)",
            params![path_id, file_name, file_id, 1],
        )?;

        Ok::<_, anyhow::Error>(())
    })
    .await?
}

fn get_or_insert(
    conn: &Connection,
    table: &str,
    column: &str,
    id_col: &str,
    value: &str,
) -> Result<i64> {
    // INSERT OR IGNORE INTO Paths (path) VALUES (?1)
    // SELECT path_id FROM Paths WHERE path = ?1
    // INSERT OR IGNORE INTO Files (hash) VALUES (?1)
    // SELECT file_id FROM Files WHERE hash = ?1
    conn.execute(
        &format!("INSERT OR IGNORE INTO {} ({}) VALUES (?1)", table, column),
        params![value],
    )?;

    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM {} WHERE {} = ?1",
        id_col, table, column
    ))?;

    let id: i64 = stmt.query_row(params![value], |row| row.get(0))?;

    Ok(id)
}
