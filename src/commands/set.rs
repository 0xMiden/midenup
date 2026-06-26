use std::io::Write;

use anyhow::Context;

use crate::{
    channel::UserChannel,
    config::Config,
    toolchain::{Toolchain, ToolchainFile},
};

const TOOLCHAIN_FILE_NAME: &str = "miden-toolchain.toml";

/// This function creates the `miden-toolchain.toml` in the present working directory.
///
/// That file contains the desired toolchain with a list of the components that make it up.
pub fn set(config: &Config, channel: &UserChannel) -> anyhow::Result<()> {
    let toolchain_file_path =
        config.working_directory.join(TOOLCHAIN_FILE_NAME).with_extension("toml");

    let installed_toolchain = Toolchain::new(channel.clone(), None, vec![]);
    let installed_toolchain = ToolchainFile::new(installed_toolchain);

    let mut toolchain_file = std::fs::File::create(toolchain_file_path)
        .context("failed to create miden-toolchain.toml")?;

    let toolchain_file_contents = toml::to_string_pretty(&installed_toolchain)
        .context("failed to generate miden-toolchain.toml")?;

    toolchain_file
        .write_all(&toolchain_file_contents.into_bytes())
        .context("failed to write miden-toolchain.toml")?;
    Ok(())
}
