use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fmt::{self, Display},
    hash::{Hash, Hasher},
    path::PathBuf,
};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::{
    Config, utils,
    version::{Authority, GitTarget},
};

/// Represents a specific release channel for a toolchain.
///
/// Different channels have different stability guarantees. See the specific details for the
/// channel you are interested in to learn more.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Channel {
    /// Channels are identified by their name. The name corresponds to the
    /// channel's version.  The version can contain suffixes such as "-custom",
    /// "-beta".
    pub name: semver::Version,

    /// This is used to tag special channels. Most notably, the current "stable"
    /// channel is marked with the [ChannelAlias::Stable] alias.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<ChannelAlias>,

    /// The set of toolchain components available in this channel
    pub components: Vec<Component>,
}

impl Channel {
    pub fn new(
        name: semver::Version,
        alias: Option<ChannelAlias>,
        components: Vec<Component>,
    ) -> Self {
        Self { name, alias, components }
    }

    pub fn get_component(&self, name: impl AsRef<str>) -> Option<&Component> {
        let name = name.as_ref();
        self.components.iter().find(|c| c.name == name)
    }

    pub fn get_component_mut(&mut self, name: impl AsRef<str>) -> Option<&mut Component> {
        let name = name.as_ref();
        self.components.iter_mut().find(|c| c.name == name)
    }
    /// Is this channel a stable release? Does not imply that it has the
    /// `stable` alias.  To find out the latest stable [Channel], use:
    /// [Manifest::get_latest_stable].
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
    /// NOTE: A component can be marked for update in the following scenarios:
    /// - The component got removed from the newer channel entirely and thus needs to be removed
    ///   from the system.
    /// - A new component is present in the upstream manifest and thus needs to be installed.
    /// - A newer version of a present component is released and thus an upgrade is due.
    /// - An *older* version of a component is released and thus a downgrade is due.
    /// - A components [Authority] got changed and thus needs to be removed and re-installed with
    ///   the new [Authority]
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

        // These are the elements that are present in boths sets. We are only
        // interested in those which need updating.
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

    pub fn get_channel_dir(&self, config: &Config) -> PathBuf {
        let installed_toolchains_dir = config.midenup_home.join("toolchains");
        installed_toolchains_dir.join(format!("{}", self.name))
    }

    /// Get all the aliases that the Channel is aware of
    pub fn get_aliases(&self) -> HashMap<Alias, CLICommand> {
        self.components.iter().fold(HashMap::new(), |mut acc, component| {
            acc.extend(component.aliases.clone());
            acc
        })
    }
}

impl Eq for Component {}
/// NOTE: Two component are "partially equal" if their names are the
/// same. This does not mean that they're equal, since they could differ
/// in fields like versions.
/// This is implmented manually, in order to make use of HashSets with
/// components.
impl PartialEq for Component {
    fn eq(&self, other: &Self) -> bool {
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

impl Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.alias {
            Some(ChannelAlias::Stable) => write!(f, "Channel stable ({})", self.name),
            Some(ChannelAlias::Tag(tag)) => write!(f, "Channel {}-{}", self.name, tag.as_ref()),
            Some(ChannelAlias::Nightly(tag)) => {
                let nightly_suffix =
                    tag.as_ref().map(|suffix| format!("-{}", suffix)).unwrap_or(String::from(""));
                write!(f, "Nightly channel {}{}", self.name, nightly_suffix)
            },
            None => write!(f, "Channel {}", self.name),
        }
    }
}

/// A special alias/tag that a channel can posses. For more information see
/// [Channel::alias].
#[derive(Serialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum ChannelAlias {
    /// Represents `stable`. Only one [Channel] can be marked as `stable` at a
    /// time.
    Stable,
    /// Represents either `nightly` or `nightly-$SUFFIX`
    Nightly(Option<Cow<'static, str>>),
    /// An ad-hoc named alias for a channel. This can be used to tag custom
    /// channels with names such as `0.15.0-stable`.
    #[serde(untagged)]
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

/// Represents the file that the [[Component]] will install in the system.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstalledFile {
    /// The component installs an executable.
    #[serde(untagged)]
    Executable {
        #[serde(rename = "installed_executable")]
        binary_name: String,
    },
    /// The component installs a MaspLibrary.
    #[serde(untagged)]
    Library {
        #[serde(rename = "installed_library")]
        library_name: String,
        /// This is the struct that contains the library which and exposes the
        /// `Library::write_to_file()` method which is used to obtain the associated
        /// `.masp` file.
        library_struct: String,
    },
}

