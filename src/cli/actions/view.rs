use crate::{
    cli::{actions::Action, globals::GlobalArgs},
    engine::view::{
        ViewTarget, build_tree, load_snapshot, parse_target, render_lines, resolve_file,
    },
};
use anyhow::Result;
use std::path::Path;

/// Handle the view action.
///
/// # Errors
/// Returns an error if the backup database is missing or cannot be read.
pub fn handle(action: Action, globals: &GlobalArgs) -> Result<()> {
    if let Action::View {
        name,
        depth,
        version,
        target,
    } = action
    {
        match target.as_deref().map(parse_target).transpose()? {
            Some(ViewTarget::Id(id)) => show_file(globals, &name, version, id)?,
            Some(ViewTarget::Path(root)) => {
                list_tree(globals, &name, version, depth, Some(&root))?;
            }
            None => list_tree(globals, &name, version, depth, None)?,
        }
    }

    Ok(())
}

fn list_tree(
    globals: &GlobalArgs,
    name: &str,
    version: Option<i64>,
    depth: usize,
    root: Option<&Path>,
) -> Result<()> {
    let Some(snapshot) = load_snapshot(&globals.home, name, version, root)? else {
        println!(
            "No completed snapshot for \"{name}\" yet — run `backup run {name}` (a previous run may have been interrupted)."
        );
        return Ok(());
    };

    if !globals.quiet {
        println!("Backup: {name}");
        println!(
            "Version: {}{}",
            snapshot.version,
            format_timestamp(snapshot.timestamp)
        );
        if let Some(root) = root {
            println!("Path: {}", root.display());
        }
        println!();
    }

    if snapshot.entries.is_empty() {
        match root {
            Some(root) => println!(
                "(no files under {} in version {})",
                root.display(),
                snapshot.version
            ),
            None => println!("(no files in version {})", snapshot.version),
        }
        return Ok(());
    }

    let tree = build_tree(&snapshot.entries);
    for line in render_lines(&tree, depth) {
        println!("{line}");
    }

    Ok(())
}

fn show_file(globals: &GlobalArgs, name: &str, version: Option<i64>, id: i64) -> Result<()> {
    let Some((resolved_version, path)) = resolve_file(&globals.home, name, version, id)? else {
        println!("No snapshots recorded for \"{name}\". Run `backup run {name}` first.");
        return Ok(());
    };

    match path {
        Some(path) => {
            println!("File #{id} (version {resolved_version}):");
            println!("  {}", path.display());
            println!("Would restore to: {}", path.display());
        }
        None => println!("No file with id {id} in version {resolved_version}."),
    }

    Ok(())
}

fn format_timestamp(timestamp: Option<i64>) -> String {
    timestamp
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .map(|dt| format!(" ({})", dt.format("%Y-%m-%d %H:%M:%S UTC")))
        .unwrap_or_default()
}
