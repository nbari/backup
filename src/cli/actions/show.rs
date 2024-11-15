use crate::cli::{actions::Action, globals::GlobalArgs};
use anyhow::{anyhow, Result};
use std::{fs, path::PathBuf};

/// Handle the create action
pub fn handle(action: Action, globals: GlobalArgs) -> Result<()> {
    if matches!(action, Action::Show) {
        let home_dir = globals.home;

        list_db_files(home_dir)?;
    }

    Ok(())
}

fn list_db_files(dir: PathBuf) -> Result<()> {
    if !dir.is_dir() {
        return Err(anyhow!("Directory does not exist"));
    };

    let mut found = false;

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            if let Some(extension) = path.extension() {
                if extension == "db" {
                    if let Some(file_name) = path.file_stem() {
                        // `file_stem` gives the file name without the extension
                        println!("{}", file_name.to_string_lossy());
                        found = true;
                    }
                }
            }
        }
    }

    if !found {
        println!("No configurations found.");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn test_list_db_files() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.db");
        File::create(&file).unwrap();

        let result = list_db_files(dir.path().to_path_buf());
        assert!(result.is_ok());
    }

    #[test]
    fn test_list_db_files_no_dir() {
        let dir = PathBuf::from("/tmp-non-existent");
        let result = list_db_files(dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_list_db_files_no_files() {
        let dir = tempdir().unwrap();
        let result = list_db_files(dir.path().to_path_buf());
        assert!(result.is_ok());
    }
}
