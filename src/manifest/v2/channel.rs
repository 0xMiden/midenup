use std::{
    collections::{BTreeSet, HashMap},
    fmt,
    path::PathBuf,
};

use anyhow::bail;
use colored::Colorize;
use serde::{Deserialize, Serialize};

use super::{Component, ComponentKind};
use crate::{
    channel::{ChannelAlias, ChannelHash, MigrationStrategy, Tags, UpstreamChannel, UpstreamMatch},
    config::Config,
    exec::Executable,
    manifest::{Alias, ManifestError},
    profile::Profile,
    toolchain::{Toolchain, ToolchainJustification},
};

/// Represents a specific release channel for a toolchain.
///
/// Different channels have different stability guarantees. See the specific details for the
/// channel you are interested in to learn more.
#[derive(Serialize, Deserialize, Debug, Clone, Hash)]
pub struct Channel {
    /// Channels are identified by their name. The name corresponds to the channel's version.
    /// The version can contain suffixes such as "-custom", "-beta".
    pub name: semver::Version,
    /// This is used to tag special channels. Most notably, the current "stable" channel is marked
    /// with the [`ChannelAlias::Stable`] alias.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<ChannelAlias>,
    /// Set of tags used to denote a special characteristic about the channel.
    ///
    /// Mainly used for locally installed channels.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<Tags>,
    /// The set of toolchain components available in this channel
    pub components: Vec<Component>,
}

enum InstallationMotive {
    ExplicitelySelected,
    Dependency { comp_name: String },
}

impl fmt::Display for InstallationMotive {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallationMotive::Dependency { comp_name } => {
                write!(f, "is a depency of component {comp_name}")
            },
            InstallationMotive::ExplicitelySelected => {
                write!(f, "was explictely selected for installation")
            },
        }
    }
}

impl Channel {
    pub fn new(
        name: semver::Version,
        alias: Option<ChannelAlias>,
        components: Vec<Component>,
        tags: Vec<Tags>,
    ) -> Self {
        Self { name, alias, components, tags }
    }

    pub fn get_component(&self, name: impl AsRef<str>) -> Option<&Component> {
        let name = name.as_ref();
        self.components.iter().find(|c| c.name == name)
    }

    pub fn get_component_mut(&mut self, name: impl AsRef<str>) -> Option<&mut Component> {
        let name = name.as_ref();
        self.components.iter_mut().find(|c| c.name == name)
    }

    /// Is this channel a stable release? Does not imply that it has the `stable` alias.
    ///
    /// To find out the latest stable [Channel], use [crate::manifest::Manifest::get_latest_stable].
    pub fn is_stable(&self) -> bool {
        self.alias.as_ref().is_none_or(|alias| matches!(alias, ChannelAlias::Stable))
    }

    pub fn is_nightly(&self) -> bool {
        self.alias
            .as_ref()
            .is_some_and(|alias| matches!(alias, ChannelAlias::Nightly(_)))
    }

    /// Determines if the current toolchain was installed "partially", i.e., containing only a
    /// subset of all the available components. This can be the case with `miden-toolchain.toml`.
    pub fn is_partially_installed(&self) -> bool {
        self.tags.iter().any(|tag| matches!(tag, Tags::Partial))
    }

    pub fn is_latest_nightly(&self) -> bool {
        self.alias
            .as_ref()
            .is_some_and(|alias| matches!(alias, ChannelAlias::Nightly(None)))
    }

    pub fn get_channel_dir(&self, config: &Config) -> PathBuf {
        let installed_toolchains_dir = config.midenup_home.join("toolchains");
        installed_toolchains_dir.join(format!("{}", self.name))
    }

    pub fn content_hash(&self) -> ChannelHash {
        use core::{fmt::Write, hash::Hash};

        use sha2::Digest;

        struct Sha256Hasher(sha2::Sha256);
        impl core::hash::Hasher for Sha256Hasher {
            fn finish(&self) -> u64 {
                panic!("finish is intended to be left unused")
            }

            fn write(&mut self, bytes: &[u8]) {
                self.0.update(bytes);
            }
        }
        let mut h = Sha256Hasher(sha2::Sha256::new());
        self.hash(&mut h);
        let out = h.0.finalize();
        let mut hex = String::with_capacity(64);
        for byte in out {
            write!(&mut hex, "{byte:02x}").expect("failed to write channel hash");
        }
        ChannelHash(hex)
    }

    /// Get all the alias names that the Channel is aware of
    pub fn get_alias_names(&self) -> BTreeSet<Alias> {
        self.components
            .iter()
            .flat_map(|c| match c.kind() {
                ComponentKind::Command { aliases, .. } => {
                    aliases.keys().cloned().collect::<Vec<_>>()
                },
                ComponentKind::CargoExtension { spec, .. }
                | ComponentKind::Executable { spec, .. } => {
                    spec.aliases.keys().cloned().collect::<Vec<_>>()
                },
                ComponentKind::Asset { .. }
                | ComponentKind::Package
                | ComponentKind::LegacyPackage { .. } => vec![],
            })
            .collect()
    }

