use crate::{
    cli::{actions::Action, globals::GlobalArgs},
    engine::view::{build_tree, load_snapshot, render_lines},
};
use anyhow::Result;

/// Handle the view action.
///
/// # Errors
/// Returns an error if the backup database is missing or cannot be read.
pub fn handle(action: Action, globals: &GlobalArgs) -> Result<()> {
    if let Action::View {
        name,
        depth,
        version,
    } = action
    {
        let Some(snapshot) = load_snapshot(&globals.home, &name, version)? else {
            println!("No snapshots recorded for \"{name}\". Run `backup run {name}` first.");
            return Ok(());
        };

        if !globals.quiet {
            println!("Backup: {name}");
            println!(
                "Version: {}{}",
                snapshot.version,
                format_timestamp(snapshot.timestamp)
            );
            println!();
        }

        if snapshot.paths.is_empty() {
            println!("(no files in this version)");
            return Ok(());
        }

        let tree = build_tree(&snapshot.paths);
        for line in render_lines(&tree, depth) {
            println!("{line}");
        }
    }

    Ok(())
}

fn format_timestamp(timestamp: Option<i64>) -> String {
    timestamp
        .and_then(|seconds| chrono::DateTime::from_timestamp(seconds, 0))
        .map(|dt| format!(" ({})", dt.format("%Y-%m-%d %H:%M:%S UTC")))
        .unwrap_or_default()
}
