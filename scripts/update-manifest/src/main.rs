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

/// Structure that wraps a git repository that has the following structure:
///
/// parent_directory
/// ├── original_<repo_name>/
/// ├── <worktree1>/
/// ├── <worktree2>/
/// ├── (...)
/// └── <worktreeN>/
#[derive(Debug)]
struct GitRepo {
    // Parent temporary directory where all the worktrees will live. This is
    // saved to simplify debugging.
    parent_directory: PathBuf,
    original_repo: PathBuf,
    worktrees: Vec<GitWorktree>,
}

impl GitRepo {
    fn original_repo_format(parent_directory: PathBuf) -> PathBuf {
        parent_directory.join("original")
    }
    fn format_git_tag(version: &semver::Version) -> String {
        let tag = version.to_string();
        format!("v{}", tag)
    }

    fn new(ccrate: Crate) -> Self {
        let temp_dir =
            tempdir::TempDir::new(format!("midenup-update-manifest-{}", ccrate.name).as_str())
                .expect("Failed to create temp directory");

        let clone_path = temp_dir.into_path();
        let original_repo_path = GitRepo::original_repo_format(clone_path.clone());

        let repo_url = ccrate.repository_url;
        let output = std::process::Command::new("git")
            .args(["clone", &repo_url, &original_repo_path.display().to_string()])
            .output()
            .expect("Failed to execute git clone");

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            panic!("git clone failed: {stderr}");
        }

        // To prevent race conditions, we create GitWorktrees ahead of time.
        let mut worktrees = Vec::new();
        {
            for version in &ccrate.versions {
                let tag = GitRepo::format_git_tag(version);
                let worktree_path = clone_path.join(&tag);

                let tagv_2 = version.to_string();

                let worktree = {
                    if let Ok(worktree) = GitWorktree::new(
                        worktree_path.clone(),
                        original_repo_path.clone(),
                        &tag,
                        version.clone(),
                    ) {
                        worktree
                    } else if let Ok(worktree) = GitWorktree::new(
                        worktree_path,
                        original_repo_path.clone(),
                        &tagv_2,
                        version.clone(),
                    ) {
                        worktree
                    } else {
                        panic!("")
                    }
                };

                worktrees.push(worktree);
            }
        }

        GitRepo {
            parent_directory: clone_path,
            original_repo: original_repo_path,
            worktrees,
        }
    }

    // Maybe remove?
    fn git_command<I, S>(&self, args: I) -> anyhow::Result<()>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<std::ffi::OsStr>,
    {
        let output = std::process::Command::new("git")
            .current_dir(&self.original_repo)
            .args(args)
            .output()
            .expect("Failed to execute git command");

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("git command failed: {stderr}");
        }

        Ok(())
    }

    // fn get_dependencies(&self, string
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

#[derive(Debug)]
struct GitWorktree {
    version: CrateVersion,
    path: PathBuf,
}

impl GitWorktree {
    pub fn new(
        path: PathBuf,
        original_repo_path: PathBuf,
        name: &str,
        version: CrateVersion,
    ) -> anyhow::Result<GitWorktree> {
        let output = std::process::Command::new("git")
            .current_dir(original_repo_path)
            .args(["worktree", "add", &path.display().to_string(), name])
            .output()
            .context("Failed to create worktree")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("git worktree add failed for {name}: {stderr}");
        }

        let worktree = GitWorktree { version, path };

