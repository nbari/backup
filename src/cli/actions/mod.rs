pub mod new;
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
}
