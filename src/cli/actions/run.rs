use crate::{
    cli::{actions::Action, globals::GlobalArgs},
    utils::hash::blake3,
};
use anyhow::{anyhow, Result};
use futures::stream::{FuturesUnordered, StreamExt};
use ignore::WalkBuilder;
use rusqlite::{params, Connection};
use std::{
    cmp,
    path::{Path, PathBuf},
};
use tokio::sync::Mutex;

/// Handle the create action
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

        // check if the database file exists
        if !db_file.exists() {
            return Err(anyhow!(
                "No backup named \"{}\" found. Create a new backup first.",
                name
            ));
        }

        let directories = get_directories_to_backup(&db_file)?;

        // Open a single connection to the database
        let conn = Connection::open(db_file)?;
        let conn = Mutex::new(conn);

        let mut tasks = FuturesUnordered::new();

        for directory in directories {
            if !directory.exists() {
                return Err(anyhow!("Directory does not exist: {}", directory.display()));
            }

            let iterator = walk_directory(&directory, no_gitignore);

            for file_result in iterator {
                match file_result {
                    Ok(file_path) => {
                        let file_path_clone = file_path.clone();

                        tasks.push(process_file(&conn, file_path_clone));

                        // Limit the number of concurrent tasks to the number of physical cores - 2
                        let num_treads =
                            cmp::min((num_cpus::get_physical() - 2).max(1), u8::MAX as usize);

                        while tasks.len() >= num_treads {
                            if let Some(Err(err)) = tasks.next().await {
                                eprintln!("Task failed: {}", err);
                            }
                        }
                    }
                    Err(err) => eprintln!("Error: {}", err),
                }
            }
        }

        // Await all tasks and handle errors
        while let Some(result) = tasks.next().await {
            if let Err(err) = result {
                eprintln!("Task failed: {}", err);
            }
        }
    }

    Ok(())
}

// query the backup database for directories to backup
fn get_directories_to_backup(db_file: &Path) -> Result<Vec<PathBuf>> {
    let conn = Connection::open(db_file)?;

    let directories: Vec<String> = conn
        .prepare("SELECT path FROM config_directories")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;

    Ok(directories.iter().map(PathBuf::from).collect())
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

async fn process_file(conn: &Mutex<Connection>, file_path: PathBuf) -> Result<()> {
    println!("Processing file: {}", file_path.display());

    // Extract path and filename
    let path = file_path
        .parent()
        .ok_or_else(|| anyhow!("Invalid file path"))?;

    let file_name = file_path
        .file_name()
        .ok_or_else(|| anyhow!("Invalid file name"))?
        .to_string_lossy()
        .to_string();

    // Compute the hash asynchronously
    let file_path_clone = file_path.clone();
    let hash = tokio::task::spawn_blocking(move || blake3(&file_path_clone))
        .await?
        .map_err(|e| anyhow!("Hash computation failed: {}", e))?;

    // Insert path into the Paths table if not exists
    let path_id = insert_or_get_path_id(conn, path).await?;

    // Insert file hash into the Files table if not exists
    let file_id = insert_or_get_file_id(conn, &hash).await?;

    // Insert file name into the FileNames table
    conn.lock().await.execute(
        "INSERT OR IGNORE INTO FileNames (path_id, name, file_id, first_version, last_version, is_deleted)
         VALUES (?1, ?2, ?3, ?4, NULL, 0)",
        params![path_id, file_name, file_id, 1], // Assuming version 1 for simplicity
    )?;

    Ok(())
}

/// Insert a path into the Paths table if it does not exist, and return its ID
async fn insert_or_get_path_id(conn: &Mutex<Connection>, path: &Path) -> Result<i64> {
    let conn = conn.lock().await;

    let path_str = path.to_string_lossy();
    conn.execute(
        "INSERT OR IGNORE INTO Paths (path) VALUES (?1)",
        params![path_str],
    )?;

    let mut stmt = conn.prepare("SELECT path_id FROM Paths WHERE path = ?1")?;
    let path_id: i64 = stmt.query_row(params![path_str], |row| row.get(0))?;

    Ok(path_id)
}

/// Insert a file hash into the Files table if it does not exist, and return its ID
async fn insert_or_get_file_id(conn: &Mutex<Connection>, hash: &str) -> Result<i64> {
    let conn = conn.lock().await;

    conn.execute(
        "INSERT OR IGNORE INTO Files (hash) VALUES (?1)",
        params![hash],
    )?;

    let mut stmt = conn.prepare("SELECT file_id FROM Files WHERE hash = ?1")?;
    let file_id: i64 = stmt.query_row(params![hash], |row| row.get(0))?;

    Ok(file_id)
}
