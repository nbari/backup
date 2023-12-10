use crate::cli::{actions::Action, commands, dispatch::handler};
use anyhow::{Context, Result};
use std::{fs, path::PathBuf};

pub fn get_config_path() -> Result<PathBuf> {
    let home_dir = dirs::home_dir().map_or_else(|| PathBuf::from("/tmp"), |path| path);

    let config_path = home_dir.join(".backup");
    fs::create_dir_all(&config_path).context(format!(
        "failed to create config directory {}",
        &config_path.display()
    ))?;

    Ok(config_path)
}

/// Start the CLI
pub fn start() -> Result<Action> {
    let config_path = get_config_path()?;

    let matches = commands::new(config_path).get_matches();

    let action = handler(&matches)?;
    Ok(action)
}
