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
#[derive(Debug, Clone, PartialEq)]
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

/// The verbatim body of a component whose `kind` this build does not recognize.
///
/// Wrapped in a newtype so it can carry the `PartialEq`/`Eq`/`Hash` impls that
/// `serde_json::Value` lacks, keeping those derives available on [ComponentKind].
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(transparent)]
pub struct OpaqueBody(pub Extra);

impl PartialEq for OpaqueBody {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for OpaqueBody {}

impl Hash for OpaqueBody {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // `serde_json::Map` is a `BTreeMap` here (the `preserve_order` feature is off), so its
        // serialization is canonical and this hash is stable across parses.
        serde_json::to_string(&self.0).unwrap_or_default().hash(state);
    }
}

/// Component kinds this build knows how to install.
///
/// This mirrors the known variants of [ComponentKind] and exists purely so that the derive can
/// generate their (de)serialization. [ComponentKind] dispatches to it by tag; see
/// [ComponentKind::KNOWN_TAGS] and the `known_tags_match_the_mirror` test, which fails if the two
/// drift apart.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum KnownKind {
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
        /// The exact filename this package installs as, e.g. `core.masp`.
        ///
        /// A crate-extracted package has no artifact to take a name from, so without this the
        /// name has to be invented -- and install and uninstall invented it differently. See
        /// [Component::installed_package_name].
        ///
        /// Spelled in kebab-case to match `installed-executable`, the analogous field naming an
        /// installed file. (`rename_all` on this enum applies to variant names, not fields, which
        /// is why its sibling `installation_method` stays snake_case.)
        #[serde(rename = "installed-package", default, skip_serializing_if = "Option::is_none")]
        installed_package: Option<String>,
    },
    /// An asset that will be installed to the toolchain's `etc` directory
    Asset,
}

/// An installable component's kind, including kinds this build does not recognize.
///
/// The variants carry no serde attributes: (de)serialization is hand-written below and delegates
/// to [KnownKind] for everything it recognizes, so there is no attribute duplication to drift.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ComponentKind {
    /// A component derived from an executable artifact
    Executable {
        installation_method: InstallationMethod,
        spec: ExecutableComponent,
    },
    /// An executable component that is invoked via `cargo`, rather than directly
    CargoExtension {
        installation_method: InstallationMethod,
        spec: ExecutableComponent,
    },
    /// A virtual component that defines a `miden` command. See [KnownKind::Command].
    Command {
        command_name: Option<String>,
        format: Executable,
        subcommands: BTreeMap<Alias, Executable>,
        aliases: BTreeMap<Alias, Executable>,
    },
    /// A logical set of one or more Miden packages
    Package,
    /// Legacy support for packages which required extraction from a Rust crate
    LegacyPackage {
        installation_method: PackageInstallationMethod,
        installed_package: Option<String>,
    },
    /// An asset that will be installed to the toolchain's `etc` directory
    Asset,
    /// A kind declared by a newer schema that this build does not know how to install.
    ///
    /// Held verbatim so it round-trips losslessly. It belongs to no profile, is never selected
    /// implicitly, and fails plan construction if named explicitly -- see
    /// [Component::is_supported].
    Unsupported { tag: String, body: OpaqueBody },
}

impl ComponentKind {
    /// The `kind` tags this build recognizes.
    ///
    /// Kept in sync with [KnownKind] by the `known_tags_match_the_mirror` test.
    pub const KNOWN_TAGS: &[&str] =
        &["executable", "cargo-extension", "command", "package", "legacy-package", "asset"];

    /// The declared `kind` tag, whether or not this build recognizes it.
    pub fn tag(&self) -> &str {
        match self {
            Self::Executable { .. } => "executable",
            Self::CargoExtension { .. } => "cargo-extension",
            Self::Command { .. } => "command",
            Self::Package => "package",
            Self::LegacyPackage { .. } => "legacy-package",
            Self::Asset { .. } => "asset",
            Self::Unsupported { tag, .. } => tag.as_str(),
        }
    }

    /// Whether this build knows how to install this kind.
    pub fn is_supported(&self) -> bool {
        !matches!(self, Self::Unsupported { .. })
    }

    fn as_known(&self) -> Option<KnownKind> {
        Some(match self {
            Self::Executable { installation_method, spec } => KnownKind::Executable {
                installation_method: installation_method.clone(),
                spec: spec.clone(),
            },
            Self::CargoExtension { installation_method, spec } => KnownKind::CargoExtension {
                installation_method: installation_method.clone(),
                spec: spec.clone(),
            },
            Self::Command {
                command_name,
                format,
                subcommands,
                aliases,
            } => KnownKind::Command {
                command_name: command_name.clone(),
                format: format.clone(),
                subcommands: subcommands.clone(),
                aliases: aliases.clone(),
            },
            Self::Package => KnownKind::Package,
            Self::LegacyPackage { installation_method, installed_package } => {
                KnownKind::LegacyPackage {
                    installation_method: installation_method.clone(),
                    installed_package: installed_package.clone(),
                }
            },
            Self::Asset => KnownKind::Asset,
            Self::Unsupported { .. } => return None,
        })
    }
}