        Ok(worktree)
    }

    fn find_dependencies(&self, cargo_toml: PathBuf) -> anyhow::Result<Vec<Dependency>> {
        let manifest = cargo_toml::Manifest::from_path(&cargo_toml)
            .with_context(|| format!("Failed to parse {}", cargo_toml.display()))?;

        let root_manifest = cargo_toml::Manifest::from_path(self.path.join("Cargo.toml")).ok();
        let workspace_deps = root_manifest
            .as_ref()
            .and_then(|m| m.workspace.as_ref())
            .map(|ws| &ws.dependencies);

        let mut deps = Vec::new();
        for (name, dep) in &manifest.dependencies {
            let version_str = match dep {
                cargo_toml::Dependency::Simple(v) => Some(v.as_str()),
                cargo_toml::Dependency::Detailed(detail) => detail.version.as_deref(),
                cargo_toml::Dependency::Inherited(_) => workspace_deps
                    .and_then(|ws| ws.get(name.as_str()))
                    .and_then(|ws_dep| match ws_dep {
                        cargo_toml::Dependency::Simple(v) => Some(v.as_str()),
                        cargo_toml::Dependency::Detailed(d) => d.version.as_deref(),
                        // This should never happen since we are at the root manifest.
                        _ => None,
                    }),
            };

            if let Some(v) = version_str {
                if let Ok(version) = v.parse::<CrateRequirement>() {
                    deps.push(Dependency::new(name.clone(), version));
                }
            }
        }

        Ok(deps)
    }

    fn find_crate_root(&self, crate_name: &str) -> anyhow::Result<PathBuf> {
        let mut dirs_to_visit = vec![self.path.clone()];

        while let Some(dir) = dirs_to_visit.pop() {
            let cargo_toml_path = dir.join("Cargo.toml");
            if cargo_toml_path.exists() {
                if let Ok(manifest) = cargo_toml::Manifest::from_path(&cargo_toml_path) {
                    if let Some(ref pkg) = manifest.package {
                        if pkg.name == crate_name {
                            return Ok(dir);
                        }
                    }
                }
            }

            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries {
                let Ok(entry) = entry else {
                    continue;
                };
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_dir() {
                    dirs_to_visit.push(entry.path());
                }
            }
        }

        bail!("Could not find crate '{crate_name}' in worktree at {}", self.path.display())
    }
}

// fn get_dependencies(repo_url: &str) ->

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

        let mut dependencies: HashMap<CrateVersion, Vec<Dependency>> = HashMap::new();
        for version in &versions {
            let deps = self.fetch_dependencies(crate_name, version).unwrap_or_else(|e| {
                panic!("Could not fetch dependencies for {crate_name}@{version}: {e}")
            });
            dependencies.insert(version.clone(), deps);
        }

        Ok(QueriedCrateInfo::new(versions, repository, dependencies))
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
    versions: Vec<CrateVersion>,
    repository: RepositoryURL,
    dependencies: HashMap<CrateVersion, Vec<Dependency>>,
}

impl QueriedCrateInfo {
    fn new(
        versions: Vec<CrateVersion>,
        repository: RepositoryURL,
        dependencies: HashMap<CrateVersion, Vec<Dependency>>,
    ) -> Self {
        Self { versions, repository, dependencies }
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
    repository_url: RepositoryURL,
}

impl Crate {
    fn new(name: CrateName, crates_io_info: QueriedCrateInfo) -> Crate {
        let versions = crates_io_info.versions;
        let repository = crates_io_info.repository;
        Crate {
            name,
            versions,
            repository_url: repository,
        }
    }
}

#[derive(Debug)]
struct CrateWithSource {
    name: CrateName,
    repository: GitRepo,
}
impl CrateWithSource {
    fn new(ccrate: Crate) -> Self {
        let name = ccrate.name.clone();
        let repository = GitRepo::new(ccrate);

        CrateWithSource { name, repository }
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
    fn new(manifest: &Manifest) -> Self {
        let client = CratesIOApi::new();

        let mut crates: Vec<Crate> = Vec::new();

        // The first iteration fetches the available known data for these
        // components over at crates.io. This loop is fairly paralellizable,
        // however, we are limited by crates.io rate-limits; that's why we are
        // doing it serially.
        // Source: https://crates.io/data-access#api
        {
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

                    let ccrate = Crate::new(crate_name, crate_info);
                    crates.push(ccrate);
                }
            }
        }

        // We now iterate again to remove un-needed version numbers. We're only
        // interested in versions present in the manifest.
        {
            let mut used_version: HashMap<CrateName, Vec<CrateVersion>> = HashMap::new();
            for channel in manifest.get_channels() {
                for component in &channel.components {
                    let Authority::Cargo { package, version } = &component.version else {
                        continue;
                    };
                    let crate_name = package.as_deref().unwrap_or(&component.name).to_string();

                    used_version.entry(crate_name).or_default().push(version.clone());
                }
            }

            for ccrate in &mut crates {
                if let Some(used) = used_version.get(&ccrate.name) {
                    ccrate.versions.retain(|v| used.contains(v));
                }
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

    let releases = Crates::new(&manifest);
    std::dbg!(&releases);
    let repos: Vec<_> =
        releases.crates.into_iter().map(|ccrate| CrateWithSource::new(ccrate)).collect();
    std::dbg!(&repos);
    // let mut updated_channels = Vec::new();
    // for mut channel in manifest.get_channels() {
    //     println!("  - Channel: {}", channel.name);
    //     let updated_channel = update_channel(&mut channel, &mut releases, &options);
    //     updated_channels.push(updated_channel);
    // }

    // let new_manifest = Manifest::update_channels(manifest, updated_channels);

    Ok(())
}
