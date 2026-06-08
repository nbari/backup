use crate::cli::commands::validators;
use clap::{Arg, ArgAction, Command, builder::NonEmptyStringValueParser};

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
                .help("Add a directory to the backup (repeatable)")
                .value_parser(validators::is_dir()),
        )
        .arg(
            Arg::new("file")
                .action(ArgAction::Append)
                .short('f')
                .long("file")
                .help("Add a file to the backup (repeatable)")
                .value_parser(validators::is_file()),
        )
        .arg(
            Arg::new("to")
                .action(ArgAction::Append)
                .short('t')
                .long("to")
                .help("Add a destination to store the backup (path or S3 target); repeatable")
                .value_parser(NonEmptyStringValueParser::new()),
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

    #[test]
    fn to_is_repeatable_and_not_existence_checked() -> anyhow::Result<()> {
        // Destinations need not exist (e.g. an S3 URL or a yet-to-be-created path).
        let matches = command().try_get_matches_from(vec![
            "new",
            "demo",
            "-t",
            "/mnt/a",
            "--to",
            "s3://bucket/x",
        ])?;
        let dests: Vec<&String> = matches
            .get_many::<String>("to")
            .unwrap_or_default()
            .collect();
        assert_eq!(dests, vec!["/mnt/a", "s3://bucket/x"]);
        Ok(())
    }
}
