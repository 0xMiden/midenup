use anyhow::bail;
use anyhow::Context;
use cargo_toml;
use clap::Parser;
use midenup::channel::semver;
use midenup::channel::Channel;
use midenup::channel::Component;
use midenup::manifest::Manifest;
use midenup::version::Authority;
use std::collections::HashMap;
use std::path::PathBuf;

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

#[derive(Debug)]
struct Dependency {
    name: CrateName,
    version: CrateRequirement,
}

impl Dependency {
    fn new(name: CrateName, version: CrateRequirement) -> Dependency {
        Dependency { name, version }
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

    fn fetch_versions(&self, crate_name: &str) -> anyhow::Result<Vec<CrateVersion>> {
        let crate_response = self.client.get_crate(crate_name)?;
        let versions: Vec<_> = crate_response
            .versions
            .into_iter()
            .filter_map(|v| v.num.parse::<semver::Version>().ok())
            .collect();

        Ok(versions)
    }

    fn fetch_dependencies(
        &self,
        crate_name: &str,
        version: &CrateVersion,
    ) -> anyhow::Result<Vec<Dependency>> {
        let api_deps = self.client.crate_dependencies(crate_name, &version.to_string())?;
        let deps = api_deps
            .into_iter()
            .filter(|d| d.kind == "normal")
            .filter_map(|d| {
                let req = d.req.parse::<CrateRequirement>().ok()?;
                Some(Dependency::new(d.crate_id, req))
            })
            .collect();
        Ok(deps)
    }
}

struct QueriedCrateInfo {
    repository: RepositoryURL,
    dependencies: HashMap<CrateVersion, Vec<Dependency>>,
}

impl QueriedCrateInfo {
    fn new(
        repository: RepositoryURL,
        dependencies: HashMap<CrateVersion, Vec<Dependency>>,
    ) -> Self {
        Self { repository, dependencies }
    }
}

type CrateName = String;
type CrateVersion = semver::Version;
type CrateRequirement = semver::VersionReq;
type MidenVMVersion = semver::Version;
type RepositoryURL = String;
#[derive(Debug)]
struct Crate {
    name: CrateName,
    versions: Vec<CrateVersion>,
}

impl Crate {
    fn new(name: CrateName, versions: Vec<CrateVersion>) -> Crate {
        Crate { name, versions }
    }
}

#[derive(Debug)]
struct CrateWithDependencies {
    name: CrateName,
    dependencies: HashMap<CrateVersion, Vec<Dependency>>,
}

#[derive(Debug)]
struct CratesWithDependencies {
    crates: Vec<CrateWithDependencies>,
}

impl CratesWithDependencies {
    fn new(crates: Crates, client: &CratesIOApi) -> Self {
        let crates_with_deps = crates
            .crates
            .into_iter()
            .map(|ccrate| {
                let mut dependencies: HashMap<CrateVersion, Vec<Dependency>> = HashMap::new();
                for version in &ccrate.versions {
                    let deps =
                        client.fetch_dependencies(&ccrate.name, version).unwrap_or_else(|e| {
                            panic!(
                                "Could not fetch dependencies for {}@{version}: {e}",
                                ccrate.name
                            )
                        });
                    dependencies.insert(version.clone(), deps);
                }
                CrateWithDependencies { name: ccrate.name, dependencies }
            })
            .collect();

        Self { crates: crates_with_deps }
    }
}

// These are crates that are the corner
enum CompatibilityCrates {
    //
}

// Dependency graph used to determine compatibility between components.
// By compatibility we mean that the underlying miden-protocol is the same.
// This assumption is based on:
// https://github.com/0xMiden/midenup/pull/142#discussion_r2749774499
// TODO: Escape hatch
struct DependencyGraph {
    //
}

// I think I like vector a bit better since it makes it a bit easier to serialize.
#[derive(Debug)]
struct Crates {
    crates: Vec<Crate>,
}

impl Crates {
    // TODO: Save in disk already known releases with their corresponding VM versions.
    // Only fetch the new ones.
    fn new(manifest: &Manifest, client: &CratesIOApi) -> Self {
        let mut used_version: HashMap<CrateName, Vec<CrateVersion>> = HashMap::new();
        let mut crates: Vec<Crate> = Vec::new();

        // The first iteration fetches the available known data for these
        // components over at crates.io. This loop is fairly paralellizable,
        // however, we are limited by crates.io rate-limits; that's why we are
        // doing it serially.
        // Source: https://crates.io/data-access#api
        {
            for channel in manifest.get_channels() {
                for component in &channel.components {
                    let Authority::Cargo { package, version } = &component.version else {
                        continue;
                    };
                    let crate_name = package.as_deref().unwrap_or(&component.name).to_string();
                    if crates.iter().any(|c| c.name == crate_name) {
                        continue;
                    }

                    let crate_info = client.fetch_versions(&crate_name).unwrap_or_else(|e| {
                        panic!("Could not query crates.io for {crate_name} for repository: {e}")
                    });

                    let ccrate = Crate::new(crate_name.clone(), crate_info);

                    used_version.entry(crate_name).or_default().push(version.clone());

                    crates.push(ccrate);
                }
            }
        }

        // We now iterate again to remove un-needed version numbers. We're only
        // interested in versions present in the manifest.
        {
            for ccrate in &mut crates {
                if let Some(used) = used_version.get(&ccrate.name) {
                    ccrate.versions.retain(|v| used.contains(v));
                }
            }
        }

        Self { crates }
    }
}

fn update_component(component: &Component, versions: &[semver::Version]) -> Component {
    todo!()
}

fn update_channel(channel: &Channel, releases: &mut Crates, options: &Options) -> Channel {
    let vm_version = get_vm_version(channel).expect("Could not find VM version in channel");
    println!("    VM version: {vm_version}");

    channel.clone()
}

fn main() -> anyhow::Result<()> {
    let cli = CliArguments::parse();

    let manifest = Manifest::load_from(&cli.uri)
        .map_err(|e| anyhow::anyhow!("Failed to load manifest from `{}`: {e}", cli.uri))?;
    println!("Manifest loaded successfully from `{}`", cli.uri);

    let options = Options::from(cli);

    let client = CratesIOApi::new();
    let crates = Crates::new(&manifest, &client);
    let cratesDeps = CratesWithDependencies::new(crates, &client);

    Ok(())
}
