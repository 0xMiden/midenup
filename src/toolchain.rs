use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, bail};
use colored::Colorize;
use serde::{Deserialize, Serialize};

use crate::{
    channel::{Channel, UserChannel},
    commands,
    config::Config,
    identity::{CustomToolchain, InstallationId, ToolchainName},
    manifest::{ComponentKind, InstallationMethod, PackageInstallationMethod},
    options::{InstallationOptions, IntentUpdate},
    profile::Profile,
    resolve::Intent,
    state::LocalState,
    version::Authority,
};

/// Represents a `miden-toolchain.toml` file.
///
/// These file contains the desired toolchain to be used.
#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct ToolchainFile {
    toolchain: Toolchain,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    patches: BTreeMap<String, Patch>,
}

impl ToolchainFile {
    pub fn new(toolchain: Toolchain) -> Self {
        let patches = toolchain.patches.clone();
        ToolchainFile { toolchain, patches }
    }

    #[inline]
    fn into_toolchain(self) -> Toolchain {
        Toolchain { patches: self.patches, ..self.toolchain }
    }
}

/// A `[patches.<component>]` entry: installs that component from `version` instead of the way
/// the channel publishes it, by building it with cargo.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    pub version: Authority,
    /// The crate to build. Required when the channel only publishes the component prebuilt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crate_name: Option<String>,
    /// Cargo features to build with, replacing the channel's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub features: Option<Vec<String>>,
}

/// The actual contents of the toolchain.
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Toolchain {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<ToolchainName>,
    pub channel: UserChannel,
    #[serde(default)]
    pub components: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<Profile>,
    #[serde(skip)]
    pub patches: BTreeMap<String, Patch>,
}

/// Used to specify why Midenup believes the current toolchain is what it is.
#[derive(Debug)]
pub enum ToolchainJustification {
    /// There exists a miden toolchain file present at `path`
    MidenToolchainFile { path: PathBuf },
    /// The system's default toolchain was overriden (via `midenup set`).
    Override,
    /// The toolchain was explicitly requested by the user
    Requested,
    /// No toolchain was specified, fallback to the default network.
    Default,
}

/// This project's request, resolved against what is *installed*.
///
/// `None` when the channel is not installed, was carried over from v1 (so nothing describes what it
/// owns), or does not contain everything the project asked for. Each of those means the same thing
/// to the caller: upstream is needed after all.
///
/// This is the active view of spec section 8.5 -- the project's request against this machine's
/// installation, not against the channel as published. A component another project installed is in
/// the superset but outside this view.
///
/// Also `None` when the installation was built with different patches than `patches`.
fn active_view(
    config: &Config,
    state: &LocalState,
    channel: &UserChannel,
    intent: &Intent,
    patches: &BTreeMap<String, Patch>,
) -> Option<Channel> {
    let version = config.local_channel(channel)?;
    let installation = state
        .get(&version)
        .filter(|installation| installation.is_managed() && installation.patches == *patches)?;

    let installed = installation.as_channel();
    let resolved = crate::resolve::resolve(&installed, intent).ok()?;

    Some(Channel::new(installed.name.clone(), resolved.into_iter().cloned().collect()))
}

/// Fails if a patch names a component outside `requested`, the project's resolved components.
fn ensure_patches_requested<'a>(
    patches: &BTreeMap<String, Patch>,
    requested: impl IntoIterator<Item = &'a str>,
) -> anyhow::Result<()> {
    let requested: HashSet<&str> = requested.into_iter().collect();
    if let Some(name) = patches.keys().find(|name| !requested.contains(name.as_str())) {
        bail!("cannot patch '{name}': the toolchain does not request it; add it to 'components'");
    }
    Ok(())
}

