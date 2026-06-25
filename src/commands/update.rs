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
    let last_updated = local_manifest.last_updated();
    match channel_type {
        Some(UserChannel::Stable) => {
            let local_stable = local_manifest.get_latest_stable().context(
                "No stable version was found. To install it, try running:
midenup install stable
",
            )?;
            println!(
                "syncing channel updates for stable (last update was {last_updated} as {})",
                &local_stable.name
            );
            let upstream_stable = config
                .manifest
                .get_latest_stable()
                // NOTE: This means that there is no stable toolchain upstream.
                //
                // This is most likely an edge-case that shouldn't happen. If it does happen, it
                // probably means there's an error in midenup's parsing.
                .context("ERROR: No stable channel found in upstream")?;

            println!(
                "latest stable is version {} (upstream last updated on {})",
                &upstream_stable.name,
                config.manifest.last_updated()
            );

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

                let install_options = InstallationOptions::from(*options);
                commands::install(config, &channel_to_install, local_manifest, &install_options)?
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

            println!(
                "syncing channel updates for {} (last update was {last_updated})",
                &local_channel.name
            );

            let upstream_counterpart =
                local_channel.find_upstream_counterpart(config).context(format!(
                    "ERROR: Couldn't find a channel upstream with version {version}. Maybe it got \
                     removed."
                ))?;

            println!("upstream last updated on {}", config.manifest.last_updated());

            update_channel(config, &local_channel, &upstream_counterpart, local_manifest, options)?
        },
        None => {
            // Update all toolchains
            let mut channels_to_update = Vec::new();
            for local_channel in local_manifest.get_channels() {
                let upstream_counterpart = local_channel.find_upstream_counterpart(config);

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
                println!(
                    "syncing channel updates for {} (last update was {last_updated})",
                    &local_channel.name
                );
                println!("upstream last updated on {}", config.manifest.last_updated());
                update_channel(config, &local_channel, &upstream_channel, local_manifest, options)?;
            }
        },
        Some(UserChannel::Nightly) => todo!(),
        Some(UserChannel::Other(_)) => todo!(),
    }
    Ok(())
}

