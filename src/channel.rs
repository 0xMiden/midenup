use std::{borrow::Cow, fmt};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::{manifest::Manifest, version::Authority};

/// Represents a specific release channel for a toolchain.
///
/// Different channels have different stability guarantees. See the specific details for the
/// channel you are interested in to learn more.
#[derive(Serialize, Deserialize, Debug)]
pub struct Channel {
    /// The name of the channel
    pub name: CanonicalChannel,
    /// The set of toolchain components available in this channel
    pub components: Vec<Component>,
}

impl Channel {
    pub fn get_component(&self, name: impl AsRef<str>) -> Option<&Component> {
        let name = name.as_ref();
        self.components.iter().find(|c| c.name == name)
    }
}

/// An installable component of a toolchain
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Component {
    /// The canonical name of this toolchain component
    pub name: Cow<'static, str>,
    /// The versioning authority for this component
    #[serde(flatten)]
    pub version: Authority,
    /// Optional features to enable, if applicable, when installing this component
    #[serde(default)]
    pub features: Vec<String>,
    /// Other components that are required if this component is installed
    #[serde(default)]
    pub requires: Vec<String>,
}

impl Component {
    pub fn new(name: impl Into<Cow<'static, str>>, version: Authority) -> Self {
        Self {
            name: name.into(),
            version,
            features: vec![],
            requires: vec![],
        }
    }
}

/// The version/stability guarantee of a [Channel]
#[derive(Serialize, Debug, Clone)]
#[serde(untagged, rename_all = "snake_case")]
pub enum CanonicalChannel {
    /// This channel represents the latest nightly versions of all components
    Nightly,
    /// This channel represents the latest stable versions of all components compatible with
    /// the specified toolchain version string.
    Version {
        version: semver::Version,
        #[serde(skip_serializing)]
        is_stable: bool,
    },
}

impl CanonicalChannel {
    // TODO: Change this to try_from considering get_stable_version could be empty
    pub fn from_input(value: ChannelType, manifest: &Manifest) -> anyhow::Result<Self> {
        match value {
            ChannelType::Nightly => Ok(CanonicalChannel::Nightly),
            ChannelType::Stable => {
                let version = manifest
                    .get_stable_version()
                    .context("Failed to obtain stable version. No versions found")?
                    .clone();
                Ok(CanonicalChannel::Version { version, is_stable: true })
            },
            ChannelType::Version(version) => {
                Ok(CanonicalChannel::Version { version, is_stable: false })
            },
        }
    }
}

impl fmt::Display for CanonicalChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nightly => f.write_str("nightly"),
            Self::Version { version, is_stable } => {
                if *is_stable {
                    f.write_str("stable")
                } else {
                    write!(f, "{version}")
                }
            },
        }
    }
}

impl Eq for CanonicalChannel {}
impl PartialEq for CanonicalChannel {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Nightly, Self::Nightly) => true,
            (Self::Nightly, _) => false,
            (_, Self::Nightly) => false,
            (Self::Version { version: x, .. }, Self::Version { version: y, .. }) => x == y,
        }
    }
}

impl Ord for CanonicalChannel {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self, other) {
            (Self::Nightly, Self::Nightly) => Ordering::Equal,
            (Self::Nightly, _) => Ordering::Greater,
            (_, Self::Nightly) => Ordering::Less,
            (Self::Version { version: x, .. }, Self::Version { version: y, .. }) => {
                x.cmp_precedence(y)
            },
        }
    }
}
impl PartialOrd for CanonicalChannel {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// The version/stability guarantee of a [Channel]
#[derive(Serialize, Debug, Clone)]
#[serde(untagged, rename_all = "snake_case")]
pub enum ChannelType {
    /// This channel represents the latest stable versions of all components
    Stable,
    /// This channel represents the latest nightly versions of all components
    Nightly,
    /// This channel represents the latest stable versions of all components compatible with
    /// the specified toolchain version string.
    Version(semver::Version),
}

impl<'de> serde::de::Deserialize<'de> for CanonicalChannel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Unexpected;
        use serde_untagged::UntaggedEnumVisitor;

        UntaggedEnumVisitor::new()
            .string(|s| {
                s.parse::<CanonicalChannel>().map_err(|err| {
                    serde::de::Error::invalid_value(Unexpected::Str(s), &err.to_string().as_str())
                })
            })
            .deserialize(deserializer)
    }
}

impl core::str::FromStr for CanonicalChannel {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use anyhow::anyhow;

        match s {
            "nightly" => Ok(Self::Nightly),
            version => semver::Version::parse(version)
                .map(|version| Self::Version { version, is_stable: false })
                .map_err(|err| anyhow!("invalid channel version: {err}")),
        }
    }
}

impl core::str::FromStr for ChannelType {
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

impl Eq for ChannelType {}
impl PartialEq for ChannelType {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Stable, Self::Stable) => true,
            (Self::Stable, _) => false,
            (_, Self::Stable) => false,
            (Self::Nightly, Self::Nightly) => true,
            (Self::Nightly, _) => false,
            (_, Self::Nightly) => false,
            (Self::Version(x), Self::Version(y)) => x == y,
        }
    }
}

impl Ord for ChannelType {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;

        match (self, other) {
            (Self::Nightly, Self::Nightly) => Ordering::Equal,
            (Self::Nightly, _) => Ordering::Greater,
            (_, Self::Nightly) => Ordering::Less,
            (Self::Stable, Self::Stable) => Ordering::Equal,
            (Self::Stable, _) => Ordering::Greater,
            (_, Self::Stable) => Ordering::Less,
            (Self::Version(x), Self::Version(y)) => x.cmp_precedence(y),
        }
    }
}

impl PartialOrd for ChannelType {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for ChannelType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stable => f.write_str("stable"),
            Self::Nightly => f.write_str("nightly"),
            Self::Version(version) => write!(f, "{version}"),
        }
    }
}
