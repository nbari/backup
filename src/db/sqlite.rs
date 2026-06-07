use crate::utils::crypto::{encrypt, generate_file_key};
use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    cmp,
    path::{Path, PathBuf},
    sync::Arc,
};
use x25519_dalek::PublicKey;

#[derive(Clone, Debug)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub hash: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RestoreEntry {
    pub path: PathBuf,
    pub hash: String,
}

#[derive(Clone)]
pub struct SqliteCatalog {
    db_path: PathBuf,
    pool: Arc<Pool<SqliteConnectionManager>>,
}

impl SqliteCatalog {
    /// Initialize a metadata catalog database.
    ///
    /// # Errors
    /// Returns an error if the database cannot be initialized.
    pub fn initialize(db_path: &Path) -> Result<Self> {
        create_metadata_schema(db_path)?;
        Self::open(db_path)
    }

    /// Open an existing metadata catalog database.
    ///
    /// # Errors
    /// Returns an error if the connection pool cannot be created.
    pub fn open(db_path: &Path) -> Result<Self> {
        Ok(Self {
            db_path: db_path.to_path_buf(),
            pool: create_connection_pool(db_path)?,
        })
    }

    #[must_use]
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Read the backup public key.
    ///
    /// # Errors
    /// Returns an error if the public key cannot be read or decoded.
    pub fn public_key(&self) -> Result<PublicKey> {
        get_public_key(&self.db_path)
    }

    /// Save the backup public key.
    ///
    /// # Errors
    /// Returns an error if the key cannot be stored.
    pub fn save_public_key(&self, public_key: &PublicKey) -> Result<()> {
        let conn = self.pool.get()?;

        let public_key_b64 = general_purpose::STANDARD.encode(public_key.as_bytes());
        conn.execute(
            "INSERT INTO Config (name, value) VALUES ('public_key', ?1)",
            params![public_key_b64],
        )?;

        Ok(())
    }

    /// Save the naming key sealed to the backup public key.
    ///
    /// The blob is ciphertext (`ephemeral_public_key || wrapped_key`) and can
    /// only be opened with the recovery mnemonic, so storing it in the catalog
    /// leaks nothing at rest.
    ///
    /// # Errors
    /// Returns an error if the sealed key cannot be stored.
    pub fn save_sealed_naming_key(&self, sealed: &[u8]) -> Result<()> {
        let conn = self.pool.get()?;

        let sealed_b64 = general_purpose::STANDARD.encode(sealed);
        conn.execute(
            "INSERT INTO Config (name, value) VALUES ('sealed_naming_key', ?1)",
            params![sealed_b64],
        )?;

        Ok(())
    }

    /// Read the sealed naming key.
    ///
    /// # Errors
    /// Returns an error if the sealed key is missing or cannot be decoded.
    pub fn sealed_naming_key(&self) -> Result<Vec<u8>> {
        let conn = self.pool.get()?;

        let sealed_b64: String = conn
            .query_row(
                "SELECT value FROM Config WHERE name='sealed_naming_key'",
                [],
                |row| row.get(0),
            )
            .map_err(|err| anyhow!("Sealed naming key not found: {err}"))?;

        Ok(general_purpose::STANDARD.decode(sealed_b64)?)
    }

    /// Save configured backup directories.
    ///
    /// # Errors
    /// Returns an error if any directory cannot be stored.
    pub fn save_directories(&self, dirs: &[PathBuf]) -> Result<()> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;

        let mut stmt = tx.prepare("INSERT OR IGNORE INTO config_directories (path) VALUES (?1)")?;

        for dir in dirs {
            stmt.execute(params![dir.to_string_lossy().to_string()])?;
        }

        drop(stmt);
        tx.commit()?;

