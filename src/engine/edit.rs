//! Edit an existing backup's configured directories and files.
//!
//! Applies additions and removals, then re-establishes the same invariant
//! `create` does: directories are collapsed to non-overlapping parents, and any
//! configured file that falls under a configured directory is dropped.

use crate::{db::sqlite::SqliteCatalog, engine::create::get_unique_dir_parents};
use anyhow::{Result, anyhow};
use std::path::PathBuf;

pub struct EditBackupRequest {
    pub name: String,
    pub config_dir: PathBuf,
    pub add_directories: Vec<PathBuf>,
    pub add_files: Vec<PathBuf>,
    pub remove_directories: Vec<PathBuf>,
    pub remove_files: Vec<PathBuf>,
}

pub struct EditBackupResult {
    pub directories: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
}

/// Apply edits to a backup's configuration and return the resulting sets.
///
/// # Errors
/// Returns an error if the backup database is missing or cannot be updated.
pub fn edit(request: EditBackupRequest) -> Result<EditBackupResult> {
    let db_path = request.config_dir.join(format!("{}.db", request.name));

    if !db_path.exists() {
        return Err(anyhow!(
            "No backup named \"{}\" found. Create a new backup first.",
            request.name
        ));
    }

    let catalog = SqliteCatalog::open(&db_path)?;

    // Directories: (existing ∪ added) \ removed, then collapse to unique parents.
    let directories = get_unique_dir_parents(merge(
        catalog.configured_directories()?,
        request.add_directories,
        &request.remove_directories,
    ));

    // Files: (existing ∪ added) \ removed, then drop any now covered by a dir.
    let files = merge(
        catalog.configured_files()?,
        request.add_files,
        &request.remove_files,
    )
    .into_iter()
    .filter(|file| !directories.iter().any(|dir| file.starts_with(dir)))
    .collect::<Vec<_>>();

    catalog.set_directories(&directories)?;
    catalog.set_files(&files)?;

    Ok(EditBackupResult { directories, files })
}

/// Combine `existing` with `add`, drop anything in `remove`, and de-duplicate
/// while preserving a stable (sorted) order.
fn merge(existing: Vec<PathBuf>, add: Vec<PathBuf>, remove: &[PathBuf]) -> Vec<PathBuf> {
    let mut result: Vec<PathBuf> = existing
        .into_iter()
        .chain(add)
        .filter(|path| !remove.contains(path))
        .collect();

    result.sort();
    result.dedup();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::create::{CreateBackupRequest, create};

    fn setup(dirs: &[&str], files: &[&str]) -> Result<(tempfile::TempDir, String)> {
        let temp_dir = tempfile::tempdir()?;
        let name = "demo".to_string();

        create(CreateBackupRequest {
            name: name.clone(),
            config_dir: temp_dir.path().to_path_buf(),
            directories: dirs.iter().map(PathBuf::from).collect(),
            files: files.iter().map(PathBuf::from).collect(),
        })?;

        Ok((temp_dir, name))
    }

    fn request(
        dir: &std::path::Path,
        name: &str,
        add_dirs: &[&str],
        add_files: &[&str],
        rm_dirs: &[&str],
        rm_files: &[&str],
    ) -> EditBackupRequest {
        EditBackupRequest {
            name: name.to_string(),
            config_dir: dir.to_path_buf(),
            add_directories: add_dirs.iter().map(PathBuf::from).collect(),
            add_files: add_files.iter().map(PathBuf::from).collect(),
            remove_directories: rm_dirs.iter().map(PathBuf::from).collect(),
            remove_files: rm_files.iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn add_and_remove_directories() -> Result<()> {
        let (temp_dir, name) = setup(&["/a/b"], &[])?;

        let result = edit(request(temp_dir.path(), &name, &["/c"], &[], &[], &[]))?;
        assert!(result.directories.contains(&PathBuf::from("/a/b")));
        assert!(result.directories.contains(&PathBuf::from("/c")));

        let result = edit(request(temp_dir.path(), &name, &[], &[], &["/a/b"], &[]))?;
        assert_eq!(result.directories, vec![PathBuf::from("/c")]);

        Ok(())
    }

    #[test]
    fn adding_parent_collapses_existing_child() -> Result<()> {
        let (temp_dir, name) = setup(&["/home/user/docs"], &[])?;

        let result = edit(request(
            temp_dir.path(),
            &name,
            &["/home/user"],
            &[],
            &[],
            &[],
        ))?;
        assert_eq!(result.directories, vec![PathBuf::from("/home/user")]);

        Ok(())
    }

    #[test]
    fn file_under_a_dir_is_dropped() -> Result<()> {
        let (temp_dir, name) = setup(&[], &["/data/notes.txt"])?;

        // Adding /data should subsume the standalone file.
        let result = edit(request(temp_dir.path(), &name, &["/data"], &[], &[], &[]))?;
        assert_eq!(result.directories, vec![PathBuf::from("/data")]);
        assert!(result.files.is_empty());

        Ok(())
    }

    #[test]
    fn editing_missing_backup_errors() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let result = edit(request(temp_dir.path(), "nope", &["/a"], &[], &[], &[]));
        assert!(result.is_err());
        Ok(())
    }
}