/// This function executes the actual update. Updates are the trickiest part of
/// the codebase.
///
/// We begin by computing the [[Update]] that needs to take place on the
/// [[compute_update]] (in some specific cases, the user to modify which
/// components get updated, like in the case of path managed components).
/// We then:
/// - Call the install function with the recently computed updated channel. Inside this function we:
///    - Copy the entire previous directory onto the new one
///    - Uninstall the components that need updating
/// - If required, uninstall the old channel
///
/// Copying the entire directory is done for two main reasons:
/// - Speeding installs up, skipping the need to re-install already installed components
/// - Most importantly, maintain `cargo install` consistency: When `cargo install`
///   is called, it generates metadata files which are then stored on the respective
///   toolchain directory (ATTOW, `.crates.toml` and `.crates.json`, for more
///   information see: https://doc.rust-lang.org/cargo/guide/cargo-home.html).
///   These files, as per the documentation, are not to be edited manually. So,
///   we take these extra precautions in order for midenup's local [[Manifest]]
///   and `cargo`'s to be synced.
fn update_channel(
    config: &Config,
    local_channel: &Channel,
    upstream_channel: &UpstreamChannel,
    local_manifest: &mut Manifest,
    options: &UpdateOptions,
) -> anyhow::Result<()> {
    let update = match compute_update(local_channel, upstream_channel, options)? {
        UpdatePlan::Abort => {
            println!("Aborting update of {} due to user input/configuration", local_channel);
            return Ok(());
        },
        UpdatePlan::Skip => {
            println!("Toolchain {} is up to date", local_channel);
            return Ok(());
        },
        UpdatePlan::Pending(update) => update,
    };

    display_warnings(&update, options);

    println!("Updating toolchain {}..", &local_channel.name);

    let Update {
        channel_to_install,
        components_to_uninstall,
        channel_to_uninstall,
    } = update;

    let install_options = InstallationOptions {
        verbose: options.verbose,
        components_to_uninstall,
    };

    commands::install(config, &channel_to_install, local_manifest, &install_options)?;

    if let Some(channel_to_install) = channel_to_uninstall {
        // If the update were to be interrupted before the uninstall finishes,
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

#[allow(clippy::large_enum_variant)]
enum UpdatePlan {
    /// The update command is being canceled/aborted due to user configuration or input
    Abort,
    /// The update has nothing to do, i.e. the toolchain is already up to date
    Skip,
    /// The update plan has been computed, and is pending application
    Pending(Update),
}

/// [[Update]] represents the set of changes that need to take place.
#[derive(Debug, Clone)]
pub struct Update {
    /// This is the channel that will be saved on the manifest and represents
    /// the newly updated channel. It contains:
    ///
    /// - The components that got added.
    /// - The components that got updated.
    /// - The components that are up to date, i.e. that stay the same.
    ///
    /// This channel also contains all the metadata from the channel it got computed from, i.e.:
    /// alias, tags, etc.
    pub channel_to_install: Channel,
    /// These are the components that need to be removed from the updated channel.
    pub components_to_uninstall: Vec<Component>,
    /// Channel that needs to be uninstalled due to a migration
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

/// This functions compares the `older` Channel, with the `newer` upstream channel and returns the
/// resulting [[Update]]. See [[Update]] for more information on what these imply.
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
fn compute_update(
    older: &Channel,
    newer: &UpstreamChannel,
    options: &UpdateOptions,
) -> anyhow::Result<UpdatePlan> {
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
                            // We don't want to migrate channels that have already been migrated
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

    // Compute the set of components in the old and new channels, in order to determine the effects
    // to apply to each component (i.e. install if present in the new channel, but not old;
    // uninstall if present in the old channel, but not new; and update if present in both sets).
    let new_channel: HashSet<ComponentByName> =
        newer.channel.components.iter().map(ComponentByName).collect();
    let current: HashSet<ComponentByName> = older.components.iter().map(ComponentByName).collect();

    // Extract the set of components to add (present in the new channel, not in the old)
    let new_components = new_channel
        .difference(&current)
        // If the channel is partially installed, then we explicitely don't want new components.
        .filter(|_| !older.is_partially_installed())
        .map(|&ComponentByName(comp)| ComponentUpdate::new(comp.clone(), UpdateStatus::Added));

    // Extract the set of components to remove (present in the old channel, not in the new)
    let old_components = current
        .difference(&new_channel)
        .map(|&ComponentByName(comp)| ComponentUpdate::new(comp.clone(), UpdateStatus::Removed));

    // Extract the set of components to update (present in both old and new channels)
    let changed_components = current.intersection(&new_channel);

    let mut components_to_install = Vec::from_iter(new_components);
    let mut components_to_uninstall = Vec::from_iter(old_components.map(|cu| cu.component));

    for component_by_name in changed_components {
        let new_component = new_channel.get(component_by_name).unwrap().0;
        let current_component = current.get(component_by_name).unwrap().0;
        // NOTE: that some components might ignore this update, such as components that were
        // installed via the filesystem.
        let update_status = {
            // If the channel got marked as migrated, then every single installed component is due
            // for an update.
            if let UpstreamMatch::Migrated(strategy) = &newer.upstream_match {
                UpdateStatus::Migrated { strategy: strategy.clone() }
            } else if !current_component.is_up_to_date(new_component) {
                // When a component needs an update, we must first uninstall the old component
                UpdateStatus::NeedsUpdate
            } else {
                UpdateStatus::UpToDate
            }
        };
        if matches!(update_status, UpdateStatus::NeedsUpdate) {
            match should_skip_component_update(current_component, options, older)? {
                ComponentUpdateDecision::Abort => return Ok(UpdatePlan::Abort),
                ComponentUpdateDecision::Keep(preserved_component) => {
                    // Do not update this component - add it to the set of components to install
                    // using the current component manifest, but do not add it to the set of
                    // components to uninstall.
                    //
                    // NOTE: This decision only occurs for components installed via path, in cases
                    // where the user explicitly does not want to install the version defined in
                    // the upstream manifest
                    components_to_install
                        .push(ComponentUpdate::new(preserved_component, update_status));
                },
                ComponentUpdateDecision::Update => {
                    // We need to reinstall this component
                    components_to_uninstall.push(current_component.clone());
                    components_to_install
                        .push(ComponentUpdate::new(new_component.clone(), update_status));
                },
            }
        } else {
            components_to_install.push(ComponentUpdate::new(new_component.clone(), update_status));
        }
    }

    // Handle migrations
    let migration = MigrationEffects::new(newer, older);

    // We check if an update is actually required. This is used for idempotency.
    {
        let all_components_up_to_date = components_to_install
            .iter()
            .all(|cu| matches!(cu.motive, UpdateStatus::UpToDate));
        if all_components_up_to_date && components_to_uninstall.is_empty() && !migration.required()
        {
            return Ok(UpdatePlan::Skip);
        }
    }

    let channel_to_install = {
        let components_to_install = components_to_install
                .into_iter()
                // We remove the metadata regarding why it needs to be installed,
                // since we already used it above.
                .map(|comp_update| comp_update.component)
                .collect::<Vec<_>>();

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

    Ok(UpdatePlan::Pending(update))
}

#[allow(clippy::large_enum_variant)]
enum ComponentUpdateDecision {
    /// Abort the update entirely
    Abort,
    /// Keep the given version of the component
    Keep(Component),
    /// Update the component to the version available in the channel
    Update,
}

fn should_skip_component_update(
    component: &Component,
    options: &UpdateOptions,
    local_channel: &Channel,
) -> anyhow::Result<ComponentUpdateDecision> {
    let skip_update = match component.get_installed_file() {
        InstalledFile::Library { .. } => false,
        InstalledFile::Executable { .. } => match component.version {
            Authority::Cargo { .. } | Authority::Git { .. } => false,
            // Since uninstalling a component from the filesystem is potentially
            // irreversible, we take special precautions before uninstalling them.
            Authority::Path { .. } => match options.path_update {
                PathUpdate::Interactive => match handle_path_uninstall_interactive(component)? {
                    InteractiveResult::Cancel => return Ok(ComponentUpdateDecision::Abort),
                    InteractiveResult::UpdateComponent => false,
                    InteractiveResult::DontUpdateComponent => true,
                },
                PathUpdate::All => false,
                PathUpdate::Off => true,
            },
        },
    };

    if skip_update && let Some(old) = local_channel.get_component(&component.name) {
        Ok(ComponentUpdateDecision::Keep(old.clone()))
    } else {
        Ok(ComponentUpdateDecision::Update)
    }
}

fn display_warnings(update: &Update, options: &UpdateOptions) {
    // Warning for components installed from a PATH.
    {
        let components_from_path: Vec<String> = update
            .channel_to_install
            .components
            .iter()
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
                "\n{}: The following elements are installed from a specific path in the \
                 filesystem.",
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
        // A pending channel uninstall can only mean one thing: the channel got
        // migrated, and every component is going to be carried over.
        if let Some(old_channel) = &update.channel_to_uninstall {
            let migrated_components: Vec<String> = update
                .channel_to_install
                .components
                .iter()
                .map(|component| {
                    format!(
                        "- {} from {} into {}",
                        component.name, old_channel, update.channel_to_install
                    )
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
}
