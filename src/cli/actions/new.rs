use crate::cli::actions::Action;
use anyhow::Result;
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;

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

        create_db_tables(db_path)?;

        if let Some(directory) = directory {
            for dir in directory {
                println!("Directory: {}", fs::canonicalize(dir)?.display());
            }
        }

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

fn create_db_tables(db_path: PathBuf) -> Result<()> {
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
        "CREATE INDEX idx_files_version ON FileNames (first_version, last_version, is_deleted)",
        [],
    )?;

    Ok(())
}
