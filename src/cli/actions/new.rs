use crate::cli::actions::Action;
use anyhow::Result;
use rusqlite::{params, Connection};
use std::{fs, path::PathBuf};

/// Handle the create action
pub fn handle(action: Action) -> Result<()> {
    if let Action::New {
        name,
        config,
        directory,
        file,
        exclude,
    } = action
    {
        let db_path = config.join(format!("{}.db", name));

        // Create the backup database tables
        create_db_tables(&db_path)?;

        let backup_dirs = get_unique_dir_parents(directory.unwrap_or_default());

        // create the config_directories table
        create_db_config_direcories_table(&db_path, backup_dirs)?;

        if let Some(file) = file {
            for file in file {
                println!("File: {}", fs::canonicalize(file)?.display());
            }
        }

        if let Some(exclude) = exclude {
            for exclude in exclude {
                println!("Exclude: {}", exclude);
            }
        }
    }

    Ok(())
}

fn create_db_tables(db_path: &PathBuf) -> Result<()> {
    let conn = Connection::open(db_path)?;

    // table to store unique file content, using content hash to avoid duplicates
    conn.execute(
        "CREATE TABLE IF NOT EXISTS Files (
    file_id INTEGER PRIMARY KEY,
    hash TEXT NOT NULL UNIQUE
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
    is_deleted BOOLEAN DEFAULT 0,    -- 1 if the file was deleted in this version, 0 otherwise

    FOREIGN KEY (path_id) REFERENCES Paths(path_id),
    FOREIGN KEY (file_id) REFERENCES Files(file_id),

    UNIQUE(path_id, name, first_version) -- Ensure unique entries by path and version
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
        "CREATE INDEX IF NOT EXISTS idx_files_version ON FileNames (first_version, last_version, is_deleted)",
        [],
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
}
