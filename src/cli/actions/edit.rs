use crate::{
    cli::{actions::Action, globals::GlobalArgs},
    engine::edit::{EditBackupRequest, EditBackupResult, edit},
};
use anyhow::Result;
use std::path::PathBuf;

/// Handle the edit action.
///
/// # Errors
/// Returns an error if the backup database is missing or cannot be updated.
pub fn handle(action: Action, globals: &GlobalArgs) -> Result<()> {
    if let Action::Edit {
        name,
        add_directories,
        add_files,
        remove_directories,
        remove_files,
    } = action
    {
        let result = edit(EditBackupRequest {
            name: name.clone(),
            config_dir: globals.home.clone(),
            add_directories,
            add_files,
            remove_directories,
            remove_files,
        })?;

        if !globals.quiet {
            print_config(&name, &result);
        }
    }

    Ok(())
}

fn print_config(name: &str, result: &EditBackupResult) {
    println!("Backup: {name}");
    print_section("Directories", &result.directories);
    print_section("Files", &result.files);
}

fn print_section(label: &str, entries: &[PathBuf]) {
    if entries.is_empty() {
        return;
    }

    println!("  {label}:");

    let mut iter = entries.iter().peekable();
    while let Some(entry) = iter.next() {
        let prefix = if iter.peek().is_none() {
            "└──"
        } else {
            "├──"
        };
        println!("  {prefix} {}", entry.display());
    }
}