        Ok(())
    }

    /// Save configured backup files.
    ///
    /// # Errors
    /// Returns an error if any file path cannot be stored.
    pub fn save_files(&self, files: &[PathBuf]) -> Result<()> {
        let mut conn = self.pool.get()?;
        let tx = conn.transaction()?;

        let directories = configured_directories(&tx)?;
        let mut stmt = tx.prepare("INSERT OR IGNORE INTO config_files (path) VALUES (?1)")?;

        for file in files {
            let is_child = directories.iter().any(|dir| file.starts_with(dir));

            if !is_child {
                stmt.execute(params![file.to_string_lossy().to_string()])?;
            }
        }

        drop(stmt);
        tx.commit()?;

        Ok(())
    }

    /// Return configured backup directories.
    ///
    /// # Errors
    /// Returns an error if the directories cannot be read.
    pub fn configured_directories(&self) -> Result<Vec<PathBuf>> {
        let conn = self.pool.get()?;
        configured_directories(&conn)
    }

    /// Return configured backup files.
    ///
    /// # Errors
    /// Returns an error if the files cannot be read.
    pub fn configured_files(&self) -> Result<Vec<PathBuf>> {
        let conn = self.pool.get()?;
        configured_files(&conn)
    }

    /// Create a backup version.
    ///
    /// # Errors
    /// Returns an error if the version cannot be stored.
    pub fn create_version(&self) -> Result<i64> {
        let conn = self.pool.get()?;

        conn.execute(
            "INSERT INTO BackupVersions (timestamp) VALUES (strftime('%s', 'now'))",
            [],
        )?;

        Ok(conn.last_insert_rowid())
    }

    /// Record a completed scan into version metadata.
    ///
    /// # Errors
    /// Returns an error if scanned files cannot be written atomically.
    pub fn record_scan(
        &self,
        public_key: PublicKey,
        version: i64,
        scanned_files: &[ScannedFile],
        close_missing_files: bool,
        progress: Option<&dyn Fn(usize)>,
    ) -> Result<()> {
        let mut conn = self.pool.get()?;
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

        let scanned_file_count = scanned_files.len();

        for (index, scanned_file) in scanned_files.iter().enumerate() {
            upsert_scanned_file(&tx, public_key, version, scanned_file)?;
            if let Some(progress) = progress {
                let written_files = index + 1;
                if written_files == scanned_file_count || written_files % 100 == 0 {
                    progress(written_files);
                }
            }
        }

        if close_missing_files {
            close_deleted_files(&tx, version)?;
        }

        tx.commit()?;

        Ok(())
    }

    /// Query restorable path/hash entries for a version.
    ///
    /// # Errors
    /// Returns an error if restore metadata cannot be read.
    pub fn restore_entries(&self, version: i64) -> Result<Vec<RestoreEntry>> {
        let conn = self.pool.get()?;
        restore_entries(&conn, version)
    }

    /// Return the most recent backup version, or `None` if no runs are recorded.
    ///
    /// # Errors
    /// Returns an error if the version metadata cannot be read.
    pub fn latest_version(&self) -> Result<Option<i64>> {
        let conn = self.pool.get()?;

        Ok(
            conn.query_row("SELECT MAX(version_id) FROM BackupVersions", [], |row| {
                row.get::<_, Option<i64>>(0)
            })?,
        )
    }

    /// Return the unix timestamp (seconds) a version was recorded.
    ///
    /// # Errors
    /// Returns an error if the version metadata cannot be read.
    pub fn version_timestamp(&self, version: i64) -> Result<Option<i64>> {
        let conn = self.pool.get()?;

        let timestamp = conn
            .query_row(
                "SELECT timestamp FROM BackupVersions WHERE version_id = ?1",
                params![version],
                |row| row.get::<_, Option<i64>>(0),
            )
            .optional()?
            .flatten();

        Ok(timestamp)
    }

    /// Count rows in a table.
    ///
    /// # Errors
    /// Returns an error if the table cannot be queried.
    #[cfg(test)]
    pub(crate) fn count_rows(&self, table: &str) -> Result<i64> {
        if !matches!(table, "Files" | "FileNames") {
            return Err(anyhow!("Unsupported count table"));
        }

        let conn = self.pool.get()?;
        let mut stmt = conn.prepare(&format!("SELECT COUNT(*) FROM {table}"))?;

        Ok(stmt.query_row([], |row| row.get(0))?)
    }

    /// Count distinct file hashes.
    ///
    /// # Errors
    /// Returns an error if file metadata cannot be queried.
    #[cfg(test)]
    pub(crate) fn count_unique_hashes(&self) -> Result<i64> {
        let conn = self.pool.get()?;

        Ok(
            conn.query_row("SELECT COUNT(DISTINCT hash) FROM Files", [], |row| {
                row.get(0)
            })?,
        )
    }

    /// Count active filename rows.
    ///
    /// # Errors
    /// Returns an error if filename metadata cannot be queried.
    #[cfg(test)]
    pub(crate) fn count_active_file_names(&self) -> Result<i64> {
        let conn = self.pool.get()?;

        Ok(conn.query_row(
            "SELECT COUNT(*) FROM FileNames WHERE last_version IS NULL",
            [],
            |row| row.get(0),
        )?)
    }

    /// Ensure private recovery material is not persisted.
    ///
    /// # Errors
    /// Returns an error if the config table cannot be queried.
    #[cfg(test)]
    pub(crate) fn recovery_secret_count(&self) -> Result<i64> {
        let conn = self.pool.get()?;

        Ok(conn.query_row(
            "SELECT COUNT(*)
             FROM Config
             WHERE name IN ('mnemonic', 'password', 'private_key')",
            [],
            |row| row.get(0),
        )?)
    }

    /// Return one wrapped file key for test verification.
    ///
    /// # Errors
    /// Returns an error if file key metadata cannot be read.
    #[cfg(test)]
    pub(crate) fn first_wrapped_file_key(&self) -> Result<(Vec<u8>, [u8; 32])> {
        let conn = self.pool.get()?;
        let (encrypted_key, ephemeral_public_key): (Vec<u8>, Vec<u8>) = conn.query_row(
            "SELECT encrypted_key, ephemeral_public_key FROM Files LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;

        let ephemeral_public_key = ephemeral_public_key
            .try_into()
            .map_err(|_| anyhow!("Invalid ephemeral public key length"))?;

        Ok((encrypted_key, ephemeral_public_key))
    }
}

