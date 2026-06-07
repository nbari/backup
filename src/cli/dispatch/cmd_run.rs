use crate::cli::actions::Action;
use anyhow::Result;
use clap::ArgMatches;

pub fn dispatch(matches: &ArgMatches) -> Result<Action> {
    Ok(Action::Run {
        name: matches
            .get_one("name")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Name required"))?,
        gitignore: matches.get_one("gitignore").copied().unwrap_or(false),
        no_ignore: matches.get_one("no-ignore").copied().unwrap_or(false),
        no_compression: matches.get_one("no-compression").copied().unwrap_or(false),
        no_encryption: matches.get_one("no-encryption").copied().unwrap_or(false),
        dry_run: matches.get_one("dry-run").copied().unwrap_or(false),
    })
}
