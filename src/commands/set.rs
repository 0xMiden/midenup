use crate::{
    channel::{Channel, ChannelAlias, UserChannel},
    manifest::Manifest,
    toolchain::Toolchain,
    utils,
    version::Authority,
    Config,
};

use serde::{Deserialize, Serialize};
use std::io::Write;

use anyhow::{bail, Context};

const TOOLCHAIN_FILE_NAME: &str = "miden-toolchain.toml";

#[derive(Serialize, Debug)]
struct ToolchainFile {
    toolchain: Toolchain,
}

pub fn set(config: &Config, channel: &UserChannel) -> anyhow::Result<()> {
    let toolchain_file_path = config.pwd.join(TOOLCHAIN_FILE_NAME).with_extension("toml");

    let current_components_list = config
        .midenup_home
        .join("toolchains")
        .join(channel.to_string())
        .join("installation-successful");

    let components = std::fs::read_to_string(current_components_list).unwrap();
    let components: Vec<String> = components.lines().map(String::from).collect();

    let installed_toolchain = Toolchain::new(channel.clone(), components);
    let installed_toolchain = ToolchainFile { toolchain: installed_toolchain };

    let mut toolchain_file = std::fs::File::create(toolchain_file_path)
        .context("Failed to create miden-toolchain.toml")?;

    let toolchain_file_contents = toml::to_string_pretty(&installed_toolchain)
        .context("Failed to generate miden-toolchain.toml")?;

    toolchain_file.write_all(&toolchain_file_contents.into_bytes()).unwrap();
    Ok(())
}
