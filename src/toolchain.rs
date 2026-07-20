use std::{
    borrow::Cow,
    collections::HashSet,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, bail};
use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::{
    channel::{Channel, Component, Tags, UserChannel},
    commands,
    config::Config,
    manifest::Manifest,
    options::InstallationOptions,
    profile::Profile,
};

/// Represents a `miden-toolchain.toml` file.
///
/// These file contains the desired toolchain to be used.
#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct ToolchainFile {
    toolchain: Toolchain,
}

impl ToolchainFile {
    pub fn new(toolchain: Toolchain) -> Self {
        ToolchainFile { toolchain }
    }

    #[inline]
    fn into_toolchain(self) -> Toolchain {
        self.toolchain
    }
}

/// The actual contents of the toolchain.
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Toolchain {
    pub channel: UserChannel,
    pub components: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<Profile>,
}

/// Used to specify why Midenup believes the current toolchain is what it is.
#[derive(Debug)]
pub enum ToolchainJustification {
    /// There exists a miden toolchain file present at `path`
    MidenToolchainFile { path: PathBuf },
    /// The system's default toolchain was overriden (via `midenup set`).
    Override,
    /// No toolchain was specified, fallback to stable.
    Default,
}

impl Toolchain {
    pub fn new(channel: UserChannel, profile: Option<Profile>, components: Vec<String>) -> Self {
        Toolchain { channel, components, profile }
    }

    /// Whether this toolchain requests a subset of its channel's components, either by listing
    /// components explicitly or through a profile that filters out optional ones.
    pub fn requests_subset(&self) -> bool {
        !self.components.is_empty() || matches!(self.profile.unwrap_or_default(), Profile::Minimal)
    }

    /// Returns the current active Toolchain according to the following prescedence:
    ///
    /// 1. The toolchain specified by a `miden-toolchain.toml` file in the present working directory
    /// 2. The toolchain that has been set as the system's default. If set, a `default` symlink is
    ///    added to the `midenup` directory.
    ///
    /// If none of the previous conditions are met, then `stable` will be used.
    pub fn current(config: &Config) -> anyhow::Result<(Toolchain, ToolchainJustification)> {
        let local_toolchain = Self::toolchain_file(&config.working_directory);
        let global_toolchain = config.midenup_home.join("toolchains").join("default");

        if let Some(local_toolchain) = local_toolchain {
            let toolchain_file_contents =
                std::fs::read_to_string(&local_toolchain).with_context(|| {
                    format!("unable to read toolchain file '{}'", local_toolchain.display())
                })?;

            let toolchain_file: ToolchainFile =
                toml::from_str(&toolchain_file_contents).context("invalid toolchain file")?;

            let current_toolchain = toolchain_file.into_toolchain();

            Ok((
                current_toolchain,
                ToolchainJustification::MidenToolchainFile { path: local_toolchain },
            ))
        } else if let Ok(channel_path) = std::fs::read_link(&global_toolchain) {
            let channel_name = channel_path
                .file_name()
                .and_then(|name| name.to_str())
                .context("unable to read channel name from directory")?;

            // NOTE: This has to be a UserChannel because the default channel could be a channel
            // like "stable"
            let user_channel = UserChannel::from_str(channel_name)?;

            let toolchain = Toolchain {
                channel: user_channel,
                components: vec![],
                profile: None,
            };

            Ok((toolchain, ToolchainJustification::Override))
        } else {
            Ok((Toolchain::default(), ToolchainJustification::Default))
        }
    }

