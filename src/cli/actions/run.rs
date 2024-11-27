use crate::cli::{actions::Action, globals::GlobalArgs};
use anyhow::{anyhow, Result};
use ignore::WalkBuilder;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// Handle the create action
pub fn handle(action: Action, globals: GlobalArgs) -> Result<()> {
    if let Action::Run {
        name,
        no_gitignore,
        no_compression: _,
        no_encryption: _,
        dry_run: _,
    } = action
    {
        let home_dir = globals.home;

        let db_file = home_dir.join(format!("{}.db", name));

        let directories = get_directories_to_backup(db_file)?;

        for directory in directories {
            if !directory.exists() {
                return Err(anyhow!("Directory does not exist: {}", directory.display()));
            }

            let iterator = walk_directory(&directory, no_gitignore);
            for file_result in iterator {
                match file_result {
                    Ok(file_path) => println!("Found Rust file: {}", file_path.display()),
                    Err(err) => eprintln!("Error: {}", err),
                }
            }
        }
    }

    Ok(())
}

// query the backup database for directories to backup
fn get_directories_to_backup(db_path: PathBuf) -> Result<Vec<PathBuf>> {
    let conn = Connection::open(db_path)?;

    let directories: Vec<String> = conn
        .prepare("SELECT path FROM config_directories")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;

    Ok(directories.iter().map(PathBuf::from).collect())
}

// Returns an iterator over files in a directory, respecting `.gitignore` rules unless `no_gitignore` is true.
fn walk_directory(
    base_dir: &Path,
    no_gitignore: bool,
) -> impl Iterator<Item = Result<PathBuf, ignore::Error>> {
    WalkBuilder::new(base_dir)
        .git_ignore(!no_gitignore)
        .build()
        .filter_map(|entry| match entry {
            Ok(e) if e.path().is_file() => Some(Ok(e.into_path())),
            Ok(_) => None,
            Err(err) => Some(Err(err)),
        })
}
