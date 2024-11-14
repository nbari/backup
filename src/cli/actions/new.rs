use crate::cli::actions::Action;
use anyhow::Result;
use rusqlite::Connection;
use std::fs;
use std::path::PathBuf;

/// Handle the create action
pub fn handle(action: Action) -> Result<()> {
    match action {
        Action::New {
            name,
            config,
            directory,
            file,
            exclude,
        } => {
            let db_path = config.join(format!("{}.db", name));

            create_db_tables(db_path)?;

            if let Some(directory) = directory {
                for dir in directory {
                    println!("Directory: {}", fs::canonicalize(dir)?.display());
                }
            }

            if let Some(file) = file {
                for file in file {
                    println!("File: {}", fs::canonicalize(file)?.display());
                }
            }

            if let Some(exclude) = exclude {
                for exclude in exclude {
                    println!("Exclude: {}", exclude);
                }
            }
        }
    }

    Ok(())
}

fn create_db_tables(db_path: PathBuf) -> Result<()> {
    let conn = Connection::open(db_path)?;

    // create the tables
    conn.execute(
        "CREATE TABLE IF NOT EXISTS Directory (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    parent_id INTEGER,
    FOREIGN KEY (parent_id) REFERENCES Directory (id)
)",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS File (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    size INTEGER NOT NULL,
    directory_id INTEGER,
    hash TEXT NOT NULL,
    FOREIGN KEY (directory_id) REFERENCES Directory (id)
)",
        [],
    )?;

    Ok(())
}
