//! Automated update script for `channel-manifest.json`.
//!
//! ## Purpose
//!
//! This script is intended to automatically update midenup's channel manifest
//! file with newer releases of components, while maintaining compatibility.
//!
//! ## How it works
//!
//! 1. First, for each Cargo component in the manifest, we query crates.io
//!    for all published versions larger than the version currently listed in the manifest.
//! 2. To determine compatibilitt, we clone each crate's git repository and, for every
//!    candidate version, create a git worktree. Then, we read its `Cargo.lock` to find which
//!    version of the compatibility crate (`miden-protocol`, `miden-objects`, or `miden-vm`)
//!    it uses.
//! 3. To determine if a component is due for an update, we find the largest published
//!    version that is both newer than the current version *and* resolves to the same
//!    compatibility crate version as the channel it belongs to.
//! 4. Finally, we update matching component versions in the manifest and write the
//!    result to disk.
//!
//! ## Assumptions
//!
//! - **Compatibility is determined by a shared protocol version.** Two components are
//!   considered compatible if their `Cargo.lock` resolves to the same version of
//!   `miden-protocol` (or its legacy equivalents `miden-objects` / `miden-vm`).
//!   This assumption is documented in:
//!   <https://github.com/0xMiden/midenup/pull/142#discussion_r2749774499>
//! - **All Cargo components have a public git repository** listed on crates.io.
//! - **Version tags follow the `v{semver}` or `{semver}` convention** in the upstream repos.
//! - **crates.io rate limit of 1 req/sec is respected** — requests are made serially.
//!
//! ## Remaining TODO items:
//! - Automatically update `rustup_channel` field if available (mainly intended
//!   for the miden compiler and cargo miden). This should be fairly simple to
//!   achieve in the GitWorktree stage.
//! - Automatically detecting uploaded artifacts on github. A bit more involved
//!   since it involves mapping crates.io releases with their github equivalent.
//!
//! ## Known limitations / QoL improvements
//!
//! ### Speed up
//!
//! At the time of writing (19/03/2026), on my machine (Apple M3 Max, 36GB)
//! it takes around ~30 seconds for the script to run. I'd say it's fair to
//! assume that there will not be a lot of variance across machines since the
//! script is mainly IO intensive.
//! Looking forward however, with the manifest only ever growing, time and
//! especially disk usage will inevitably go up. To mitigate this a few
//! things could be tried:
//!
//! 1. Saving the state of CratesWithCompatibility on disk might reduce the
//!    need re-process them.
//!
//! 2. Parallelizing operations. This shouldn't be too hard since almost all
//!    the operations are independent from each other and are mainly IO
//!    related.
//!
//! 3. Chaning how the pipeline is applied. Currently, the processing
//!    pipeline is applied to all components grouped together, meaning that
//!    every item is on the same stage of the pipeline. However, if instead,
//!    we were to apply the pipeline to every component individually, we
//!    could save up on a lot of disk space and memory, since these freed up
//!    as soon as they are applied to the manifest.
use anyhow::bail;
use anyhow::Context;
use clap::Parser;
use midenup::channel::semver;
use midenup::channel::Channel;
use midenup::manifest::{AvailableUpdates, ComponentUpdate, Manifest};
use midenup::version::Authority;
use std::collections::HashMap;
use std::fmt::Display;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "update-manifest", about = "Parse and update midenup's manifest.")]
struct CliArguments {
    /// URI of the manifest to parse (supports file:// and https://)
    uri: String,
}

fn get_vm_version(channel: &Channel) -> Option<&semver::Version> {
    let vm = channel.get_component("vm")?;
    match &vm.version {
        Authority::Cargo { version, .. } => Some(version),
        _ => None,
    }
}

fn get_protocol_version(channel: &Channel) -> Option<&semver::Version> {
    // Try explicit protocol component first (0.13.3+)
    if let Some(proto) = channel.get_component("protocol") {
        if let Authority::Cargo { version, .. } = &proto.version {
            return Some(version);
        }
    }
    // Fallback: vm version as proxy for legacy channels
    get_vm_version(channel)
}

fn compute_available_updates(
    compatibility: &CratesWithCompatibility,
    manifest: &Manifest,
) -> AvailableUpdates {
    let mut updates = Vec::new();

    for channel in manifest.get_channels() {
        let Some(channel_protocol_version) = get_protocol_version(channel) else {
            eprintln!("Warning: channel {} has no protocol version, skipping", channel.name);
            continue;
        };

        for component in &channel.components {
            let Authority::Cargo { package, version: current_version } = &component.version else {
                continue;
            };
            let crate_name = package.as_deref().unwrap_or(&component.name);

            let Some(ccrate) =
                compatibility.compatibility_mappings.iter().find(|c| c.name == crate_name)
            else {
                continue;
            };

            let latest = ccrate
                .compatibility
                .iter()
                .filter(|(v, proto)| *v > current_version && *proto == channel_protocol_version)
                .map(|(v, _)| v)
                .max()
                .cloned();

            if let Some(latest_version) = latest {
                updates.push(ComponentUpdate {
                    channel_name: channel.name.clone(),
                    component_name: component.name.to_string(),
                    current_version: current_version.clone(),
                    latest_version,
                });
            }
        }
    }

    AvailableUpdates { updates }
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
                        eprintln!(
                            "{}",
                            format!(
                                "Failed to form GitWorktree: {} {}",
                                original_repo_path.display(),
                                version
                            )
                        );
                        continue;
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
}

