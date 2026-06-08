use crate::db::sqlite::SqliteCatalog;
use anyhow::{Result, anyhow};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub struct BackupDefinition {
    pub name: String,
    pub directories: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
    pub destinations: Vec<String>,
}

/// List configured backups in a config directory.
///
/// # Errors
/// Returns an error if backup databases cannot be listed or read.
pub fn list(config_dir: &Path) -> Result<Vec<BackupDefinition>> {
    let db_files = get_db_files(config_dir)?;
    let mut backups = Vec::new();

    for db_file in db_files {
        let catalog = SqliteCatalog::open(&db_file)?;
        let name = db_file
            .file_stem()
            .ok_or_else(|| anyhow!("Invalid backup database file name"))?
            .to_string_lossy()
            .to_string();

        backups.push(BackupDefinition {
            name,
            directories: catalog.configured_directories()?,
            files: catalog.configured_files()?,
            destinations: catalog.configured_destinations()?,
        });
    }

    Ok(backups)
}

fn get_db_files(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Err(anyhow!("Directory does not exist"));
    }

    let mut db_files = Vec::new();

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file()
            && let Some(extension) = path.extension()
            && extension == "db"
        {
            db_files.push(path);
        }
    }

    Ok(db_files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn test_get_db_files() -> Result<()> {
        let dir = tempdir()?;
        let file = dir.path().join("test.db");
        File::create(&file)?;

        let r = get_db_files(dir.path())?;
        assert!(!r.is_empty());
        assert_eq!(r.len(), 1);
        Ok(())
    }

    #[test]
    fn test_get_db_files_no_dir() {
        let dir = PathBuf::from("/tmp-non-existent");
        let result = get_db_files(&dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_db_files_no_files() -> Result<()> {
        let dir = tempdir()?;
        let result = get_db_files(dir.path());
        assert!(result.is_ok());
        Ok(())
    }
}