impl InstalledFile {
    pub fn get_library_struct(&self) -> Option<&str> {
        match &self {
            InstalledFile::Executable { .. } => None,
            InstalledFile::Library { library_struct, .. } => Some(library_struct),
        }
    }
}

impl Display for InstalledFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            InstalledFile::Executable { binary_name: executable_name } => {
                f.write_str(executable_name)
            },
            InstalledFile::Library { library_name, .. } => f.write_str(library_name),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
/// Represents each possible "word" variant that is passed to the Command
/// line. These are used to resolve an [[Alias]] to its associated command.
/// NOTE: In the manifest
pub enum CliCommand {
    /// Resolve the command to a [[Component]]'s corresponding executable.
    Executable,
    /// Resolve the command to a [[Toolchain]]'s library path (<toolchain>/lib)
    #[serde(rename = "lib_path")]
    LibPath,
    /// Resolve the command to a [[Toolchain]]'s etc path (<toolchain>/lib)
    /// Optionally, it can contain a file name.
    #[serde(rename = "etc_path")]
    EtcPath(Option<String>),
    /// An argument that is passed verbatim, as is.
    #[serde(untagged)]
    Verbatim(String),
}

impl CliCommand {
    pub fn resolve_command(
        &self,
        channel: &Channel,
        component: &Component,
        config: &Config,
    ) -> anyhow::Result<String> {
        match self {
            CliCommand::Executable => {
                let name = &component.name;
                let component = channel.get_component(name).with_context(|| {
                    format!(
                        "Component named {} is not present in toolchain version {}",
                        name, channel.name
                    )
                })?;

                Ok(component.get_cli_display())
            },
            CliCommand::LibPath => {
                let channel_dir = channel.get_channel_dir(config);

                let toolchain_path = channel_dir.join("lib");

                Ok(toolchain_path.to_string_lossy().to_string())
            },
            CliCommand::EtcPath(file) => {
                let channel_dir = channel.get_channel_dir(config);

                let toolchain_path = channel_dir.join("etc");
                let full_path = if let Some(file) = file {
                    toolchain_path.join(file)
                } else {
                    toolchain_path
                };

                Ok(full_path.to_string_lossy().to_string())
            },
            CliCommand::Verbatim(name) => Ok(name.to_string()),
        }
    }
}

pub type Alias = String;
/// List of the commands that need to be run when [[Alias]] is called.
pub type CLICommand = Vec<CliCommand>;
/// An installable component of a toolchain
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Component {
    /// The canonical name of this toolchain component.
    pub name: Cow<'static, str>,
    /// The versioning authority for this component.
    #[serde(flatten)]
    pub version: Authority,
    /// Optional features to enable, if applicable, when installing this
    /// component.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<String>,
    /// Other components that are required if this component is installed.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<String>,
    /// Commands used to call the [[Component]]'s associated executable.
    /// IMPORTANT: This requires the [[Component::installed_file]] field to be
    /// an [[InstalledFile::Executable]] either explicitly or implicitly.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    call_format: Vec<CliCommand>,
    /// If not None, then this component requires a specific toolchain to
    /// compile.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rustup_channel: Option<String>,
    /// This field is used for crates that install files whose name is different
    /// than that of the crate. For instance: miden-vm's executable is stored as
    /// 'miden'.
    /// This field indicates which type of file the component will install.
    /// IMPORTANT: If this field is missing from the manifest, then it means
    /// that the component will install an executable that is named just like
    /// the crate. To access this value, use [[Component::get_installed_file]].
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(flatten)]
    installed_file: Option<InstalledFile>,
    /// This HashMap associates each alias to the corresponding command that
    /// needs to be executed.
    /// NOTE: The list of commands that is resolved can have an "arbitrary"
    /// ordering: the executable associated with this command is not forced to
    /// come in first.
    ///
    /// Here's an example aliases entry in a manifest.json:
    ///
    /// ```json
    /// {
    ///   "name": "component-name",
    ///   "package": "component-package",
    ///   "version": "X.Y.Z",
    ///   "installed_executable": "miden-component",
    ///   "aliases": {
    ///       "alias1": [{"resolve": "component-name"}, {"verbatim": "argument"}],
    ///       "alias2": [{"verbatim": "cargo"}, {"resolve": "component-name"}, {"verbatim": "build"}]
    ///     }
    /// },
    /// ```
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub aliases: HashMap<Alias, CLICommand>,
    /// If the component requires initialization, this field holds the
    /// initialization subcommand(s).
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub initialization: Vec<String>,
}

