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
    let Some(Update {
        mut channel_to_install,
        components_to_uninstall,
        channel_to_uninstall,
    }) = compute_update(local_channel, upstream_channel)
    else {
        println!("Toolchain {} is up to date", local_channel);
        return Ok(());
    };

    display_warnings(&channel_to_install, &upstream_channel.channel, options);

    for component in channel_to_install.components.iter_mut() {
        let skip_update = match component.get_installed_file() {
            InstalledFile::Library { .. } => false,
            InstalledFile::Executable { .. } => match component.version {
                Authority::Cargo { .. } | Authority::Git { .. } => false,
                // Since uninstalling a component from the filesystem is potentially
                // irreversible, we take special precautions before uninstalling them.
                Authority::Path { .. } => match options.path_update {
                    PathUpdate::Interactive => {
                        match handle_path_uninstall_interactive(component)? {
                            InteractiveResult::Cancel => return Ok(()),
                            InteractiveResult::UpdateComponent => false,
                            InteractiveResult::DontUpdateComponent => true,
                        }
                    },
                    PathUpdate::All => false,
                    PathUpdate::Off => true,
                },
            },
        };

        if skip_update && let Some(old) = local_channel.get_component(&component.name) {
            *component = old.clone();
        }
    }

    let install_options = InstallationOptions {
        verbose: options.verbose,
        components_to_uninstall,
    };

    commands::install(config, &channel_to_install, local_manifest, &install_options)?;

    if let Some(channel_to_install) = channel_to_uninstall {
        // If the update were to be interrumpted before the uninstall finishes,
        // re-running `midenup update` would finish the process.
        // This does mean that channel migration is a non-atomic operation.
        commands::uninstall(config, &channel_to_install, local_manifest)?;
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
pub enum UpdateStatus {
    /// This component was added to the toolchain and wasn't there before.
    Added,
    /// This component was removed and is no longer part of the toolchain.
    Removed,
    /// A newer version was released.
    NeedsUpdate,
    /// The entire channel was migrated.
    Migrated { strategy: MigrationStrategy },
    /// The component doesn't need updating.
    UpToDate,
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
pub struct ComponentUpdate {
    pub component: Component,
    pub motive: UpdateStatus,
}

impl ComponentUpdate {
    fn new(component: Component, motive: UpdateStatus) -> ComponentUpdate {
        ComponentUpdate { component, motive }
    }
}

#[derive(Debug, Clone)]
/// [[Update]] represents the set of changes that need to take place.
pub struct Update {
    /// This is the channel that will be saved on the manifest and represents
    /// the newly updated channel. It contains:
    /// - The components that got added.
    /// - The components that got updated.
    /// - The components that are up to date, i.e. that stay the same.
    ///
    /// This channel also contains all the metadata from the channel it got
    /// computed from, i.e.: alias, tags, etc.
    pub channel_to_install: Channel,
    /// These are the components that need to be removed from the updated
    /// channel. These contain:
    /// - The components that got removed.
    pub components_to_uninstall: Vec<Component>,
    /// Channel that needs to be uninstalled. This can be caused due to:
    /// - A channel migration
    pub channel_to_uninstall: Option<Channel>,
}

impl Update {
    fn new(
        channel_to_install: Channel,
        components_to_uninstall: Vec<Component>,
        channel_to_uninstall: Option<Channel>,
    ) -> Update {
        Update {
            channel_to_install,
            components_to_uninstall,
            channel_to_uninstall,
        }
    }
}
/// This functions compares the Channel &older, with a newer upstream channel
/// [newer] and returns the resulting [[Update]]. See [[Update]] for more information
/// on what these imply.
///
/// Regarding components, one can be marked for update in the following scenarios:
///
/// - The component got removed from the upstream channel entirely and thus needs to be removed from
///   the system.
/// - A new component is present in the upstream manifest and thus needs to be installed.
/// - A newer version of a present component is released and thus an upgrade is due.
/// - An *older* version of a component is released and thus a downgrade is due.
/// - A components [Authority] got changed and thus needs to be removed and re-installed with the
///   new [Authority]
///
/// There is one notable exception to this rule which is when a channel is migrated into a different
/// channel. In that case, every component is marked for update.
pub fn compute_update(older: &Channel, newer: &UpstreamChannel) -> Option<Update> {
    struct MigrationEffects<'a> {
        strategy: Option<&'a MigrationStrategy>,
        newer: &'a Channel,
        older: &'a Channel,
    }

    impl<'a> MigrationEffects<'a> {
        fn new(upstream_channel: &'a UpstreamChannel, older: &'a Channel) -> Self {
            match &upstream_channel.upstream_match {
                UpstreamMatch::UpstreamCounterpart => Self {
                    strategy: None,
                    newer: &upstream_channel.channel,
                    older,
                },
                UpstreamMatch::Migrated(migration_strategy) => {
                    match migration_strategy {
                        MigrationStrategy::NameChange { old_channel: _old_channel } => {
                            // We don't want to migrate channels that have already
                            // been migrated
                            // See https://github.com/0xMiden/midenup/issues/193
                            if older.name == upstream_channel.channel.name {
                                Self {
                                    strategy: None,
                                    newer: &upstream_channel.channel,
                                    older,
                                }
                            } else {
                                Self {
                                    strategy: Some(migration_strategy),
                                    newer: &upstream_channel.channel,
                                    older,
                                }
                            }
                        },
                    }
                },
            }
        }

        /// Applies the migration to `channel` in place.
        fn migrate_channel(&self, channel: &mut Channel) {
            match self.strategy {
                Some(MigrationStrategy::NameChange { .. }) => {
                    // The old channel needs to have its name match the
                    // upstream channel's
                    channel.name = self.newer.name.clone();
                },
                None => (),
            }
        }

        fn channel_to_uninstall(&self) -> Option<Channel> {
            #[allow(clippy::manual_map)]
            match self.strategy {
                Some(MigrationStrategy::NameChange { .. }) => Some(self.older.clone()),
                None => None,
            }
        }

        fn required(&self) -> bool {
            self.strategy.is_some()
        }
    }

    // We turn the components into hashsets in order to compute the venn diagram-like operations
    let new_channel: HashSet<ComponentByName> =
        newer.channel.components.iter().map(ComponentByName).collect();
    let current: HashSet<ComponentByName> = older.components.iter().map(ComponentByName).collect();

    // This is the subset of new components present in the channel since last sync.
    let new_components = new_channel
        .difference(&current)
        // If the channel is partially installed, then we explicitely don't
        // want new components.
        .filter(|_| !older.is_partially_installed())
        .map(|&ComponentByName(comp)| ComponentUpdate::new(comp.clone(), UpdateStatus::Added));

    // This is the subset of old components that need to be removed.
    let old_components = current
        .difference(&new_channel)
        .map(|&ComponentByName(comp)| ComponentUpdate::new(comp.clone(), UpdateStatus::Removed));

    let mut components_to_install = Vec::new();
    let mut components_to_uninstall =
        Vec::from_iter(old_components.map(|comp_update| comp_update.component));

    // These are the elements that are present in boths sets. We compute wether they need updating
    // or are kept as is.
    for component_by_name in current.iter()
        // We filter these components since they were already taken into account
        // on the new_components set
        .filter(|comp| new_channel.contains(*comp))
    {
        let new_component = new_channel.get(component_by_name);
        let current_component = component_by_name.0;
        let component_update = match new_component {
            // This should't be possible, but if somehow the component is missing, then we
            // trigger an update for said component regardless.
            None => ComponentUpdate::new(current_component.clone(), UpdateStatus::Added),
            // Note that some components might ignore this update, such as
            // components that were installed via the filesystem.
            Some(&ComponentByName(new_component)) => {
                // If the channel got marked as migrated, then every single
                // installed component is due for an update.
                if let UpstreamMatch::Migrated(strategy) = &newer.upstream_match {
                    ComponentUpdate::new(
                        current_component.clone(),
                        UpdateStatus::Migrated { strategy: strategy.clone() },
                    )
                } else {
                    if !current_component.is_up_to_date(new_component) {
                        // When a component needs an update, we must first
                        // uninstall the old component
                        components_to_uninstall.push(current_component.clone());
                        ComponentUpdate::new(new_component.clone(), UpdateStatus::NeedsUpdate)
                    } else {
                        ComponentUpdate::new(current_component.clone(), UpdateStatus::UpToDate)
                    }
                }
            },
        };
        components_to_install.push(component_update)
    }

    let components_to_install: Vec<ComponentUpdate> =
        new_components.chain(components_to_install).collect();

    // Handle migrations
    let migration = MigrationEffects::new(newer, older);

    // We check if an Update is actually due. This is used for safe `midenup update`
    // re-calls.
    {
        let all_components_up_to_date = components_to_install
            .iter()
            .all(|comp_update| matches!(comp_update.motive, UpdateStatus::UpToDate));
        if all_components_up_to_date && components_to_uninstall.is_empty() && !migration.required()
        {
            return None;
        }
    }

    let channel_to_install = {
        let components_to_install = {
            let components_to_install = components_to_install
                .into_iter()
                // We remove the metadata regarding why it needs to be installed,
                // since we already used it above.
                .map(|comp_update| comp_update.component);
            Vec::from_iter(components_to_install)
        };

        // We clone the older channel as a template in order to get the metadata
        // from the installed channel (tags, etc).
        let mut channel_to_install = older.clone();

        channel_to_install.components = components_to_install;

        // If no migration is needed, this will be a no-op
        migration.migrate_channel(&mut channel_to_install);

        channel_to_install
    };

    let channel_to_uninstall = migration.channel_to_uninstall();

    let update = Update::new(channel_to_install, components_to_uninstall, channel_to_uninstall);

    Some(update)
}

fn display_warnings(channel_to_install: &Channel, _newer: &Channel, options: &UpdateOptions) {
    let components_with_motive = channel_to_install.components.iter();

    // Warning for components installed from a PATH.
    {
        let components_from_path: Vec<String> = components_with_motive
            .filter_map(|component| match &component.version {
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
    // {
    //     let migrated_components: Vec<String> = components_with_motive
    //         .filter_map(|update| match &update.motive {
    //             UpdateStatus::Migrated { strategy } => Some((&update.component, strategy)),
    //             _ => None,
    //         })
    //         .map(|(component, strategy)| match strategy {
    //             MigrationStrategy::NameChange { old_channel } => {
    //                 format!("- {} from {} into {}", component.name, old_channel, newer)
    //             },
    //         })
    //         .collect();
    //     if !migrated_components.is_empty() {
    //         println!(
    //             "{}: The following elements are going to be migrated.",
    //             "WARNING".yellow().bold(),
    //         );

    //         for component_message in migrated_components {
    //             println!("{}", component_message);
    //         }
    //     }
    // }
}
