use anyhow::{Context, bail};
use colored::Colorize;

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

    // Depending on users input, the channel that will be install might differt
    // slighltly from the upstream channel.
    let mut channel_to_install = upstream_channel.clone();

    let components_to_delete = local_channel.components_to_update(&channel_to_install);
    if components_to_delete.is_empty() {
        return Ok(());
    }

    let mut path_warning_displayed = false;
    let mut exes_to_uninstall = Vec::new();
    let mut libs_to_uninstall = Vec::new();
    for component in components_to_delete {
        let mut update_new_element = true;
        if DEPENDENCIES.contains(&(component.name.as_ref())) {
            // Libraries
            let lib_path =
                toolchain_dir.join("lib").join(component.name.as_ref()).with_extension("masp");
            libs_to_uninstall.push(lib_path);
        } else {
            // Executables
            match component.version {
                Authority::Cargo { package, .. } => {
                    let package_name = package.unwrap_or(component.name.to_string());
                    exes_to_uninstall.push(package_name);
                },
                Authority::Git { crate_name, .. } => {
                    exes_to_uninstall.push(crate_name);
                },
                // Since uninstalling a component from the filesystem is
                // irreversible and potentially irreproducible, we take special
                // precautions before uninstalling.
                Authority::Path { path, crate_name, .. } => {
                    if !path_warning_displayed {
                        println!(
                            "{}: This toolchain contains elements installed from a path in the filesystem.",
                            "WARNING".yellow().bold(),
                        );
                        path_warning_displayed = true;
                    }

                    println!(
                        "\
- {} is installed from {}.
Would you like to update this component? (N/y/c)
   - N: no, skip this component
   - y: yes, update this component
   - c: cancel the update all-together (no changes will be applied)",
                        crate_name.bold(),
                        path.display(),
                    );
                    let mut input = String::new();
                    std::io::stdin().read_line(&mut input).context("Failed to read input")?;
                    let input = input.trim().to_ascii_lowercase();
                    match input.as_str() {
                        "y" => {
                            println!("Updating {crate_name}");
                            exes_to_uninstall.push(crate_name);
                        },
                        "c" => {
                            println!("Cancelling update, no changes will be applied.");
                            return Ok(());
                        },
                        _ => {
                            println!("Skipping {crate_name}, it will not be updated");
                            update_new_element = false;
                        },
                    }
                },
            }
        }
        // If the user doesn't want to update the current element, then we do not write said
        // component to the install.rs file. we write the old component we replace
        // the element from upstream_channel with the corresponding
        // local_channel
        if !update_new_element {
            let Some(component_to_install) = channel_to_install.get_component_mut(&component.name)
            else {
                // This can occur when the following occurs simultaneously:
                // - A user doesn't want to uninstall a component and
                // - Said component is not present in the upstream channel, which means that the
                //   component got removed from the toolchain entirely after the update.
                continue;
            };

            // SAFETY: If the component is installed, it *must* be present on
            // the local_channel.
            let local_component =
                local_channel.get_component(&component_to_install.name).cloned().unwrap();

            *component_to_install = local_component;
        }
    }

    // The update begins
    {
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

        for lib in libs_to_uninstall {
            std::fs::remove_file(&lib).context(format!("Couldn't delete {}", lib.display()))?;
        }

        for exe in exes_to_uninstall {
            uninstall_executable(exe, &toolchain_dir)?;
        }

        commands::install(config, &channel_to_install, local_manifest, options)?;
    }
    Ok(())
}
