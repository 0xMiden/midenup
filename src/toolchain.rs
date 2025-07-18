use std::path::PathBuf;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::{Config, channel::UserChannel, commands, manifest::Manifest};

/// Represents a `miden-toolchain.toml` file
#[derive(Serialize, Deserialize, Debug)]
pub(crate) struct ToolchainFile {
    toolchain: Toolchain,
}

/// The actual contents of the toolchain
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

    pub fn current() -> anyhow::Result<Self> {
        let toolchain_file = Self::toolchain_file()?;
        if !toolchain_file.exists() {
            // The default toolchain is stable
            //
            // TODO(pauls): If we support setting global defaults at some point, we'll need
            // to adjust this.
            return Ok(Self::default());
        }

        let toolchain_file_contents =
            std::fs::read_to_string(&toolchain_file).with_context(|| {
                format!("unable to read toolchain file '{}'", toolchain_file.display())
            })?;

        let toolchain_file: ToolchainFile =
            toml::from_str(&toolchain_file_contents).context("invalid toolchain file")?;

        let current_toolchain = toolchain_file.inner_toolchain();

        Ok(current_toolchain)
    }

    pub fn ensure_current_is_installed(
        config: &Config,
        local_manifest: &mut Manifest,
    ) -> anyhow::Result<Self> {
        let current_toolchain = Self::current()?;
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
            println!("Found current toolchain to be {}. Now installing it.", channel.name);
            commands::install(config, channel, local_manifest)?
        }

        // Now installed
        Ok(current_toolchain)
    }
}
