use crate::cli::commands::validators;
use clap::{Arg, ArgAction, Command, builder::NonEmptyStringValueParser};

pub fn command() -> Command {
    Command::new("edit")
        .about("Edit a backup configuration (add or remove directories and files)")
        .arg(
            Arg::new("name")
                .help("Name of the backup configuration")
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
        .arg(
            Arg::new("rm-dir")
                .action(ArgAction::Append)
                .long("rm-dir")
                .help("Remove a configured directory (path need not exist)")
                .value_parser(NonEmptyStringValueParser::new()),
        )
        .arg(
            Arg::new("rm-file")
                .action(ArgAction::Append)
                .long("rm-file")
                .help("Remove a configured file (path need not exist)")
                .value_parser(NonEmptyStringValueParser::new()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use std::path::PathBuf;

    fn matches_for(args: &[&str]) -> Result<clap::ArgMatches> {
        Ok(command().try_get_matches_from(args)?)
    }

    #[test]
    fn name_is_required() {
        assert!(command().try_get_matches_from(vec!["edit"]).is_err());
    }

    #[test]
    fn rm_flags_accept_nonexistent_paths() -> Result<()> {
        let matches = matches_for(&[
            "edit",
            "demo",
            "--rm-dir",
            "/gone/dir",
            "--rm-file",
            "/gone/file.txt",
        ])?;

        let rm_dirs: Vec<&String> = matches
            .get_many::<String>("rm-dir")
            .unwrap_or_default()
            .collect();
        let rm_files: Vec<&String> = matches
            .get_many::<String>("rm-file")
            .unwrap_or_default()
            .collect();

        assert_eq!(rm_dirs, vec!["/gone/dir"]);
        assert_eq!(rm_files, vec!["/gone/file.txt"]);
        Ok(())
    }

    #[test]
    fn add_flags_append() -> Result<()> {
        // "." exists, so the directory validator passes.
        let matches = matches_for(&["edit", "demo", "-d", ".", "-d", "."])?;
        let count = matches
            .get_many::<PathBuf>("directory")
            .unwrap_or_default()
            .count();
        assert_eq!(count, 2);
        Ok(())
    }
}
