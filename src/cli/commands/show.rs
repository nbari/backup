use clap::Command;

pub fn command() -> Command {
    Command::new("show").about("Show available backup configurations")
}
