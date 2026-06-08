//! Render the backed-up file tree of a snapshot.
//!
//! Surfaces the versioned metadata (`SqliteCatalog::view_entries`) as a
//! directory tree so a user can browse what a backup contains before restoring.
//! Files carry their stable id (`FileNames.name_id`) for addressing; directories
//! are navigated by path. Tree construction is pure and unit-tested here; the
//! CLI action only prints.

use crate::db::sqlite::{SqliteCatalog, ViewEntry};
use anyhow::{Result, anyhow};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

/// A point-in-time snapshot of the files tracked by a backup.
pub struct Snapshot {
    pub version: i64,
    pub timestamp: Option<i64>,
    pub entries: Vec<ViewEntry>,
}

/// How a `view` target addresses the snapshot.
#[derive(Debug, Eq, PartialEq)]
pub enum ViewTarget {
    /// A file id (`FileNames.name_id`).
    Id(i64),
    /// A directory path to drill into.
    Path(PathBuf),
}

/// A node in the backed-up file tree.
#[derive(Debug, Default)]
pub struct TreeNode {
    children: BTreeMap<String, TreeNode>,
    is_file: bool,
    /// File id (`name_id`) for leaf files; `None` for directories.
    id: Option<i64>,
    /// Number of file leaves contained anywhere beneath this node.
    file_count: usize,
}

/// Classify a `view` target argument as a file id or a directory path.
///
/// Accepts `7` or `#7` for ids and an absolute path (`/home/user`) for drill-down.
///
/// # Errors
/// Returns an error if the argument is neither a numeric id nor an absolute path.
pub fn parse_target(raw: &str) -> Result<ViewTarget> {
    if raw.starts_with('/') {
        return Ok(ViewTarget::Path(PathBuf::from(raw)));
    }

    let digits = raw.strip_prefix('#').unwrap_or(raw);
    if let Ok(id) = digits.parse::<i64>() {
        return Ok(ViewTarget::Id(id));
    }

    Err(anyhow!(
        "expected a numeric id (e.g. 7 or #7) or an absolute path (e.g. /home/user); got \"{raw}\""
    ))
}

/// Load a snapshot for a backup, optionally scoped to a directory subtree.
///
/// Returns `Ok(None)` when the backup exists but has no recorded versions yet.
///
/// # Errors
/// Returns an error if the backup database is missing or cannot be read.
pub fn load_snapshot(
    config_dir: &Path,
    name: &str,
    version: Option<i64>,
    root: Option<&Path>,
) -> Result<Option<Snapshot>> {
    let Some((catalog, version)) = open_at_version(config_dir, name, version)? else {
        return Ok(None);
    };

    let entries = catalog.view_entries(version, root)?;
    let timestamp = catalog.version_timestamp(version)?;

    Ok(Some(Snapshot {
        version,
        timestamp,
        entries,
    }))
}

/// Resolve a file id to its full path at a version.
///
/// Returns `Ok(None)` when the backup has no recorded versions. Otherwise returns
/// the resolved version paired with the file's path, or `None` for that path when
/// no file with the id is active at that version.
///
/// # Errors
/// Returns an error if the backup database is missing or cannot be read.
pub fn resolve_file(
    config_dir: &Path,
    name: &str,
    version: Option<i64>,
    id: i64,
) -> Result<Option<(i64, Option<PathBuf>)>> {
    let Some((catalog, version)) = open_at_version(config_dir, name, version)? else {
        return Ok(None);
    };

    let path = catalog.file_path_at_version(id, version)?;
    Ok(Some((version, path)))
}

/// Build a tree from snapshot entries; leaf files keep their id.
#[must_use]
pub fn build_tree(entries: &[ViewEntry]) -> TreeNode {
    let mut root = TreeNode::default();

    for entry in entries {
        let mut node = &mut root;
        for component in path_segments(&entry.path) {
            node = node.children.entry(component).or_default();
        }
        node.is_file = true;
        node.id = Some(entry.id);
    }

    root.finalize();
    root
}

