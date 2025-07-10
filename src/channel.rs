use std::{
    borrow::Cow,
    collections::HashSet,
    fmt,
    hash::{Hash, Hasher},
};

use serde::{Deserialize, Serialize};

use crate::{
    utils,
    version::{Authority, GitTarget},
};

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

    /// This functions compares the Channel &self, with a newer channel [newer]
    /// and returns the list of [Components] that need to be updated.
    pub fn components_to_update(&self, newer: &Self) -> Vec<Component> {
        let new_channel: HashSet<&Component> = HashSet::from_iter(newer.components.iter());
        let current = HashSet::from_iter(self.components.iter());

        // This is the subset of new components present in the channel since
        // last sync.
        // NOTE: Equality between components is done via their name, see
        // [Component::eq].
        let new_components = new_channel.difference(&current);

        // This is the subset of old components that need to be removed.
        let old_components = current.difference(&new_channel);

        // These are the elements that are present in boths sets. We need which
        // components need updating.
        let components_to_update = current.intersection(&new_channel).filter(|current_component| {
            let new_component = new_channel.get(*current_component);
            if let Some(new_component) = new_component {
                // We only want to update components that share the same name but
                // differ in some other field.
                !current_component.is_up_to_date(new_component)
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

    /// NOTE: This method is used to check if the current Component is up to
    /// date with its upstream equivalent. This is used to check if they
    /// different in fields BESIDES the name. The [Component::eq] implementation
    /// only tests name equality and is only used to check for components that
    /// got added/removed.
    pub fn is_up_to_date(&self, upstream: &Self) -> bool {
        match (&self.version, &upstream.version) {
            // NOTE: Components that are installed via git BRANCHES are a special
            // case because we need to check if new commits have been pushed since
            // the component was installed.  When these components are installed,
            // the lastest available commit hash is saved with them in the local
            // manifest. We use this to check if an update is in order.
            // Do note that the upstream manifest is not needed for these.
            (
                Authority::Git {
                    repository_url: repository_url_a,
                    target:
                        GitTarget::Branch {
                            name: name_a,
                            latest_revision: local_revision,
                        },
                    ..
                },
                Authority::Git {
                    repository_url: repository_url_b,
                    target: GitTarget::Branch { name: name_b, .. },
                    ..
                },
            ) => {
                if name_a != name_b {
                    return false;
                }
                if repository_url_a != repository_url_b {
                    return false;
                }

                // If, for whatever reason, we fail to find the latest hash,
                // we simply leave it empty. That does mean that an update
                // will be triggered even if the component does not need it.
                let latest_upstream_revision =
                    utils::find_latest_hash(repository_url_b.as_str(), name_b).ok();

                match (local_revision, latest_upstream_revision) {
                    (Some(local_revision), Some(upstream_revision)) => {
                        if *local_revision != upstream_revision {
                            return false;
                        }
                    },
                    // If either is missing, trigger an update regardless.
                    _ => {
                        return false;
                    },
                };

                return true;
            },
            (version_a, version_b) => {
                if version_a != version_b {
                    return false;
                }
            },
        };

        if self.features != upstream.features {
            return false;
        }

        if self.requires != upstream.requires {
            return false;
        }

        if self.rustup_channel != upstream.rustup_channel {
            return false;
        }

        if self.installed_file != upstream.installed_file {
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
        version::{Authority, GitTarget},
    };

    #[test]
    /// This tests checks that the [Channel::components_to_update] functions behaves as intended.
    /// Here the following updates need to be performed:
    /// - vm requires update 0.12.0 -> 0.15.0
    /// - std requires downgrade from 0.15.0 -> 0.12.0
    /// - a so called "removed-component" needs to be deleted
    /// - a so called "new-component" needs to be added
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
                version: Authority::Cargo {
                    package: Some(String::from("miden-stdlib")),
                    version: semver::Version::new(0, 12, 0),
                },
                features: Vec::new(),
                requires: Vec::new(),
                rustup_channel: None,
                installed_file: None,
            },
            Component {
                name: std::borrow::Cow::Borrowed("new-component"),
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

        assert_eq!(components.len(), 4);
        assert!(components.iter().any(|c| c.name == "vm"));
        assert!(components.iter().any(|c| c.name == "removed-component"));
        assert!(components.iter().any(|c| c.name == "std"));
        assert!(components.iter().any(|c| c.name == "new-component"));
    }

    #[test]
    /// Since the components that are tracked via git branches need special
    /// treatment, we need to check that their behavior complies even if their
    /// Authority changes.
    fn update_component_from_git_to_cargo() {
        let old_components = [Component {
            name: std::borrow::Cow::Borrowed("miden-client"),
            version: Authority::Git {
                repository_url: String::from("https://github.com/0xMiden/miden-client.git"),
                crate_name: String::from("miden-client-cli"),
                target: GitTarget::Branch {
                    name: String::from("main"),
                    latest_revision: None,
                },
            },
            features: Vec::new(),
            requires: Vec::new(),
            rustup_channel: None,
            installed_file: None,
        }];

        let new_components = [Component {
            name: std::borrow::Cow::Borrowed("miden-client"),
            version: Authority::Cargo {
                package: Some(String::from("miden-client-cli")),
                version: semver::Version::new(0, 15, 0),
            },
            features: Vec::new(),
            requires: Vec::new(),
            rustup_channel: None,
            installed_file: None,
        }];

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

        assert_eq!(components.len(), 1);
    }
}
