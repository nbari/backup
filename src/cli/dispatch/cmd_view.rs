use crate::cli::actions::Action;
use anyhow::Result;
use clap::ArgMatches;

pub fn dispatch(matches: &ArgMatches) -> Result<Action> {
    Ok(Action::View {
        name: matches
            .get_one("name")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Name required"))?,
        depth: matches.get_one("depth").copied().unwrap_or(2),
        version: matches.get_one("version").copied(),
        target: matches.get_one("target").cloned(),
    })
}
