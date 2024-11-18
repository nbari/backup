pub mod cmd_new;
pub mod cmd_run;
pub mod cmd_show;

use crate::cli::actions::Action;
use anyhow::{Context, Result};

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
        Some("show") => cmd_show::dispatch(),
        Some("run") => cmd_run::dispatch(get_subcommand_matches(matches, "run")?),

        _ => todo!(),
    }
}