    /// Get all the aliases that the Channel is aware of
    pub fn get_aliases(&self) -> Result<HashMap<Alias, Executable>, ManifestError> {
        use std::collections::hash_map::Entry;

        let mut aliases = HashMap::new();
        let mut alias_definitions = HashMap::new();
        for component in self.components.iter() {
            match component.kind() {
                ComponentKind::CargoExtension { spec, .. }
                | ComponentKind::Executable { spec, .. } => {
                    for (alias, exe) in spec.aliases.iter() {
                        match aliases.entry(alias.clone()) {
                            Entry::Vacant(entry) => {
                                alias_definitions.insert(alias.as_str(), component.name.clone());
                                entry.insert(exe.clone());
                            },
                            Entry::Occupied(_) => {
                                let prev = alias_definitions[alias.as_str()].to_string();
                                return Err(ManifestError::ConflictingAlias {
                                    prev_component: prev,
                                    component: component.name.to_string(),
                                    alias: alias.clone(),
                                });
                            },
                        }
                    }
                },
                ComponentKind::Command {
                    command_name: name,
                    format,
                    aliases: command_aliases,
                    ..
                } => {
                    let name = name.as_deref().unwrap_or(component.name.as_ref());
                    for (alias, exe) in core::iter::once((name, format))
                        .chain(command_aliases.iter().map(|(k, v)| (k.as_str(), v)))
                    {
                        match aliases.entry(alias.to_string()) {
                            Entry::Vacant(entry) => {
                                alias_definitions.insert(alias, component.name.clone());
                                entry.insert(exe.clone());
                            },
                            Entry::Occupied(_) => {
                                let prev = alias_definitions[alias].to_string();
                                return Err(ManifestError::ConflictingAlias {
                                    prev_component: prev,
                                    component: component.name.to_string(),
                                    alias: alias.to_string(),
                                });
                            },
                        }
                    }
                },
                ComponentKind::Asset { .. }
                | ComponentKind::Package
                | ComponentKind::LegacyPackage { .. } => (),
            }
        }

        Ok(aliases)
    }

    /// Get all of the components in this channel that would be installed for `profile`
    ///
    /// This returns `Err` if the component graph is invalid due to:
    ///
    /// * Invalid references to components that don't exist in the same channel
    pub fn component_graph(&self, profile: &Profile) -> anyhow::Result<ComponentGraph<'_>> {
        let mut g = petgraph::graphmap::DiGraphMap::<usize, ()>::default();
        let install_everything = matches!(profile, Profile::Complete);
        let mut worklist = Vec::with_capacity(self.components.len());
        for (i, c) in self.components.iter().enumerate() {
            if install_everything || c.profiles.contains(profile) {
                worklist.push(i);
            }
        }
        while let Some(i) = worklist.pop() {
            g.add_node(i);
            let c = &self.components[i];
            for required in c.requires.iter() {
                let Some(j) =
                    self.components.iter().position(|c| c.name.as_ref() == required.as_str())
                else {
                    bail!(
                        "invalid requirement on '{required}' by '{}' in the {} channel: no such \
                         component",
                        c.name,
                        self
                    );
                };
                if !g.contains_node(j)
                    && !install_everything
                    && !self.components[j].profiles.contains(profile)
                {
                    worklist.push(j);
                }
                g.add_node(j);
                g.add_edge(i, j, ());
            }
        }

