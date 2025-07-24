use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::channel::UserChannel;

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
    pub fn current() -> anyhow::Result<Self> {
        // Check for a `miden-toolchain.toml` file in $CWD
        let cwd = std::env::current_dir().context("unable to read current working directory")?;
        let toolchain_file = cwd.join("miden-toolchain").with_extension("toml");
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

        Ok(toolchain_file.inner_toolchain())
    }
}