/// The components whose patch differs between `a` and `b`, including those patched in only one.
pub(crate) fn changed_patches(
    a: &BTreeMap<String, Patch>,
    b: &BTreeMap<String, Patch>,
) -> Vec<String> {
    let changed: BTreeSet<&String> =
        a.keys().chain(b.keys()).filter(|name| a.get(*name) != b.get(*name)).collect();
    changed.into_iter().cloned().collect()
}

/// Returns `channel` with each patched component switched to a cargo build of its patch.
pub(crate) fn apply_patches(
    channel: &Channel,
    patches: &BTreeMap<String, Patch>,
) -> anyhow::Result<Channel> {
    let mut patched = channel.clone();
    for (name, patch) in patches {
        let Some(component) = patched.components.iter_mut().find(|c| c.name == name.as_str())
        else {
            bail!("cannot patch '{name}': channel {} has no such component", channel.name);
        };

        match &mut component.kind {
            ComponentKind::Executable { installation_method, .. }
            | ComponentKind::CargoExtension { installation_method, .. } => {
                let (crate_name, rustup_channel, features) = match installation_method.clone() {
                    InstallationMethod::Cargo { crate_name, rustup_channel, features }
                    | InstallationMethod::PrebuiltWithCargoFallback {
                        crate_name,
                        rustup_channel,
                        features,
                    } => (
                        patch.crate_name.clone().unwrap_or(crate_name),
                        rustup_channel,
                        patch.features.clone().unwrap_or(features),
                    ),
                    InstallationMethod::Prebuilt => {
                        let Some(crate_name) = patch.crate_name.clone() else {
                            bail!(
                                "cannot patch '{name}': it is only published prebuilt, so the \
                                 patch must set 'crate_name'"
                            );
                        };
                        (crate_name, None, patch.features.clone().unwrap_or_default())
                    },
                };
                *installation_method =
                    InstallationMethod::Cargo { crate_name, rustup_channel, features };
            },
            ComponentKind::LegacyPackage {
                installation_method: PackageInstallationMethod::Cargo { crate_name, features, .. },
                ..
            } => {
                if let Some(patched_crate) = &patch.crate_name {
                    *crate_name = patched_crate.clone();
                }
                if let Some(patched_features) = &patch.features {
                    *features = patched_features.clone();
                }
            },
            kind => bail!("cannot patch '{name}': a {} is not built with cargo", kind.tag()),
        }

        component.version = patch.version.clone();
        component.artifacts = Default::default();
    }

    Ok(patched)
}

impl Toolchain {
    pub fn new(channel: UserChannel, profile: Option<Profile>, components: Vec<String>) -> Self {
        Toolchain {
            name: None,
            channel,
            components,
            profile,
            patches: BTreeMap::new(),
        }
    }

    /// The selector names a local installation; the channel describes its source.
    pub fn selector(&self) -> UserChannel {
        self.name
            .clone()
            .map(UserChannel::Custom)
            .unwrap_or_else(|| self.channel.clone())
    }

    pub fn installation_id(&self, config: &Config) -> Option<InstallationId> {
        config.local_installation_id(&self.selector())
    }

    fn validate(&self) -> anyhow::Result<()> {
        if matches!(self.channel, UserChannel::Custom(_)) {
            if self.name.is_some()
                || !self.patches.is_empty()
                || self.profile.is_some()
                || !self.components.is_empty()
            {
                bail!(
                    "a custom: selector selects an existing installation; use an upstream channel \
                     and 'name' to declare a variant"
                );
            }
        } else if !self.patches.is_empty() && self.name.is_none() {
            bail!(
                "toolchain patches require [toolchain] name; add a unique name to isolate this \
                 project's installation"
            );
        }
        Ok(())
    }

