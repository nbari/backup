use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose};
use rusqlite::Connection;
use std::path::Path;
use x25519_dalek::PublicKey;

/// Create the metadata schema used by backup databases.
///
/// # Errors
/// Returns an error if the database cannot be opened or any schema statement fails.
pub(crate) fn create_metadata_schema(db_path: &Path) -> Result<()> {
    let conn = Connection::open(db_path)?;

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS Config (
            name TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS Files (
            file_id INTEGER PRIMARY KEY,
            hash TEXT NOT NULL UNIQUE,
            encrypted_key BLOB NOT NULL,
            ephemeral_public_key BLOB NOT NULL
        );

        CREATE TABLE IF NOT EXISTS Paths (
            path_id INTEGER PRIMARY KEY,
            path TEXT NOT NULL UNIQUE
        );

        CREATE TABLE IF NOT EXISTS FileNames (
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

        CREATE TABLE IF NOT EXISTS BackupVersions (
            version_id INTEGER PRIMARY KEY,
            timestamp DATETIME DEFAULT CURRENT_TIMESTAMP
        );

        CREATE INDEX IF NOT EXISTS idx_files_version
            ON FileNames(first_version, last_version);

        CREATE UNIQUE INDEX IF NOT EXISTS idx_filenames_one_active
            ON FileNames(path_id, name)
            WHERE last_version IS NULL;

        CREATE INDEX IF NOT EXISTS idx_filenames_path_history
            ON FileNames(path_id, name, first_version, last_version);",
    )?;

    Ok(())
}

/// Read the backup public key from a `SQLite` database.
/// # Errors
/// Returns an error if the database cannot be read or the stored key is invalid.
pub fn get_public_key(db_path: &Path) -> Result<PublicKey> {
    let conn = Connection::open(db_path)?;

    let mut stmt = conn.prepare("SELECT value FROM Config WHERE name='public_key'")?;

    let key: String = stmt.query_row([], |row| row.get(0))?;

    let key = general_purpose::STANDARD.decode(key)?;

    if key.len() != 32 {
        return Err(anyhow!("Invalid private key length"));
    }

    // Convert Vec<u8> to [u8; 32]
    let key_array: [u8; 32] = key
        .try_into()
        .map_err(|_| anyhow!("Failed to convert key to 32-byte array"))?;

    Ok(PublicKey::from(key_array))
}
