use std::{borrow::Cow, collections::BTreeMap, hash::Hash, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::{
    artifact::Artifacts,
    config::Config,
    exec::Executable,
    manifest::{Alias, ManifestError, v2::unknown::Extra},
    profile::Profile,
    utils,
    version::{Authority, GitTarget},
};

/// An installable component of a toolchain
#[derive(Debug, Clone)]
pub struct Component {
    /// The canonical name of this toolchain component.
    pub name: Cow<'static, str>,
    /// The versioning authority for this component.
    pub version: Authority,
    /// The component kind
    pub kind: ComponentKind,
    /// The set of profiles this component is included in
    pub profiles: Vec<Profile>,
    /// Other components that are required if this component is installed.
    pub requires: Vec<String>,
    /// Pre-built artifacts for this component, if available.
    pub artifacts: Artifacts,
    /// Fields declared by a newer schema that this build does not recognize.
    ///
    /// Preserved verbatim so an older `midenup` rewriting a manifest -- `update-manifest`, most
    /// importantly -- cannot silently strip them.
    pub extra: Extra,
}

/// The derivable part of [Component].
///
/// [Component] cannot simply add `#[serde(flatten)] extra` next to its flattened `kind`: a
/// catch-all flatten alongside another flatten also captures the keys that flatten consumed, and
/// serialization then emits them twice. So the typed fields are derived here and [Component]'s own
/// `Serialize`/`Deserialize` compose this with the extras.
#[derive(Serialize, Deserialize, Debug, Clone, Hash)]
struct ComponentBase {
    name: Cow<'static, str>,
    version: Authority,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    profiles: Vec<Profile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    requires: Vec<String>,
    #[serde(default, skip_serializing_if = "Artifacts::is_empty")]
    artifacts: Artifacts,
}

impl Component {
    fn base(&self) -> ComponentBase {
        ComponentBase {
            name: self.name.clone(),
            version: self.version.clone(),
            profiles: self.profiles.clone(),
            requires: self.requires.clone(),
            artifacts: self.artifacts.clone(),
        }
    }
}

/// Hashes everything except [Component::extra].
///
/// Unknown fields cannot be known to affect installed files, and a hash over them would make
/// otherwise-identical components compare unequal purely because one was authored by a newer
/// publisher. `serde_json::Value` does not implement `Hash` either.
impl Hash for Component {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.base().hash(state);
        self.kind.hash(state);
    }
}

impl Serialize for Component {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;

        let mut out = serde_json::to_value(self.base()).map_err(S::Error::custom)?;
        let kind = serde_json::to_value(&self.kind).map_err(S::Error::custom)?;

        let object = out.as_object_mut().expect("ComponentBase serializes to an object");
        if let serde_json::Value::Object(kind) = kind {
            object.extend(kind);
        }
        for (key, value) in self.extra.iter() {
            object.entry(key.clone()).or_insert_with(|| value.clone());
        }

