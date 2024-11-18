use crate::cli::actions::Action;
use anyhow::Result;
use clap::ArgMatches;

pub fn dispatch(matches: &ArgMatches) -> Result<Action> {
    Ok(Action::Run {
        name: matches
            .get_one("name")
            .map(|s: &String| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Name required"))?,
        no_gitignore: matches.get_one("no-gitignore").copied().unwrap_or(false),
        no_compression: matches.get_one("no-compression").copied().unwrap_or(false),
        no_encryption: matches.get_one("no-encryption").copied().unwrap_or(false),
        dry_run: matches.get_one("dry-run").copied().unwrap_or(false),
    })
}
