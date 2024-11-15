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
        exclude: Option<Vec<String>>,
        config: PathBuf,
    },
    Show,
    Run {
        name: String,
    },
}
