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

fn get_vm_version(channel: &Channel) -> Option<&semver::Version> {
    let vm = channel.get_component("vm")?;
    match &vm.version {
        Authority::Cargo { version, .. } => Some(version),
        _ => None,
    }
}

struct CratesIOApi {
    client: crates_io_api::SyncClient,
}

impl CratesIOApi {
    fn new() -> CratesIOApi {
        let client = crates_io_api::SyncClient::new(
            "midenup (https://github.com/0xMiden/midenup)",
            std::time::Duration::from_millis(1000),
        )
        .expect("Invalid user agent. Check: https://docs.rs/crates_io_api/latest/crates_io_api/struct.SyncClient.html#method.new to see the correct format");

        CratesIOApi { client }
    }
    fn fetch_info(&self, crate_name: &str) -> anyhow::Result<QueriedCrateInfo> {
        let crate_response = self.client.get_crate(crate_name)?;
        let versions: Vec<_> = crate_response
            .versions
            .into_iter()
            .filter_map(|v| v.num.parse::<semver::Version>().ok())
            .collect();
        let repository = crate_response
            .crate_data
            .repository
            .unwrap_or_else(|| panic!("Crate {crate_name} has no repository URL"));

        let crate_info = QueriedCrateInfo { versions, repository };

        Ok(crate_info)
    }
}

struct QueriedCrateInfo {
    versions: Vec<CrateVersion>,
    repository: RepositoryURL,
}

type CrateName = String;
type CrateVersion = semver::Version;
type MidenVMVersion = semver::Version;
type RepositoryURL = String;
#[derive(Debug)]
struct Crate {
    name: CrateName,
    versions: Vec<CrateVersion>,
    repository: RepositoryURL,
}

impl Crate {
    fn new(name: CrateName, crates_io_info: QueriedCrateInfo) -> Crate {
        let versions = crates_io_info.versions;
        let repository = crates_io_info.repository;
        Crate { name, versions, repository }
    }
}

// I think I like vector a bit better since it makes it a bit easier to serialize.
#[derive(Debug)]
struct Crates {
    crates: Vec<Crate>,
}

impl Crates {
    // TODO: Save in disk already known releases with their corresponding VM versions.
    // Only fetch the new ones.
    // We iterate over the Manifest twice since creating the releases struct
    // consists of a lot of paralellizable IO operations.
    fn new(manifest: &Manifest) -> Self {
        let client = CratesIOApi::new();

        let mut crates: Vec<Crate> = Vec::new();
        let placeholder = semver::Version::new(0, 0, 0);

        for channel in manifest.get_channels() {
            for component in &channel.components {
                let Authority::Cargo { package, .. } = &component.version else {
                    continue;
                };
                let crate_name = package.as_deref().unwrap_or(&component.name).to_string();
                if crates.iter().any(|c| c.name == crate_name) {
                    continue;
                }

                let crate_info = client.fetch_info(&crate_name).unwrap_or_else(|e| {
                    panic!("Could not query crates.io for {crate_name} for repository: {e}")
                });

                // let versions: HashMap<CrateVersion, MidenVMVersion> =
                //     versions.into_iter().map(|v| (v, placeholder.clone())).collect();

                let ccrate = Crate::new(crate_name, crate_info);
                crates.push(ccrate);
            }
        }
        Self { crates }
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

fn update_channel(channel: &Channel, releases: &mut Crates, options: &Options) -> Channel {
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

    let mut releases = Crates::new(&manifest);
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
