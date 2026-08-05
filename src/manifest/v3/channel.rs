use std::{
    collections::{BTreeSet, HashMap},
    fmt,
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use super::{Component, ComponentKind};
use crate::{
    channel::{UpstreamChannel, UpstreamMatch},
    config::Config,
    exec::Executable,
    manifest::{Alias, ManifestError, v3::unknown::Extra},
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
    /// The channel this one supersedes, if any.
    ///
    /// An installation of the named channel is carried here when it is updated: intent transfers
    /// verbatim, `var/` is renamed so client data follows, and the old publication is removed once
    /// the new state record commits (spec section 11.4).
    ///
    /// Replaces the v1 `tags: [{ migration: { old_channel } }]` array. Migration is a property of
    /// the *upstream* channel, and expressing it as one field rather than as a member of an
    /// open-ended tag list means it can be found without pattern-matching over unrelated values.
    /// The other tag, `partial`, described local state and is now derived (section 8.6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrates_from: Option<semver::Version>,
    /// The set of toolchain components available in this channel
    pub components: Vec<Component>,
    /// Fields declared by a newer schema that this build does not recognize.
    ///
    /// Safe to derive here: `Channel` has no other flattened field.
    #[serde(flatten)]
    pub extra: Extra,
}

impl Channel {
    pub fn new(name: semver::Version, components: Vec<Component>) -> Self {
        Self {
            name,
            components,
            migrates_from: None,
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

    /// Whether this channel holds fewer components than the `upstream` one it is compared against.
    ///
    /// Derived, never recorded (spec section 8.6): a stored "partial" flag would be a second answer
    /// to a question the component set already answers, and two answers can disagree.
    pub fn is_partially_installed(&self, upstream: &Channel) -> bool {
        self.components.len() < upstream.components.len()
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

    /// Finds the upstream channel this one corresponds to.
    ///
    /// Either they share a version, or an upstream channel declares `migrates_from: <this
    /// channel>`. A same-version match wins: a channel that still exists upstream is not migrated
    /// away from, whatever some other channel claims to supersede.
    pub fn find_upstream_counterpart(&self, config: &Config) -> Option<UpstreamChannel> {
        let upstream_manifest = config.upstream_manifest().ok()?;

        if let Some(same_version) =
            upstream_manifest.get_channels().find(|upstream| upstream.name == self.name)
        {
            return Some(UpstreamChannel::new(
                same_version.clone(),
                UpstreamMatch::UpstreamCounterpart,
                config,
            ));
        }

        let successor = upstream_manifest
            .get_channels()
            .find(|upstream| upstream.migrates_from.as_ref() == Some(&self.name))?;

        Some(UpstreamChannel::new(
            successor.clone(),
            UpstreamMatch::Migrated { old_channel: self.name.clone() },
            config,
        ))
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
        write!(f, "{}", self.name)
    }
}
