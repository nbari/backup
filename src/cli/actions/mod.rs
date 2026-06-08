pub mod edit;
pub mod new;
pub mod restore;
pub mod run;
pub mod show;
pub mod verify;
pub mod view;

use std::path::PathBuf;

#[derive(Debug)]
pub enum Action {
    New {
        name: String,
        directory: Option<Vec<PathBuf>>,
        file: Option<Vec<PathBuf>>,
        destination: Vec<String>,
        config: PathBuf,
    },
    Show,
    Run {
        name: String,
        gitignore: bool,
        no_ignore: bool,
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
        add_destinations: Vec<String>,
        remove_directories: Vec<PathBuf>,
        remove_files: Vec<PathBuf>,
        remove_destinations: Vec<String>,
    },
    Restore {
        name: String,
        target: Option<String>,
        version: Option<i64>,
        into: Option<PathBuf>,
    },
    Verify {
        name: String,
        repair: bool,
    },
}