    pub fn ensure_current_is_installed(
        config: &Config,
        local_manifest: &mut Manifest,
    ) -> anyhow::Result<(Self, ToolchainJustification, Channel)> {
        let (current_toolchain, justification) = Toolchain::current(config)?;
        let desired_channel = &current_toolchain.channel;

        let Some(upstream_channel) = config.manifest.get_channel(desired_channel) else {
            bail!(
                "channel '{}' is set because {}, however the channel doesn't exist or is \
                 unavailable",
                desired_channel,
                match justification {
                    ToolchainJustification::Default => Cow::Borrowed("it is the default"),
                    ToolchainJustification::MidenToolchainFile { path } => {
                        Cow::Owned(format!("it is set in {}", path.display()))
                    },
                    ToolchainJustification::Override =>
                        Cow::Borrowed("it was set using 'midenup set'"),
                }
            );
        };

        let installed_channel = local_manifest.get_channel_by_name(&upstream_channel.name);

        // The subset of the channel that the current toolchain requests. A toolchain with a
        // complete profile and no explicit component list requests the whole channel; upstream
        // tags are not carried over, since local channels manage their own.
        let partial_channel = if current_toolchain.requests_subset() {
            upstream_channel.create_subset(&current_toolchain, &justification)
        } else {
            Channel {
                tags: Vec::new(),
                ..upstream_channel.clone()
            }
        };

        let channel_to_install = match installed_channel {
            Some(installed_channel) => {
                // The channel is already installed, so we compute the missing components.
                let Some(new_channel) = complete_channel(
                    &current_toolchain,
                    upstream_channel,
                    installed_channel,
                    &partial_channel,
                ) else {
                    println!(
                        "{}: current toolchain is {desired_channel} and is installed",
                        "info".white().bold()
                    );
                    // A partial channel may contain fewer components than the toolchain's
                    // request spans, so the installed subset is the active one.
                    let active_channel = if installed_channel.is_partially_installed() {
                        installed_channel.clone()
                    } else {
                        partial_channel
                    };
                    return Ok((current_toolchain, justification, active_channel));
                };

                new_channel
            },
            None => {
                println!(
                    "{}: current toolchain is {desired_channel}, but not yet installed",
                    "info".white().bold()
                );
                partial_channel.clone()
            },
        };

        // The channel computed above contains exactly the components that need
        // to be installed, so no profile-based filtering must be applied on
        // top of it (optional components in it were explicitly requested).
        let install_options = InstallationOptions {
            profile: Profile::Complete,
            ..Default::default()
        };
        commands::install(config, &channel_to_install, local_manifest, &install_options)?;

        // Now installed. A partial channel may contain fewer components than the toolchain's
        // request spans, so the installed subset is the active one.
        let active_channel = if channel_to_install.is_partially_installed() {
            channel_to_install
        } else {
            partial_channel
        };
        Ok((current_toolchain, justification, active_channel))
    }

    /// Returns the `miden-toolchain.toml` file, if it exists.
    ///
    /// It looks for the file from the present working directory upwards, until the root directory
    /// is reached.
    fn toolchain_file(working_directory: &Path) -> Option<PathBuf> {
        // Check for a `miden-toolchain.toml` file in $CWD and recursively upwards.
        let mut current_dir = Some(working_directory);
        let mut toolchain_file = None;
        while let Some(current_path) = current_dir {
            let current_file = current_path.join("miden-toolchain").with_extension("toml");
            if current_file.exists() {
                toolchain_file = Some(current_file);
                break;
            }
            current_dir = current_path.parent();
        }

        toolchain_file
    }
}