    /// Explicit selection of a named installation retains its stored recipe.
    pub(crate) fn install_selected(
        config: &Config,
        state: &mut LocalState,
        selector: &UserChannel,
        options: &InstallationOptions,
    ) -> anyhow::Result<()> {
        let id = config.local_installation_id(selector).context("toolchain is not installed")?;
        let installed = state.get_by_id(&id).cloned().with_context(|| {
            format!("{selector} is not installed; install it from its declaring project")
        })?;
        let custom = installed.custom.as_ref().context("expected a custom toolchain")?;
        let definition = Toolchain {
            name: Some(custom.name.clone()),
            channel: custom.channel.clone(),
            profile: options.profile,
            components: options.components.clone(),
            patches: installed.patches.clone(),
        };
        let intent = if options.profile.is_some() || !options.components.is_empty() {
            IntentUpdate::Replace(definition.intent())
        } else {
            IntentUpdate::Preserve
        };
        definition.install(config, state, &ToolchainJustification::Requested, intent)?;
        Ok(())
    }

    /// Returns the current active Toolchain according to the following precedence:
    ///
    /// 1. An explicit toolchain specified on the command-line
    /// 2. The `MIDENUP_TOOLCHAIN` env var has been set
    /// 3. The toolchain specified by a `miden-toolchain.toml` file in the present working directory
    /// 4. The toolchain that has been set as the system's default. If set, a `default` symlink is
    ///    added to the `midenup` directory.
    ///
    /// If none of the previous conditions are met, then the default network (`mainnet`) is used.
    pub fn current(
        config: &Config,
        toolchain_override: Option<&str>,
    ) -> anyhow::Result<(Toolchain, ToolchainJustification)> {
        let local_toolchain = Self::toolchain_file(&config.working_directory);
        let global_toolchain = config.midenup_home.join("toolchains").join("default");
        let env_override = std::env::var("MIDENUP_TOOLCHAIN").ok();

        if let Some(channel_name) = toolchain_override.or(env_override.as_deref()) {
            let channel = channel_name
                .parse::<UserChannel>()
                .with_context(|| format!("invalid channel name '{channel_name}'"))?;
            let toolchain = Toolchain {
                name: None,
                channel,
                components: vec![],
                profile: None,
                patches: BTreeMap::new(),
            };

            Ok((toolchain, ToolchainJustification::Requested))
        } else if let Some(local_toolchain) = local_toolchain {
            let toolchain_file_contents =
                std::fs::read_to_string(&local_toolchain).with_context(|| {
                    format!("unable to read toolchain file '{}'", local_toolchain.display())
                })?;

            let toolchain_file: ToolchainFile =
                toml::from_str(&toolchain_file_contents).context("invalid toolchain file")?;

            let mut current_toolchain = toolchain_file.into_toolchain();

            // A relative patch path is relative to the file declaring it.
            let project_dir = local_toolchain.parent().expect("a file has a parent directory");
            for patch in current_toolchain.patches.values_mut() {
                if let Authority::Path { path, .. } = &mut patch.version {
                    let absolute = project_dir.join(&*path);
                    *path = absolute.canonicalize().unwrap_or(absolute);
                }
            }

            current_toolchain.validate()?;
            Ok((
                current_toolchain,
                ToolchainJustification::MidenToolchainFile { path: local_toolchain },
            ))
        } else if let Ok(channel_path) = std::fs::read_link(&global_toolchain) {
            let channel_name = channel_path
                .file_name()
                .and_then(|name| name.to_str())
                .context("unable to read channel name from directory")?;

            // NOTE: This has to be a UserChannel because the default channel could be a channel
            // like "stable"
            let user_channel = if channel_path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|parent| parent == "custom")
            {
                UserChannel::Custom(channel_name.parse()?)
            } else {
                UserChannel::from_str(channel_name)?
            };

            let toolchain = Toolchain {
                name: None,
                channel: user_channel,
                components: vec![],
                profile: None,
                patches: BTreeMap::new(),
            };

            Ok((toolchain, ToolchainJustification::Override))
        } else {
            Ok((Toolchain::default(), ToolchainJustification::Default))
        }
    }

    /// Makes sure the current toolchain is usable, installing it if it is not.
    ///
    /// The offline path comes first, and is the common one: if everything this project asks for is
    /// already installed, this answers from `state.json` alone. Only when something is missing does
    /// it reach for the upstream manifest -- and that is exactly the moment it becomes a writer, so
    /// that is where it takes the lock (spec sections 13.1 and 13.2).
    pub fn ensure_current_is_installed(
        config: &Config,
        state: &mut LocalState,
        toolchain_override: Option<&str>,
    ) -> anyhow::Result<(Self, ToolchainJustification, Option<Channel>)> {
        let (current_toolchain, justification) = Toolchain::current(config, toolchain_override)?;
        // Only a `miden-toolchain.toml` narrows the view: section 8.5 defines the active view as
        // *this project's* request -- profile plus components from its toolchain file. The other
        // three justifications carry no request at all; `profile` and `components` are
        // synthesized empty, so resolving them narrows dispatch to the `minimal` profile and hides
        // whatever else this machine installed deliberately -- a component added with
        // `midenup install <channel> --component <name>` belongs to no profile, and would
        // otherwise be missing from `miden help toolchain` and reachable only through the
        // outside-the-view fallback, warning that an explicitly installed component is not part of
        // the toolchain it was installed into. Without a project file there is nothing to narrow
        // by, so there is no view and dispatch works against the installed set.
        let project_view =
            matches!(justification, ToolchainJustification::MidenToolchainFile { .. });

        let view = match current_toolchain.installed_view(config, state)? {
            Some(view) => view,
            None => {
                // Dispatch becomes a writer only when installation is needed. Re-read state
                // after waiting: another process may have installed the toolchain in the meantime.
                let _lock = crate::lock::acquire(&config.midenup_home)?;
                *state = config.local_state()?;
                current_toolchain.ensure_installed(config, state, &justification)?
            },
        };

        Ok((current_toolchain, justification, project_view.then_some(view)))
    }

    fn intent(&self) -> Intent {
        Intent {
            profiles: [self.profile.unwrap_or_default()].into_iter().collect(),
            roots: self.components.iter().cloned().collect(),
        }
    }

    /// Resolves and validates the active view entirely from the installed snapshot.
    fn installed_view(
        &self,
        config: &Config,
        state: &LocalState,
    ) -> anyhow::Result<Option<Channel>> {
        if let UserChannel::Custom(name) = &self.channel {
            let installation =
                state.get_by_id(&InstallationId::Custom(name.clone())).with_context(|| {
                    format!(
                        "custom toolchain '{name}' is not installed; install it from its \
                         declaring project"
                    )
                })?;
            return Ok(installation.is_managed().then(|| installation.as_channel()));
        }
        let view = if let Some(name) = &self.name {
            match state.get_by_id(&InstallationId::Custom(name.clone())) {
                Some(installed) => {
                    let custom =
                        installed.custom.as_ref().expect("custom identity has a definition");
                    if custom.channel != self.channel || installed.patches != self.patches {
                        bail!(
                            "custom toolchain '{name}' has a different channel or patches; choose \
                             another name, or run `midenup install` in this project to replace \
                             its definition"
                        );
                    }
                    if installed.is_managed() {
                        let channel = installed.as_channel();
                        crate::resolve::resolve(&channel, &self.intent()).ok().map(|resolved| {
                            Channel::new(
                                channel.name.clone(),
                                resolved.into_iter().cloned().collect(),
                            )
                        })
                    } else {
                        None
                    }
                },
                None => None,
            }
        } else {
            active_view(config, state, &self.channel, &self.intent(), &self.patches)
        };
        if let Some(view) = &view {
            ensure_patches_requested(
                &self.patches,
                view.components.iter().map(|component| component.name.as_ref()),
            )?;
            crate::info!("current toolchain is {} and is installed", self.channel);
        }
        Ok(view)
    }

    /// Ensures this toolchain is usable. The caller must hold the home lock and have reloaded state
    /// after acquiring it. Activation adds to the shared installation, never shrinking it.
    pub(crate) fn ensure_installed(
        &self,
        config: &Config,
        state: &mut LocalState,
        justification: &ToolchainJustification,
    ) -> anyhow::Result<Channel> {
        if let Some(view) = self.installed_view(config, state)? {
            return Ok(view);
        }
        self.install(config, state, justification, IntentUpdate::Union(self.intent()))
    }

    /// Installs the active toolchain using project settings with explicit CLI selections applied.
    /// Like an explicit-channel install, this replaces the recorded intent, even if every requested
    /// component is already installed. The caller must hold the home lock.
    pub(crate) fn install_current(
        config: &Config,
        state: &mut LocalState,
        options: &InstallationOptions,
    ) -> anyhow::Result<Self> {
        let (mut toolchain, justification) = Self::current(config, None)?;
        if matches!(toolchain.channel, UserChannel::Custom(_)) {
            Self::install_selected(config, state, &toolchain.channel, options)?;
            return Ok(toolchain);
        }
        toolchain.profile = options.profile.or(toolchain.profile);
        toolchain.components.extend(options.components.iter().cloned());
        toolchain.install(
            config,
            state,
            &justification,
            IntentUpdate::Replace(toolchain.intent()),
        )?;
        Ok(toolchain)
    }

    /// Shared installation path for an active toolchain. Lock ownership and whether intent is
    /// replaced or accumulated are decided by the caller, outside the publication machinery.
    fn install(
        &self,
        config: &Config,
        state: &mut LocalState,
        justification: &ToolchainJustification,
        intent_update: IntentUpdate,
    ) -> anyhow::Result<Channel> {
        self.validate()?;
        let desired_channel = &self.channel;
        let custom = self
            .name
            .clone()
            .map(|name| CustomToolchain { name, channel: self.channel.clone() });
        if let Some(custom) = &custom
            && let Some(installed) = state.get_by_id(&InstallationId::Custom(custom.name.clone()))
        {
            let old = &installed.custom.as_ref().expect("named installation").channel;
            let network = |channel: &UserChannel| match channel {
                UserChannel::Named(name) => Some(name.to_string()),
                _ => None,
            };
            if network(old) != network(&self.channel) {
                bail!(
                    "cannot change the network identity of custom toolchain '{}'; choose a new \
                     name to keep runtime data separate",
                    custom.name
                );
            }
        }
        let intent = if matches!(intent_update, IntentUpdate::Preserve) {
            custom
                .as_ref()
                .and_then(|custom| state.get_by_id(&InstallationId::Custom(custom.name.clone())))
                .map(|installed| installed.intent.clone())
                .unwrap_or_else(|| self.intent())
        } else {
            self.intent()
        };
        // Install against the full upstream channel, with the active project's patches applied.
        let manifest = config.upstream_manifest()?;
        let Some(upstream) = manifest.get_channel(desired_channel) else {
            bail!(
                "channel '{}' is set because {}, however the channel doesn't exist or is \
                 unavailable",
                desired_channel,
                match justification {
                    ToolchainJustification::Default => Cow::Borrowed("it is the default"),
                    ToolchainJustification::MidenToolchainFile { path } => {
                        Cow::Owned(format!("it is set in {}", path.display()))
                    },
                    ToolchainJustification::Requested =>
                        Cow::Borrowed("it was explicitly requested on the command line"),
                    ToolchainJustification::Override =>
                        Cow::Borrowed("it was set using 'midenup set'"),
                }
            );
        };

        let mut patched = apply_patches(upstream, &self.patches)?;
        patched.sync(config);
        let channel = &patched;

        let resolved =
            crate::resolve::resolve(channel, &intent).with_context(|| match &justification {
                ToolchainJustification::MidenToolchainFile { path } => {
                    format!("unable to resolve the toolchain declared in {}", path.display())
                },
                _ => format!("unable to resolve the {} toolchain", channel.name),
            })?;
        ensure_patches_requested(
            &self.patches,
            resolved.iter().map(|component| component.name.as_ref()),
        )?;

        let upstream_view = Channel::new(
            channel.name.clone(),
            resolved.iter().map(|component| (*component).clone()).collect(),
        );

        // Name both the network and the version it resolves to, so the user knows what is about
        // to be installed before anything is fetched.
        let target = match desired_channel {
            UserChannel::Version(_) => channel.name.to_string(),
            UserChannel::Named(network) => format!("{network} ({})", channel.name),
            UserChannel::Custom(_) => {
                unreachable!("custom selections install their recorded definition")
            },
        };

        let id = custom
            .as_ref()
            .map(|c| InstallationId::Custom(c.name.clone()))
            .unwrap_or_else(|| InstallationId::Version(channel.name.clone()));
        let target = if self.name.is_some() {
            format!("{id} from {target}")
        } else {
            target
        };
        match state.get_by_id(&id).filter(|installation| installation.is_managed()) {
            Some(installed) if installed.patches != self.patches => {
                crate::info!("reinstalling the current toolchain {target} to apply its patches");
            },
            Some(_) if matches!(intent_update, IntentUpdate::Replace(_)) => {
                crate::info!("installing the current toolchain {target}");
            },
            Some(installed) => {
                let installed_components: HashSet<&str> =
                    HashSet::from_iter(installed.components.iter().map(|comp| comp.name.as_ref()));
                crate::info!("installing missing components of the current toolchain {target}:");
                for component in resolved
                    .iter()
                    .map(|component| component.name.as_ref())
                    .filter(|name| !installed_components.contains(name))
                {
                    crate::note!("- {}", component.bold());
                }
            },
            None => {
                crate::info!("current toolchain is {target}, but not yet installed");
            },
        }

        let options = InstallationOptions {
            custom: custom.clone(),
            intent_update: Some(intent_update),
            // The project named this network, so it is installed here: without the link, the next
            // dispatch would not find it and install again.
            network: match desired_channel {
                UserChannel::Named(name) if custom.is_none() => Some(name.to_string()),
                _ => None,
            },
            patches: self.patches.clone(),
            ..Default::default()
        };

        commands::install(config, channel, state, &options)?;

        // Now installed
        Ok(upstream_view)
    }

    /// Returns the `miden-toolchain.toml` file, if it exists.
    ///
    /// It looks for the file from the present working directory upwards, until the root directory
    /// is reached.
    fn toolchain_file(working_directory: &Path) -> Option<PathBuf> {
        // Check for a `miden-toolchain.toml` file in $CWD and recursively upwards.
        let mut current_dir = Some(working_directory);
        let mut toolchain_file = None;
        while let Some(current_path) = current_dir {
            let current_file = current_path.join("miden-toolchain").with_extension("toml");
            if current_file.exists() {
                toolchain_file = Some(current_file);
                break;
            }
            current_dir = current_path.parent();
        }

        toolchain_file
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::version::GitTarget;

    const VM: &str = r#"{"artifacts": {"miden-vm": {"uri": "https://example.invalid/miden-vm"}}, "installation_method": {"kind": "prebuilt"}, "installed-executable": "miden-vm", "kind": "executable", "name": "vm", "profiles": ["minimal"], "version": {"kind": "registry", "version": "0.29.4"}}"#;
    const CORE: &str = r#"{"artifacts": {"miden-core.masp": {"uri": "https://example.invalid/core.masp"}}, "kind": "package", "name": "core", "profiles": ["minimal"], "version": {"kind": "registry", "version": "0.29.4"}}"#;

    fn channel() -> Channel {
        Channel::new(
            semver::Version::new(0, 16, 0),
            vec![serde_json::from_str(VM).unwrap(), serde_json::from_str(CORE).unwrap()],
        )
    }

    fn git_patch(crate_name: Option<&str>) -> Patch {
        Patch {
            version: Authority::Git {
                repository_url: "https://github.com/0xMiden/miden-vm.git".to_string(),
                subpath: None,
                target: GitTarget::Tag { name: "v0.22.1".to_string() },
            },
            crate_name: crate_name.map(String::from),
            features: None,
        }
    }

    fn patches(name: &str, patch: Patch) -> BTreeMap<String, Patch> {
        [(name.to_string(), patch)].into_iter().collect()
    }

    #[test]
    fn a_patch_outside_the_requested_components_is_rejected() {
        let vm_patch = patches("vm", git_patch(Some("miden-vm")));

        ensure_patches_requested(&vm_patch, ["vm", "core"]).unwrap();
        let err = ensure_patches_requested(&vm_patch, ["core"]).unwrap_err();
        assert!(err.to_string().contains("does not request it"), "{err}");
    }

    #[test]
    fn changed_patches_covers_added_removed_and_modified() {
        let registry = Patch {
            version: Authority::Registry { version: semver::Version::new(0, 29, 4) },
            crate_name: None,
            features: None,
        };
        let installed: BTreeMap<String, Patch> = [
            ("vm".to_string(), git_patch(Some("miden-vm"))),
            ("midenc".to_string(), registry.clone()),
            ("debug".to_string(), registry.clone()),
        ]
        .into_iter()
        .collect();
        let current: BTreeMap<String, Patch> = [
            ("vm".to_string(), git_patch(Some("miden-vm"))),
            ("midenc".to_string(), git_patch(Some("midenc"))),
            ("client".to_string(), registry),
        ]
        .into_iter()
        .collect();

        assert_eq!(changed_patches(&installed, &current), ["client", "debug", "midenc"]);
        assert!(changed_patches(&installed, &installed).is_empty());
    }

    #[test]
    fn a_toolchain_file_reads_patches() {
        let file: ToolchainFile = toml::from_str(
            r#"
            [toolchain]
            channel = "0.16.0"
            components = ["vm"]

            [patches.vm]
            crate_name = "miden-vm"
            features = ["executable"]
            version = { kind = "git", repository_url = "https://github.com/0xMiden/miden-vm.git", tag = "v0.22.1" }
            "#,
        )
        .unwrap();

        let expected = Patch {
            features: Some(vec!["executable".to_string()]),
            ..git_patch(Some("miden-vm"))
        };
        assert_eq!(file.into_toolchain().patches, patches("vm", expected));
    }

    #[test]
    fn a_patched_prebuilt_component_becomes_a_cargo_build() {
        let patch = Patch {
            features: Some(vec!["executable".to_string()]),
            ..git_patch(Some("miden-vm"))
        };
        let patched = apply_patches(&channel(), &patches("vm", patch)).unwrap();
        let vm = patched.get_component("vm").unwrap();

        assert_eq!(vm.version, git_patch(None).version);
        assert!(vm.artifacts.is_empty());
        assert!(matches!(
            vm.kind(),
            ComponentKind::Executable {
                installation_method: InstallationMethod::Cargo { crate_name, features, .. },
                ..
            } if crate_name == "miden-vm" && features == &["executable"]
        ));
        assert_eq!(patched.get_component("core"), channel().get_component("core"));
    }

    #[test]
    fn patching_a_prebuilt_component_requires_a_crate_name() {
        let err = apply_patches(&channel(), &patches("vm", git_patch(None))).unwrap_err();
        assert!(err.to_string().contains("crate_name"), "{err}");
    }

    #[test]
    fn patching_an_unknown_component_fails() {
        let err =
            apply_patches(&channel(), &patches("mv", git_patch(Some("miden-vm")))).unwrap_err();
        assert!(err.to_string().contains("no such component"), "{err}");
    }

    #[test]
    fn a_package_cannot_be_patched() {
        let err =
            apply_patches(&channel(), &patches("core", git_patch(Some("miden-core")))).unwrap_err();
        assert!(err.to_string().contains("is not built with cargo"), "{err}");
    }
}
