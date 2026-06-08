use crate::cli::actions::Action;
use anyhow::Result;
use clap::ArgMatches;

pub fn dispatch(matches: &ArgMatches) -> Result<Action> {
    Ok(Action::Verify {
        name: matches
            .get_one("name")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Name required"))?,
        repair: matches.get_flag("repair"),
    })
}
