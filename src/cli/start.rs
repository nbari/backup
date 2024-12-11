use crate::cli::{actions::Action, commands, dispatch::handler, globals::GlobalArgs, telemetry};
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
pub fn start() -> Result<(Action, GlobalArgs)> {
    telemetry::init(None)?;

    let config_path = get_config_path()?;

    let global_args = GlobalArgs::new(&config_path);

    let matches = commands::new(config_path).get_matches();

    let action = handler(&matches)?;

    Ok((action, global_args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_config_path() {
        let config_path = get_config_path().unwrap();
        assert_eq!(config_path, dirs::home_dir().unwrap().join(".backup"));
    }
}