impl From<KnownKind> for ComponentKind {
    fn from(value: KnownKind) -> Self {
        match value {
            KnownKind::Executable { installation_method, spec } => {
                Self::Executable { installation_method, spec }
            },
            KnownKind::CargoExtension { installation_method, spec } => {
                Self::CargoExtension { installation_method, spec }
            },
            KnownKind::Command {
                command_name,
                format,
                subcommands,
                aliases,
            } => Self::Command {
                command_name,
                format,
                subcommands,
                aliases,
            },
            KnownKind::Package => Self::Package,
            KnownKind::LegacyPackage { installation_method, installed_package } => {
                Self::LegacyPackage { installation_method, installed_package }
            },
            KnownKind::Asset => Self::Asset,
        }
    }
}

impl Serialize for ComponentKind {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.as_known() {
            Some(known) => known.serialize(serializer),
            // Emitted verbatim, including its own `kind` tag.
            None => match self {
                Self::Unsupported { body, .. } => body.0.serialize(serializer),
                _ => unreachable!("as_known only returns None for Unsupported"),
            },
        }
    }
}

impl<'de> Deserialize<'de> for ComponentKind {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;

        let value = serde_json::Value::deserialize(deserializer)?;
        let tag = value
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| D::Error::missing_field("kind"))?;

        // Dispatch on the tag explicitly rather than relying on an untagged fallback. An untagged
        // fallback also swallows a *malformed known* kind -- `{"kind":"executable"}` missing
        // `installed-executable` would silently become Unsupported instead of erroring.
        if !Self::KNOWN_TAGS.contains(&tag) {
            let tag = tag.to_string();
            let body = match value {
                serde_json::Value::Object(map) => map,
                _ => Extra::new(),
            };
            return Ok(Self::Unsupported { tag, body: OpaqueBody(body) });
        }

        KnownKind::deserialize(value).map(Self::from).map_err(D::Error::custom)
    }
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

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, Hash)]
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
            ComponentKind::Asset
            | ComponentKind::Command { .. }
            | ComponentKind::Unsupported { .. } => None,
        }
    }

    pub fn is_callable(&self) -> bool {
        self.kind.is_callable()
    }

    /// Whether this build knows how to install this component.
    ///
    /// An unsupported component belongs to no profile regardless of what its `profiles` field
    /// says, so it is never selected implicitly; naming it as an explicit root is an error.
    pub fn is_supported(&self) -> bool {
        self.kind.is_supported()
    }

    /// The exact filename a `legacy-package` component installs as.
    ///
    /// The schema field is optional, and when absent this falls back to `<component>.masp` --
    /// which is what install has always written. The point of having a single accessor is that
    /// install and uninstall can no longer disagree: they previously invented the name
    /// independently, install from the component name and uninstall from the kebab-cased crate
    /// name, so uninstalling `protocol` looked for `miden-protocol.masp` and removed nothing.
    pub fn installed_package_name(&self) -> Option<String> {
        match self.kind() {
            ComponentKind::LegacyPackage { installed_package, .. } => {
                Some(installed_package.clone().unwrap_or_else(|| format!("{}.masp", self.name)))
            },
            _ => None,
        }
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
            ComponentKind::Asset
            | ComponentKind::Package
            | ComponentKind::LegacyPackage { .. }
            | ComponentKind::Unsupported { .. } => return None,
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

#[cfg(test)]
mod unsupported_tests {
    use super::*;
    use crate::manifest::VersionedManifest;

    fn manifest_with_kind(kind: &str) -> String {
        serde_json::json!({
            "manifest_version": "2.0.0",
            "date": 1735689600,
            "channels": [{"name": "0.15.0", "components": [
                {"name": "vm", "version": {"kind": "registry", "version": "0.15.0"},
                 "kind": "executable",
                 "installation_method": {"kind": "cargo", "crate_name": "miden-vm"},
                 "installed-executable": "miden-vm", "profiles": ["minimal"]},
                {"name": "futurething", "version": {"kind": "registry", "version": "1.0.0"},
                 "kind": kind, "profiles": ["minimal"], "some-future-field": true}
            ]}]
        })
        .to_string()
    }

    /// One unknown component kind must not make the whole manifest unreadable -- that would brick
    /// every older midenup for every channel the first time a new kind ships.
    #[test]
    fn unknown_kind_parses_and_does_not_abort_the_manifest() {
        let m = VersionedManifest::parse_str(&manifest_with_kind("wasm-module"))
            .expect("unknown kind must not make the manifest unparseable");
        let channel = m.get_channels().next().unwrap();

        assert!(channel.get_component("vm").is_some(), "known components stay usable");

        let c = channel.get_component("futurething").unwrap();
        assert!(matches!(c.kind(), ComponentKind::Unsupported { tag, .. } if tag == "wasm-module"));
        assert!(!c.is_supported());
        assert_eq!(c.kind().tag(), "wasm-module");
    }

    #[test]
    fn unknown_kind_round_trips_losslessly() {
        let m = VersionedManifest::parse_str(&manifest_with_kind("wasm-module")).unwrap();
        let out: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();

        let c = out["channels"][0]["components"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "futurething")
            .expect("component preserved");

        assert_eq!(c["kind"], serde_json::json!("wasm-module"));
        assert_eq!(c["some-future-field"], serde_json::json!(true));
        assert_eq!(c["profiles"], serde_json::json!(["minimal"]));
    }

    /// A *malformed known* kind must error, not silently degrade into `Unsupported`.
    ///
    /// This is why dispatch is written by hand: `#[serde(untagged)]` on a fallback variant was
    /// measured to swallow `{"kind":"executable"}` with a missing `installed-executable`, turning
    /// a manifest typo into an opaque component that would simply never install.
    #[test]
    fn a_malformed_known_kind_is_an_error_not_an_unsupported_component() {
        let bad = serde_json::json!({
            "manifest_version": "2.0.0",
            "date": 1735689600,
            "channels": [{"name": "0.15.0", "components": [
                {"name": "vm", "version": {"kind": "registry", "version": "0.15.0"},
                 "kind": "executable",
                 "installation_method": {"kind": "cargo", "crate_name": "miden-vm"}}
            ]}]
        })
        .to_string();

        assert!(
            VersionedManifest::parse_str(&bad).is_err(),
            "a known kind missing a required field must be rejected"
        );
    }

    #[test]
    fn a_component_without_a_kind_is_an_error() {
        let bad = serde_json::json!({
            "manifest_version": "2.0.0",
            "date": 1735689600,
            "channels": [{"name": "0.15.0", "components": [
                {"name": "vm", "version": {"kind": "registry", "version": "0.15.0"}}
            ]}]
        })
        .to_string();

        assert!(VersionedManifest::parse_str(&bad).is_err(), "`kind` is required");
    }

    /// Guards against `KNOWN_TAGS` drifting from the `KnownKind` mirror.
    #[test]
    fn known_tags_match_the_mirror() {
        for tag in ComponentKind::KNOWN_TAGS {
            let value = match *tag {
                "executable" | "cargo-extension" => serde_json::json!({
                    "kind": tag,
                    "installation_method": {"kind": "cargo", "crate_name": "c"},
                    "installed-executable": "e"
                }),
                "command" => serde_json::json!({"kind": tag, "format": ["docker"]}),
                "package" => serde_json::json!({"kind": tag}),
                "legacy-package" => serde_json::json!({
                    "kind": tag,
                    "installation_method": {
                        "kind": "cargo", "crate_name": "c", "extractor": "x()"
                    }
                }),
                "asset" => serde_json::json!({"kind": tag}),
                other => panic!("KNOWN_TAGS lists '{other}' but this test has no case for it"),
            };

            let parsed = ComponentKind::deserialize(value).unwrap_or_else(|err| {
                panic!("KNOWN_TAGS lists '{tag}' but KnownKind cannot parse it: {err}")
            });
            assert!(parsed.is_supported(), "'{tag}' must not degrade to Unsupported");
            assert_eq!(parsed.tag(), *tag);
        }
    }
}

#[cfg(test)]
mod initialization_tests {
    use crate::manifest::VersionedManifest;

    fn manifest_with_initialization() -> String {
        serde_json::json!({
            "manifest_version": "2.0.0",
            "date": 1735689600,
            "channels": [{"name": "0.15.0", "components": [{
                "name": "client",
                "version": {"kind": "registry", "version": "0.15.0"},
                "kind": "executable",
                "installation_method": {"kind": "cargo", "crate_name": "miden-client-cli"},
                "installed-executable": "miden-client",
                "initialization": ["/bin/false", "SENTINEL-MUST-NOT-RUN"]
            }]}]
        })
        .to_string()
    }

    /// `initialization` must survive parse -> serialize untouched.
    ///
    /// It is retained but never executed: removing it would be a breaking schema change for a
    /// feature that is expected to come back. The sentinel argument is deliberately conspicuous so
    /// that any code path which ever ran it would be obvious in a process listing or test output.
    #[test]
    fn initialization_round_trips_and_is_never_dropped() {
        let m = VersionedManifest::parse_str(&manifest_with_initialization()).expect("parse");
        let out: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();

        assert_eq!(
            out["channels"][0]["components"][0]["initialization"],
            serde_json::json!(["/bin/false", "SENTINEL-MUST-NOT-RUN"])
        );
    }

    /// Guards against `initialization` acquiring an execution path.
    ///
    /// Only the v1 converter and the v2 schema may mention it. Wiring it up to a subprocess would
    /// necessarily touch another file -- the executor, the dispatcher, an install command -- and
    /// this test fires when that happens. Crude, but it fails loudly at exactly the right moment,
    /// which is far cheaper than trying to observe the absence of a side effect at runtime.
    #[test]
    fn no_new_code_path_references_initialization() {
        const ALLOWED: &[&str] = &["manifest/v1/component.rs", "manifest/v2/component.rs"];

        fn walk(dir: &std::path::Path, found: &mut Vec<String>, root: &std::path::Path) {
            for entry in std::fs::read_dir(dir).expect("readable source dir").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, found, root);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && std::fs::read_to_string(&path).is_ok_and(|text| {
                        // Only *code* counts. Documentation that merely explains why
                        // `initialization` is excluded from something is not an execution path,
                        // and flagging it would train people to widen the allowlist reflexively --
                        // which is exactly how a guard like this stops working.
                        text.lines()
                            .map(str::trim_start)
                            .filter(|line| !line.starts_with("//"))
                            .any(|line| line.contains("initialization"))
                    })
                {
                    let rel = path.strip_prefix(root).unwrap_or(&path);
                    found.push(rel.display().to_string());
                }
            }
        }

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut found = Vec::new();
        walk(&root, &mut found, &root);
        found.sort();

        let mut expected: Vec<String> = ALLOWED.iter().map(|s| s.to_string()).collect();
        expected.sort();

        assert_eq!(
            found, expected,
            "`initialization` is recorded but must never be executed. If you added a legitimate \
             new reference, extend ALLOWED; if you wired it to a subprocess, do not."
        );
    }
}