impl Component {
    pub fn new(name: impl Into<Cow<'static, str>>, version: Authority) -> Self {
        Self {
            name: name.into(),
            version,
            features: vec![],
            requires: vec![],
            call_format: vec![],
            rustup_channel: None,
            installed_file: None,
            initialization: vec![],
            aliases: HashMap::new(),
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
                    utils::git::find_latest_hash(repository_url_b.as_str(), name_b).ok();

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
            (
                Authority::Path {
                    path: path_a,
                    crate_name: crate_name_a,
                    last_modification: last_modification_a,
                },
                Authority::Path {
                    path: path_b,
                    crate_name: crate_name_b,
                    last_modification: last_modification_b,
                },
            ) => {
                if path_a != path_b {
                    return false;
                }
                if crate_name_a != crate_name_b {
                    return false;
                }

                let local_latest = last_modification_a;

                let latest_registered_modification =
                    utils::fs::latest_modification(path_b).ok().map(|modification| {
                        // std::dbg!(&modification.1);
                        modification.0
                    });

                // last_modification_b should almost always be None, since the
                // latest modification time is checked on demand. However, if
                // for whatever reason, the manifest contains a latest
                // modification time, we honor it.
                let new_latest = last_modification_b.or(latest_registered_modification);

                match (local_latest, new_latest) {
                    (Some(local_latest), Some(new_latest)) => {
                        return new_latest <= *local_latest;
                    },
                    // If anything failed, we simply mark the component as
                    // needing an update.
                    // The idea being that components installed from a path are
                    // skipped during updates by default and are only updated if
                    // the user explicitly passes the necessary flags.
                    _ => return false,
                }
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

    /// Returns the name of the executable corresponding to this component.
    /// If the component does not specify the installed file name, that means
    /// that it installs and executable named exactly like the crate.
    pub fn get_installed_file(&self) -> InstalledFile {
        if let Some(installed_file) = &self.installed_file {
            installed_file.clone()
        } else {
            InstalledFile::Executable { binary_name: self.name.to_string() }
        }
    }

    /// Returns the String representation under which midenup calls a component.
    pub fn get_cli_display(&self) -> String {
        format!("miden {}", self.name)
    }

    /// Returns the String representation under which midenup calls a component.
    pub fn get_call_format(&self) -> Vec<CliCommand> {
        if self.call_format.is_empty() {
            vec![CliCommand::Executable]
        } else {
            self.call_format.clone()
        }
    }
}

/// User-facing channel reference. The main difference with this and [Channel]
/// is the definition of "stable". The definition of "stable" 'under the hood'
/// is the lastest available non-nightly channel. If the user passes
/// [UserChannel::Stable] as the target channel, we then handle the mapping from
/// it to the underlying [Channel] representation.
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

impl Display for UserChannel {
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
    use std::collections::HashMap;

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
                call_format: Vec::new(),
                rustup_channel: None,
                installed_file: None,
                initialization: vec![],
                aliases: HashMap::new(),
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
                call_format: Vec::new(),
                installed_file: None,
                initialization: vec![],
                aliases: HashMap::new(),
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
                call_format: Vec::new(),
                installed_file: None,
                initialization: vec![],
                aliases: HashMap::new(),
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
                call_format: Vec::new(),
                installed_file: None,
                initialization: vec![],
                aliases: HashMap::new(),
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
                call_format: Vec::new(),
                installed_file: None,
                initialization: vec![],
                aliases: HashMap::new(),
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
                call_format: Vec::new(),
                installed_file: None,
                initialization: vec![],
                aliases: HashMap::new(),
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
                call_format: Vec::new(),
                installed_file: None,
                initialization: vec![],
                aliases: HashMap::new(),
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
                call_format: Vec::new(),
                installed_file: None,
                initialization: vec![],
                aliases: HashMap::new(),
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
            name: std::borrow::Cow::Borrowed("client"),
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
            call_format: Vec::new(),
            rustup_channel: None,
            installed_file: None,
            initialization: vec![],
            aliases: HashMap::new(),
        }];

        let new_components = [Component {
            name: std::borrow::Cow::Borrowed("client"),
            version: Authority::Cargo {
                package: Some(String::from("miden-client-cli")),
                version: semver::Version::new(0, 15, 0),
            },
            features: Vec::new(),
            requires: Vec::new(),
            rustup_channel: None,
            call_format: Vec::new(),
            installed_file: None,
            initialization: vec![],
            aliases: HashMap::new(),
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
