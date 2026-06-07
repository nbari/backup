use crate::{cli::globals::GlobalArgs, engine::show::list};
use anyhow::Result;
use std::path::PathBuf;

/// Handle the show action.
///
/// # Errors
/// Returns an error if backup databases cannot be listed or read.
pub fn handle(globals: &GlobalArgs) -> Result<()> {
    let backups = list(&globals.home)?;

    if backups.is_empty() {
        println!("No Backup files found.");
        return Ok(());
    }

    let mut backup_iter = backups.iter().peekable();

    while let Some(backup) = backup_iter.next() {
        println!("Backup: {}", backup.name);

        if !backup.directories.is_empty() {
            print_tree("Directories", &backup.directories, 2);
        }

        if !backup.files.is_empty() {
            println!();
            print_tree("Files", &backup.files, 2);
        }

        if backup_iter.peek().is_some() {
            println!();
        }
    }

    Ok(())
}

fn print_tree(label: &str, entries: &[PathBuf], indent: usize) {
    println!("{:indent$}{}:", "", label, indent = indent);

    let mut iter = entries.iter().peekable();

    while let Some(entry) = iter.next() {
        let is_last = iter.peek().is_none();
        let prefix = if is_last { "└──" } else { "├──" };

        println!(
            "{:indent$}{} {}",
            "",
            prefix,
            entry.display(),
            indent = indent
        );
    }
}
