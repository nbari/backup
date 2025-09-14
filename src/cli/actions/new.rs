use crate::cli::actions::Action;
use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use bip39::{Language, Mnemonic};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use tracing::debug;
use x25519_dalek::{PublicKey, StaticSecret};

/// Handle the create action
pub fn handle(action: Action) -> Result<()> {
    if let Action::New {
        name,
        config,
        directory,
        file,
    } = action
    {
        let db_path = config.join(format!("{}.db", name));

        // check if file already exists
        if db_path.exists() {
            return Err(anyhow::anyhow!(
                "A backup with the name '{}' already exists",
                name
            ));
        }

        // Create the backup database tables
        create_db_tables(&db_path)?;

        let mnemonic = Mnemonic::generate_in(Language::English, 12)?;

        // Derive the keypair from the mnemonic
        let seed = mnemonic.to_seed("");

        let mut seed_bytes = [0u8; 32];

        seed_bytes.copy_from_slice(&seed[0..32]);

        let private_key = StaticSecret::from(seed_bytes);

        let public_key = PublicKey::from(&private_key);

        debug!("Public Key: {:?}", hex::encode(public_key.as_bytes()));

        // save the public key to the database
        save_public_key(&db_path, &public_key)?;

        let backup_dirs = get_unique_dir_parents(directory.unwrap_or_default());

        // create the config_directories table
        create_db_config_direcories_table(&db_path, backup_dirs)?;

        // create the config_files tables
        // exclude files if they are within the directories that are being backed up
        create_db_config_files_table(&db_path, file.unwrap_or_default())?;

        // Display the mnemonic to the user
        let m = mnemonic.to_string();

        let words: Vec<&str> = m.split_whitespace().collect();

        println!("Your recovery phrase is:\n");

        println!("[ {} ]\n", mnemonic);

        for (i, word) in words.iter().enumerate() {
            print!("{:2}. {:12}", i + 1, word);
            if (i + 1) % 4 == 0 {
                println!(); // New line every 4 words
            }
        }

        println!("\n\nPlease write this down and store it in a safe place.");
    }

    Ok(())
}

fn create_db_tables(db_path: &PathBuf) -> Result<()> {
    let conn = Connection::open(db_path)?;

    // table to store config info, like public key
    conn.execute(
        "CREATE TABLE IF NOT EXISTS Config (
    name TEXT PRIMARY KEY,
    value TEXT NOT NULL
)",
        [],
    )?;

    // table to store unique file content, using content hash to avoid duplicates
    conn.execute(
        "CREATE TABLE IF NOT EXISTS Files (
    file_id INTEGER PRIMARY KEY,
    hash TEXT NOT NULL UNIQUE,
    encrypted_key BLOB NOT NULL,
    ephemeral_public_key BLOB NOT NULL
)",
        [],
    )?;

    // table to store directory paths
    conn.execute(
        "CREATE TABLE IF NOT EXISTS Paths(
    path_id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE
)",
        [],
    )?;

    // table to store files with version tracking
    conn.execute(
        "CREATE TABLE IF NOT EXISTS FileNames (
    name_id INTEGER PRIMARY KEY,
    path_id INTEGER NOT NULL,        -- Foreign key referencing Paths table
    name TEXT NOT NULL,              -- Name of the file in the Path
    file_id INTEGER NOT NULL,        -- Foreign key referencing Files for content hash
    first_version INTEGER NOT NULL,  -- The version in which this file path first appeared
    last_version INTEGER,            -- The last version this file path was valid (NULL if still valid)

    FOREIGN KEY (path_id) REFERENCES Paths(path_id),
    FOREIGN KEY (file_id) REFERENCES Files(file_id),

    UNIQUE(path_id, file_id, name) -- Ensure unique entries by path and version
)",
        [],
    )?;

    // Table to track each backup version
    conn.execute(
        "CREATE TABLE IF NOT EXISTS BackupVersions (
    version_id INTEGER PRIMARY KEY,
    timestamp DATETIME DEFAULT CURRENT_TIMESTAMP -- Timestamp when the backup was created
)",
        [],
    )?;

    // Index for efficient file retrieval by version
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_files_version ON FileNames (first_version, last_version)",
        [],
    )?;

    Ok(())
}

fn save_public_key(db_path: &PathBuf, public_key: &PublicKey) -> Result<()> {
    let conn = Connection::open(db_path)?;

    let public_key_b64 = STANDARD.encode(public_key.as_bytes());
    conn.execute(
        "INSERT INTO Config (name, value) VALUES ('public_key', ?1)",
        params![public_key_b64],
    )?;

    Ok(())
}

