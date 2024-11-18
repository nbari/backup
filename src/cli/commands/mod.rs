pub mod cmd_edit;
pub mod cmd_new;
pub mod cmd_run;
pub mod cmd_show;

use clap::{
    builder::styling::{AnsiColor, Effects, Styles},
    Arg, ColorChoice, Command,
};
use std::{env, path::PathBuf};

pub fn new(config_path: PathBuf) -> Command {
    let styles = Styles::styled()
        .header(AnsiColor::Yellow.on_default() | Effects::BOLD)
        .usage(AnsiColor::Green.on_default() | Effects::BOLD)
        .literal(AnsiColor::Blue.on_default() | Effects::BOLD)
        .placeholder(AnsiColor::Green.on_default());

    Command::new("backup")
        .about("Create compressed and encrypted backups")
        .after_help("Create .gitignore file in the backup directory to exclude files")
        .arg_required_else_help(true)
        .version(env!("CARGO_PKG_VERSION"))
        .color(ColorChoice::Auto)
        .styles(styles)
        .arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .help("Path to the configuration files")
                .default_value(config_path.into_os_string())
                .global(true),
        )
        .subcommand(cmd_edit::command())
        .subcommand(cmd_new::command())
        .subcommand(cmd_run::command())
        .subcommand(cmd_show::command())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_new() {
        let command = new(PathBuf::from("."));

        assert_eq!(command.get_name(), "backup");
        assert_eq!(
            command.get_about().unwrap().to_string(),
            "Create compressed and encrypted backups"
        );
        assert_eq!(
            command.get_version().unwrap().to_string(),
            env!("CARGO_PKG_VERSION")
        );
    }
}
