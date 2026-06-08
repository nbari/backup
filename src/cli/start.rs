use crate::cli::{actions::Action, commands, dispatch::handler, globals::GlobalArgs, telemetry};
use anyhow::{Context, Result};
use std::{fs, path::PathBuf};

/// Default configuration directory: `~/.backup` (or `/tmp/.backup` if the home
/// directory cannot be determined).
///
/// This only *computes* the path — it does not create it. The directory is
/// created later by [`resolve_config_dir`], once the effective path is known
/// (the user may override it with `-c/--config`), so we never create `~/.backup`
/// when a different config dir was requested.
#[must_use]
pub fn default_config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".backup")
}

/// Resolve the effective config directory from parsed args and ensure it exists.
///
/// `-c/--config` is a **global** flag (it works before or after the subcommand)
/// whose default is [`default_config_dir`]. Every command reads the config
/// directory from here via [`GlobalArgs::home`], so honoring the flag in one
/// place keeps `new`/`run`/`view`/`edit`/`verify` consistent.
fn resolve_config_dir(matches: &clap::ArgMatches) -> Result<PathBuf> {
    let config_dir = matches
        .get_one::<String>("config")
        .map_or_else(default_config_dir, PathBuf::from);

    fs::create_dir_all(&config_dir)
        .with_context(|| format!("failed to create config directory {}", config_dir.display()))?;

    Ok(config_dir)
}

/// Start the CLI
/// # Errors
/// Returns an error if configuration, telemetry setup, or command dispatch fails.
pub fn start() -> Result<(Action, GlobalArgs)> {
    // The default is only used to populate clap's `--config` default value; the
    // *effective* directory (after `-c/--config`) is resolved from the matches.
    let matches = commands::new(default_config_dir()).get_matches();
    let quiet = matches.get_flag("quiet");

    let config_dir = resolve_config_dir(&matches)?;
    let global_args = GlobalArgs::new(&config_dir, quiet);

    telemetry::init()?;
    let action = handler(&matches)?;

    Ok((action, global_args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_dir_is_dot_backup_under_home() -> Result<()> {
        let home_dir = dirs::home_dir().context("home directory not found")?;
        assert_eq!(default_config_dir(), home_dir.join(".backup"));
        Ok(())
    }

    /// Regression: `-c/--config` must be honored for *every* command, not just
    /// `new`. Previously `globals.home` was hardcoded to the default, so
    /// `run`/`view`/`edit`/`verify` silently ignored the flag.
    #[test]
    fn config_flag_overrides_default_dir_and_is_created() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let custom = tmp.path().join("nested").join("cfg");
        let custom_str = custom.to_str().context("non-utf8 temp path")?;

        // `-c` placed before the subcommand (it is a global flag).
        let matches = commands::new(default_config_dir())
            .try_get_matches_from(vec!["backup", "-c", custom_str, "show"])?;

        let resolved = resolve_config_dir(&matches)?;
        assert_eq!(resolved, custom);
        assert!(custom.is_dir(), "resolve_config_dir should create the dir");
        Ok(())
    }

    #[test]
    fn config_flag_works_after_subcommand() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let custom = tmp.path().join("cfg");
        let custom_str = custom.to_str().context("non-utf8 temp path")?;

        // Global flags also parse when given after the subcommand name.
        let matches = commands::new(default_config_dir())
            .try_get_matches_from(vec!["backup", "show", "-c", custom_str])?;

        assert_eq!(resolve_config_dir(&matches)?, custom);
        Ok(())
    }

    #[test]
    fn config_defaults_when_flag_absent() -> Result<()> {
        let matches =
            commands::new(default_config_dir()).try_get_matches_from(vec!["backup", "show"])?;

        // The parsed value falls back to the default (don't call resolve_config_dir
        // here — it would create ~/.backup as a side effect).
        let parsed = matches.get_one::<String>("config").map(PathBuf::from);
        assert_eq!(parsed, Some(default_config_dir()));
        Ok(())
    }
}
