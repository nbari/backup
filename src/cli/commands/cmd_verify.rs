use crate::cli::commands::validators;
use clap::{Arg, ArgAction, Command};

pub fn command() -> Command {
    Command::new("verify")
        .about("Check that every stored blob still exists in each destination")
        .long_about(
            "Re-check each destination against the catalog: are all content blobs \
             still present? `run` trusts the catalog when deciding what to upload, so \
             blobs deleted directly from a destination would otherwise go unnoticed.\n\n\
             With --repair, missing blobs are restored: copied from a healthy \
             destination when one still has the blob, or re-sealed from the source \
             file when it is gone everywhere. Re-sealing reads the original files and \
             may prompt for the recovery mnemonic.",
        )
        .arg(
            Arg::new("name")
                .help("Name of the backup. Use \"show\" to see current configurations")
                .required(true)
                .value_parser(validators::is_alphanumeric()),
        )
        .arg(
            Arg::new("repair")
                .long("repair")
                .help("Restore missing blobs (copy from a healthy destination, else re-seal from source)")
                .action(ArgAction::SetTrue),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_is_required() {
        assert!(command().try_get_matches_from(vec!["verify"]).is_err());
    }

    #[test]
    fn repair_defaults_to_false() -> anyhow::Result<()> {
        let matches = command().try_get_matches_from(vec!["verify", "demo"])?;
        assert!(!matches.get_flag("repair"));
        Ok(())
    }

    #[test]
    fn parses_repair_flag() -> anyhow::Result<()> {
        let matches = command().try_get_matches_from(vec!["verify", "demo", "--repair"])?;
        assert!(matches.get_flag("repair"));
        Ok(())
    }
}
