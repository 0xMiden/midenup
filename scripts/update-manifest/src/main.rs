use clap::Parser;
use midenup::channel::semver;
use midenup::channel::Channel;
use midenup::channel::Component;
use midenup::manifest::Manifest;
use midenup::version::Authority;
use std::collections::HashMap;

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

fn find_all_versions(crate_name: &str) -> anyhow::Result<Vec<semver::Version>> {
    let client = crates_io_api::SyncClient::new(
        "midenup (https://github.com/0xMiden/midenup)",
        std::time::Duration::from_millis(1000),
    )?;
    let crate_response = client.get_crate(crate_name)?;
    let versions = crate_response
        .versions
        .into_iter()
        .filter_map(|v| v.num.parse::<semver::Version>().ok())
        .collect();
    Ok(versions)
}

fn get_vm_version(channel: &Channel) -> Option<&semver::Version> {
    let vm = channel.get_component("vm")?;
    match &vm.version {
        Authority::Cargo { version, .. } => Some(version),
        _ => None,
    }
}

type CrateName = String;
type CrateVersion = semver::Version;
type MidenVMVersion = semver::Version;

#[derive(Debug)]
struct Releases {
    releases: HashMap<CrateName, HashMap<CrateVersion, MidenVMVersion>>,
}

impl Releases {
    // TODO: Save in disk already known releases with their corresponding VM versions.
    // Only fetch the new ones.
    // We iterate over the Manifest twice since creating the releases struct
    // consists of a lot of paralellizable IO operations.
    fn new(manifest: &Manifest) -> Self {
        let mut releases: HashMap<CrateName, HashMap<CrateVersion, MidenVMVersion>> =
            HashMap::new();
        let placeholder = semver::Version::new(0, 0, 0);
        for channel in manifest.get_channels() {
            for component in &channel.components {
                let Authority::Cargo { package, .. } = &component.version else {
                    continue;
                };
                let crate_name = package.as_deref().unwrap_or(&component.name);
                if releases.contains_key(crate_name) {
                    continue;
                }
                let versions = find_all_versions(crate_name)
                    .unwrap_or_else(|e| panic!("Could not query crates.io for {crate_name}: {e}"));
                let version_map: HashMap<CrateVersion, MidenVMVersion> =
                    versions.into_iter().map(|v| (v, placeholder.clone())).collect();
                releases.insert(crate_name.to_string(), version_map);
            }
        }
        Self { releases }
    }

    // fn get(&mut self, crate_name: &str) -> anyhow::Result<&Vec<semver::Version>> {
    //     if !self.releases.contains_key(crate_name) {
    //         let versions = find_all_versions(crate_name)?;
    //         self.releases.insert(crate_name.to_string(), versions);
    //     }
    //     Ok(self.releases.get(crate_name).unwrap())
    // }
}

fn update_component(component: &Component, versions: &[semver::Version]) -> Component {
    todo!()
}

fn update_channel(channel: &Channel, releases: &mut Releases, options: &Options) -> Channel {
    let vm_version = get_vm_version(channel).expect("Could not find VM version in channel");
    println!("    VM version: {vm_version}");

    // for component in &channel.components {
    //     let Authority::Cargo { package, .. } = &component.version else {
    //         continue;
    //     };
    //     let crate_name = package.as_deref().unwrap_or(&component.name);
    //     let versions = releases
    //         .get(crate_name)
    //         .unwrap_or_else(|e| panic!("Could not query crates.io for {crate_name}: {e}"));
    //     for version in versions {
    //         println!("    {crate_name} {version}");
    //     }
    // }

    channel.clone()
}

fn main() -> anyhow::Result<()> {
    let cli = CliArguments::parse();

    let manifest = Manifest::load_from(&cli.uri)
        .map_err(|e| anyhow::anyhow!("Failed to load manifest from `{}`: {e}", cli.uri))?;
    println!("Manifest loaded successfully from `{}`", cli.uri);

    let options = Options::from(cli);

    let mut releases = Releases::new(&manifest);
    std::dbg!(&releases);
    let mut updated_channels = Vec::new();
    for mut channel in manifest.get_channels() {
        println!("  - Channel: {}", channel.name);
        let updated_channel = update_channel(&mut channel, &mut releases, &options);
        updated_channels.push(updated_channel);
    }

    let new_manifest = Manifest::update_channels(manifest, updated_channels);

    Ok(())
}
