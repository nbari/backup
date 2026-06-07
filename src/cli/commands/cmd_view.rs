use clap::{Arg, Command, builder::NonEmptyStringValueParser, value_parser};

pub fn command() -> Command {
    Command::new("view")
        .visible_alias("browse")
        .about("Browse the backed-up file tree of a snapshot")
        .arg(
            Arg::new("name")
                .help("Name of the backup. Use \"show\" to see current configurations")
                .value_parser(NonEmptyStringValueParser::new())
                .required(true),
        )
        .arg(
            Arg::new("depth")
                .short('d')
                .long("depth")
                .help("Levels of the tree to show (0 = full tree)")
                .value_parser(value_parser!(usize))
                .default_value("2"),
        )
        .arg(
            Arg::new("version")
                .long("version")
                .help("Snapshot version to view (defaults to the latest)")
                .value_parser(value_parser!(i64)),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    fn matches_for(args: &[&str]) -> Result<clap::ArgMatches> {
        Ok(command().try_get_matches_from(args)?)
    }

    #[test]
    fn defaults() -> Result<()> {
        let matches = matches_for(&["view", "test"])?;
        assert_eq!(
            matches.get_one::<String>("name").map(String::as_str),
            Some("test")
        );
        assert_eq!(matches.get_one::<usize>("depth").copied(), Some(2));
        assert_eq!(matches.get_one::<i64>("version").copied(), None);
        Ok(())
    }

    #[test]
    fn depth_and_version_parse() -> Result<()> {
        let matches = matches_for(&["view", "test", "--depth", "0", "--version", "3"])?;
        assert_eq!(matches.get_one::<usize>("depth").copied(), Some(0));
        assert_eq!(matches.get_one::<i64>("version").copied(), Some(3));
        Ok(())
    }

    #[test]
    fn depth_short_flag_parses() -> Result<()> {
        let matches = matches_for(&["view", "test", "-d", "3"])?;
        assert_eq!(matches.get_one::<usize>("depth").copied(), Some(3));
        Ok(())
    }

    #[test]
    fn browse_alias_resolves() -> Result<()> {
        let matches = matches_for(&["browse", "test"])?;
        assert_eq!(
            matches.get_one::<String>("name").map(String::as_str),
            Some("test")
        );
        Ok(())
    }

    #[test]
    fn name_is_required() {
        assert!(command().try_get_matches_from(vec!["view"]).is_err());
    }

    #[test]
    fn empty_name_is_rejected() {
        assert!(command().try_get_matches_from(vec!["view", ""]).is_err());
    }
}
