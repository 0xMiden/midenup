use std::{
    collections::HashSet,
    hash::{Hash, Hasher},
};

use anyhow::Context;
use colored::Colorize;

use crate::{
    channel::{
        Channel, Component, InstalledFile, MigrationStrategy, UpstreamChannel, UpstreamMatch,
        UserChannel,
    },
    commands::{self},
    config::Config,
    manifest::Manifest,
    options::{InstallationOptions, PathUpdate, UpdateOptions},
    version::Authority,
};

/// Updates installed toolchains
pub fn update(
    config: &Config,
    channel_type: Option<&UserChannel>,
    local_manifest: &mut Manifest,
    options: &UpdateOptions,
) -> anyhow::Result<()> {
    match channel_type {
        Some(UserChannel::Stable) => {
            let local_stable = local_manifest.get_latest_stable().context(
                "No stable version was found. To install it, try running:
midenup install stable
",
            )?;
            let upstream_stable = config
                .manifest
                .get_latest_stable()
                // NOTE: This means that there is no stable toolchain upstream.
                //
                // This is most likely an edge-case that shouldn't happen. If it does happen, it
                // probably means there's an error in midenup's parsing.
                .context("ERROR: No stable channel found in upstream")?;

            if upstream_stable.name > local_stable.name {
                let component_subset: Option<HashSet<_>> = if local_stable.is_partially_installed()
                {
                    Some(local_stable.components.iter().map(|comp| comp.name.clone()).collect())
                } else {
                    None
                };

                let channel_to_install = {
                    let components = upstream_stable
                        .components
                        .clone()
                        .into_iter()
                        .filter(|comp| {
                            if let Some(component_subset) = &component_subset {
                                let name = &comp.name;
                                component_subset.contains(name)
                            } else {
                                true
                            }
                        })
                        .collect();

                    Channel {
                        name: upstream_stable.name.clone(),
                        alias: upstream_stable.alias.clone(),
                        tags: local_stable.tags.clone(),
                        components,
                    }
                };

                commands::install(
                    config,
                    &channel_to_install,
                    local_manifest,
                    &((*options).into()),
                )?
            } else {
                println!("Nothing to update, you are all up to date");
            }
        },
        Some(UserChannel::Version(version)) => {
            // Check if any individual component changed since the last the manifest was synced
            let local_channel = local_manifest
                .get_channel(&UserChannel::Version(version.clone()))
                .context(format!("ERROR: No installed channel found with version {version}"))?
                .clone();

            let upstream_counterpart =
                local_channel.find_upstream_counterpart(&config.manifest).context(format!(
                    "ERROR: Couldn't find a channel upstream with version {version}. Maybe it got \
                     removed."
                ))?;

            update_channel(config, &local_channel, &upstream_counterpart, local_manifest, options)?
        },
        None => {
            // Update all toolchains
            let mut channels_to_update = Vec::new();
            for local_channel in local_manifest.get_channels() {
                let upstream_counterpart =
                    local_channel.find_upstream_counterpart(&config.manifest);
                let Some(upstream_channel) = upstream_counterpart else {
                    // NOTE: A bit of an edge case. If the channel is present in the local manifest
                    // but not in upstream, then it probably either:
                    //
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

/// This function executes the actual update. It is in charge of "preparing the environmet" to then
/// call [`commands::install`]. Preparation primarily consists of:
///
/// - Uninstalling components (via `cargo uninstall`).
/// - Removing the installation indicator file.
///
/// The channel that is finally installed might differ slighltly from the upstream channel in the
/// following scenarios:
///
/// - A component is explicitely not updated. In that the case, the "old" component will be written
///   to the install.rs file to ensure consistency.
fn update_channel(
    config: &Config,
    local_channel: &Channel,
    upstream_channel: &UpstreamChannel,
    local_manifest: &mut Manifest,
    options: &UpdateOptions,
) -> anyhow::Result<()> {
    // These are the components that require updating
    let comp_to_delete_with_motive = components_to_update(local_channel, upstream_channel);

    let channel_to_install = upstream_channel.channel.clone();

    if comp_to_delete_with_motive.is_empty() {
        println!("Toolchain {} is up to date", local_channel);
        return Ok(());
    }

    display_warnings(&comp_to_delete_with_motive, &upstream_channel.channel, options);

    let mut components_to_update: Vec<Update> = Vec::new();
    for update in comp_to_delete_with_motive.iter() {
        let component = &update.component;
        let motive = &update.motive;

        // Added components have nothing to uninstall but must still be installed,
        // so they pass straight through.
        if matches!(motive, UpdateMotive::Added) {
            components_to_update.push(update.clone());
            continue;
        }

        let do_update = match component.get_installed_file() {
            InstalledFile::Library { .. } => true,
            InstalledFile::Executable { .. } => match component.version {
                Authority::Cargo { .. } | Authority::Git { .. } => true,
                // Since uninstalling a component from the filesystem is potentially
                // irreversible, we take special precautions before uninstalling them.
                Authority::Path { .. } => match options.path_update {
                    PathUpdate::Interactive => {
                        match handle_path_uninstall_interactive(component)? {
                            InteractiveResult::Cancel => return Ok(()),
                            InteractiveResult::UpdateComponent => true,
                            InteractiveResult::DontUpdateComponent => false,
                        }
                    },
                    PathUpdate::All => true,
                    PathUpdate::Off => false,
                },
            },
        };

        if do_update {
            components_to_update.push(update.clone());
        }
    }

    let install_options = InstallationOptions {
        verbose: options.verbose,
        components_to_update,
    };

    commands::install(config, &channel_to_install, local_manifest, &install_options)?;

    let was_migrated = matches!(upstream_channel.upstream_match, UpstreamMatch::Migrated(_));
    if was_migrated {
        // If the update were to be interrumpted before the uninstall finishes,
        // re-running `midenup update` would finish the process.
        // This does mean that channel migration is a non-atomic operation.
        commands::uninstall(config, local_channel, local_manifest)?;
    };

    Ok(())
}

enum InteractiveResult {
    /// Cancel the update all together. Useful for potential miss-clicks.
    Cancel,
    UpdateComponent,
    DontUpdateComponent,
}

fn handle_path_uninstall_interactive(component: &Component) -> anyhow::Result<InteractiveResult> {
    let component_name = &component.name;
    println!(
        "Would you like to update this component? (N/y/c)
   - N: no, skip this component
   - y: yes, update this component
   - c: cancel the update all-together (no changes will be applied)"
    );

    let mut input = String::new();
    std::io::stdin().read_line(&mut input).context("Failed to read input")?;
    let input = input.trim().to_ascii_lowercase();
    match input.as_str() {
        "y" => {
            println!("Updating {component_name}");
            Ok(InteractiveResult::UpdateComponent)
        },
        "c" => {
            println!("Cancelling update, no changes will be applied.");
            Ok(InteractiveResult::Cancel)
        },
        _ => {
            println!("Skipping {component_name}, it will not be updated");
            Ok(InteractiveResult::DontUpdateComponent)
        },
    }
}

#[derive(Debug, Clone)]
pub enum UpdateMotive {
    /// This component was added to the toolchain and wasn't there before.
    Added,
    /// This component was removed and is no longer part of the toolchain.
    Removed,
    /// A newer version was released.
    NewerVersion,
    /// The entire channel was migrated.
    Migrated { strategy: MigrationStrategy },
}

/// Wrapper around `&Component` that defines `Hash`/`Eq` by name only, so we can
/// use `HashSet` set operations (difference, intersection, contains) keyed on
/// names. This is not a property of component themselves, hence the wrapper
/// type.
///
/// See https://stackoverflow.com/a/65671830 as a reference.
#[derive(Debug, Clone)]
struct ComponentByName<'a>(&'a Component);

impl PartialEq for ComponentByName<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0.name == other.0.name
    }
}
impl Eq for ComponentByName<'_> {}
impl Hash for ComponentByName<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.name.hash(state);
    }
}

