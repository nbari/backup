use crate::cli::actions::Action;
use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

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

        // create the config_files tables
        // exclude files if they are within the directories that are being backed up
        create_db_config_files_table(&db_path, file.unwrap_or_default())?;

        // create the config_exclusions table
        let exclusions = exclude.unwrap_or_default();
        create_db_config_exclusions_table(&db_path, exclusions)?;
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

fn parse_gitignore_pattern(pattern: &str) -> (String, String) {
    if pattern.starts_with('!') {
        ("negation".to_string(), pattern[1..].to_string()) // Remove `!` and classify as negation
    } else if pattern.contains("**") {
        ("recursive".to_string(), pattern.to_string()) // Recursive patterns
    } else if pattern.contains('*') {
        ("wildcard".to_string(), pattern.to_string()) // Wildcard patterns
    } else {
        ("path".to_string(), pattern.to_string()) // Literal path
    }
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

fn create_db_config_exclusions_table(db_path: &PathBuf, exclusions: Vec<String>) -> Result<()> {
    let conn = Connection::open(db_path)?;

    // Table to track each backup version
    conn.execute(
        "CREATE TABLE IF NOT EXISTS config_exclusions (
    id INTEGER PRIMARY KEY,
    pattern TEXT NOT NULL,      -- Raw pattern from the input
    type TEXT NOT NULL,         -- 'path', 'wildcard', 'recursive', or 'negation'
    UNIQUE(pattern)             -- Ensure no duplicate patterns
)",
        [],
    )?;

    // Prepare the insert statement
    let mut stmt =
        conn.prepare("INSERT OR IGNORE INTO config_exclusions (type, pattern) VALUES (?1, ?2)")?;

    for pattern in exclusions {
        let (pattern_type, processed_pattern) = parse_gitignore_pattern(&pattern);
        stmt.execute(params![pattern_type, processed_pattern])?;
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

    #[test]
    fn test_create_db_config_exclusions_table() {
        let temp_dir = tempfile::tempdir().unwrap();

        let db_path = temp_dir.path().join("test.db");

        let exclusions = vec![
            "/a/b/c".to_string(),
            "!/a/b/d".to_string(),
            "**/c".to_string(),
            "*.txt".to_string(),
        ];

        create_db_tables(&db_path).unwrap();

        create_db_config_exclusions_table(&db_path, exclusions).unwrap();

        let conn = Connection::open(&db_path).unwrap();

        let mut stmt = conn
            .prepare("SELECT pattern, type FROM config_exclusions")
            .unwrap();

        let exclusions_iter = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap();

        let result: Vec<(String, String)> =
            exclusions_iter.filter_map(|result| result.ok()).collect();

        assert_eq!(result.len(), 4);
        assert!(result.contains(&("/a/b/c".to_string(), "path".to_string())));
        assert!(result.contains(&("/a/b/d".to_string(), "negation".to_string())));
        assert!(result.contains(&("**/c".to_string(), "recursive".to_string())));
        assert!(result.contains(&("*.txt".to_string(), "wildcard".to_string())));
    }
}