/// Render a tree as box-drawing lines with a left id gutter for files.
///
/// `depth` limits how many levels are shown; directories at the limit are
/// summarized with their file count. `depth == 0` renders the full tree.
#[must_use]
pub fn render_lines(root: &TreeNode, depth: usize) -> Vec<String> {
    let mut rows: Vec<(Option<i64>, String)> = Vec::new();
    let entries: Vec<(&String, &TreeNode)> = root.children.iter().collect();
    let total = entries.len();

    for (index, (name, node)) in entries.into_iter().enumerate() {
        render_node(name, node, depth, 1, "", index + 1 == total, &mut rows);
    }

    let id_width = rows
        .iter()
        .filter_map(|(id, _)| id.map(|id| id.to_string().len()))
        .max()
        .unwrap_or(0);

    rows.into_iter()
        .map(|(id, line)| {
            if id_width == 0 {
                return line;
            }
            let gutter = match id {
                Some(id) => format!("[{id:>id_width$}]"),
                None => " ".repeat(id_width + 2),
            };
            format!("{gutter} {line}")
        })
        .collect()
}

fn open_at_version(
    config_dir: &Path,
    name: &str,
    version: Option<i64>,
) -> Result<Option<(SqliteCatalog, i64)>> {
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

    Ok(Some((catalog, version)))
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
    rows: &mut Vec<(Option<i64>, String)>,
) {
    let (display, target) = collapse(name, node);
    let connector = if is_last { "└── " } else { "├── " };

    if target.is_file {
        rows.push((target.id, format!("{prefix}{connector}{display}")));
        return;
    }

    if depth != 0 && level >= depth {
        let count = target.file_count;
        let unit = if count == 1 { "file" } else { "files" };
        rows.push((
            None,
            format!("{prefix}{connector}{display}/ ({count} {unit})"),
        ));
        return;
    }

    rows.push((None, format!("{prefix}{connector}{display}/")));

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
            rows,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(items: &[(i64, &str)]) -> Vec<ViewEntry> {
        items
            .iter()
            .map(|(id, path)| ViewEntry {
                id: *id,
                path: PathBuf::from(path),
            })
            .collect()
    }

    fn tree(items: &[(i64, &str)]) -> TreeNode {
        build_tree(&entries(items))
    }

    fn child<'a>(node: &'a TreeNode, key: &str) -> Result<&'a TreeNode> {
        node.children
            .get(key)
            .ok_or_else(|| anyhow!("missing child {key}"))
    }

    #[test]
    fn parse_target_classifies_ids_and_paths() -> Result<()> {
        assert_eq!(parse_target("7")?, ViewTarget::Id(7));
        assert_eq!(parse_target("#7")?, ViewTarget::Id(7));
        assert_eq!(
            parse_target("/a/b")?,
            ViewTarget::Path(PathBuf::from("/a/b"))
        );
        assert!(parse_target("nope").is_err());
        Ok(())
    }

    #[test]
    fn build_tree_assigns_ids_and_counts_files() -> Result<()> {
        let root = tree(&[(1, "/a/b/f1"), (2, "/a/b/f2"), (3, "/a/c/f3")]);

        let slash = child(&root, "/")?;
        assert_eq!(slash.file_count, 3);

        let a = child(slash, "a")?;
        assert_eq!(a.file_count, 3);
        assert_eq!(child(child(a, "b")?, "f1")?.id, Some(1));
        assert_eq!(child(child(a, "c")?, "f3")?.id, Some(3));
        // Directories carry no id.
        assert_eq!(a.id, None);

        Ok(())
    }

    #[test]
    fn render_shows_id_gutter_for_files_and_blanks_for_dirs() {
        let lines = render_lines(
            &tree(&[
                (1, "/home/user1/docs/a.txt"),
                (12, "/home/user1/docs/b.txt"),
            ]),
            0,
        );

        // Gutter is sized to the widest id (12 -> width 2): "[ 1]" / "[12]";
        // the directory line gets a blank, aligned gutter.
        assert_eq!(
            lines,
            vec![
                "     └── /home/user1/docs/".to_string(),
                "[ 1]     ├── a.txt".to_string(),
                "[12]     └── b.txt".to_string(),
            ]
        );
    }

    #[test]
    fn depth_truncates_directories_with_counts() {
        let root = tree(&[
            (1, "/srv/data/logs/a.log"),
            (2, "/srv/data/logs/b.log"),
            (3, "/srv/data/conf/app.toml"),
        ]);

        // depth 2: only directory summaries remain, so there is no id gutter.
        let lines = render_lines(&root, 2);
        assert_eq!(
            lines,
            vec![
                "└── /srv/data/".to_string(),
                "    ├── conf/ (1 file)".to_string(),
                "    └── logs/ (2 files)".to_string(),
            ]
        );

        // depth 0: full tree, files listed with their ids.
        let full = render_lines(&root, 0);
        assert!(full.iter().any(|line| line.contains("app.toml")));
        assert!(
            full.iter()
                .any(|line| line.contains("[1]") || line.contains("[2]") || line.contains("[3]"))
        );
    }

    // --- DB-backed integration tests ---

    use crate::db::sqlite::ScannedFile;
    use x25519_dalek::{PublicKey, StaticSecret};

    fn public_key() -> PublicKey {
        PublicKey::from(&StaticSecret::from([7u8; 32]))
    }

    fn scanned(path: &str, hash: &str) -> ScannedFile {
        ScannedFile {
            path: PathBuf::from(path),
            hash: hash.to_string(),
        }
    }

    #[test]
    fn view_entries_returns_ids_and_scopes_to_subtree() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let catalog = SqliteCatalog::initialize(&dir.path().join("t.db"))?;
        catalog.save_public_key(&public_key())?;

        let version = catalog.create_version()?;
        catalog.record_scan(
            public_key(),
            &crate::db::sqlite::SealedKeys::new(),
            version,
            &[
                scanned("/srv/a/x.txt", "h1"),
                scanned("/srv/a/y.txt", "h2"),
                scanned("/srv/b/z.txt", "h3"),
            ],
            true,
            None,
        )?;

        let all = catalog.view_entries(version, None)?;
        assert_eq!(all.len(), 3);
        assert!(all.iter().all(|entry| entry.id > 0));

        let scoped = catalog.view_entries(version, Some(Path::new("/srv/a")))?;
        assert_eq!(scoped.len(), 2);
        assert!(scoped.iter().all(|entry| entry.path.starts_with("/srv/a")));

        Ok(())
    }

    #[test]
    fn file_path_at_version_is_version_aware() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let catalog = SqliteCatalog::initialize(&dir.path().join("t.db"))?;
        catalog.save_public_key(&public_key())?;

        // v1: the file exists.
        let v1 = catalog.create_version()?;
        catalog.record_scan(
            public_key(),
            &crate::db::sqlite::SealedKeys::new(),
            v1,
            &[scanned("/srv/a/x.txt", "h1")],
            true,
            None,
        )?;

        let id = catalog
            .view_entries(v1, None)?
            .first()
            .ok_or_else(|| anyhow!("expected one entry"))?
            .id;

        // v2: the file is gone, so it is closed at v1.
        let v2 = catalog.create_version()?;
        catalog.record_scan(
            public_key(),
            &crate::db::sqlite::SealedKeys::new(),
            v2,
            &[],
            true,
            None,
        )?;

        assert_eq!(
            catalog.file_path_at_version(id, v1)?,
            Some(PathBuf::from("/srv/a/x.txt"))
        );
        assert_eq!(catalog.file_path_at_version(id, v2)?, None);

        Ok(())
    }
}