// extract the parent directory of each path and return only the unique parent directories
fn get_unique_dir_parents(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    // Sort the input directories lexicographically (shorter paths come first for easier comparison)
    dirs.sort();

    // Filter out subdirectories or descendants
    let mut result = Vec::new();
    for dir in dirs {
        // Only add the directory if it is not a descendant of any directory already in the result
        if !result.iter().any(|parent| dir.starts_with(parent)) {
            result.push(dir);
        }
    }

    result
}

fn create_db_config_direcories_table(db_path: &PathBuf, dirs: Vec<PathBuf>) -> Result<()> {
    let conn = Connection::open(db_path)?;

    // Table to track each backup version
    conn.execute(
        "CREATE TABLE IF NOT EXISTS config_directories (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE
)",
        [],
    )?;

    // Prepare the insert statement
    let mut stmt = conn.prepare("INSERT OR IGNORE INTO config_directories (path) VALUES (?1)")?;

    // Insert each directory into the database
    for dir in dirs {
        stmt.execute(params![dir.to_string_lossy().to_string()])?;
    }

    Ok(())
}

fn create_db_config_files_table(db_path: &PathBuf, files: Vec<PathBuf>) -> Result<()> {
    let conn = Connection::open(db_path)?;

    // Table to track each backup version
    conn.execute(
        "CREATE TABLE IF NOT EXISTS config_files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE
)",
        [],
    )?;

    // Prepare the insert statement
    let mut stmt = conn.prepare("INSERT OR IGNORE INTO config_files (path) VALUES (?1)")?;

    // Get all directory paths from config_directories table
    let mut dirs_stmt = conn.prepare("SELECT path FROM config_directories")?;
    let dirs_iter = dirs_stmt.query_map([], |row| row.get::<_, String>(0))?;

    // Collect all directory paths
    let dirs: Vec<String> = dirs_iter.filter_map(|result| result.ok()).collect();

    // Insert files only if they are not children of any of the directories
    for file in files {
        let file_path = file.to_string_lossy().to_string();

        // Check if file is a child of any of the directories
        let is_child = dirs.iter().any(|dir| {
            let dir_path = Path::new(dir);
            file.starts_with(dir_path)
        });

        // Only insert file if it is not a child of any directory
        if !is_child {
            stmt.execute(params![file_path])?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_unique_dir_parents() {
        let dirs = vec![
            PathBuf::from("/a/b/c"),
            PathBuf::from("/a/b/d"),
            PathBuf::from("/a/b/c/d"),
            PathBuf::from("/a/b"),
            PathBuf::from("/b"),
            PathBuf::from("/b/c"),
            PathBuf::from("/b/cc"),
            PathBuf::from("/b/d"),
        ];

        let result = get_unique_dir_parents(dirs);

        assert_eq!(result.len(), 2);
        assert_eq!(result[0], PathBuf::from("/a/b"));
        assert_eq!(result[1], PathBuf::from("/b"));
    }

    // test the create_config_directories_table function
    #[test]
    fn test_create_db_config_directoris_and_files_table() {
        let temp_dir = tempfile::tempdir().unwrap();

        let db_path = temp_dir.path().join("test.db");

        let dirs = vec![
            PathBuf::from("/a/b/c"),
            PathBuf::from("/a/b/d"),
            PathBuf::from("/a/b/c/d"),
            PathBuf::from("/a/b"),
            PathBuf::from("/b"),
            PathBuf::from("/b/c"),
            PathBuf::from("/b/cc"),
            PathBuf::from("/b/d"),
        ];

        create_db_tables(&db_path).unwrap();

        let backup_dirs = get_unique_dir_parents(dirs);

        create_db_config_direcories_table(&db_path, backup_dirs).unwrap();

        let conn = Connection::open(&db_path).unwrap();

        let mut stmt = conn.prepare("SELECT path FROM config_directories").unwrap();

        let dirs_iter = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();

        let result: Vec<String> = dirs_iter.filter_map(|result| result.ok()).collect();

        assert_eq!(result.len(), 2);
        assert!(result.contains(&"/a/b".to_string()));
        assert!(result.contains(&"/b".to_string()));

        let files = vec![
            PathBuf::from("/a/b/c/file1.txt"),
            PathBuf::from("/a/b/c/d/file2.txt"),
            PathBuf::from("/a/file3.txt"),
            PathBuf::from("/z/file4.txt"),
        ];

        create_db_config_files_table(&db_path, files).unwrap();

        let mut stmt = conn.prepare("SELECT path FROM config_files").unwrap();

        let files_iter = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();

        let result: Vec<String> = files_iter.filter_map(|result| result.ok()).collect();

        assert_eq!(result.len(), 2);
        assert!(result.contains(&"/a/file3.txt".to_string()));
        assert!(result.contains(&"/z/file4.txt".to_string()));
    }
}
