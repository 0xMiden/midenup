use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context};

use crate::{
    channel::{Channel, UserChannel},
    commands,
    manifest::Manifest,
    Config,
};

/// Updates installed toolchains
pub fn update(config: &Config, channel_type: Option<&UserChannel>) -> anyhow::Result<()> {
    let local_manifest_path = config.midenup_home.join("manifest").with_extension("json");
    let local_manifest_uri = format!(
        "file://{}",
        local_manifest_path.to_str().context("Couldn't convert miden directory")?,
    );

    // TODO(fabrio): This could fail either because the file doesn't exist of
    // because the json is ill formatted. There should be a destinction
    let local_manifest = Manifest::load_from(local_manifest_uri).context(
        "No installed toolchains found. To install stable, run:
midenup install stable
",
    )?;

    match channel_type {
        Some(UserChannel::Stable) => {
            let local_stable = local_manifest.get_latest_stable();
            let upstream_stable = config.manifest.get_latest_stable().expect("TODO: Remove unwrap");

            // Check if local latest stable is older than upstream's
            let local_stable = local_stable.context(
                "No stable version was found. To install it, try running:
midenup install stable
",
            )?;

            if upstream_stable.name > local_stable.name {
                commands::install(config, upstream_stable)?
            } else {
                std::println!("Nothing to update, you are all up to date");
            }
        },
        // NOTE: I'd like to save the enum variant in a variable, like so:
        // Some(user_channel) if matches!(user_channel, &UserChannel::Version(_)) => {
        // but the compiler complains that I'm not matching every variable
        Some(UserChannel::Version(version)) => {
            // Check if any individual component changed since the last the
            // manifest was synced
            let local_channel = local_manifest
                .get_channel(&UserChannel::Version(version.clone()))
                .context("TODO: Think what this means")?;

            let upstream_channel = config
                .manifest
                .get_channel(&UserChannel::Version(version.clone()))
                .context("TODO: Think what this means")?;

            update_channel(config, local_channel, upstream_channel)?
        },
        None => {
            // Update all toolchains
            for local_channel in local_manifest.channels.iter() {
                let upstream_channel =
                    config.manifest.channels.iter().find(|up_c| up_c.name == local_channel.name);
                let Some(upstream_channel) = upstream_channel else {
                    // NOTE: A bit of an edge case. If the channel is present in
                    // the local manifest but not in upstream, then it probably
                    // is a developer toolchain. For more information see:
                    // https://github.com/0xMiden/midenup/pull/11#discussion_r2147289872
                    continue;
                };
                update_channel(config, local_channel, upstream_channel)?;
            }
        },
        Some(UserChannel::Nightly) => todo!(),
        Some(UserChannel::Other(_)) => todo!(),
    }
    todo!()
}

// TODO(fabrio): Use this function for path resolution here and in the install
// script. Move as [Component] associated function
// TODO(fabrio): Clean up? Use AsRef if possible
fn get_path_to_component(toolchain_dir: &Path, component_name: &str) -> PathBuf {
    let libs = ["std", "base"];
    if libs.contains(&component_name) {
        toolchain_dir.join("lib").join(component_name).with_extension("masp")
    } else {
        toolchain_dir.join("bin").join(component_name).with_extension("masp")
    }
}

fn update_channel(
    config: &Config,
    local_channel: &Channel,
    upstream_channel: &Channel,
) -> anyhow::Result<()> {
    let installed_toolchains_dir = config.midenup_home.join("toolchains");
    let toolchain_dir = installed_toolchains_dir.join(format!("{}", &local_channel.name));

    let updates = local_channel.components_to_update(upstream_channel);
    let files_to_remove: Vec<_> =
        updates.iter().map(|c| get_path_to_component(&toolchain_dir, &c.name)).collect();
    for file in files_to_remove {
        std::fs::remove_file(file).context("Couldn't delete {file}")?;
    }

    commands::install(config, upstream_channel)?;
    Ok(())
}
