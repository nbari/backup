use crate::cli::{actions::Action, commands, dispatch::handler, globals::GlobalArgs, telemetry};
use anyhow::{Context, Result};
use std::{fs, path::PathBuf};

pub fn get_config_path() -> Result<PathBuf> {
    let home_dir = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));

    let config_path = home_dir.join(".backup");
    fs::create_dir_all(&config_path).context(format!(
        "failed to create config directory {}",
        config_path.display()
    ))?;

    Ok(config_path)
}

/// Start the CLI
/// # Errors
/// Returns an error if configuration, telemetry setup, or command dispatch fails.
pub fn start() -> Result<(Action, GlobalArgs)> {
    let config_path = get_config_path()?;

    let matches = commands::new(config_path.clone()).get_matches();
    let quiet = matches.get_flag("quiet");

    let global_args = GlobalArgs::new(&config_path, quiet);

    telemetry::init()?;
    let action = handler(&matches)?;

    Ok((action, global_args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_config_path() -> Result<()> {
        let config_path = get_config_path()?;
        let home_dir = dirs::home_dir().context("home directory not found")?;
        assert_eq!(config_path, home_dir.join(".backup"));
        Ok(())
    }
}