        Ok(ComponentGraph { channel: self, graph: g })
    }

    /// Creates a "partial channel" from the original channel, given a toolchain "Partial" in this
    /// context refers to the fact that the channel will not install all the available components,
    /// but rather a subset.
    pub fn create_subset(
        &self,
        current_toolchain: &Toolchain,
        toolchain_justification: &ToolchainJustification,
    ) -> Option<Channel> {
        let profile = current_toolchain.profile.unwrap_or_default();
        let mut requested_components = Vec::new();
        let mut components_to_install: Vec<Component> = Vec::new();
        let mut components_not_found: HashMap<String, Vec<InstallationMotive>> = HashMap::new();

        match profile {
            Profile::Empty | Profile::Minimal => {
                // Select components that are
                requested_components.extend(self.components.iter().filter_map(|c| {
                    if c.profiles.contains(&profile) {
                        Some(c.name.as_ref())
                    } else {
                        None
                    }
                }));
                for extra_component in current_toolchain.components.iter() {
                    if !requested_components.contains(&extra_component.as_str()) {
                        requested_components.push(extra_component.as_str());
                    }
                }
            },
            Profile::Complete => {
                // Select all components from the manifest
                requested_components.extend(self.components.iter().map(|c| c.name.as_ref()));
                // We add any non-duplicate extra components here so that we can catch invalid
                // components below
                for extra_component in current_toolchain.components.iter() {
                    if !requested_components.contains(&extra_component.as_str()) {
                        requested_components.push(extra_component.as_str());
                    }
                }
            },
        }

        for component_name in requested_components {
            let Some(component) = self.get_component(component_name) else {
                // NOTE: In order to provide more helpful error messages, we collect all the missing
                // components and return a single error message at the end.
                components_not_found
                    .entry(component_name.to_string())
                    .or_default()
                    .push(InstallationMotive::ExplicitelySelected);

                continue;
            };
            components_to_install.push(component.clone());

            for depenency_name in &component.requires {
                let Some(dependency) = self.get_component(depenency_name) else {
                    components_not_found.entry(depenency_name.to_string()).or_default().push(
                        InstallationMotive::Dependency { comp_name: component_name.to_string() },
                    );
                    continue;
                };

                if !components_to_install.iter().any(|c| c.name == dependency.name) {
                    components_to_install.push(dependency.clone());
                }
            }
        }
        if !components_not_found.is_empty() {
            println!(
                "{}: Some elements present in the current Toolchain are not present in the \
                 upstream channel: {}",
                "WARNING".yellow().bold(),
                self.name
            );
            println!();

            for (missing_component_name, motive) in components_not_found {
                let motives = motive
                    .iter()
                    .map(|motive| motive.to_string())
                    .collect::<Vec<String>>()
                    .join(" and ");

                println!(
                    "- {missing_component_name}, which {motives}, is missing in upstream channel"
                );
            }

            println!();
            println!("These components will be ignored for the current install.");
            println!();
            // TODO: Add messages for the other justifications
            #[allow(clippy::single_match)]
            match toolchain_justification {
                ToolchainJustification::MidenToolchainFile { path } => println!(
                    "Check the `miden_toolchain.toml` file in {} to see if any component is \
                     misspelled or got removed from upstream",
                    path.display()
                ),
                _ => (),
            }
        }

        let partial_channel = Channel {
            name: self.name.clone(),
            alias: self.alias.clone(),
            tags: vec![Tags::Partial],
            components: components_to_install,
        };

        Some(partial_channel)
    }

    /// Checks wheter the channel [other] is Self's upstream counterpart.
    /// Currently this can happen in two scenarios:
    /// - They share the same name (i.e. version).
    /// - The upstream version is tagged as having being migrated from self's .
    pub fn find_upstream_counterpart(&self, config: &Config) -> Option<UpstreamChannel> {
        let upstream_manifest = &config.manifest;
        let mut upstream_counterpart = None;

        for upstream_channel in upstream_manifest.get_channels() {
            // They share version
            let equal_name = self.name == upstream_channel.name;
            if equal_name {
                let upstream_match = UpstreamMatch::UpstreamCounterpart;
                upstream_counterpart =
                    Some(UpstreamChannel::new(upstream_channel.clone(), upstream_match, config));
                break;
            };

            let was_migrated = upstream_channel.tags.iter().find_map(|tag| match tag {
                Tags::Migration { migration } => match migration {
                    // A channel is only considered as "migrated" if it's
                    // current name matches the "old_channel" field of an
                    // upstream channel.
                    MigrationStrategy::NameChange { old_channel } => {
                        if old_channel == &self.name {
                            Some(migration)
                        } else {
                            None
                        }
                    },
                },
                _ => None,
            });

            if let Some(migration) = was_migrated {
                let upstream_match = UpstreamMatch::Migrated(migration.clone());
                upstream_counterpart =
                    Some(UpstreamChannel::new(upstream_channel.clone(), upstream_match, config));
                break;
            };
        }
        upstream_counterpart
    }

    // Syncs the channel to the latest changes
    pub(crate) fn sync(&mut self, config: &Config) {
        for comp in self.components.iter_mut() {
            comp.sync(config);
        }
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.alias {
            Some(ChannelAlias::Stable) => write!(f, "stable ({})", self.name),
            Some(ChannelAlias::Tag(tag)) => write!(f, "{}-{}", self.name, tag.as_ref()),
            Some(ChannelAlias::Nightly(tag)) => {
                let nightly_suffix =
                    tag.as_ref().map(|suffix| format!("-{}", suffix)).unwrap_or(String::from(""));
                write!(f, "nightly-{}{}", self.name, nightly_suffix)
            },
            None => write!(f, "{}", self.name),
        }
    }
}

pub struct ComponentGraph<'a> {
    channel: &'a Channel,
    graph: petgraph::graphmap::DiGraphMap<usize, ()>,
}

impl<'a> ComponentGraph<'a> {
    pub fn toposort(&self) -> anyhow::Result<impl Iterator<Item = &'a Component>> {
        match petgraph::algo::toposort(&self.graph, None) {
            Ok(sorted) => Ok(sorted.into_iter().map(|c| &self.channel.components[c])),
            Err(cycle) => bail!(
                "invalid component graph: cycle is formed due to requirements of '{}'",
                self.channel.components[cycle.node_id()].name.as_ref()
            ),
        }
    }
}
