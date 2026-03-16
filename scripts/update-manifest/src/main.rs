use clap::Parser;
use midenup::channel::semver;
use midenup::channel::Channel;
use midenup::manifest::Manifest;
use midenup::version::Authority;

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
    fn from(cli: CliArguments) -> Options {
        Options {
            //
            uri: cli.uri,
        }
    }
}

fn get_vm_version(channel: &Channel) -> Option<&semver::Version> {
    let vm = channel.get_component("vm")?;
    match &vm.version {
        Authority::Cargo { version, .. } => Some(version),
        _ => None,
    }
}

fn update_channel(channel: &Channel, options: &Options) -> Channel {
    let vm_version = get_vm_version(channel).expect("Could find VM channel");
    println!("    VM version: {:?}", vm_version);
    // todo!();
    channel.clone()
}

fn main() -> anyhow::Result<()> {
    let cli = CliArguments::parse();

    let manifest = Manifest::load_from(&cli.uri)
        .map_err(|e| anyhow::anyhow!("Failed to load manifest from `{}`: {e}", cli.uri))?;
    println!("Manifest loaded successfully from `{}`", cli.uri);

    let options = Options::from(cli);

    let mut updated_channels = Vec::new();
    for mut channel in manifest.get_channels() {
        println!("  - Channel: {}", channel.name);
        let updated_channel = update_channel(&mut channel, &options);
        updated_channels.push(updated_channel);
    }

    let new_manifest = Manifest::update_channels(manifest, updated_channels);

    Ok(())
}
