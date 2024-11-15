use clap::{Arg, Command};

pub fn command() -> Command {
    Command::new("run").about("Run backup").arg(
        Arg::new("name")
            .help("Name of the backup. Use \"show\" to see current configurations")
            .required(true),
    )
}
