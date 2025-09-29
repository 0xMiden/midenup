use anyhow::Context;

use crate::{
    Config, InstallationOptions,
    channel::{Channel, InstalledFile, UserChannel},
    commands::{self, uninstall::uninstall_executable},
    manifest::Manifest,
    version::Authority,
};

/// Updates installed toolchains
pub fn update(
    config: &Config,
    channel_type: Option<&UserChannel>,
    local_manifest: &mut Manifest,
    options: &InstallationOptions,
) -> anyhow::Result<()> {
    match channel_type {
        Some(UserChannel::Stable) => {
            let local_stable = local_manifest.get_latest_stable().context(
                "No stable version was found. To install it, try running:
midenup install stable
",
            )?;
            // NOTE: This means that there is no stable toolchain upstream.  This
            // is most likely an edge-case that shouldn't happen. If it does
            // happen, it probably means there's an error in midenup's parsing.
            let upstream_stable = config
                .manifest
                .get_latest_stable()
                .context("ERROR: No stable channel found in upstream")?;

            // Check if local latest stable is older than upstream's
            if upstream_stable.name > local_stable.name {
                commands::install(config, upstream_stable, local_manifest, options)?
            } else {
                println!("Nothing to update, you are all up to date");
            }
        },
        Some(UserChannel::Version(version)) => {
            // Check if any individual component changed since the last the
            // manifest was synced
            let local_channel = local_manifest
                .get_channel(&UserChannel::Version(version.clone()))
                .context(format!("ERROR: No installed channel found with version {version}"))?
                .clone();

            let upstream_channel = config
                .manifest
                .get_channel(&UserChannel::Version(version.clone()))
                .context(format!(
                    "ERROR: Couldn't find a channel upstream with version {version}. Maybe it got removed."
                ))?;

            update_channel(config, &local_channel, upstream_channel, local_manifest, options)?
        },
        None => {
            // Update all toolchains
            let mut channels_to_update = Vec::new();
            for local_channel in local_manifest.get_channels() {
                let upstream_channel =
                    config.manifest.get_channels().find(|up_c| up_c.name == local_channel.name);
                let Some(upstream_channel) = upstream_channel else {
                    // NOTE: A bit of an edge case. If the channel is present in
                    // the local manifest but not in upstream, then it probably
                    // either:
                    // - is a developer toolchain.
                    // - the upstream channel got removed from upstream (possibly for being too
                    //   old/deprecated/got rolled back)
                    continue;
                };
                channels_to_update.push((local_channel.clone(), upstream_channel.clone()));
            }

            for (local_channel, upstream_channel) in channels_to_update {
                update_channel(config, &local_channel, &upstream_channel, local_manifest, options)?;
            }
        },
        Some(UserChannel::Nightly) => todo!(),
        Some(UserChannel::Other(_)) => todo!(),
    }
    Ok(())
}

/// This function executes the actual update. It is in charge of "preparing the
/// environmet" to then call [commands::install]. That preparation mainly
/// consists of:
/// - Uninstalls components (via cargo uninstall).
/// - Removes the installation indicator file.
fn update_channel(
    config: &Config,
    local_channel: &Channel,
    upstream_channel: &Channel,
    local_manifest: &mut Manifest,
    options: &InstallationOptions,
) -> anyhow::Result<()> {
    let installed_toolchains_dir = config.midenup_home.join("toolchains");
    let toolchain_dir = installed_toolchains_dir.join(format!("{}", &local_channel.name));

    // NOTE: After deleting the files we need to remove the "all is installed
    // file" to trigger a re-installation
    let installation_indicator = toolchain_dir.join("installation-successful");
    std::fs::remove_file(&installation_indicator).context(format!(
        "Couldn't delete installation complete indicator in: {}",
        &installation_indicator.display()
    ))?;

    let updates = local_channel.components_to_update(upstream_channel);

    let (libraries, executables): (Vec<_>, Vec<_>) = updates
        .iter()
        .partition(|c| matches!(c.get_installed_file(), InstalledFile::Library { .. }));

    for lib in libraries {
        let lib_path = toolchain_dir.join("lib").join(lib.name.as_ref()).with_extension("masp");
        std::fs::remove_file(&lib_path)
            .context(format!("Couldn't delete {}", &lib_path.display()))?;
    }

    let toolchain_dir = config
        .midenup_home
        .join("toolchains")
        .join(format!("{}", &upstream_channel.name));

    for exe in executables {
        match &exe.version {
            Authority::Cargo { package, .. } => {
                let package_name = package.as_deref().unwrap_or(exe.name.as_ref());
                uninstall_executable(package_name, &toolchain_dir)?;
            },
            Authority::Git { crate_name, .. } => {
                uninstall_executable(crate_name, &toolchain_dir)?;
            },
            Authority::Path { .. } => {
                // We simply skip components that are pointing to a Path. We
                // leave it to the user to determine when a component should be
                // updated. They'd simply need to update the workspace manually.
            },
        }
    }

    commands::install(config, upstream_channel, local_manifest, options)?;
    Ok(())
}
