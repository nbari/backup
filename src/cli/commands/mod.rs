pub mod new;

use clap::{
    builder::styling::{AnsiColor, Effects, Styles},
    ColorChoice, Command,
};

use std::env;

pub fn new() -> Command {
    let styles = Styles::styled()
        .header(AnsiColor::Yellow.on_default() | Effects::BOLD)
        .usage(AnsiColor::Green.on_default() | Effects::BOLD)
        .literal(AnsiColor::Blue.on_default() | Effects::BOLD)
        .placeholder(AnsiColor::Green.on_default());

    Command::new("backup")
        .about("create encrypted backups")
        .arg_required_else_help(true)
        .version(env!("CARGO_PKG_VERSION"))
        .color(ColorChoice::Auto)
        .styles(styles)
        .subcommand(new::command())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new() {
        let command = new();

        assert_eq!(command.get_name(), "backup");
        assert_eq!(
            command.get_about().unwrap().to_string(),
            "create encrypted backups"
        );
        assert_eq!(
            command.get_version().unwrap().to_string(),
            env!("CARGO_PKG_VERSION")
        );
    }
}
