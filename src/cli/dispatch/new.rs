use crate::cli::actions::Action;
use anyhow::Result;
use clap::ArgMatches;
use std::path::PathBuf;

pub fn dispatch(matches: &ArgMatches) -> Result<Action> {
    Ok(Action::New {
        name: matches
            .get_one("name")
            .map(|s: &String| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("Name required"))?,

        config: matches
            .get_one::<String>("config")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("Config required"))?,

        directory: Some(
            matches
                .get_many::<PathBuf>("directory")
                .unwrap_or_default()
                .map(|v| v.to_path_buf())
                .collect::<Vec<_>>(),
        ),

        file: Some(
            matches
                .get_many::<PathBuf>("file")
                .unwrap_or_default()
                .map(|v| v.to_path_buf())
                .collect::<Vec<_>>(),
        ),

        exclude: Some(
            matches
                .get_many::<String>("exclude")
                .unwrap_or_default()
                .map(String::from)
                .collect::<Vec<_>>(),
        ),
    })
}
