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
    manifest::Manifest,
    options::InstallationOptions,
    profile::Profile,
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

    pub fn ensure_current_is_installed(
        config: &Config,
        local_manifest: &mut Manifest,
    ) -> anyhow::Result<(Self, ToolchainJustification, Option<Channel>)> {
        let (current_toolchain, justification) = Toolchain::current(config)?;
        let desired_channel = &current_toolchain.channel;

        let Some(channel) = config.manifest.get_channel(desired_channel) else {
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

        let partial_channel = channel.create_subset(&current_toolchain, &justification);
        let channel_to_install = partial_channel.as_ref().unwrap_or(channel);

        if let Some(installed_channel) =
            local_manifest.get_channel_by_name(&channel_to_install.name)
        {
            let required_components: HashSet<&str> = HashSet::from_iter(
                channel_to_install.components.iter().map(|comp| comp.name.as_ref()),
            );

            let installed_components: HashSet<&str> = HashSet::from_iter(
                installed_channel.components.iter().map(|comp| comp.name.as_ref()),
            );

            let missing_components: Vec<_> =
                required_components.difference(&installed_components).collect();

            if missing_components.is_empty() {
                println!(
                    "{}: current toolchain is {desired_channel} and is installed",
                    "info".white().bold()
                );
                return Ok((current_toolchain, justification, partial_channel));
            }

            println!(
                "{}: installing missing components of the current toolchain:",
                "info".white().bold()
            );
            for component in missing_components {
                println!("- {}", component.white().bold());
            }
        } else {
            println!(
                "{}: current toolchain is {desired_channel}, but not yet installed",
                "info".white().bold()
            );
        }

        commands::install(
            config,
            channel_to_install,
            local_manifest,
            &InstallationOptions::default(),
        )?;

        // Now installed
        Ok((current_toolchain, justification, partial_channel))
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
