pub mod cmd_edit;
pub mod cmd_new;
pub mod cmd_restore;
pub mod cmd_run;
pub mod cmd_show;
pub mod cmd_verify;
pub mod cmd_view;

use crate::cli::actions::Action;
use anyhow::{Context, Result, anyhow};

/// Helper function to get subcommand matches
pub fn get_subcommand_matches<'a>(
    matches: &'a clap::ArgMatches,
    subcommand: &str,
) -> Result<&'a clap::ArgMatches> {
    matches
        .subcommand_matches(subcommand)
        .context("arguments not found")
}

pub fn handler(matches: &clap::ArgMatches) -> Result<Action> {
    match matches.subcommand_name() {
        Some("new") => cmd_new::dispatch(get_subcommand_matches(matches, "new")?),
        Some("show") => Ok(cmd_show::dispatch()),
        Some("run") => cmd_run::dispatch(get_subcommand_matches(matches, "run")?),
        Some("view") => cmd_view::dispatch(get_subcommand_matches(matches, "view")?),
        Some("edit") => cmd_edit::dispatch(get_subcommand_matches(matches, "edit")?),
        Some("restore") => cmd_restore::dispatch(get_subcommand_matches(matches, "restore")?),
        Some("verify") => cmd_verify::dispatch(get_subcommand_matches(matches, "verify")?),

        _ => Err(anyhow!("Unsupported command")),
    }
}