#[derive(Debug, Clone)]
pub struct Update {
    pub component: Component,
    pub motive: UpdateMotive,
}

impl Update {
    fn new(component: Component, motive: UpdateMotive) -> Update {
        Update { component, motive }
    }
}
/// This functions compares the Channel &older, with a newer channel [newer] and returns the list
/// of [Components] that need to be updated.
///
/// NOTE: A component can be marked for update in the following scenarios:
///
/// - The component got removed from the newer channel entirely and thus needs to be removed from
///   the system.
/// - A new component is present in the upstream manifest and thus needs to be installed.
/// - A newer version of a present component is released and thus an upgrade is due.
/// - An *older* version of a component is released and thus a downgrade is due.
/// - A components [Authority] got changed and thus needs to be removed and re-installed with the
///   new [Authority]
///
/// There is one notable exception to this rule which is when a channel is migrated into a different
/// channel. In that case, every component is marked for update.
pub fn components_to_update(older: &Channel, newer: &UpstreamChannel) -> Vec<Update> {
    let new_channel: HashSet<ComponentByName> =
        newer.channel.components.iter().map(ComponentByName).collect();
    let current: HashSet<ComponentByName> = older.components.iter().map(ComponentByName).collect();

    // This is the subset of new components present in the channel since last sync.
    let new_components = new_channel
        .difference(&current)
        .map(|&ComponentByName(comp)| (comp.clone(), UpdateMotive::Added));

    // This is the subset of old components that need to be removed.
    let old_components = current
        .difference(&new_channel)
        .map(|&ComponentByName(comp)| (comp.clone(), UpdateMotive::Removed));

    // These are the elements that are present in boths sets. We are only interested in those which
    // need updating.
    let components_to_update = current
        .iter()
        .filter(|comp| new_channel.contains(*comp))
        .filter_map(|&ComponentByName(current_component)| {
            let new_component = new_channel.get(&ComponentByName(current_component));
            match new_component {
                // This should't be possible, but if somehow the component is missing, then we
                // trigger an update for said component regardless.
                None => Some((current_component.clone(), UpdateMotive::Added)),
                // Note that some components might ignore this update, such as
                // components that were installed via the filesystem.
                Some(&ComponentByName(new_component)) => {
                    // If the channel got marked as migrated, then every single
                    // installed component is due for an update.
                    if let UpstreamMatch::Migrated(strategy) = &newer.upstream_match {
                        Some((
                            current_component.clone(),
                            UpdateMotive::Migrated { strategy: strategy.clone() },
                        ))
                    } else {
                        let mut current_component = current_component.clone();
                        if !current_component.is_up_to_date(new_component) {
                            Some((current_component, UpdateMotive::NewerVersion))
                        } else {
                            None
                        }
                    }
                },
            }
        });

    let components = new_components
        .chain(old_components)
        .chain(components_to_update)
        .map(|(comp, motive)| Update::new(comp, motive));

    Vec::from_iter(components)
}

