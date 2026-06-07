use clap::{Arg, Command, builder::NonEmptyStringValueParser};

pub fn command() -> Command {
    Command::new("run")
        .about("Run backup")
        .arg(
            Arg::new("name")
                .help("Name of the backup. Use \"show\" to see current configurations")
                .value_parser(NonEmptyStringValueParser::new())
                .required(true),
        )
        .arg(
            Arg::new("no-gitignore")
                .long("no-gitignore")
                .help("Ignore parsing .gitignore files in the backup directory")
                .num_args(0),
        )
        .arg(
            Arg::new("no-compression")
                .long("no-compression")
                .help("Do not compress the backup")
                .num_args(0),
        )
        .arg(
            Arg::new("no-encryption")
                .long("no-encryption")
                .help("Do not encrypt the backup")
                .num_args(0),
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .help("Do not create the backup, only show what would be done")
                .num_args(0),
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
    fn test_argumets_default() -> Result<()> {
        let matches = matches_for(&["run", "test"])?;
        assert_eq!(
            matches.get_one::<String>("name").map(String::as_str),
            Some("test")
        );
        assert_eq!(
            matches.get_one::<bool>("no-gitignore").copied(),
            Some(false)
        );
        assert_eq!(
            matches.get_one::<bool>("no-compression").copied(),
            Some(false)
        );
        assert_eq!(
            matches.get_one::<bool>("no-encryption").copied(),
            Some(false)
        );
        assert_eq!(matches.get_one::<bool>("dry-run").copied(), Some(false));
        Ok(())
    }

    #[test]
    fn test_argumets_no_gitignore() -> Result<()> {
        let matches = matches_for(&["run", "test", "--no-gitignore"])?;
        assert_eq!(
            matches.get_one::<String>("name").map(String::as_str),
            Some("test")
        );
        assert_eq!(matches.get_one::<bool>("no-gitignore").copied(), Some(true));
        assert_eq!(
            matches.get_one::<bool>("no-compression").copied(),
            Some(false)
        );
        assert_eq!(
            matches.get_one::<bool>("no-encryption").copied(),
            Some(false)
        );
        assert_eq!(matches.get_one::<bool>("dry-run").copied(), Some(false));
        Ok(())
    }

    #[test]
    fn test_argumets_no_compression() -> Result<()> {
        let matches = matches_for(&["run", "test", "--no-compression"])?;
        assert_eq!(
            matches.get_one::<String>("name").map(String::as_str),
            Some("test")
        );
        assert_eq!(
            matches.get_one::<bool>("no-gitignore").copied(),
            Some(false)
        );
        assert_eq!(
            matches.get_one::<bool>("no-compression").copied(),
            Some(true)
        );
        assert_eq!(
            matches.get_one::<bool>("no-encryption").copied(),
            Some(false)
        );
        assert_eq!(matches.get_one::<bool>("dry-run").copied(), Some(false));
        Ok(())
    }

    #[test]
    fn test_argumets_no_encryption() -> Result<()> {
        let matches = matches_for(&["run", "test", "--no-encryption"])?;
        assert_eq!(
            matches.get_one::<String>("name").map(String::as_str),
            Some("test")
        );
        assert_eq!(
            matches.get_one::<bool>("no-gitignore").copied(),
            Some(false)
        );
        assert_eq!(
            matches.get_one::<bool>("no-compression").copied(),
            Some(false)
        );
        assert_eq!(
            matches.get_one::<bool>("no-encryption").copied(),
            Some(true)
        );
        assert_eq!(matches.get_one::<bool>("dry-run").copied(), Some(false));
        Ok(())
    }

    #[test]
    fn test_argumets_dry_run() -> Result<()> {
        let matches = matches_for(&["run", "test", "--dry-run"])?;
        assert_eq!(
            matches.get_one::<String>("name").map(String::as_str),
            Some("test")
        );
        assert_eq!(
            matches.get_one::<bool>("no-gitignore").copied(),
            Some(false)
        );
        assert_eq!(
            matches.get_one::<bool>("no-compression").copied(),
            Some(false)
        );
        assert_eq!(
            matches.get_one::<bool>("no-encryption").copied(),
            Some(false)
        );
        assert_eq!(matches.get_one::<bool>("dry-run").copied(), Some(true));
        Ok(())
    }

    #[test]
    fn test_argumets_all() -> Result<()> {
        let matches = matches_for(&[
            "run",
            "test",
            "--no-gitignore",
            "--no-compression",
            "--no-encryption",
            "--dry-run",
        ])?;
        assert_eq!(
            matches.get_one::<String>("name").map(String::as_str),
            Some("test")
        );
        assert_eq!(matches.get_one::<bool>("no-gitignore").copied(), Some(true));
        assert_eq!(
            matches.get_one::<bool>("no-compression").copied(),
            Some(true)
        );
        assert_eq!(
            matches.get_one::<bool>("no-encryption").copied(),
            Some(true)
        );
        assert_eq!(matches.get_one::<bool>("dry-run").copied(), Some(true));
        Ok(())
    }

    #[test]
    fn test_argumets_no_gitignore_and_no_compression() -> Result<()> {
        let matches = matches_for(&["run", "test", "--no-gitignore", "--no-compression"])?;
        assert_eq!(
            matches.get_one::<String>("name").map(String::as_str),
            Some("test")
        );
        assert_eq!(matches.get_one::<bool>("no-gitignore").copied(), Some(true));
        assert_eq!(
            matches.get_one::<bool>("no-compression").copied(),
            Some(true)
        );
        assert_eq!(
            matches.get_one::<bool>("no-encryption").copied(),
            Some(false)
        );
        assert_eq!(matches.get_one::<bool>("dry-run").copied(), Some(false));
        Ok(())
    }

    #[test]
    fn test_arguments_no_gitignore_and_no_dry_run() -> Result<()> {
        let matches = matches_for(&["run", "test", "--no-gitignore", "--dry-run"])?;
        assert_eq!(
            matches.get_one::<String>("name").map(String::as_str),
            Some("test")
        );
        assert_eq!(matches.get_one::<bool>("no-gitignore").copied(), Some(true));
        assert_eq!(
            matches.get_one::<bool>("no-compression").copied(),
            Some(false)
        );
        assert_eq!(
            matches.get_one::<bool>("no-encryption").copied(),
            Some(false)
        );
        assert_eq!(matches.get_one::<bool>("dry-run").copied(), Some(true));
        Ok(())
    }

    #[test]
    fn test_arguments_invalid() {
        let m = command().try_get_matches_from(vec!["run"]);
        assert!(m.is_err());
    }

    #[test]
    fn test_arguments_invalid_name() {
        let m = command().try_get_matches_from(vec!["run", ""]);
        assert!(m.is_err());
    }
}