/// Computes the channel required to complete an installed channel.
///
/// A channel tagged as Partial is considered valid, since the components are explicitly required.
/// Otherwise, detects missing components, e.g. ones added upstream after the channel was installed.
///
/// The resulting channel keeps the `Partial` tag while it remains a strict subset of the
/// upstream channel and either the installed channel or the request was partial; completing the
/// full span drops the tag.
///
/// Returns `None` when no components are missing.
fn complete_channel(
    current_toolchain: &Toolchain,
    upstream_channel: &Channel,
    installed_channel: &Channel,
    partial_channel: &Channel,
) -> Option<Channel> {
    // NOTE: Components are compared by name; versions may differ.
    let installed_components: HashSet<&str> = installed_channel
        .components
        .iter()
        .map(|component| component.name.as_ref())
        .collect();

    let partially_installed = installed_channel.is_partially_installed();
    let required_components: Vec<&Component> =
        if partially_installed && current_toolchain.requests_subset() {
            current_toolchain
                .components
                .iter()
                .filter_map(|name| upstream_channel.get_component(name))
                .flat_map(|component| {
                    std::iter::once(component).chain(
                        component
                            .requires
                            .iter()
                            .filter_map(|dependency| upstream_channel.get_component(dependency)),
                    )
                })
                .collect()
        } else {
            partial_channel.components.iter().collect()
        };

    let mut seen = HashSet::new();
    let missing_components: Vec<&Component> = required_components
        .into_iter()
        .filter(|component| !installed_components.contains(component.name.as_ref()))
        .filter(|component| seen.insert(component.name.clone()))
        .collect();

    if missing_components.is_empty() {
        return None;
    }

    println!(
        "{}: installing missing components of the current toolchain:",
        "info".white().bold()
    );
    for component in &missing_components {
        println!("- {}", component.name.white().bold());
    }

    // We add the missing components.
    let mut new_channel = installed_channel.clone();
    for component in missing_components {
        new_channel.components.push(component.clone());
    }

    // The channel stays tagges as partial while it remains a strict subset of the upstream channel
    // and either the installed channel or the request hand-picked its components; completing the
    // full span drops the tag so future updates track upstream again.
    let spans_upstream = upstream_channel
        .components
        .iter()
        .all(|component| new_channel.get_component(&component.name).is_some());
    new_channel.tags.retain(|tag| !matches!(tag, Tags::Partial));
    if !spans_upstream && (partially_installed || partial_channel.is_partially_installed()) {
        new_channel.tags.push(Tags::Partial);
    }

    Some(new_channel)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::Authority;

    fn component(name: &'static str) -> Component {
        Component::new(
            name,
            Authority::Cargo {
                package: None,
                version: semver::Version::new(1, 0, 0),
            },
        )
    }

    fn channel(components: &[&'static str], partial: bool) -> Channel {
        Channel {
            name: semver::Version::new(0, 1, 0),
            alias: None,
            tags: if partial { vec![Tags::Partial] } else { vec![] },
            components: components.iter().map(|name| component(name)).collect(),
        }
    }

    fn toolchain(profile: Option<Profile>, components: &[&str]) -> Toolchain {
        Toolchain::new(
            UserChannel::default(),
            profile,
            components.iter().map(|name| name.to_string()).collect(),
        )
    }

    /// A partial channel is valid as-is: without explicit component requests nothing gets
    /// installed, whether the minimal profile is implied or explicit.
    #[test]
    fn partial_channel_without_explicit_requests_is_left_alone() {
        let upstream = channel(&["base", "client", "std"], false);
        let installed = channel(&["base"], true);
        // Subset derived from the minimal profile.
        let partial = channel(&["base", "client", "std"], false);

        for profile in [None, Some(Profile::Minimal)] {
            let result =
                complete_channel(&toolchain(profile, &[]), &upstream, &installed, &partial);
            assert!(result.is_none(), "profile {profile:?} must leave the partial channel alone");
        }
    }

    /// Explicitly requested components and their dependencies (and nothing else) complete the
    /// channel. The result remains a strict subset, so it is tagged as partial, whether the pin
    /// comes from the installed channel (case 1) or from the request (case 2).
    #[test]
    fn requested_components_are_added_and_pin_the_channel_as_partial() {
        let mut upstream = channel(&["base", "client", "std", "vm"], false);
        upstream.get_component_mut("client").unwrap().requires = vec!["std".to_string()];

        // Case 1: the pin comes from the installed channel; the request spans the whole channel
        // (every component is mandatory), so it is untagged.
        let installed = channel(&["base"], true);
        let request = channel(&["base", "client", "std", "vm"], false);

        let completed =
            complete_channel(&toolchain(None, &["client"]), &upstream, &installed, &request)
                .expect("client should be missing");

        assert!(completed.get_component("client").is_some());
        // client's dependency gets pulled in as well
        assert!(completed.get_component("std").is_some());
        // vm is in the request's span but was never explicitly requested
        assert!(completed.get_component("vm").is_none());
        assert!(completed.is_partially_installed());

        // Case 2: the pin comes from the request; with vm optional, the minimal span plus the
        // explicit list is a strict subset.
        upstream.get_component_mut("vm").unwrap().optional = true;
        let installed = channel(&["base"], false);
        let request = channel(&["base", "client", "std"], true);

        let completed =
            complete_channel(&toolchain(None, &["client"]), &upstream, &installed, &request)
                .expect("client and std should be missing");

        assert!(completed.get_component("client").is_some());
        assert!(completed.get_component("std").is_some());
        assert!(completed.get_component("vm").is_none());
        assert!(
            completed.is_partially_installed(),
            "a channel shaped by an explicit component list must be tagged as partial"
        );
    }

    /// Completing the full upstream span never tags the channel: a complete profile drops the
    /// pin (case 1), and an untagged channel adopts new upstream components and stays untagged
    /// (case 2).
    #[test]
    fn completing_the_full_span_leaves_the_channel_untagged() {
        let upstream = channel(&["base", "client"], false);
        // Both profiles' subsets span the whole channel, so the request is untagged.
        let request = channel(&["base", "client"], false);

        for (installed, profile) in [
            (channel(&["base"], true), Some(Profile::Complete)),
            (channel(&["base"], false), None),
        ] {
            let completed =
                complete_channel(&toolchain(profile, &[]), &upstream, &installed, &request)
                    .expect("client should be missing");

            assert_eq!(completed.components.len(), 2);
            assert!(
                !completed.is_partially_installed(),
                "profile {profile:?} must complete the full span untagged"
            );
        }
    }
}
