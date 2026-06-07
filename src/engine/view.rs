//! Render the backed-up file tree of a snapshot.
//!
//! Surfaces the versioned metadata (`SqliteCatalog::restore_entries`) as a
//! directory tree so a user can browse what a backup contains before restoring.
//! Tree construction is pure and unit-tested here; the CLI action only prints.

use crate::db::sqlite::SqliteCatalog;
use anyhow::{Result, anyhow};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

/// A point-in-time snapshot of the files tracked by a backup.
pub struct Snapshot {
    pub version: i64,
    pub timestamp: Option<i64>,
    pub paths: Vec<PathBuf>,
}

/// A node in the backed-up file tree.
#[derive(Debug, Default)]
pub struct TreeNode {
    children: BTreeMap<String, TreeNode>,
    is_file: bool,
    /// Number of file leaves contained anywhere beneath this node.
    file_count: usize,
}

/// Load a snapshot for a backup.
///
/// Returns `Ok(None)` when the backup exists but has no recorded versions yet.
///
/// # Errors
/// Returns an error if the backup database is missing or cannot be read.
pub fn load_snapshot(
    config_dir: &Path,
    name: &str,
    version: Option<i64>,
) -> Result<Option<Snapshot>> {
    let db_file = config_dir.join(format!("{name}.db"));

    if !db_file.exists() {
        return Err(anyhow!(
            "No backup named \"{name}\" found. Create a new backup first."
        ));
    }

    let catalog = SqliteCatalog::open(&db_file)?;

    let version = match version {
        Some(version) => version,
        None => match catalog.latest_version()? {
            Some(version) => version,
            None => return Ok(None),
        },
    };

    let paths = catalog
        .restore_entries(version)?
        .into_iter()
        .map(|entry| entry.path)
        .collect();
    let timestamp = catalog.version_timestamp(version)?;

    Ok(Some(Snapshot {
        version,
        timestamp,
        paths,
    }))
}

/// Build a tree from a set of absolute file paths.
#[must_use]
pub fn build_tree(paths: &[PathBuf]) -> TreeNode {
    let mut root = TreeNode::default();

    for path in paths {
        let mut node = &mut root;
        for component in path_segments(path) {
            node = node.children.entry(component).or_default();
        }
        node.is_file = true;
    }

    root.finalize();
    root
}

/// Render a tree as box-drawing lines.
///
/// `depth` limits how many levels are shown; directories at the limit are
/// summarized with their file count. `depth == 0` renders the full tree.
#[must_use]
pub fn render_lines(root: &TreeNode, depth: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let entries: Vec<(&String, &TreeNode)> = root.children.iter().collect();
    let total = entries.len();

    for (index, (name, node)) in entries.into_iter().enumerate() {
        render_node(name, node, depth, 1, "", index + 1 == total, &mut lines);
    }

    lines
}

impl TreeNode {
    /// Populate `file_count` for every node; returns the file leaves beneath self.
    fn finalize(&mut self) -> usize {
        if self.children.is_empty() {
            self.file_count = 0;
            return usize::from(self.is_file);
        }

        let total = self.children.values_mut().map(TreeNode::finalize).sum();
        self.file_count = total;
        total
    }
}

fn path_segments(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| match component {
            Component::RootDir => "/".to_string(),
            Component::Prefix(prefix) => prefix.as_os_str().to_string_lossy().into_owned(),
            Component::Normal(segment) => segment.to_string_lossy().into_owned(),
            Component::CurDir => ".".to_string(),
            Component::ParentDir => "..".to_string(),
        })
        .collect()
}

/// Collapse a chain of single-child directories into one display segment, e.g.
/// `/` → `home` → `user1` becomes `/home/user1`. Stops at a branch or a file so
/// the contents stay listable.
fn collapse<'a>(name: &str, node: &'a TreeNode) -> (String, &'a TreeNode) {
    let mut display = name.to_string();
    let mut current = node;

    while !current.is_file && current.children.len() == 1 {
        let Some((child_name, child)) = current.children.iter().next() else {
            break;
        };
        if child.is_file {
            break;
        }

        // Avoid a leading "//" when collapsing the filesystem root.
        if display == "/" {
            display.push_str(child_name);
        } else {
            display.push('/');
            display.push_str(child_name);
        }
        current = child;
    }

    (display, current)
}

