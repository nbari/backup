use crate::db::sqlite::SqliteCatalog;
use anyhow::{Result, anyhow};
use bip39::{Language, Mnemonic};
use std::path::PathBuf;
use tracing::debug;
use x25519_dalek::StaticSecret;

pub struct CreateBackupRequest {
    pub name: String,
    pub config_dir: PathBuf,
    pub directories: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
}

pub struct CreateBackupResult {
    pub recovery_phrase: String,
    pub db_path: PathBuf,
}

/// Create a backup definition and initialize its metadata catalog.
///
/// # Errors
/// Returns an error if the backup database cannot be created or initialized.
pub fn create(request: CreateBackupRequest) -> Result<CreateBackupResult> {
    let db_path = request.config_dir.join(format!("{}.db", request.name));

    if db_path.exists() {
        return Err(anyhow!(
            "A backup with the name '{}' already exists",
            request.name
        ));
    }

    let catalog = SqliteCatalog::initialize(&db_path)?;

    let mnemonic = Mnemonic::generate_in(Language::English, 12)?;
    let seed = mnemonic.to_seed("");

    let mut seed_bytes = [0u8; 32];
    let seed_prefix = seed
        .get(..32)
        .ok_or_else(|| anyhow!("Mnemonic seed is too short"))?;
    seed_bytes.copy_from_slice(seed_prefix);

    let private_key = StaticSecret::from(seed_bytes);
    let public_key = x25519_dalek::PublicKey::from(&private_key);

    debug!("Public Key: {:?}", hex::encode(public_key.as_bytes()));

    catalog.save_public_key(&public_key)?;

    let backup_dirs = get_unique_dir_parents(request.directories);
    catalog.save_directories(&backup_dirs)?;
    catalog.save_files(&request.files)?;

    Ok(CreateBackupResult {
        recovery_phrase: mnemonic.to_string(),
        db_path,
    })
}

fn get_unique_dir_parents(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    dirs.sort();

    let mut result = Vec::new();
    for dir in dirs {
        if !result.iter().any(|parent| dir.starts_with(parent)) {
            result.push(dir);
        }
    }

    result
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
        assert!(result.contains(&PathBuf::from("/a/b")));
        assert!(result.contains(&PathBuf::from("/b")));
    }

    // test the create_config_directories_table function
    #[test]
    fn test_create_db_config_directoris_and_files_table() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;

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

        let catalog = SqliteCatalog::initialize(&db_path)?;

        let backup_dirs = get_unique_dir_parents(dirs);

        catalog.save_directories(&backup_dirs)?;

        let result = catalog.configured_directories()?;
        assert_eq!(result.len(), 2);
        assert!(result.contains(&PathBuf::from("/a/b")));
        assert!(result.contains(&PathBuf::from("/b")));

        let files = vec![
            PathBuf::from("/a/b/c/file1.txt"),
            PathBuf::from("/a/b/c/d/file2.txt"),
            PathBuf::from("/a/file3.txt"),
            PathBuf::from("/z/file4.txt"),
        ];

        catalog.save_files(&files)?;

        let result = catalog.configured_files()?;
        assert_eq!(result.len(), 2);
        assert!(result.contains(&PathBuf::from("/a/file3.txt")));
        assert!(result.contains(&PathBuf::from("/z/file4.txt")));

        Ok(())
    }
}