#[derive(Debug)]
struct Dependency {
    name: CrateName,
    version: CrateVersion,
}

impl Dependency {
    fn new(name: CrateName, version: CrateVersion) -> Dependency {
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

    fn find_compatibility(&self) -> anyhow::Result<Dependency> {
        let lock_path = self.path.join("Cargo.lock");
        let lockfile = cargo_lock::Lockfile::load(&lock_path)
            .with_context(|| format!("Failed to load Cargo.lock at {}", lock_path.display()))?;

        let compatibility_names: std::collections::HashSet<String> = [
            CompatibilityCrates::MidenProtocol.to_string(),
            CompatibilityCrates::MidenObjects.to_string(),
            CompatibilityCrates::MidenVM.to_string(),
        ]
        .into();

        let deps: Vec<_> = lockfile
            .packages
            .into_iter()
            .filter(|p| compatibility_names.contains(p.name.as_str()))
            .map(|p| {
                let version = p
                    .version
                    .to_string()
                    .parse::<semver::Version>()
                    .expect("cargo-lock versions are always valid semver");
                Dependency::new(p.name.to_string(), version)
            })
            .collect();

        if deps.len() > 1 {
            eprintln!(
                "Warning: found {} compatibility entries in {}; expected 1. Using the first.",
                deps.len(),
                lock_path.display()
            );
        }

        deps.into_iter().next().ok_or_else(|| {
            anyhow::anyhow!("No compatibility crate found in {}", lock_path.display())
        })
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
struct CratesWithSource {
    crates: Vec<CrateWithSource>,
}
impl CratesWithSource {
    fn new(crates: Crates) -> Self {
        let crates: Vec<_> =
            crates.crates.into_iter().map(|ccrate| CrateWithSource::new(ccrate)).collect();
        CratesWithSource { crates }
    }
}

type MidenProtocolVersion = semver::Version;
#[derive(Debug)]
struct CrateWithCompatibility {
    name: CrateName,
    compatibility: HashMap<CrateVersion, MidenProtocolVersion>,
}

#[derive(Debug)]
struct CratesWithCompatibility {
    compatibility_mappings: Vec<CrateWithCompatibility>,
}

impl CratesWithCompatibility {
    fn new(crates_with_source: CratesWithSource) -> Self {
        let compatibility_mappings = crates_with_source
            .crates
            .into_iter()
            .map(|ccrate| {
                let compatibility = ccrate
                    .repository
                    .worktrees
                    .iter()
                    .filter_map(|worktree| {
                        let dep = worktree.find_compatibility().unwrap_or_else(|e| {
                            panic!(
                                "Could not find compatibility for {}@{}: {e}",
                                ccrate.name, worktree.version
                            )
                        });
                        Some((worktree.version.clone(), dep.version))
                    })
                    .collect();

                CrateWithCompatibility { name: ccrate.name.clone(), compatibility }
            })
            .collect();

        CratesWithCompatibility { compatibility_mappings }
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

enum CompatibilityCrates {
    MidenProtocol,
    // Legacy miden protocol name
    MidenObjects,
    MidenVM,
}
impl Display for CompatibilityCrates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            CompatibilityCrates::MidenProtocol => f.write_str("miden-protocol"),
            CompatibilityCrates::MidenObjects => f.write_str("miden-objects"),
            CompatibilityCrates::MidenVM => f.write_str("miden-vm"),
        }
    }
}

#[derive(Debug)]
struct Crates {
    crates: Vec<Crate>,
}

impl Crates {
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
            let mut min_version: HashMap<CrateName, CrateVersion> = HashMap::new();
            for channel in manifest.get_channels() {
                for component in &channel.components {
                    let Authority::Cargo { package, version } = &component.version else {
                        continue;
                    };
                    let crate_name = package.as_deref().unwrap_or(&component.name).to_string();
                    let entry = min_version.entry(crate_name).or_insert_with(|| version.clone());
                    if version < entry {
                        *entry = version.clone();
                    }
                }
            }

            for ccrate in &mut crates {
                if let Some(min) = min_version.get(&ccrate.name) {
                    ccrate.versions.retain(|v| v >= min);
                }
            }
        }

        Self { crates }
    }
}

fn main() -> anyhow::Result<()> {
    let cli = CliArguments::parse();

    let mut manifest = Manifest::load_from(&cli.uri)
        .map_err(|e| anyhow::anyhow!("Failed to load manifest from `{}`: {e}", cli.uri))?;

    let releases = Crates::new(&manifest);
    let crates = CratesWithSource::new(releases);
    let compatibility = CratesWithCompatibility::new(crates);
    let available_updates = compute_available_updates(&compatibility, &manifest);
    manifest.apply_updates(&available_updates);
    manifest.save_to(std::path::Path::new("manifest/channel-manifest.json"))?;
    println!("Manifest saved to manifest/channel-manifest.json");

    Ok(())
}