fn render_node(
    name: &str,
    node: &TreeNode,
    depth: usize,
    level: usize,
    prefix: &str,
    is_last: bool,
    lines: &mut Vec<String>,
) {
    let (display, target) = collapse(name, node);
    let connector = if is_last { "└── " } else { "├── " };

    if target.is_file {
        lines.push(format!("{prefix}{connector}{display}"));
        return;
    }

    if depth != 0 && level >= depth {
        let count = target.file_count;
        let unit = if count == 1 { "file" } else { "files" };
        lines.push(format!("{prefix}{connector}{display}/ ({count} {unit})"));
        return;
    }

    lines.push(format!("{prefix}{connector}{display}/"));

    let child_prefix = format!("{prefix}{}", if is_last { "    " } else { "│   " });
    let entries: Vec<(&String, &TreeNode)> = target.children.iter().collect();
    let total = entries.len();

    for (index, (child_name, child)) in entries.into_iter().enumerate() {
        render_node(
            child_name,
            child,
            depth,
            level + 1,
            &child_prefix,
            index + 1 == total,
            lines,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(paths: &[&str]) -> TreeNode {
        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        build_tree(&paths)
    }

    fn child<'a>(node: &'a TreeNode, key: &str) -> Result<&'a TreeNode> {
        node.children
            .get(key)
            .ok_or_else(|| anyhow!("missing child {key}"))
    }

    #[test]
    fn build_tree_groups_paths_and_counts_files() -> Result<()> {
        let root = tree(&["/a/b/f1", "/a/b/f2", "/a/c/f3"]);

        // root -> "/" -> "a" -> {b, c}
        let slash = child(&root, "/")?;
        assert_eq!(slash.file_count, 3);

        let a = child(slash, "a")?;
        assert_eq!(a.file_count, 3);
        assert_eq!(child(a, "b")?.file_count, 2);
        assert_eq!(child(a, "c")?.file_count, 1);

        Ok(())
    }

    #[test]
    fn single_child_chains_collapse() {
        let lines = render_lines(
            &tree(&["/home/user1/docs/a.txt", "/home/user1/docs/b.txt"]),
            0,
        );

        // The /home/user1/docs chain collapses into one line.
        assert_eq!(
            lines,
            vec![
                "└── /home/user1/docs/".to_string(),
                "    ├── a.txt".to_string(),
                "    └── b.txt".to_string(),
            ]
        );
    }

    #[test]
    fn depth_truncates_directories_with_counts() {
        let root = tree(&[
            "/srv/data/logs/a.log",
            "/srv/data/logs/b.log",
            "/srv/data/conf/app.toml",
        ]);

        // depth 2: collapsed root is level 1, its children are level 2 (truncated).
        let lines = render_lines(&root, 2);
        assert_eq!(
            lines,
            vec![
                "└── /srv/data/".to_string(),
                "    ├── conf/ (1 file)".to_string(),
                "    └── logs/ (2 files)".to_string(),
            ]
        );

        // depth 0: full tree, files listed.
        let full = render_lines(&root, 0);
        assert!(full.iter().any(|line| line.ends_with("app.toml")));
        assert!(full.iter().any(|line| line.ends_with("a.log")));
    }

    #[test]
    fn duplicate_content_does_not_change_tree() {
        // Identical content under different names still shows both paths.
        let lines = render_lines(&tree(&["/d/dup_a.txt", "/d/dup_b.txt"]), 0);
        assert_eq!(
            lines,
            vec![
                "└── /d/".to_string(),
                "    ├── dup_a.txt".to_string(),
                "    └── dup_b.txt".to_string(),
            ]
        );
    }
}
