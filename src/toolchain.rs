use std::{
    borrow::Cow,
    collections::HashSet,
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
    options::{InstallationOptions, IntentUpdate},
    profile::Profile,
    resolve::Intent,
    state::LocalState,
};

/// Represents a `miden-toolchain.toml` file.
///
/// These file contains the desired toolchain to be used.
#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct ToolchainFile {
    toolchain: Toolchain,
}

impl ToolchainFile {
    pub fn new(toolchain: Toolchain) -> Self {
        ToolchainFile { toolchain }
    }

    #[inline]
    fn into_toolchain(self) -> Toolchain {
        self.toolchain
    }
}

/// The actual contents of the toolchain.
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Toolchain {
    pub channel: UserChannel,
    pub components: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<Profile>,
}

/// Used to specify why Midenup believes the current toolchain is what it is.
#[derive(Debug)]
pub enum ToolchainJustification {
    /// There exists a miden toolchain file present at `path`
    MidenToolchainFile { path: PathBuf },
    /// The system's default toolchain was overriden (via `midenup set`).
    Override,
    /// No toolchain was specified, fallback to stable.
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
fn active_view(
    config: &Config,
    state: &LocalState,
    channel: &UserChannel,
    intent: &Intent,
) -> Option<Channel> {
    let version = config.local_channel(channel)?;
    let installation = state.get(&version).filter(|installation| installation.is_managed())?;

    let installed = installation.as_channel();
    let resolved = crate::resolve::resolve(&installed, intent).ok()?;

    Some(Channel::new(installed.name.clone(), resolved.into_iter().cloned().collect()))
}

impl Toolchain {
    pub fn new(channel: UserChannel, profile: Option<Profile>, components: Vec<String>) -> Self {
        Toolchain { channel, components, profile }
    }

    /// Returns the current active Toolchain according to the following prescedence:
    ///
    /// 1. The toolchain specified by a `miden-toolchain.toml` file in the present working directory
    /// 2. The toolchain that has been set as the system's default. If set, a `default` symlink is
    ///    added to the `midenup` directory.
    ///
    /// If none of the previous conditions are met, then `stable` will be used.
    pub fn current(config: &Config) -> anyhow::Result<(Toolchain, ToolchainJustification)> {
        let local_toolchain = Self::toolchain_file(&config.working_directory);
        let global_toolchain = config.midenup_home.join("toolchains").join("default");

        if let Some(local_toolchain) = local_toolchain {
            let toolchain_file_contents =
                std::fs::read_to_string(&local_toolchain).with_context(|| {
                    format!("unable to read toolchain file '{}'", local_toolchain.display())
                })?;

            let toolchain_file: ToolchainFile =
                toml::from_str(&toolchain_file_contents).context("invalid toolchain file")?;

            let current_toolchain = toolchain_file.into_toolchain();

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
            let user_channel = UserChannel::from_str(channel_name)?;

            let toolchain = Toolchain {
                channel: user_channel,
                components: vec![],
                profile: None,
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
    ) -> anyhow::Result<(Self, ToolchainJustification, Option<Channel>)> {
        let (current_toolchain, justification) = Toolchain::current(config)?;
        let desired_channel = &current_toolchain.channel;

        // Resolve the project's declared toolchain into an exact component set. An omitted
        // profile means `minimal` (Profile::default), and listed components are explicit roots on
        // top of it.
        let intent = Intent {
            profiles: [current_toolchain.profile.unwrap_or_default()].into_iter().collect(),
            roots: current_toolchain.components.iter().cloned().collect(),
        };

        // Everything the project asked for is installed: nothing to fetch, nothing to install.
        //
        // The active view is resolved against the *installed* snapshot rather than upstream, which
        // is what section 8.5 says it is -- this project's request, against what this machine has.
        if let Some(view) = active_view(config, state, desired_channel, &intent) {
            println!(
                "{}: current toolchain is {desired_channel} and is installed",
                "info".white().bold()
            );
            return Ok((current_toolchain, justification, Some(view)));
        }

        // Something is missing, so upstream is needed after all.
        let manifest = config.upstream_manifest()?;
        let Some(channel) = manifest.get_channel(desired_channel) else {
            bail!(
                "channel '{}' is set because {}, however the channel doesn't exist or is \
                 unavailable",
                desired_channel,
                match justification {
                    ToolchainJustification::Default => Cow::Borrowed("it is the default"),
                    ToolchainJustification::MidenToolchainFile { path } => {
                        Cow::Owned(format!("it is set in {}", path.display()))
                    },
                    ToolchainJustification::Override =>
                        Cow::Borrowed("it was set using 'midenup set'"),
                }
            );
        };

        let resolved =
            crate::resolve::resolve(channel, &intent).with_context(|| match &justification {
                ToolchainJustification::MidenToolchainFile { path } => {
                    format!("unable to resolve the toolchain declared in {}", path.display())
                },
                _ => format!("unable to resolve the {} toolchain", channel.name),
            })?;

        let upstream_view = Some(Channel::new(
            channel.name.clone(),
            resolved.iter().map(|component| (*component).clone()).collect(),
        ));

        match state.get(&channel.name).filter(|installation| installation.is_managed()) {
            Some(installed) => {
                let installed_components: HashSet<&str> =
                    HashSet::from_iter(installed.components.iter().map(|comp| comp.name.as_ref()));
                println!(
                    "{}: installing missing components of the current toolchain:",
                    "info".white().bold()
                );
                for component in resolved
                    .iter()
                    .map(|component| component.name.as_ref())
                    .filter(|name| !installed_components.contains(name))
                {
                    println!("- {}", component.white().bold());
                }
            },
            None => {
                println!(
                    "{}: current toolchain is {desired_channel}, but not yet installed",
                    "info".white().bold()
                );
            },
        }

        // Dispatch has decided it must install, which makes it a writer. It takes the lock here,
        // for the install only, and releases it before exec'ing the component: two `miden`
        // invocations in two project directories are otherwise two concurrent writers against one
        // MIDENUP_HOME, with no user involved in making that happen.
        let _lock = crate::lock::acquire(&config.midenup_home)?;

        // Another invocation may have installed it while we waited. Re-read rather than plan
        // against what was true before the wait.
        *state = config.local_state()?;
        if let Some(view) = active_view(config, state, desired_channel, &intent) {
            return Ok((current_toolchain, justification, Some(view)));
        }

        // Activation goes through exactly the same code path as everything else: the full upstream
        // channel, and an intent that *adds* this project's request to whatever other projects have
        // already asked for. Activating one project must never take components away from another.
        let options = InstallationOptions {
            intent_update: Some(IntentUpdate::Union(intent.clone())),
            ..Default::default()
        };

        commands::install(config, channel, state, &options)?;

        // Now installed
        Ok((current_toolchain, justification, upstream_view))
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
