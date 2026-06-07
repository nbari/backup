use crate::cli::commands::validators;
use clap::{Arg, ArgAction, Command};

pub fn command() -> Command {
    Command::new("new")
        .about("Create a new backup configuration")
        .arg(
            Arg::new("name")
                .help("Name of the backup")
                .required(true)
                .value_parser(validators::is_alphanumeric()),
        )
        .arg(
            Arg::new("directory")
                .action(ArgAction::Append)
                .short('d')
                .long("dir")
                .help("Add a directory to the backup")
                .value_parser(validators::is_dir()),
        )
        .arg(
            Arg::new("file")
                .action(ArgAction::Append)
                .short('f')
                .long("file")
                .help("Add a file to the backup")
                .value_parser(validators::is_file()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validator_is_alphanumeric() {
        let test_cases = vec![
            ("backup", "test", true),
            ("backup", "test123", true),
            ("backup", "12345", true),
            ("backup", "test!", false),
            ("backup", "test 123", false),
            ("backup", "test@test", false),
            ("backup", "n~", false),
            ("backup", "n", true),
            ("backup", "_", false),
            ("backup", "_A", true),
            ("backup", "Z_", true),
        ];

        for (c, name, should_succeed) in test_cases {
            let cmd = command();

            let m = cmd.try_get_matches_from(vec![c, name]);

            if should_succeed {
                assert!(m.is_ok());
            } else {
                assert!(m.is_err());
            }
        }
    }
}
