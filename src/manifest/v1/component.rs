use std::{
    borrow::Cow,
    collections::BTreeMap,
    fmt,
    hash::Hash,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::authority::Authority as OldAuthority;
use crate::{
    config::Config,
    manifest::{Alias, ManifestError},
    utils,
    version::GitTarget,
};

#[derive(Deserialize, Default, Debug, Clone, Hash)]
#[serde(transparent)]
pub struct Artifacts {
    artifacts: Vec<String>,
}

/// An installable component of a toolchain
#[derive(Deserialize, Debug, Clone, Hash)]
pub struct Component {
    /// The canonical name of this toolchain component.
    pub name: Cow<'static, str>,
    /// The versioning authority for this component.
    #[serde(flatten)]
    pub version: OldAuthority,
    /// Indicates that this component is not required for a minimal toolchain
    #[serde(default)]
    pub optional: bool,
    /// Optional features to enable, if applicable, when installing this component.
    #[serde(default)]
    pub features: Vec<String>,
    /// Other components that are required if this component is installed.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Commands used to call the [Component]'s associated executable.
    ///
    /// IMPORTANT: This requires the [`Component::installed_file`] field to be an
    /// [`InstalledFile::Executable`] either explicitly or implicitly.
    #[serde(default)]
    call_format: Vec<CliCommand>,
    /// If not None, then this component requires a specific toolchain to compile.
    #[serde(default)]
    pub rustup_channel: Option<String>,
    /// This field is used for crates that install files whose name is different than that of the
    /// crate.
    ///
    /// For instance: `miden-vm`'s executable is stored as 'miden'. This field indicates which type
    /// of file the component will install.
    ///
    /// IMPORTANT: If this field is missing from the manifest, then it means that the component
    /// will install an executable that is named just like the crate. To access this value, use
    /// [`Component::get_installed_file`].
    #[serde(default)]
    #[serde(flatten)]
    installed_file: Option<InstalledFile>,
    /// A map that associates each alias to the corresponding command that needs to be executed.
    ///
    /// NOTE: The list of commands that is resolved can have an "arbitrary" ordering: the
    /// executable associated with this command is not forced to come in first.
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
    pub aliases: BTreeMap<Alias, CliCommands>,
    /// The file used by midenup's 'miden' to call the components executable.
    ///
    /// If `None`, then the component's file will be saved as `miden <name>`. This distinction
    /// exists mainly for components like `cargo-miden`, which differ in how they are called.
    #[serde(default)]
    symlink_name: Option<String>,
    #[serde(default)]
    pub initialization: Vec<String>,
    /// Pre-built artifact.
    #[serde(default)]
    pub artifacts: Artifacts,
}

impl TryFrom<Component> for crate::manifest::v2::Component {
    type Error = ManifestError;

    fn try_from(v1: Component) -> Result<Self, Self::Error> {
        use crate::profile::Profile;
        let Component {
            name,
            version,
            optional,
            features,
            requires,
            call_format,
            rustup_channel,
            installed_file,
            aliases,
            symlink_name,
            initialization,
            artifacts,
        } = v1;
        let crate_name = match &version {
            OldAuthority::Cargo { package, .. } => package.clone(),
            OldAuthority::Git { crate_name, .. } => Some(crate_name.clone()),
            OldAuthority::Path { crate_name, .. } => Some(crate_name.clone()),
        };
        let authority = crate::version::Authority::from(version);
        let mut profiles = vec![];
        if !optional {
            profiles.push(Profile::Minimal);
        }
        let mut v2artifacts = crate::artifact::Artifacts::default();
        for artifact in artifacts.artifacts {
            let artifact = match &authority {
                crate::version::Authority::Registry { version } => {
                    let version = version.to_string();
                    artifact.replace(&version, "%version")
                },
                _ => artifact,
            };
            let filename = artifact.split("/").last().unwrap_or(name.as_ref()).to_string();
            let filename = filename
                .strip_suffix("-aarch64-apple-darwin")
                .or_else(|| filename.strip_suffix("-x86_64-unknown-linux-gnu"))
                .unwrap_or(filename.as_str());
            if artifact.ends_with(".masp") {
                v2artifacts.insert(
                    filename.to_string(),
                    crate::artifact::Artifact::TargetAgnostic { uri: artifact, digest: None },
                );
            } else if artifact.contains("aarch64-apple-darwin") {
                let uri = artifact.replace("aarch64-apple-darwin", "%target");
                let artifact =
                    v2artifacts.artifacts.entry(filename.to_string()).or_insert_with(|| {
                        crate::artifact::Artifact::TargetSpecific {
                            uri,
                            substitutions: None,
                            targets: Default::default(),
                            digest: None,
                        }
                    });
                let crate::artifact::Artifact::TargetSpecific { targets, .. } = artifact else {
                    continue;
                };
                targets.insert("aarch64-apple-darwin".to_string(), Default::default());
            } else if artifact.contains("x86_64-unknown-linux-gnu") {
                let uri = artifact.replace("x86_64-unknown-linux-gnu", "%target");
                let artifact =
                    v2artifacts.artifacts.entry(filename.to_string()).or_insert_with(|| {
                        crate::artifact::Artifact::TargetSpecific {
                            uri,
                            substitutions: None,
                            targets: Default::default(),
                            digest: None,
                        }
                    });
                let crate::artifact::Artifact::TargetSpecific { targets, .. } = artifact else {
                    continue;
                };
                targets.insert("x86_64-unknown-linux-gnu".to_string(), Default::default());
            }
        }
        let kind = match installed_file {
            None => {
                let installation_method = if v2artifacts.is_empty() {
                    crate::manifest::InstallationMethod::Cargo {
                        crate_name: crate_name.unwrap_or_else(|| name.to_string()),
                        rustup_channel,
                        features,
                    }
                } else {
                    crate::manifest::InstallationMethod::Prebuilt
                };
                let call_format = if call_format.is_empty() {
                    None
                } else {
                    Some(
                        crate::exec::Executable::try_from(call_format)
                            .expect("invalid call_format"),
                    )
                };
                let initialization = if initialization.is_empty() {
                    None
                } else {
                    Some(
                        crate::exec::Executable::try_from(initialization)
                            .expect("invalid initialization format"),
                    )
                };
                let aliases = aliases
                    .into_iter()
                    .map(|(k, v)| {
                        (
                            k,
                            crate::exec::Executable::try_from(v)
                                .expect("invalid alias invocation format"),
                        )
                    })
                    .collect();
                let spec = crate::manifest::ExecutableComponent {
                    symlink_name,
                    installed_executable: name.to_string(),
                    call_format,
                    initialization,
                    aliases,
                    hide: false,
                };
                crate::manifest::ComponentKind::Executable { installation_method, spec }
            },
            Some(InstalledFile::Executable { binary_name, alias_only }) => {
                let installation_method = if v2artifacts.is_empty() {
                    crate::manifest::InstallationMethod::Cargo {
                        crate_name: crate_name.unwrap_or_else(|| name.to_string()),
                        rustup_channel,
                        features,
                    }
                } else {
                    crate::manifest::InstallationMethod::Prebuilt
                };
                let call_format = if call_format.is_empty() {
                    None
                } else {
                    Some(
                        crate::exec::Executable::try_from(call_format)
                            .expect("invalid call_format"),
                    )
                };
                let initialization = if initialization.is_empty() {
                    None
                } else {
                    Some(
                        crate::exec::Executable::try_from(initialization)
                            .expect("invalid initialization format"),
                    )
                };
                let aliases = aliases
                    .into_iter()
                    .map(|(k, v)| {
                        (
                            k,
                            crate::exec::Executable::try_from(v)
                                .expect("invalid alias invocation format"),
                        )
                    })
                    .collect();
                let spec = crate::manifest::ExecutableComponent {
                    symlink_name,
                    installed_executable: binary_name,
                    call_format,
                    initialization,
                    aliases,
                    hide: alias_only,
                };
                if alias_only {
                    crate::manifest::ComponentKind::CargoExtension { installation_method, spec }
                } else {
                    crate::manifest::ComponentKind::Executable { installation_method, spec }
                }
            },
            Some(InstalledFile::Library { library_struct, library_name: _ }) => {
                if v2artifacts.is_empty() {
                    crate::manifest::ComponentKind::LegacyPackage {
                        installation_method: crate::manifest::PackageInstallationMethod::Cargo {
                            crate_name: crate_name.unwrap_or_else(|| name.to_string()),
                            features,
                            extractor: format!("{library_struct}::default().as_ref()"),
                        },
                    }
                } else {
                    crate::manifest::ComponentKind::Package
                }
            },
        };

        Ok(crate::manifest::v2::Component {
            name,
            version: authority,
            kind,
            profiles,
            requires,
            extra: Default::default(),
            artifacts: v2artifacts,
        })
    }
}

impl Component {
    pub fn new(name: impl Into<Cow<'static, str>>, version: OldAuthority) -> Self {
        Self {
            name: name.into(),
            version,
            optional: false,
            features: vec![],
            requires: vec![],
            call_format: vec![],
            rustup_channel: None,
            installed_file: None,
            aliases: BTreeMap::new(),
            symlink_name: None,
            initialization: Vec::new(),
            artifacts: Default::default(),
        }
    }

    /// This method is used to check if the current [Component] is up to date with its  upstream
    /// equivalent.
    ///
    /// This is used to check if they different in fields _besides_ the name. The [`Component::eq`]
    /// implementation only tests name equality and is only used to check for components that got
    /// added/removed.
    ///
    /// WARNING: The idea behind this function is to early return when a
    /// difference is found, and fallback to "UpToDate" if none are
    /// found. Therefore, there should be *no* early returns that return
    /// `UpToDate`, since they might skip a field that differes later on.
    pub fn is_up_to_date(&self, upstream: &Self) -> bool {
        match (&self.version, &upstream.version) {
            (
                OldAuthority::Git {
                    repository_url: repository_url_a,
                    crate_name: crate_a,
                    subpath: subpath_a,
                    target:
                        GitTarget::Branch {
                            name: name_a,
                            latest_revision: local_revision,
                        },
                },
                OldAuthority::Git {
                    repository_url: repository_url_b,
                    crate_name: crate_b,
                    subpath: subpath_b,
                    target:
                        GitTarget::Branch {
                            name: name_b,
                            latest_revision: upstream_revision,
                        },
                },
            ) => {
                if repository_url_a != repository_url_b {
                    return false;
                }

                if crate_a != crate_b {
                    return false;
                }

                if subpath_a != subpath_b {
                    return false;
                }

                if repository_url_a != repository_url_b {
                    return false;
                }

                if name_a != name_b {
                    return false;
                }

                match (local_revision, upstream_revision) {
                    (Some(local_revision), Some(upstream_revision)) => {
                        if *local_revision != *upstream_revision {
                            return false;
                        }
                    },
                    // If either is missing, trigger an update regardless.
                    _ => {
                        return false;
                    },
                };
            },
            (
                OldAuthority::Path {
                    path: path_a,
                    crate_name: crate_name_a,
                    last_modification: last_modification_a,
                },
                OldAuthority::Path {
                    path: path_b,
                    crate_name: crate_name_b,
                    last_modification: last_modification_b,
                },
            ) => {
                if *path_a != *path_b {
                    return false;
                }
                if *crate_name_a != *crate_name_b {
                    return false;
                }

                match (last_modification_a, last_modification_b) {
                    (Some(local_latest), Some(new_latest)) => {
                        if new_latest > local_latest {
                            return false;
                        }
                    },
                    // If anything failed, we simply mark the component as needing an update.
                    // The idea being that components installed from a path are skipped during
                    // updates by default and are only updated if the user explicitly passes the
                    // necessary flags.
                    _ => return false,
                }
            },
            (
                OldAuthority::Cargo { package: package_a, version: version_a },
                OldAuthority::Cargo { package: package_b, version: version_b },
            ) => {
                if package_a != package_b {
                    return false;
                }

                if version_a != version_b {
                    return false;
                }
            },
            _ => {
                // This case includes all the cases where the Authorities differ,
                // which are never considered "up to date".
                return false;
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
    ///
    /// If the component does not specify the installed file name, that means that it installs and
    /// executable named exactly like the crate.
    pub fn get_installed_file(&self) -> InstalledFile {
        if let Some(installed_file) = &self.installed_file {
            installed_file.clone()
        } else {
            InstalledFile::Executable {
                binary_name: self.name.to_string(),
                // If not specified, all executable components are *not* alias_only
                alias_only: false,
            }
        }
    }

    pub fn set_installed_file(&mut self, installed_file: Option<InstalledFile>) {
        self.installed_file = installed_file;
    }

    /// Returns the string representation under which midenup calls a component.
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

    /// Returns the string representation under which midenup calls a component.
    pub fn get_call_format(&self) -> Vec<CliCommand> {
        if self.call_format.is_empty() {
            vec![CliCommand::Executable]
        } else {
            self.call_format.clone()
        }
    }

    // Sync to the latest changes.
    pub fn sync(&mut self, config: &Config) {
        match &mut self.version {
            OldAuthority::Path {
                path,
                crate_name: _crate_name,
                last_modification,
            } => {
                // If, for whatever reason, we fail to find the latest
                // registered modification, we simply leave it empty. That does
                // mean that an update will be triggered even if the component
                // does not need it.
                let path = if path.is_absolute() {
                    Cow::Borrowed(&*path)
                } else {
                    Cow::Owned(config.working_directory.join(&*path))
                };
                let latest_registered_modification =
                    utils::fs::latest_modification(&path).ok().map(|modification| modification.0);
                *last_modification = latest_registered_modification;
            },
            // NOTE: Components that are installed via git BRANCHES are a special case because we
            // need to check if new commits have been pushed since the component was installed.
            // When these components are installed, the lastest available commit hash is saved with
            // them in the local manifest. We use this to check if an update is in order. Do note
            // that the upstream manifest is not needed for these.
            OldAuthority::Git {
                repository_url,
                crate_name: _crate_name,
                subpath: _,
                target,
            } => {
                match target {
                    GitTarget::Branch { name: branch_name, latest_revision } => {
                        // If, for whatever reason, we fail to find the latest hash, we
                        // simply leave it empty. That does mean that an update will be
                        // triggered even if the component does not need it.
                        let latest_upstream_revision =
                            utils::git::find_latest_hash(repository_url.as_str(), branch_name).ok();

                        *latest_revision = latest_upstream_revision;
                    },
                    GitTarget::Revision { hash: _hash } => {},
                    GitTarget::Tag { name: _name } => {},
                }
            },
            OldAuthority::Cargo { package: _package, version: _version } => {},
        }
    }
}

/// Represents the file that the [Component] will install in the system.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum InstalledFile {
    /// The component installs an executable.
    #[serde(untagged)]
    Executable {
        #[serde(rename = "installed_executable")]
        binary_name: String,
        /// The component is executable (i.e. a CLI Binary) but it is *not* intended to be executed
        /// on its own, rather only under aliases.
        ///
        /// An example of this behavior is `cargo miden`, which is only intended to be executed
        /// under the `miden new` and `miden build` aliases.
        ///
        /// IMPORTANT: In order for alias_only to take effect, binary_name *must* also be specified
        /// in the manifest.
        #[serde(default)]
        alias_only: bool,
    },
    /// The component installs a MaspLibrary.
    #[serde(untagged)]
    Library {
        #[serde(rename = "installed_library")]
        library_name: String,
        /// This is the name of the struct which exposes the `Library::write_to_file()` function,
        /// that is used to generate the associated `.masp` file.
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

impl fmt::Display for InstalledFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            InstalledFile::Executable {
                binary_name: executable_name,
                alias_only: _,
            } => f.write_str(executable_name),
            InstalledFile::Library { library_name, library_struct: _ } => f.write_str(library_name),
        }
    }
}

/// Represents each possible "word" variant that is passed to the command line.
///
/// These are used to resolve an [Alias] to its associated command.
#[derive(Serialize, Deserialize, Debug, Clone, Hash)]
#[serde(rename_all = "snake_case")]
pub enum CliCommand {
    /// Resolve the command to a [Component]'s corresponding executable.
    Executable,
    /// Resolve the command to a toolchain library directory (`<toolchain>/lib`)
    #[serde(rename = "lib_path")]
    LibPath,
    /// Resolve the command to a toolchain var directory (`<toolchain>/var`).
    ///
    /// Optionally, it can contain a file name, which represents a file in `<toolchain>/var/<file>`.
    // NOTE: Potentially in the future, we might want this to be an Optional field
    #[serde(rename = "var_path")]
    VarPath,
    /// An argument that is passed verbatim, as is.
    #[serde(untagged)]
    Verbatim(String),
}

/// List of the commands that need to be run when [Alias] is called.
pub type CliCommands = Vec<CliCommand>;

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