#[cfg(test)]
mod legacy_package_tests {
    use crate::manifest::VersionedManifest;

    fn manifest(installed_package: Option<&str>) -> String {
        let mut component = serde_json::json!({
            "name": "protocol",
            "version": {"kind": "registry", "version": "0.15.3"},
            "kind": "legacy-package",
            "installation_method": {
                "kind": "cargo",
                "crate_name": "miden-protocol",
                "extractor": "miden_protocol::ProtocolLib::default().as_ref()"
            }
        });
        if let Some(name) = installed_package {
            component["installed-package"] = serde_json::json!(name);
        }
        serde_json::json!({
            "manifest_version": "2.0.0",
            "date": 1735689600,
            "channels": [{"name": "0.15.0", "components": [component]}]
        })
        .to_string()
    }

    fn component_of(src: &str) -> crate::manifest::Component {
        VersionedManifest::parse_str(src)
            .expect("parse")
            .get_channels()
            .next()
            .unwrap()
            .get_component("protocol")
            .unwrap()
            .clone()
    }

    /// Absent `installed-package` falls back to `<component>.masp`, which is what install has
    /// always written.
    #[test]
    fn an_absent_installed_package_falls_back_to_the_component_name() {
        assert_eq!(
            component_of(&manifest(None)).installed_package_name().as_deref(),
            Some("protocol.masp")
        );
    }

