use clap::{builder::ValueParser, Arg, ArgAction, Command};
use std::{fs, path::PathBuf};

// alpahnumeric validator
pub fn validator_is_alphanumeric() -> ValueParser {
    ValueParser::from(move |s: &str| -> std::result::Result<String, String> {
        if s.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Ok(s.to_string());
        }

        Err("Only [a-Z0-9] alphanumeric characters are allowed".to_string())
    })
}

pub fn validator_is_file() -> ValueParser {
    ValueParser::from(move |s: &str| -> std::result::Result<PathBuf, String> {
        if let Ok(metadata) = fs::metadata(s) {
            if metadata.is_file() {
                return Ok(PathBuf::from(s));
            }
        }

        Err(format!("Invalid file path or file does not exist: '{s}'"))
    })
}

pub fn validator_is_dir() -> ValueParser {
    ValueParser::from(move |s: &str| -> std::result::Result<PathBuf, String> {
        if let Ok(metadata) = fs::metadata(s) {
            if metadata.is_dir() {
                return Ok(PathBuf::from(s));
            }
        }

        Err(format!(
            "Invalid directory path or directory does not exist: '{s}'"
        ))
    })
}

pub fn command() -> Command {
    Command::new("new")
        .about("Create a new backup configuration")
        .arg(
            Arg::new("name")
                .help("Name of the backup")
                .required(true)
                .value_parser(validator_is_alphanumeric()),
        )
        .arg(
            Arg::new("directory")
                .action(ArgAction::Append)
                .short('d')
                .long("dir")
                .help("Add a directory to the backup")
                .value_parser(validator_is_dir()),
        )
        .arg(
            Arg::new("file")
                .action(ArgAction::Append)
                .short('f')
                .long("file")
                .help("Add a file to the backup")
                .value_parser(validator_is_file()),
        )
        .arg(
            Arg::new("exclude")
                .action(ArgAction::Append)
                .short('e')
                .long("exclude")
                .help("Exclude a file or directory from the backup"),
        )
}
