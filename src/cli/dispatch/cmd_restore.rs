use crate::cli::actions::Action;
use anyhow::Result;
use clap::ArgMatches;
use std::path::PathBuf;

pub fn dispatch(matches: &ArgMatches) -> Result<Action> {
    Ok(Action::Restore {
        name: matches
            .get_one("name")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Name required"))?,
        target: matches.get_one::<String>("target").cloned(),
        version: matches.get_one("version").copied(),
        into: matches.get_one::<String>("into").map(PathBuf::from),
    })
}
