use std::path::PathBuf;

use anyhow::{Context, bail};

use crate::{
    Config, InstallationOptions,
    channel::{Channel, UserChannel},
    commands::{self, install::DEPENDENCIES, uninstall::uninstall_executable},
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

    let required_updates = local_channel.components_to_update(upstream_channel);
    if required_updates.is_empty() {
        return Ok(());
    }

    let (libraries, executables): (Vec<_>, Vec<_>) =
        required_updates.iter().partition(|c| DEPENDENCIES.contains(&(c.name.as_ref())));

    // Check if any executables are installed via [[Authority::Path]]. If so,
    // ask before uninstalling.
    let executables_installed_from_path = executables
        .iter()
        .filter_map(|exe| match &exe.version {
            Authority::Path { path, crate_name } => Some((crate_name, path)),
            _ => None,
        })
        .collect::<Vec<(&String, &PathBuf)>>();

    if !executables_installed_from_path.is_empty() {
        println!("WARNING: This toolchain contains the following elements installed from paths.");
        for (exe, path) in executables_installed_from_path {
            println!("- {exe} is installed from {}", path.display())
        }
        println!("Are you sure you want to proceed? (N/y)");

        let mut input = String::new();
        std::io::stdin().read_line(&mut input).context("Failed to read input")?;
        if input.to_ascii_lowercase().as_str() != "y" {
            println!("No updates will trigger");
            return Ok(());
        }
    }

    // The update begins

    // We remove the "installation-successful" file to trigger a re-installation.
    let installation_indicator = toolchain_dir.join("installation-successful");
    match std::fs::remove_file(&installation_indicator) {
        Ok(()) => (),
        // NOTE: If the installation indicator is not present, then it means
        // that an update got stopped mid way through. If that's the case, then
        // this update run will bring the toolchain back to a valid state.
        Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound) => (),
        Err(e) => bail!(format!(
            "Couldn't delete installation complete indicator in: {}\
             because of {e}",
            &installation_indicator.display()
        )),
    }

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
            Authority::Path { crate_name, .. } => {
                uninstall_executable(crate_name, &toolchain_dir)?;
            },
        }
    }

    commands::install(config, upstream_channel, local_manifest, options)?;
    Ok(())
}
