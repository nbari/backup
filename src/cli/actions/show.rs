use crate::cli::{actions::Action, globals::GlobalArgs};
use anyhow::{anyhow, Result};
use rusqlite::Connection;
use std::{fs, path::PathBuf};

/// Handle the create action
pub fn handle(action: Action, globals: GlobalArgs) -> Result<()> {
    if matches!(action, Action::Show) {
        let home_dir = globals.home;

        let db_files = get_db_files(home_dir)?;

        if db_files.is_empty() {
            println!("No Backup files found.");
            return Ok(());
        }

        let mut db_iter = db_files.iter().peekable();

        while let Some(db_file) = db_iter.next() {
            let conn = Connection::open(db_file)?;

            if let Some(file_name) = db_file.file_stem() {
                // `file_stem` gives the file name without the extension
                println!("Backup: {}", file_name.to_string_lossy());
            }

            // Fetch paths from config_directories
            let directories: Vec<String> = conn
                .prepare("SELECT path FROM config_directories")?
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?;

            // Fetch paths from config_files
            let files: Vec<String> = conn
                .prepare("SELECT path FROM config_files")?
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?;

            // Print results using tree format
            if !directories.is_empty() {
                print_tree("Directories", &directories, 2);
            }

            if !files.is_empty() {
                // Print a blank line before files
                println!();
                print_tree("Files", &files, 2);
            }

            // Print a blank line only if there's a next item
            if db_iter.peek().is_some() {
                println!();
            }
        }
    }

    Ok(())
}

fn print_tree(label: &str, entries: &[String], indent: usize) {
    // Print the label with formatting
    println!("{:indent$}{}:", "", label, indent = indent);

    let mut iter = entries.iter().peekable();

    while let Some(entry) = iter.next() {
        let is_last = iter.peek().is_none();
        let prefix = if is_last {
            "└──" // For the last entry
        } else {
            "├──" // For other entries
        };

        println!("{:indent$}{} {}", "", prefix, entry, indent = indent);
    }
}

fn get_db_files(dir: PathBuf) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Err(anyhow!("Directory does not exist"));
    };

    let mut db_files = Vec::new();

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() {
            if let Some(extension) = path.extension() {
                if extension == "db" {
                    db_files.push(path);
                }
            }
        }
    }

    Ok(db_files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn test_get_db_files() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.db");
        File::create(&file).unwrap();

        let result = get_db_files(dir.path().to_path_buf());
        assert!(result.is_ok());
        let r = result.unwrap();
        assert!(!r.is_empty());
        assert_eq!(r.len(), 1);
    }

    #[test]
    fn test_get_db_files_no_dir() {
        let dir = PathBuf::from("/tmp-non-existent");
        let result = get_db_files(dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_db_files_no_files() {
        let dir = tempdir().unwrap();
        let result = get_db_files(dir.path().to_path_buf());
        assert!(result.is_ok());
    }
}
