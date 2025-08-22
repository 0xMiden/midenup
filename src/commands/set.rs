use std::io::Write;

use anyhow::{bail, Context};

use crate::{
    channel::UserChannel,
    config::ToolchainInstallationStatus,
    toolchain::{Toolchain, ToolchainFile},
    Config,
};

const TOOLCHAIN_FILE_NAME: &str = "miden-toolchain.toml";

/// This function creates the [miden-toolchain.toml] in the present working
/// directory. This file contains the desired [Toolchain] with a list of the
/// components that make it up.
pub fn set(config: &Config, channel: &UserChannel) -> anyhow::Result<()> {
    let toolchain_file_path =
        config.working_directory.join(TOOLCHAIN_FILE_NAME).with_extension("toml");

    let Some(internal_channel) = config.manifest.get_channel(channel) else {
        bail!("channel '{}' doesn't exist or is unavailable", channel);
    };

    let installation_indicator =
        config.midenup_home_2.check_toolchain_installation(internal_channel);

    let components = {
        match installation_indicator {
            ToolchainInstallationStatus::FullyInstalled(path) => std::fs::read_to_string(path)
                .unwrap_or_else(|e| {
                    println!(
                        "WARNING: Failed to read installed components file. Defaulting to empty components list\
                         ERROR encountered: {e}"
                    );
                    String::default()
                }),
            _ => {
                println!(
                    "WARNING: Non present toolchain was set. Component list will be left empty"
                );
                String::default()
            },
        }
    };

    let components: Vec<String> = components.lines().map(String::from).collect();

    let installed_toolchain = Toolchain::new(channel.clone(), components);
    let installed_toolchain = ToolchainFile::new(installed_toolchain);

    let mut toolchain_file = std::fs::File::create(toolchain_file_path)
        .context("Failed to create miden-toolchain.toml")?;

    let toolchain_file_contents = toml::to_string_pretty(&installed_toolchain)
        .context("Failed to generate miden-toolchain.toml")?;

    toolchain_file
        .write_all(&toolchain_file_contents.into_bytes())
        .context("Failed to write miden-toolchain.toml")?;
    Ok(())
}
