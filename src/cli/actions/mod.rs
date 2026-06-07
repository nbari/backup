pub mod edit;
pub mod new;
pub mod restore;
pub mod run;
pub mod show;
pub mod view;

use std::path::PathBuf;

#[derive(Debug)]
pub enum Action {
    New {
        name: String,
        directory: Option<Vec<PathBuf>>,
        file: Option<Vec<PathBuf>>,
        config: PathBuf,
    },
    Show,
    Run {
        name: String,
        gitignore: bool,
        no_ignore: bool,
        no_compression: bool,
        no_encryption: bool,
        dry_run: bool,
    },
    View {
        name: String,
        depth: usize,
        version: Option<i64>,
        target: Option<String>,
    },
    Edit {
        name: String,
        add_directories: Vec<PathBuf>,
        add_files: Vec<PathBuf>,
        remove_directories: Vec<PathBuf>,
        remove_files: Vec<PathBuf>,
    },
    Restore {
        name: String,
        target: Option<String>,
        version: Option<i64>,
        into: Option<PathBuf>,
    },
}
