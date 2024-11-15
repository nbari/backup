pub mod cmd_new;
pub mod cmd_run;
pub mod cmd_show;

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
        Some("new") => cmd_new::dispatch(sub_m("new")?),
        Some("show") => cmd_show::dispatch(),
        Some("run") => cmd_run::dispatch(sub_m("run")?),

        _ => todo!(),
    }
}
