use clap::Parser;
use midenup::manifest::Manifest;

#[derive(Parser)]
#[command(name = "update-manifest", about = "Parse and update midenup's manifest.")]
/// Options parsed by
struct CliArguments {
    /// URI of the manifest to parse (supports file:// and https://)
    uri: String,
}

struct Options {
    /// URI of the manifest to parse (supports file:// and https://)
    uri: String,
}

impl Options {
    fn from(cli: CliArguments) {
        Options {
            //
            uri: cli.uri,
        }
    }
}

fn update_channel(chanel: &mut Channel) {
    todo!();
}

fn main() -> anyhow::Result<()> {
    let cli = CliArguments::parse();

    let manifest = Manifest::load_from(&cli.uri)
        .map_err(|e| anyhow::anyhow!("Failed to load manifest from `{}`: {e}", cli.uri))?;

    println!("Manifest loaded successfully from `{}`", cli.uri);

    for channel in manifest.get_channels() {
        println!("  - Channel: {}", channel.name);
    }

    Ok(())
}
