pub mod new;

use crate::cli::actions::Action;
use anyhow::{Context, Result};

pub fn handler(matches: &clap::ArgMatches) -> Result<Action> {
    // Closure to return subcommand matches
    let sub_m = |subcommand| -> Result<&clap::ArgMatches> {
        matches
            .subcommand_matches(subcommand)
            .context("arguments not found")
    };

    match matches.subcommand_name() {
        Some("new") => new::dispatch(sub_m("new")?),

        _ => todo!(),
    }
}
