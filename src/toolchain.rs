use std::{path::PathBuf, str::FromStr};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::{Config, channel::UserChannel, commands, manifest::Manifest};

/// Represents a `miden-toolchain.toml` file. These file contains the desired
/// toolchain to be used.
#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct ToolchainFile {
    toolchain: Toolchain,
}

/// The actual contents of the toolchain.
#[derive(Serialize, Deserialize, Debug)]
pub struct Toolchain {
    pub channel: UserChannel,
    pub components: Vec<String>,
}

impl ToolchainFile {
    pub fn new(toolchain: Toolchain) -> Self {
        ToolchainFile { toolchain }
    }

    fn inner_toolchain(self) -> Toolchain {
        self.toolchain
    }
}

impl Default for Toolchain {
    fn default() -> Self {
        Self {
            channel: UserChannel::Stable,
            components: vec![
                "std".to_string(),
                "base".to_string(),
                "vm".to_string(),
                "miden-client".to_string(),
                "midenc".to_string(),
                "cargo-miden".to_string(),
            ],
        }
    }
}

impl Toolchain {
    pub fn new(channel: UserChannel, components: Vec<String>) -> Self {
        Toolchain { channel, components }
    }

    fn toolchain_file() -> anyhow::Result<PathBuf> {
        // Check for a `miden-toolchain.toml` file in $CWD
        let cwd = std::env::current_dir().context("unable to read current working directory")?;
        let toolchain_file = cwd.join("miden-toolchain").with_extension("toml");
        Ok(toolchain_file)
    }

    /// Returns the current active Toolchain according to the following prescedence:
    /// 1. The toolchain specified by a `miden-toolchain.toml` file in the
    ///    present working directory
    /// 2. The toolchain that has been set as the system's default. If set, a
    ///    `default` symlink is added to the `midenup` directory.
    ///
    /// If none of the previous conditions are met, then `stable` will be used.
    pub fn current(config: &Config) -> anyhow::Result<Self> {
        let local_toolchain = Self::toolchain_file()?;
        let global_toolchain = config.midenup_home.join("toolchains").join("default");
        if local_toolchain.exists() {
            let toolchain_file_contents =
                std::fs::read_to_string(&local_toolchain).with_context(|| {
                    format!("unable to read toolchain file '{}'", local_toolchain.display())
                })?;

            let toolchain_file: ToolchainFile =
                toml::from_str(&toolchain_file_contents).context("invalid toolchain file")?;

            let current_toolchain = toolchain_file.inner_toolchain();

            Ok(current_toolchain)
        } else if global_toolchain.exists() {
            let channel_path = std::fs::read_link(&global_toolchain)
                .context("Couldn't read default symlink. Is it a symlink?")?;
            let channel_name =
                UserChannel::from_str(channel_path.file_name().unwrap().to_str().unwrap())?;

            let installed_components_file = {
                if channel_path.join("installation-successful").exists() {
                    "installation-successful"
                } else if channel_path.join(".installation-in-progress").exists() {
                    ".installation-in-progress"
                } else {
                    bail!("Couldn't file either a .installation-in-progress or a installation-successful file in {}", channel_path.display())
                }
            };
            let components_file = global_toolchain.join(installed_components_file);
            let components = std::fs::read_to_string(components_file)?;
            let components: Vec<String> = components.lines().map(String::from).collect();
            let toolchain = Toolchain { channel: channel_name, components };

            Ok(toolchain)
        } else {
            Ok(Self::default())
        }
    }

    pub fn ensure_current_is_installed(
        config: &Config,
        local_manifest: &mut Manifest,
    ) -> anyhow::Result<Self> {
        let current_toolchain = Self::current(config)?;
        let desired_channel = &current_toolchain.channel;

        let Some(channel) = config.manifest.get_channel(desired_channel) else {
            let toolchain_file = Self::toolchain_file()?;
            bail!(
                "Channel '{}' is set in {}, however the channel doesn't exist or is unavailable",
                desired_channel,
                toolchain_file.display()
            );
        };

        let channel_dir = config.midenup_home.join("toolchains").join(format!("{}", channel.name));
        if !channel_dir.exists() {
            println!("Found current toolchain to be {desired_channel}. Now installing it.",);
            commands::install(config, channel, local_manifest)?
        }

        // Now installed
        Ok(current_toolchain)
    }
}