fn display_warnings(components_with_motive: &[Update], newer: &Channel, options: &UpdateOptions) {
    let components_with_motive = components_with_motive.iter();

    // Warning for components installed from a PATH.
    {
        let components_from_path: Vec<String> = components_with_motive
            .clone()
            .filter_map(|update| match &update.component.version {
                Authority::Path { path, crate_name, .. } => Some((path, crate_name)),
                _ => None,
            })
            .map(|(path, crate_name)| {
                format!("- {} is installed from {}.\n", crate_name.bold(), path.display(),)
            })
            .collect();
        if !components_from_path.is_empty() {
            println!(
                "{}: The following elements are installed from a specific path in the filesystem.",
                "WARNING".yellow().bold(),
            );

            if matches!(options.path_update, PathUpdate::Off) {
                println!(
                    "
To make midenup update them all, pass the '--path-update=all' flag to `midenup update`.
Alternatively, pass the '--path-update=interactive' flag to interactively select which \
                     path-managed components to update.",
                );
            }
            for component_message in components_from_path {
                println!("{}", component_message);
            }
        }
    }

    // Warning for migrated components
    {
        let migrated_components: Vec<String> = components_with_motive
            .filter_map(|update| match &update.motive {
                UpdateMotive::Migrated { strategy } => Some((&update.component, strategy)),
                _ => None,
            })
            .map(|(component, strategy)| match strategy {
                MigrationStrategy::NameChange { old_channel } => {
                    format!("- {} from {} into {}", component.name, old_channel, newer)
                },
            })
            .collect();
        if !migrated_components.is_empty() {
            println!(
                "{}: The following elements are going to be migrated.",
                "WARNING".yellow().bold(),
            );

            for component_message in migrated_components {
                println!("{}", component_message);
            }
        }
    }
}
