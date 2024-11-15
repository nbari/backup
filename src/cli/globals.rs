use std::path::{Path, PathBuf};

// Define the global arguments
#[derive(Debug, Clone, Default)]
pub struct GlobalArgs {
    pub home: PathBuf,
}

impl GlobalArgs {
    #[must_use]
    pub fn new(home_dir: &Path) -> Self {
        Self {
            home: home_dir.to_path_buf(),
        }
    }
}
