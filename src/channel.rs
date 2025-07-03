use std::{
    borrow::Cow,
    collections::HashSet,
    fmt,
    hash::{Hash, Hasher},
};

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
        let new: HashSet<&Component> = HashSet::from_iter(newer.components.iter());
        let current: HashSet<&Component> = HashSet::from_iter(self.components.iter());

        // This is the subset of new components present in the channel since
        // last sync.
        let new_components = new.difference(&current);

        // This is the subset of old components that need to be removed.
        let old_components = current.difference(&new);

        // These are the elements that are present in boths sets. We need which
        // components need updating.
        // NOTE: Equality between components is done via their name, see
        // [Component::eq].
        let components_to_update = current.intersection(&new).filter(|current_component| {
            let new_component = new.get(*current_component);
            // We are only interested in component which are different
            if let Some(new_component) = new_component {
                !current_component.is_the_same(new_component)
            } else {
                // This should't be possible, but if somehow the component is
                // missing, then we trigger an update for said component
                // regardless.
                true
            }
        });

        let components = new_components
            .chain(old_components)
            .chain(components_to_update)
            .map(|c| (*c).clone());

        Vec::from_iter(components)
    }
}

impl Eq for Component {}
impl PartialEq for Component {
    fn eq(&self, other: &Self) -> bool {
        // NOTE: Two component are "partially equal" if their names are the
        // same. This does not mean that they're equal, since they could differ
        // in fields like versions.
        // This is implmented manually, in order to make use of HashSets with
        // components.
        self.name == other.name
    }
}
impl Hash for Component {
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        self.name.hash(state)
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
#[derive(Serialize, Deserialize, Debug, Clone)]
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

    /// NOTE: This method is used to check if two components share properties
    /// BESIDES the name. The [Component::eq] implementation (which only tests
    /// name equality) is used to comply with the std's requirements.
    pub fn is_the_same(&self, other: &Self) -> bool {
        if self.version != other.version {
            return false;
        }
        if self.features != other.features {
            return false;
        }
        if self.requires != other.requires {
            return false;
        }
        if self.rustup_channel != other.rustup_channel {
            return false;
        }
        if self.installed_file != other.installed_file {
            return false;
        }
        true
    }
}

/// User-facing channel reference
#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum UserChannel {
    Stable,
    Nightly,
    #[serde(untagged)]
    Version(semver::Version),
    #[serde(untagged)]
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

#[cfg(test)]
mod tests {
    use crate::{
        channel::{Channel, Component},
        version::Authority,
    };

    #[test]
    fn check_components_to_update() {
        let old_components = [
            Component {
                name: std::borrow::Cow::Borrowed("vm"),
                version: Authority::Cargo {
                    package: Some(String::from("miden-vm")),
                    version: semver::Version::new(0, 12, 0),
                },
                features: vec![String::from("executable"), String::from("concurrent")],
                requires: Vec::new(),
                rustup_channel: None,
                installed_file: None,
            },
            Component {
                name: std::borrow::Cow::Borrowed("std"),
                version: Authority::Cargo {
                    package: Some(String::from("miden-stdlib")),
                    version: semver::Version::new(0, 15, 0),
                },
                features: Vec::new(),
                requires: Vec::new(),
                rustup_channel: None,
                installed_file: None,
            },
            Component {
                name: std::borrow::Cow::Borrowed("removed-component"),
                // This should be present in the update vector
                // Component that got removed
                version: Authority::Cargo {
                    package: Some(String::from("deleted-repo")),
                    version: semver::Version::new(0, 82, 77),
                },
                features: Vec::new(),
                requires: Vec::new(),
                rustup_channel: None,
                installed_file: None,
            },
            Component {
                name: std::borrow::Cow::Borrowed("base"),
                version: Authority::Cargo {
                    package: Some(String::from("miden-lib")),
                    version: semver::Version::new(0, 9, 0),
                },
                features: Vec::new(),
                requires: Vec::new(),
                rustup_channel: None,
                installed_file: None,
            },
        ];

        let new_components = [
            Component {
                name: std::borrow::Cow::Borrowed("vm"),
                // This should be present in the update vector
                // Newer version
                version: Authority::Cargo {
                    package: Some(String::from("miden-vm")),
                    version: semver::Version::new(0, 15, 0),
                },
                features: vec![String::from("executable"), String::from("concurrent")],
                requires: Vec::new(),
                rustup_channel: None,
                installed_file: None,
            },
            Component {
                name: std::borrow::Cow::Borrowed("std"),
                // This should be present in the update vector
                // Newer version
                version: Authority::Cargo {
                    package: Some(String::from("miden-stdlib")),
                    version: semver::Version::new(0, 16, 0),
                },
                features: Vec::new(),
                requires: Vec::new(),
                rustup_channel: None,
                installed_file: None,
            },
            Component {
                name: std::borrow::Cow::Borrowed("new-component"),
                // This should be present in the update vector
                // New component all together.
                version: Authority::Cargo {
                    package: Some(String::from("new-repo")),
                    version: semver::Version::new(78, 69, 87),
                },
                features: Vec::new(),
                requires: Vec::new(),
                rustup_channel: None,
                installed_file: None,
            },
            Component {
                name: std::borrow::Cow::Borrowed("base"),
                version: Authority::Cargo {
                    package: Some(String::from("miden-lib")),
                    version: semver::Version::new(0, 9, 0),
                },
                features: Vec::new(),
                requires: Vec::new(),
                rustup_channel: None,
                installed_file: None,
            },
        ];

        let old = Channel {
            name: semver::Version::new(0, 0, 1),
            alias: None,
            components: old_components.to_vec(),
        };

        let new = Channel {
            name: semver::Version::new(0, 0, 2),
            alias: None,
            components: new_components.to_vec(),
        };

        let components = old.components_to_update(&new);

        // To see how many elements and a brief explanation regarding why, run
        // the following command from the project root:
        // grep "This should be present in the update vector" src/channel.rs -A 1 -B 1
        // Sidenote: This line will also appear

        assert_eq!(components.len(), 4);
        assert!(components.iter().any(|c| c.name == "vm"));
        assert!(components.iter().any(|c| c.name == "removed-component"));
        assert!(components.iter().any(|c| c.name == "std"));
        assert!(components.iter().any(|c| c.name == "new-component"));
    }
}
