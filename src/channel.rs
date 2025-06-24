use std::{borrow::Cow, fmt};

use serde::{Deserialize, Serialize};

use crate::version::Authority;

/// Represents a specific release channel for a toolchain.
///
/// Different channels have different stability guarantees. See the specific details for the
/// channel you are interested in to learn more.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Channel {
    pub name: semver::Version,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<ChannelAlias>,

    /// The set of toolchain components available in this channel
    pub components: Vec<Component>,
}

impl Channel {
    pub fn get_component(&self, name: impl AsRef<str>) -> Option<&Component> {
        let name = name.as_ref();
        self.components.iter().find(|c| c.name == name)
    }
    /// Is this channel a stable release? Does not imply that it has the `stable` alias
    pub fn is_stable(&self) -> bool {
        self.alias.as_ref().is_none_or(|alias| matches!(alias, ChannelAlias::Stable))
    }

    pub fn is_nightly(&self) -> bool {
        self.alias
            .as_ref()
            .is_some_and(|alias| matches!(alias, ChannelAlias::Nightly(_)))
    }

    pub fn is_latest_nightly(&self) -> bool {
        self.alias
            .as_ref()
            .is_some_and(|alias| matches!(alias, ChannelAlias::Nightly(None)))
    }

    pub fn components_to_update(&self, newer: &Self) -> Vec<Component> {
        // NOTE: This wouldn't work if they have different amount of elements
        // let old_components = self.components.iter();
        let new_components = newer.components.iter();
        let mut updates = Vec::new();

        for new_component in new_components {
            let old_component = self.components.iter().find(|c| c.name == new_component.name);
            if let Some(old_component) = old_component {
                if old_component != new_component {
                    updates.push(new_component.clone());
                }
            } else {
                // If the new component does not exist in the old component
                // list, then that means it was added in an update and must be
                // installed
                updates.push(new_component.clone());
            };
        }

        updates
    }
}

impl PartialEq for Channel {
    fn eq(&self, other: &Self) -> bool {
        // NOTE: To channels are equal regardless of their aliases
        let equal_name = self.name == other.name;
        if !equal_name {
            return false;
        }

        let my_components: std::collections::HashSet<Component> =
            self.components.clone().into_iter().collect();

        let other_components: std::collections::HashSet<Component> =
            self.components.clone().into_iter().collect();

        let equal_components = other_components == my_components;

        if !equal_components {
            return false;
        }

        true
    }
}

#[derive(Serialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum ChannelAlias {
    /// Represents `stable`
    Stable,
    /// Represents either `nightly` or `nightly-$SUFFIX`
    Nightly(Option<Cow<'static, str>>),
    /// An ad-hoc named alias for a channel
    Tag(Cow<'static, str>),
}

impl<'de> serde::de::Deserialize<'de> for ChannelAlias {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Unexpected;
        use serde_untagged::UntaggedEnumVisitor;

        UntaggedEnumVisitor::new()
            .string(|s| {
                s.parse::<ChannelAlias>().map_err(|err| {
                    serde::de::Error::invalid_value(Unexpected::Str(s), &err.to_string().as_str())
                })
            })
            .deserialize(deserializer)
    }
}

impl core::str::FromStr for ChannelAlias {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "stable" => Ok(Self::Stable),
            "nightly" => Ok(Self::Nightly(None)),
            tag => match tag.strip_prefix("nightly-") {
                Some(suffix) => Ok(Self::Nightly(Some(Cow::Owned(suffix.to_string())))),
                None => Ok(Self::Tag(Cow::Owned(tag.to_string()))),
            },
        }
    }
}

/// An installable component of a toolchain
#[derive(Serialize, Deserialize, Debug, Clone, Hash, PartialEq, Eq)]
pub struct Component {
    /// The canonical name of this toolchain component
    pub name: Cow<'static, str>,
    /// The versioning authority for this component
    #[serde(flatten)]
    pub version: Authority,
    /// Optional features to enable, if applicable, when installing this component
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Other components that are required if this component is installed
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    /// If not None, then this component requires a specific toolchain to compile.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rustup_channel: Option<String>,

    /// This field is used for crates that install files whose name is different than that of the
    /// crate. For instance: miden-vm's executable is stored as 'miden'.
    /// NOTE: Check if this example still holds
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_file: Option<String>,
}

impl Component {
    pub fn new(name: impl Into<Cow<'static, str>>, version: Authority) -> Self {
        Self {
            name: name.into(),
            version,
            features: vec![],
            requires: vec![],
            rustup_channel: None,
            installed_file: None,
        }
    }
}

/// User-facing channel reference
#[derive(Serialize, Debug, Clone)]
#[serde(untagged, rename_all = "snake_case")]
pub enum UserChannel {
    // This variant is tried first, then stable, then nightly, then fallback
    Version(semver::Version),
    Stable,
    Nightly,
    Other(Cow<'static, str>),
}

impl fmt::Display for UserChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(version) => write!(f, "{version}"),
            Self::Stable => f.write_str("stable"),
            Self::Nightly => f.write_str("nightly"),
            Self::Other(custom_name) => write!(f, "{custom_name}"),
        }
    }
}

impl<'de> serde::de::Deserialize<'de> for UserChannel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Unexpected;
        use serde_untagged::UntaggedEnumVisitor;

        UntaggedEnumVisitor::new()
            .string(|s| {
                s.parse::<UserChannel>().map_err(|err| {
                    serde::de::Error::invalid_value(Unexpected::Str(s), &err.to_string().as_str())
                })
            })
            .deserialize(deserializer)
    }
}

impl core::str::FromStr for UserChannel {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use anyhow::anyhow;

        match s {
            "stable" => Ok(Self::Stable),
            "nightly" => Ok(Self::Nightly),
            version => semver::Version::parse(version)
                .map(Self::Version)
                .map_err(|err| anyhow!("invalid channel version: {err}")),
        }
    }
}
