use clap::{Arg, ArgAction, Command, builder::NonEmptyStringValueParser};

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
            Arg::new("gitignore")
                .long("gitignore")
                .help("Also apply .gitignore rules while scanning")
                .conflicts_with("no-ignore")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("no-ignore")
                .long("no-ignore")
                .help("Do not apply .backupignore or .gitignore rules")
                .conflicts_with("gitignore")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("dry-run")
                .long("dry-run")
                .help("Do not create the backup, only show what would be done")
                .action(ArgAction::SetTrue),
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
        assert_eq!(matches.get_one::<bool>("gitignore").copied(), Some(false));
        assert_eq!(matches.get_one::<bool>("no-ignore").copied(), Some(false));
        assert_eq!(matches.get_one::<bool>("dry-run").copied(), Some(false));
        Ok(())
    }

    #[test]
    fn test_argumets_gitignore() -> Result<()> {
        let matches = matches_for(&["run", "test", "--gitignore"])?;
        assert_eq!(matches.get_one::<bool>("gitignore").copied(), Some(true));
        assert_eq!(matches.get_one::<bool>("no-ignore").copied(), Some(false));
        Ok(())
    }

    #[test]
    fn test_argumets_no_ignore() -> Result<()> {
        let matches = matches_for(&["run", "test", "--no-ignore"])?;
        assert_eq!(matches.get_one::<bool>("no-ignore").copied(), Some(true));
        Ok(())
    }

    #[test]
    fn test_argumets_dry_run() -> Result<()> {
        let matches = matches_for(&["run", "test", "--dry-run"])?;
        assert_eq!(matches.get_one::<bool>("dry-run").copied(), Some(true));
        Ok(())
    }

    #[test]
    fn test_argumets_gitignore_and_dry_run() -> Result<()> {
        let matches = matches_for(&["run", "test", "--gitignore", "--dry-run"])?;
        assert_eq!(matches.get_one::<bool>("gitignore").copied(), Some(true));
        assert_eq!(matches.get_one::<bool>("dry-run").copied(), Some(true));
        Ok(())
    }

    #[test]
    fn test_arguments_invalid() {
        assert!(command().try_get_matches_from(vec!["run"]).is_err());
    }

    #[test]
    fn test_arguments_invalid_name() {
        assert!(command().try_get_matches_from(vec!["run", ""]).is_err());
    }

    #[test]
    fn test_arguments_gitignore_conflicts_with_no_ignore() {
        let m = command().try_get_matches_from(vec!["run", "test", "--gitignore", "--no-ignore"]);
        assert!(m.is_err());
    }
}
