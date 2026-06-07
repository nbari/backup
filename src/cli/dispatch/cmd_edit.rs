use crate::cli::actions::Action;
use anyhow::Result;
use clap::ArgMatches;
use std::path::PathBuf;

pub fn dispatch(matches: &ArgMatches) -> Result<Action> {
    Ok(Action::Edit {
        name: matches
            .get_one("name")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Name required"))?,

        add_directories: matches
            .get_many::<PathBuf>("directory")
            .unwrap_or_default()
            .cloned()
            .collect(),

        add_files: matches
            .get_many::<PathBuf>("file")
            .unwrap_or_default()
            .cloned()
            .collect(),

        remove_directories: matches
            .get_many::<String>("rm-dir")
            .unwrap_or_default()
            .map(PathBuf::from)
            .collect(),

        remove_files: matches
            .get_many::<String>("rm-file")
            .unwrap_or_default()
            .map(PathBuf::from)
            .collect(),
    })
}