    /// The declared name wins when present.
    #[test]
    fn a_declared_installed_package_is_used_verbatim() {
        assert_eq!(
            component_of(&manifest(Some("custom.masp"))).installed_package_name().as_deref(),
            Some("custom.masp")
        );
    }

    /// Regression: install wrote `lib/<component>.masp` while uninstall removed
    /// `lib/<kebab-crate-name>.masp`, so uninstalling `protocol` looked for
    /// `miden-protocol.masp` and removed nothing. Both now resolve through one accessor.
    #[test]
    fn the_name_does_not_depend_on_the_crate_name() {
        let resolved = component_of(&manifest(None)).installed_package_name().unwrap();
        assert_ne!(
            resolved, "miden-protocol.masp",
            "the installed name must come from the component, not the crate"
        );
        assert_eq!(resolved, "protocol.masp");
    }

    #[test]
    fn only_legacy_packages_resolve_an_installed_package_name() {
        let src = serde_json::json!({
            "manifest_version": "2.0.0",
            "date": 1735689600,
            "channels": [{"name": "0.15.0", "components": [{
                "name": "core",
                "version": {"kind": "registry", "version": "0.1.0"},
                "kind": "package",
                "artifacts": {"core.masp": {"uri": "https://example.invalid/core.masp"}}
            }]}]
        })
        .to_string();
        let manifest = VersionedManifest::parse_str(&src).unwrap();
        let core = manifest.get_channels().next().unwrap().get_component("core").unwrap();
        assert!(core.installed_package_name().is_none());
    }
}
