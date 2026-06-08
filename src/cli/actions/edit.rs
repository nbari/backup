use crate::{
    cli::{actions::Action, globals::GlobalArgs},
    engine::edit::{EditBackupRequest, EditBackupResult, edit},
};
use anyhow::Result;

/// Handle the edit action.
///
/// # Errors
/// Returns an error if the backup database is missing or cannot be updated.
pub fn handle(action: Action, globals: &GlobalArgs) -> Result<()> {
    if let Action::Edit {
        name,
        add_directories,
        add_files,
        add_destinations,
        remove_directories,
        remove_files,
        remove_destinations,
    } = action
    {
        let result = edit(EditBackupRequest {
            name: name.clone(),
            config_dir: globals.home.clone(),
            add_directories,
            add_files,
            add_destinations,
            remove_directories,
            remove_files,
            remove_destinations,
        })?;

        if !globals.quiet {
            print_config(&name, &result);
        }
    }

    Ok(())
}

fn print_config(name: &str, result: &EditBackupResult) {
    println!("Backup: {name}");
    let dirs: Vec<String> = result
        .directories
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    let files: Vec<String> = result
        .files
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    print_section("Directories", &dirs);
    print_section("Files", &files);
    print_section("Destinations", &result.destinations);
}

fn print_section(label: &str, entries: &[String]) {
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
        println!("  {prefix} {entry}");
    }
}
