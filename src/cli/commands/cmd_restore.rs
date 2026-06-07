use crate::cli::commands::validators;
use clap::{Arg, Command, builder::NonEmptyStringValueParser, value_parser};

pub fn command() -> Command {
    Command::new("restore")
        .about("Restore files from a backup (not implemented yet)")
        .arg(
            Arg::new("name")
                .help("Name of the backup. Use \"show\" to see current configurations")
                .required(true)
                .value_parser(validators::is_alphanumeric()),
        )
        .arg(
            Arg::new("target")
                .help("A file id (e.g. 7 or #7) or an absolute path; omit to restore everything")
                .value_parser(NonEmptyStringValueParser::new()),
        )
        .arg(
            Arg::new("version")
                .long("version")
                .help("Snapshot version to restore from (defaults to the latest)")
                .value_parser(value_parser!(i64)),
        )
        .arg(
            Arg::new("into")
                .short('C')
                .long("into")
                .help("Directory to restore into (defaults to the original paths)")
                .value_parser(NonEmptyStringValueParser::new()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    #[test]
    fn name_is_required() {
        assert!(command().try_get_matches_from(vec!["restore"]).is_err());
    }

    #[test]
    fn parses_target_version_and_into() -> Result<()> {
        let matches = command().try_get_matches_from(vec![
            "restore",
            "demo",
            "7",
            "--version",
            "3",
            "--into",
            "/tmp/out",
        ])?;
        assert_eq!(
            matches.get_one::<String>("target").map(String::as_str),
            Some("7")
        );
        assert_eq!(matches.get_one::<i64>("version").copied(), Some(3));
        assert_eq!(
            matches.get_one::<String>("into").map(String::as_str),
            Some("/tmp/out")
        );
        Ok(())
    }
}
