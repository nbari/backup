pub mod new;
pub mod run;
pub mod show;

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
        no_gitignore: bool,
        no_compression: bool,
        no_encryption: bool,
        dry_run: bool,
    },
}
