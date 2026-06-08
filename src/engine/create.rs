use crate::{
    db::sqlite::SqliteCatalog,
    engine::wkey,
    utils::crypto::{content_keypair, generate_naming_key, seal_naming_key},
};
use anyhow::{Result, anyhow};
use bip39::{Language, Mnemonic};
use std::path::PathBuf;
use tracing::debug;

pub struct CreateBackupRequest {
    pub name: String,
    pub config_dir: PathBuf,
    pub directories: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
    pub destinations: Vec<String>,
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
    let (_, public_key) = content_keypair(&mnemonic)?;

    debug!("Public Key: {:?}", hex::encode(public_key.as_bytes()));

    // Generate the naming key, seal it to the public key for at-rest recovery,
    // and cache the plaintext in {name}.wkey so cron runs stay unattended. The
    // public key and the sealed naming key are written atomically.
    let naming_key = generate_naming_key();
    let sealed = seal_naming_key(&naming_key, &public_key)?;
    catalog.save_keys(&public_key, &sealed)?;
    wkey::write_naming_key(&request.config_dir, &request.name, &naming_key)?;

    let backup_dirs = get_unique_dir_parents(request.directories);
    catalog.save_directories(&backup_dirs)?;
    catalog.save_files(&request.files)?;
    catalog.save_destinations(&request.destinations)?;

    Ok(CreateBackupResult {
        recovery_phrase: mnemonic.to_string(),
        db_path,
    })
}

/// Collapse a set of directories to non-overlapping parents (drops any directory
/// that is nested under another in the set).
pub(crate) fn get_unique_dir_parents(mut dirs: Vec<PathBuf>) -> Vec<PathBuf> {
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
    use crate::utils::crypto::unseal_naming_key;

    #[test]
    fn create_writes_wkey_and_naming_key_recovers_from_mnemonic() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let name = "demo";

        let result = create(CreateBackupRequest {
            name: name.to_string(),
            config_dir: temp_dir.path().to_path_buf(),
            directories: Vec::new(),
            files: Vec::new(),
            destinations: Vec::new(),
        })?;

        // The cache exists after create and holds a 32-byte naming key.
        let cached = wkey::load_naming_key(temp_dir.path(), name)?
            .ok_or_else(|| anyhow!("wkey cache should exist after create"))?;

        // Simulate a fresh machine: drop the cache and recover from .db + mnemonic.
        std::fs::remove_file(wkey::wkey_path(temp_dir.path(), name))?;
        assert!(wkey::load_naming_key(temp_dir.path(), name)?.is_none());

        let mnemonic = Mnemonic::parse_in_normalized(Language::English, &result.recovery_phrase)?;
        let catalog = SqliteCatalog::open(&result.db_path)?;
        let recovered = unseal_naming_key(&catalog.sealed_naming_key()?, &mnemonic)?;

        assert_eq!(*cached, *recovered);

        // save_keys wrote both halves atomically: the public key reads back and
        // matches the one derived from the recovery mnemonic.
        let (_, expected_public) = content_keypair(&mnemonic)?;
        assert_eq!(catalog.public_key()?.to_bytes(), expected_public.to_bytes());

        Ok(())
    }

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
