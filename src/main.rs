use clap::FromArgMatches;
use midenup::commands::Midenup;

fn main() -> anyhow::Result<()> {
    curl::init();

    let cli = <Midenup as clap::CommandFactory>::command();
    let matches = cli.get_matches();
    let cli = Midenup::from_arg_matches(&matches).map_err(|err| err.exit()).unwrap();

    let config = cli.config()?;

    cli.execute(&config)
}
