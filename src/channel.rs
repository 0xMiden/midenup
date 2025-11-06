use std::{
    borrow::Cow,
    collections::HashMap,
    fmt::{self, Display},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::{
    Config,
    artifact::{Artifacts, TargetTriple, TargetTripleError},
    toolchain::{Toolchain, ToolchainJustification},
    utils,
    version::{Authority, GitTarget},
};

/// Tags used to identify special qualities of a specific channel.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum Tags {
    /// The channel is partially installed, i.e. only a subset of components
    /// have been installed.
    Partial,
}

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

    /// Set of tags used to denote a special characteristic about the channel.
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
impl Display for InstallationMotive {
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

    /// Determines if the current toolchain was installed "partially", i.e.,
    /// containing only a subset of all the available components. This can be the
    /// case with `miden-toolchain.toml`.
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

    /// Get all the aliases that the Channel is aware of
    pub fn get_aliases(&self) -> HashMap<Alias, CLICommand> {
        self.components.iter().fold(HashMap::new(), |mut acc, component| {
            acc.extend(component.aliases.clone());
            acc
        })
    }

    /// Creates a "partial channel" from the original channel, given a toolchain
    /// "Partial" in this context refers to the fact that the channel will not
    /// install all the available components, but rather a subset.
    pub fn create_subset(
        &self,
        current_toolchain: &Toolchain,
        toolchain_justification: &ToolchainJustification,
    ) -> Option<Channel> {
        if current_toolchain.components.is_empty() {
            return None;
        }
        let mut components_to_install: Vec<Component> = Vec::new();

        let mut components_not_found: HashMap<String, Vec<InstallationMotive>> = HashMap::new();

        for component_name in current_toolchain.components.iter() {
            let Some(component) = self.get_component(component_name) else {
                // NOTE: In order to provide more helpful error messages, we
                // collect all the missing components and return a single error
                // message at the end.
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

                components_to_install.push(dependency.clone());
            }
        }
        if !components_not_found.is_empty() {
            println!(
                "{}: Some elements present in the current Toolchain are not present in the upstream channel: {}",
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
                    "Check the `miden_toolchain.toml` file in {} to see if any \
                         component is misspelled or got removed from upstream",
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
        /// This is the name of the struct which exposes the
        /// `Library::write_to_file()` function, that is used to generate the
        /// associated `.masp` file.
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
    pub fn get_path_from(&self, toolchain_dir: &Path) -> PathBuf {
        match &self {
            exe @ InstalledFile::Executable { .. } => {
                toolchain_dir.join("bin").join(exe.to_string())
            },
            lib @ InstalledFile::Library { .. } => toolchain_dir.join("lib").join(lib.to_string()),
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
    /// Resolve the command to a [[Toolchain]]'s var directory (<toolchain>/var).
    /// Optionally, it can contain a file name, which represents a file in
    /// <toolchain>/var/<file>.
    // NOTE: Potentially in the future, we might want this to be an Optional field
    #[serde(rename = "var_path")]
    VarPath,
    /// An argument that is passed verbatim, as is.
    #[serde(untagged)]
    Verbatim(String),
}

impl fmt::Display for CliCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            CliCommand::Executable => write!(f, "executable"),
            CliCommand::LibPath => write!(f, "lib_path"),
            CliCommand::VarPath => write!(f, "var_path"),
            CliCommand::Verbatim(word) => write!(f, "verbatim: {word}"),
        }
    }
}

pub fn resolve_command(
    commands: &[CliCommand],
    channel: &Channel,
    component: &Component,

    config: &Config,
) -> anyhow::Result<Vec<String>> {
    let mut resolution = Vec::with_capacity(commands.len());
    let mut commands = commands.iter();

    while let Some(command) = commands.next() {
        match command {
            CliCommand::Executable => {
                let name = &component.name;
                let component = channel.get_component(name).with_context(|| {
                    format!(
                        "Component named {} is not present in toolchain version {}",
                        name, channel.name
                    )
                })?;

                resolution.push(component.get_cli_display());
            },
            CliCommand::LibPath => {
                let channel_dir = channel.get_channel_dir(config);

                let toolchain_path = channel_dir.join("lib");

                resolution.push(toolchain_path.to_string_lossy().to_string())
            },
            // The VarPath must be followed by a file name.
            CliCommand::VarPath => {
                let channel_dir = channel.get_channel_dir(config);

                let toolchain_path = channel_dir.join("var");

                let next_command =
                    commands.next().context("var_path needs to be followed by a path name")?;

                let CliCommand::Verbatim(directory_name) = next_command else {
                    bail!(format!("After var_path a file is required. Got: {}", next_command))
                };

                let full_path = toolchain_path.join(directory_name);

                resolution.push(full_path.to_string_lossy().to_string())
            },
            CliCommand::Verbatim(name) => resolution.push(name.to_string()),
        }
    }

    Ok(resolution)
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
    /// The file used by midenup's 'miden' to call the components executable.
    /// If None, then the component's file will be saved as 'miden <name>'.
    /// This distinction exists mainly for components like cargo-miden, which
    /// differ in how they are called.
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    symlink_name: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub initialization: Vec<String>,
    /// Pre-built artifact.
    #[serde(flatten)]
    artifacts: Option<Artifacts>,
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
            aliases: HashMap::new(),
            symlink_name: None,
            initialization: Vec::new(),
            artifacts: None,
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

    /// Returns the name of symlink associated with a component.
    pub fn get_symlink_name(&self) -> String {
        if let Some(symlink_name) = &self.symlink_name {
            symlink_name.clone()
        } else {
            format!("miden {}", self.name)
        }
    }

    /// Returns the String representation under which midenup calls a component.
    pub fn get_call_format(&self) -> Vec<CliCommand> {
        if self.call_format.is_empty() {
            vec![CliCommand::Executable]
        } else {
            self.call_format.clone()
        }
    }

    /// Returns the URI for a given [target] (if available).
    pub fn get_uri_for(&self, target: TargetTriple2) -> Result<String, Vec<TargetTripleError>> {
        self.artifacts
            .as_ref()
            .and_then(|artifacts| artifacts.get_uri_for(&target, &self.name))
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
