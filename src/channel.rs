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
    pub name: semver::Version,

    #[serde(skip_deserializing)]
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

    /// Is this channel the current version carrying the `stable` alias
    pub fn is_latest_stable(&self) -> bool {
        self.alias.as_ref().is_some_and(|alias| matches!(alias, ChannelAlias::Stable))
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
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged, rename_all = "snake_case")]
pub enum ChannelAlias {
    /// Represents `stable`
    Stable,
    /// Represents either `nightly` or `nightly-$SUFFIX`
    Nightly(Option<Cow<'static, str>>),
    /// An ad-hoc named alias for a channel
    Tag(Cow<'static, str>),
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

// /// The internal [Channel] representation.
// #[derive(Serialize, Debug, Clone)]
// #[serde(untagged, rename_all = "snake_case")]
// pub enum CanonicalChannel {
//     /// This channel represents the latest nightly versions of all components
//     Nightly,
//     /// This channel represents the latest stable versions of all components compatible with
//     /// the specified toolchain version string.
//     Version {
//         version: semver::Version,
//         #[serde(skip_serializing)]
//         is_stable: bool,
//     },
// }

// impl CanonicalChannel {
//     pub fn from_input(value: ChannelType, manifest: &Manifest) -> anyhow::Result<Self> {
//         match value {
//             ChannelType::Nightly => Ok(CanonicalChannel::Nightly),
//             ChannelType::Stable => {
//                 let stable = manifest
//                     .get_stable_version()
//                     .context("Failed to obtain stable version. No versions found")?;

//                 debug_assert!(matches!(
//                     &stable.name,
//                     &CanonicalChannel::Version { is_stable: true, .. }
//                 ));

//                 // NOTE: This gets cloned because semver::Version doesn't implement Copy
//                 Ok(stable.name.clone())
//             },
//             ChannelType::Version(version) => {
//                 Ok(CanonicalChannel::Version { version, is_stable: false })
//             },
//         }
//     }
// }

// impl fmt::Display for CanonicalChannel {
//     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
//         match self {
//             Self::Nightly => f.write_str("nightly"),
//             Self::Version { version, is_stable } => {
//                 if *is_stable {
//                     f.write_str("stable")
//                 } else {
//                     write!(f, "{version}")
//                 }
//             },
//         }
//     }
// }

// impl Eq for CanonicalChannel {}
// impl PartialEq for CanonicalChannel {
//     fn eq(&self, other: &Self) -> bool {
//         match (self, other) {
//             (Self::Nightly, Self::Nightly) => true,
//             (Self::Nightly, _) => false,
//             (_, Self::Nightly) => false,
//             (Self::Version { version: x, .. }, Self::Version { version: y, .. }) => x == y,
//         }
//     }
// }

// impl<'de> serde::de::Deserialize<'de> for CanonicalChannel {
//     fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
//     where
//         D: serde::Deserializer<'de>,
//     {
//         use serde::de::Unexpected;
//         use serde_untagged::UntaggedEnumVisitor;

//         UntaggedEnumVisitor::new()
//             .string(|s| {
//                 s.parse::<CanonicalChannel>().map_err(|err| {
//                     serde::de::Error::invalid_value(Unexpected::Str(s),
// &err.to_string().as_str())                 })
//             })
//             .deserialize(deserializer)
//     }
// }

// impl core::str::FromStr for CanonicalChannel {
//     type Err = anyhow::Error;

//     fn from_str(s: &str) -> Result<Self, Self::Err> {
//         use anyhow::anyhow;

//         // NOTE: Currently, when parsing from a str, all versions are marked as
//         // not stable, they are marked as stable after the entire Manifest is
//         // parsed
//         match s {
//             "nightly" => Ok(Self::Nightly),
//             version => semver::Version::parse(version)
//                 .map(|version| Self::Version { version, is_stable: false })
//                 .map_err(|err| anyhow!("invalid channel version: {err}")),
//         }
//     }
// }

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

// impl Eq for ChannelType {}
// impl PartialEq for ChannelType {
//     fn eq(&self, other: &Self) -> bool {
//         match (self, other) {
//             (Self::Stable, Self::Stable) => true,
//             (Self::Stable, _) => false,
//             (_, Self::Stable) => false,
//             (Self::Nightly, Self::Nightly) => true,
//             (Self::Nightly, _) => false,
//             (_, Self::Nightly) => false,
//             (Self::Version(x), Self::Version(y)) => x == y,
//         }
//     }
// }

// impl Ord for ChannelType {
//     fn cmp(&self, other: &Self) -> std::cmp::Ordering {
//         use std::cmp::Ordering;

//         match (self, other) {
//             (Self::Nightly, Self::Nightly) => Ordering::Equal,
//             (Self::Nightly, _) => Ordering::Greater,
//             (_, Self::Nightly) => Ordering::Less,
//             (Self::Stable, Self::Stable) => Ordering::Equal,
//             (Self::Stable, _) => Ordering::Greater,
//             (_, Self::Stable) => Ordering::Less,
//             (Self::Version(x), Self::Version(y)) => x.cmp_precedence(y),
//         }
//     }
// }

// impl PartialOrd for ChannelType {
//     fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
//         Some(self.cmp(other))
//     }
// }

// impl fmt::Display for ChannelType {
//     fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
//         match self {
//             Self::Stable => f.write_str("stable"),
//             Self::Nightly => f.write_str("nightly"),
//             Self::Version(version) => write!(f, "{version}"),
//         }
//     }
// }