/// Create the metadata schema used by backup databases.
///
/// # Errors
/// Returns an error if the database cannot be opened or any schema statement fails.
pub fn create_metadata_schema(db_path: &Path) -> Result<()> {
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

        CREATE TABLE IF NOT EXISTS config_directories (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL UNIQUE
        );

        CREATE TABLE IF NOT EXISTS config_files (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL UNIQUE
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

fn create_connection_pool(db_file: &Path) -> Result<Arc<Pool<SqliteConnectionManager>>> {
    let manager = SqliteConnectionManager::file(db_file).with_init(|conn| {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;",
        )
    });

    let pool_size = u32::try_from(cmp::min(num_cpus::get_physical(), 32))?;

    Ok(Arc::new(
        Pool::builder().max_size(pool_size).build(manager)?,
    ))
}

fn configured_directories(conn: &Connection) -> Result<Vec<PathBuf>> {
    let directories: Vec<String> = conn
        .prepare("SELECT path FROM config_directories")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;

    Ok(directories.iter().map(PathBuf::from).collect())
}

fn configured_files(conn: &Connection) -> Result<Vec<PathBuf>> {
    let files: Vec<String> = conn
        .prepare("SELECT path FROM config_files")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;

    Ok(files.iter().map(PathBuf::from).collect())
}

fn restore_entries(conn: &Connection, version: i64) -> Result<Vec<RestoreEntry>> {
    let mut stmt = conn.prepare(
        "SELECT Paths.path, FileNames.name, Files.hash
         FROM FileNames
         JOIN Paths ON Paths.path_id = FileNames.path_id
         JOIN Files ON Files.file_id = FileNames.file_id
         WHERE FileNames.first_version <= ?1
           AND (
               FileNames.last_version IS NULL
               OR FileNames.last_version >= ?1
           )
         ORDER BY Paths.path, FileNames.name",
    )?;

    let mut entries = stmt
        .query_map(params![version], |row| {
            let parent: String = row.get(0)?;
            let name: String = row.get(1)?;
            let hash: String = row.get(2)?;

            Ok(RestoreEntry {
                path: PathBuf::from(parent).join(name),
                hash,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    entries.sort();

    Ok(entries)
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

    Ok(stmt.query_row(params![path], |row| row.get(0))?)
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
