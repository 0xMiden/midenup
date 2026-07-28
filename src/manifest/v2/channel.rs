use std::{
    collections::{BTreeSet, HashMap},
    fmt,
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use super::{Component, ComponentKind};
use crate::{
    channel::{ChannelAlias, MigrationStrategy, Tags, UpstreamChannel, UpstreamMatch},
    config::Config,
    exec::Executable,
    manifest::{Alias, ManifestError, v2::unknown::Extra},
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
    /// Fields declared by a newer schema that this build does not recognize.
    ///
    /// Safe to derive here: `Channel` has no other flattened field.
    #[serde(flatten)]
    pub extra: Extra,
}

impl Channel {
    pub fn new(
        name: semver::Version,
        alias: Option<ChannelAlias>,
        components: Vec<Component>,
        tags: Vec<Tags>,
    ) -> Self {
        Self {
            name,
            alias,
            components,
            tags,
            extra: Extra::new(),
        }
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
                ComponentKind::Asset
                | ComponentKind::Package
                | ComponentKind::LegacyPackage { .. }
                | ComponentKind::Unsupported { .. } => vec![],
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
                ComponentKind::Asset
                | ComponentKind::Package
                | ComponentKind::LegacyPackage { .. }
                | ComponentKind::Unsupported { .. } => (),
            }
        }

        Ok(aliases)
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
