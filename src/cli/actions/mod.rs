pub mod new;
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
    },
}
