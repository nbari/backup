use clap::{App, Arg};

fn main() {
    let matches = App::new("backup")
        .version(env!("CARGO_PKG_VERSION"))
        .arg(
            Arg::with_name("quiet")
                .required(false)
                .takes_value(false)
                .long("quiet")
                .short("q")
                .help("suppress output messages"),
        )
        .get_matches();
}
