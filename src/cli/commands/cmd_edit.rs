use clap::{builder::ValueParser, Arg, ArgAction, Command};
use std::{fs, path::PathBuf};

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
    Command::new("edit")
        .about("Edit backup configuration")
        .arg(
            Arg::new("name")
                .help("Name of the backup configuration")
                .required(true),
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
}
