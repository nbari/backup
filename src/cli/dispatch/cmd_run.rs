use crate::cli::actions::Action;
use anyhow::Result;
use clap::ArgMatches;

pub fn dispatch(matches: &ArgMatches) -> Result<Action> {
    Ok(Action::Run {
        name: matches
            .get_one("name")
            .map(|s: &String| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Name required"))?,
    })
}