        out.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Component {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;

        let value = serde_json::Value::deserialize(deserializer)?;

        let kind = ComponentKind::deserialize(value.clone()).map_err(D::Error::custom)?;
        let base = ComponentBase::deserialize(value.clone()).map_err(D::Error::custom)?;

        // Anything neither the base nor the kind round-trips is unknown to this build.
        let mut extra = match &value {
            serde_json::Value::Object(map) => map.clone(),
            _ => Extra::new(),
        };
        for known in [
            serde_json::to_value(&base).map_err(D::Error::custom)?,
            serde_json::to_value(&kind).map_err(D::Error::custom)?,
        ] {
            if let serde_json::Value::Object(known) = known {
                for key in known.keys() {
                    extra.remove(key);
                }
            }
        }

        Ok(Component {
            name: base.name,
            version: base.version,
            kind,
            profiles: base.profiles,
            requires: base.requires,
            artifacts: base.artifacts,
            extra,
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ComponentKind {
    /// A component derived from an executable artifact
    Executable {
        /// How the executable artifact should be installed
        installation_method: InstallationMethod,
        /// The generic details of how this executable component is installed/executed
        #[serde(flatten)]
        spec: ExecutableComponent,
    },
    /// An executable component that is invoked via `cargo`, rather than directly
    CargoExtension {
        /// How the Cargo extension should be installed
        installation_method: InstallationMethod,
        /// The generic details of how this executable component is installed/executed
        #[serde(flatten)]
        spec: ExecutableComponent,
    },
    /// A virtual component that defines a `miden` command.
    ///
    /// This component kind is like `Executable`, but its associated artifact, if any, is not
    /// itself executable. Instead, these components express their execution semantics in terms of
    /// system-managed software, or other `midenup`-managed components/assets.
    ///
    /// ## Example
    ///
    /// The following is what the node component looks like expressed as this kind of component:
    ///
    /// ```json
    /// {
    ///     "kind": "command",
    ///     "format": [
    ///         "docker",
    ///         "compose",
    ///         "-f", "%etc(node/docker-compose.yml)",
    ///         "-f", "%etc(node/telemetry.yml)",
    ///         "-f", "%etc(node/monitor.yml)",
    ///     ],
    ///     "subcommands": {
    ///         "up": ["up", "-d"]
    ///         "down": ["down", "--remove-orphans"]
    ///         "delete": ["down", "-v", "--remove-orphans"]
    ///         "logs": ["logs", "-f"]
    ///     },
    /// }
    /// ```
    Command {
        /// If set, the command is invoked as `miden <name>` rather than `miden <component>`
        #[serde(default)]
        #[serde(skip_serializing_if = "Option::is_none")]
        command_name: Option<String>,
        /// If set, this specifies what `miden <component>` translates to when called
        ///
        /// This MUST be set if `aliases` is empty, i.e. it is not valid to define a virtual
        /// component when no means to invoke it.
        #[serde(default)]
        #[serde(skip_serializing_if = "Executable::is_empty")]
        format: Executable,
        /// Defines logical subcommands of this command.
        ///
        /// If non-empty, then the first user-provided argument passed to the command will be
        /// resolved against this list, and an error will be raised if an invalid subcommand is
        /// named. The expansion of the subcommand will be appended to `format`, followed by any
        /// remaining user-provided arguments.
        ///
        /// If no subcommands are defined, then all user-defined arguments are appended to `format`
        #[serde(default)]
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        subcommands: BTreeMap<Alias, Executable>,
        /// A map of `miden` aliases defined by this component
        ///
        /// This allows defining extra top-level `miden` command aliases as a logical part of this
        /// component, similar to other executable component kinds.
        #[serde(default)]
        #[serde(skip_serializing_if = "BTreeMap::is_empty")]
        aliases: BTreeMap<Alias, Executable>,
    },
    /// A logical set of one or more Miden packages
    ///
    /// This component kind requires prebuilt artifacts for all packages
    Package,
    /// Legacy support for packages which required extraction from a Rust crate
    LegacyPackage {
        /// How the package artifact will be installed
        installation_method: PackageInstallationMethod,
    },
    /// An asset that will be installed to the toolchain's `etc` directory
    Asset {
        /// If true, then the asset is a compressed file that needs to be extracted upon download
        #[serde(default)]
        compressed: bool,
    },
}

impl FromStr for ComponentKind {
    type Err = ManifestError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map_err(|err| ManifestError::Invalid(err.to_string()))
    }
}

impl ComponentKind {
    pub fn is_callable(&self) -> bool {
        matches!(
            self,
            Self::CargoExtension { .. } | Self::Executable { .. } | Self::Command { .. }
        )
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub struct ExecutableComponent {
    /// An [Executable] that represents how this component should be invoked when called direct,
    /// if specific arguments are required in addition to whatever the user may provide.
    ///
    /// If empty, it is presumed that the component can be invoked without any additional args,
    /// unless `hide` is set to true, in which case direct invocation is disabled entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) call_format: Option<Executable>,
    /// The command used to initialize this component, if applicable
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) initialization: Option<Executable>,
    /// A map of [Executable] aliases defined by this component
    ///
    /// NOTE: These executables do not have to invoke the component binary itself.
    ///
    /// ## Example
    ///
    /// ```json
    /// {
    ///   "name": "component-name",
    ///   "version": {
    ///       "kind": "cargo",
    ///       "version": "X.Y.Z",
    ///   },
    ///   "kind": "executable-crate",
    ///   "crate-name": "component-package",
    ///   "installed-executable": "miden-component",
    ///   "aliases": {
    ///       "alias1": ["%installed-executable", "argument"],
    ///       "alias2": ["cargo", "component-name"]
    ///     }
    ///   },
    /// },
    /// ```
    #[serde(default)]
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) aliases: BTreeMap<Alias, Executable>,
    /// The file used by midenup's 'miden' to call the components executable.
    ///
    /// If `None`, then the component's file will be saved as `miden <name>`. This distinction
    /// exists mainly for components like `cargo-miden`, which differ in how they are called.
    ///
    /// A symlink is never created for executables for which `hide` is `true`
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) symlink_name: Option<String>,
    /// The name of the executable binary that will be installed
    pub(crate) installed_executable: String,
    /// Prevent this component from being directly executed via the `miden` CLI
    ///
    /// This should be used when the component is either:
    ///
    /// 1. Executable, but should only be executed via its defined aliases
    /// 2. Not directly executable, but provides executable aliases
    ///
    /// An example of this behavior is `cargo miden`, which is only intended to be executed
    /// via the `miden new` alias.
    ///
    /// NOTE: If this is set, at least one entry in `aliases` must be present
    #[serde(default)]
    pub(crate) hide: bool,
}

impl ExecutableComponent {
    #[inline(always)]
    pub fn is_hidden(&self) -> bool {
        self.hide
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum InstallationMethod {
    /// Requires prebuilt artifacts to be defined for the component
    Prebuilt,
    /// Use prebuilt artifacts where possible, but fall back to `cargo install` if unavailable
    PrebuiltWithCargoFallback {
        /// The name of the crate
        crate_name: String,
        /// If not None, then this component requires a specific toolchain to compile.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rustup_channel: Option<String>,
        /// Optional features to enable, if applicable, when installing this component.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        features: Vec<String>,
    },
    /// Installation is performed via `cargo install` to the current toolchain
    Cargo {
        /// The name of the crate
        crate_name: String,
        /// If not None, then this component requires a specific toolchain to compile.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rustup_channel: Option<String>,
        /// Optional features to enable, if applicable, when installing this component.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        features: Vec<String>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PackageInstallationMethod {
    /// Requires prebuilt artifacts to be defined for the component
    Prebuilt,
    /// Installation is performed by depending on the given crate in a Cargo script, and extracting
    /// the `Package` from the crate using a provided extractor function.
    ///
    /// The extracted `Package` is then written to disk as required by the package component.
    Cargo {
        /// The name of the crate
        crate_name: String,
        /// Optional features to enable, if applicable, when depending on `crate_name`.
        ///
        /// By default, the generated Cargo script will set `default-features = "false"`, so you
        /// should provide any features required to obtain the `Package` here.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        features: Vec<String>,
        /// A Rust expression which will be executed to obtain the package from the installed crate
        ///
        /// The expression will be able to reference the crate containing the `Package` in order
        /// to extract it from that crate. The resulting `Package` will then be written to disk for
        /// later use.
        ///
        /// ## Example
        ///
        /// ```json
        /// {
        ///     "crate_name": "miden-core-lib",
        ///     "extractor": "miden_core_lib::CoreLibrary::default().package()"
        /// }
        /// ```
        extractor: String,
    },
}

impl Component {
    /// Get the [ComponentKind] of this component
    #[inline(always)]
    pub fn kind(&self) -> &ComponentKind {
        &self.kind
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
                Authority::Git {
                    repository_url: repository_url_a,
                    subpath: subpath_a,
                    target:
                        GitTarget::Branch {
                            name: name_a,
                            latest_revision: local_revision,
                        },
                },
                Authority::Git {
                    repository_url: repository_url_b,
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
                Authority::Path {
                    path: path_a,
                    last_modification: last_modification_a,
                },
                Authority::Path {
                    path: path_b,
                    last_modification: last_modification_b,
                },
            ) => {
                if *path_a != *path_b {
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
                Authority::Registry { version: version_a },
                Authority::Registry { version: version_b },
            ) => {
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

        self.kind == upstream.kind
    }

    pub fn crate_name(&self) -> Option<&str> {
        match self.kind() {
            ComponentKind::CargoExtension { installation_method, .. }
            | ComponentKind::Executable { installation_method, .. } => match installation_method {
                InstallationMethod::Prebuilt => Some(self.name.as_ref()),
                InstallationMethod::PrebuiltWithCargoFallback { crate_name, .. }
                | InstallationMethod::Cargo { crate_name, .. } => Some(crate_name.as_str()),
            },
            ComponentKind::LegacyPackage {
                installation_method: PackageInstallationMethod::Cargo { crate_name, .. },
                ..
            } => Some(crate_name.as_str()),
            ComponentKind::LegacyPackage { .. } | ComponentKind::Package => {
                Some(self.name.as_ref())
            },
            ComponentKind::Asset { .. } | ComponentKind::Command { .. } => None,
        }
    }

    pub fn is_callable(&self) -> bool {
        self.kind.is_callable()
    }

    /// Returns the string representation under which midenup calls a component, if it is callable
    pub fn get_cli_display(&self) -> Option<String> {
        let name = match &self.kind {
            ComponentKind::Command { command_name: name, .. } => {
                name.as_deref().unwrap_or_else(|| self.name.as_ref())
            },
            ComponentKind::Executable { spec, .. } | ComponentKind::CargoExtension { spec, .. }
                if spec.hide =>
            {
                return None;
            },
            ComponentKind::Executable { .. } | ComponentKind::CargoExtension { .. } => {
                self.name.as_ref()
            },
            ComponentKind::Asset { .. }
            | ComponentKind::Package
            | ComponentKind::LegacyPackage { .. } => return None,
        };
        Some(format!("miden {name}"))
    }

    /// Returns the name of symlink associated with an executable component, if it is one
    pub fn get_symlink_name(&self) -> Option<String> {
        match &self.kind {
            ComponentKind::Executable {
                spec: ExecutableComponent { symlink_name: Some(symlink_name), .. },
                ..
            }
            | ComponentKind::CargoExtension {
                spec: ExecutableComponent { symlink_name: Some(symlink_name), .. },
                ..
            } => Some(symlink_name.clone()),
            _ => self.get_cli_display(),
        }
    }

    /// Returns the string representation under which midenup calls a component.
    pub fn get_call_format(&self) -> Option<Executable> {
        match &self.kind {
            ComponentKind::CargoExtension { spec, .. } | ComponentKind::Executable { spec, .. } => {
                Some(spec.call_format.clone().unwrap_or_else(Executable::default_call_format))
            },
            ComponentKind::Command { format, .. } => Some(format.clone()),
            _ => None,
        }
    }

    // Sync to the latest changes.
    pub fn sync(&mut self, config: &Config) {
        match &mut self.version {
            Authority::Path { path, last_modification } => {
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
            Authority::Git { repository_url, subpath: _, target } => {
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
            Authority::Registry { .. } => {},
        }
    }
}
