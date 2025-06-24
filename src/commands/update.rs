use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::{
    channel::{Channel, ChannelAlias, UserChannel},
    commands,
    manifest::Manifest,
    Config,
};

/// Updates installed toolchains
pub fn update(
    config: &Config,
    channel_type: Option<&UserChannel>,
    local_manifest: &mut Manifest,
) -> anyhow::Result<()> {
    // TODO(fabrio): This could fail either because the file doesn't exist of
    // because the json is ill formatted. There should be a destinction
    match channel_type {
        Some(UserChannel::Stable) => {
            let local_stable = local_manifest.get_latest_stable().context(
                "No stable version was found. To install it, try running:
midenup install stable
",
            )?;
            // NOTE: This means that there is no stable toolchain upstram.  This
            // is most likely an edge-case that shouldn't happen. If it does
            // happen, it probably means that there's an error in midenup
            let upstream_stable = config
                .manifest
                .get_latest_stable()
                .expect("ERROR: No stable channel found in upstream");

            // Check if local latest stable is older than upstream's
            if upstream_stable.name > local_stable.name {
                commands::install(config, upstream_stable, local_manifest)?
            } else {
                std::println!("Nothing to update, you are all up to date");
            }
        },
        // NOTE: I'd like to save the enum variant in a variable, like so:
        // Some(user_channel) if matches!(user_channel, &UserChannel::Version(_)) => {
        // but the compiler complains that I'm not matching every variant
        Some(UserChannel::Version(version)) => {
            // Check if any individual component changed since the last the
            // manifest was synced
            let local_channel = local_manifest
                .get_channel(&UserChannel::Version(version.clone()))
                .context(format!("ERROR: No installed channel found with version {}", version))?
                .clone();

            let upstream_channel = config
                .manifest
                .get_channel(&UserChannel::Version(version.clone()))
                .context(format!(
                    "ERROR: Couldn't find a channel upstream with version {}. Maybe it got removed.",
                    version
                ))?;

            update_channel(config, &local_channel, upstream_channel, local_manifest)?
        },
        None => {
            // Update all toolchains
            for local_channel in local_manifest.channels.clone().iter() {
                let upstream_channel =
                    config.manifest.channels.iter().find(|up_c| up_c.name == local_channel.name);
                let Some(upstream_channel) = upstream_channel else {
                    // NOTE: A bit of an edge case. If the channel is present in
                    // the local manifest but not in upstream, then it probably
                    // either:
                    // - is a developer toolchain.
                    // - the upstream channel got removed from upstream (possibly for being too
                    //   old/deprecated/got rolled back)
                    continue;
                };
                update_channel(config, local_channel, upstream_channel, local_manifest)?;
            }
        },
        Some(UserChannel::Nightly) => todo!(),
        Some(UserChannel::Other(_)) => todo!(),
    }
    Ok(())
}

// TODO(fabrio): Use this function for path resolution here and in the install
// script. Move as [Component] associated function
// TODO(fabrio): Clean up? Use AsRef if possible
fn get_path_to_component(toolchain_dir: &Path, component_name: &str) -> PathBuf {
    let libs = ["std", "base"];
    if libs.contains(&component_name) {
        toolchain_dir.join("lib").join(component_name).with_extension("masp")
    } else {
        toolchain_dir.join("bin").join(component_name)
    }
}

fn update_channel(
    config: &Config,
    local_channel: &Channel,
    upstream_channel: &Channel,
    local_manifest: &mut Manifest,
) -> anyhow::Result<()> {
    let installed_toolchains_dir = config.midenup_home.join("toolchains");
    let toolchain_dir = installed_toolchains_dir.join(format!("{}", &local_channel.name));

    let updates = local_channel.components_to_update(upstream_channel);
    let files_to_remove: Vec<_> = updates
        .iter()
        .map(|c| {
            get_path_to_component(
                &toolchain_dir,
                &c.installed_file.clone().unwrap_or(c.name.to_string()),
            )
        })
        .collect();
    for file in files_to_remove {
        std::fs::remove_file(file).context("Couldn't delete {file}")?;
    }

    // Before adding the new stable channel, remove the stable alias from all
    // the channels that have it.
    // NOTE: This should be only a single channel, we check for multiple just in
    // case.
    for channel in local_manifest
        .channels
        .iter_mut()
        .filter(|c| c.alias.as_ref().is_some_and(|a| matches!(a, ChannelAlias::Stable)))
    {
        channel.alias = None
    }

    // NOTE: If the channel already exists in the local manifest, remove the old version. This
    // happens when updating
    local_manifest.channels.retain(|c| c.name != upstream_channel.name);

    commands::install(config, upstream_channel, local_manifest)?;
    Ok(())
}
