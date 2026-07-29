//! Bringing an installed channel up to date with upstream.
//!
//! Update owns exactly two decisions: *which components have to be re-acquired*, and *which intent
//! the result is recorded under*. Everything else -- what the installed set should be, what to
//! stage, what to publish -- belongs to [`commands::install`], which re-resolves the persisted
//! intent against the new upstream channel.
//!
//! That is a deliberate narrowing. Update used to hand-build the channel to install, and the two
//! filters it applied there were both wrong: `update stable` intersected the new channel with the
//! locally installed component *names*, and a partially installed channel suppressed every new
//! component. Between them, a `minimal` installation could never gain a component newly tagged
//! `minimal`, and a project-activated toolchain could never gain anything at all.

use anyhow::Context;
use colored::Colorize;

use crate::{
    channel::{Channel, MigrationStrategy, UpstreamChannel, UpstreamMatch, UserChannel},
    commands,
    config::Config,
    manifest::Component,
    options::{InstallationOptions, IntentUpdate, PathUpdate, UpdateOptions},
    state::{Installation, LocalState},
    version::Authority,
};

/// Updates installed toolchains.
pub fn update(
    config: &Config,
    channel_type: Option<&UserChannel>,
    state: &mut LocalState,
    options: &UpdateOptions,
) -> anyhow::Result<()> {
    match channel_type {
        Some(UserChannel::Stable) => {
            let local_stable = state.latest_stable().cloned().context(
                "No stable version was found. To install it, try running:\nmidenup install \
                 stable\n",
            )?;

            println!("syncing channel updates for stable (installed as {})", local_stable.channel);

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
                upstream_stable.name,
                config.manifest.last_updated()
            );

            if upstream_stable.name > local_stable.channel {
                // A version bump. The installation is carried to the new channel: its intent
                // transfers verbatim and is re-resolved there, so the new channel gets everything
                // the old one was asked for -- including components that did not exist yet.
                let mut upstream = upstream_stable.clone();
                upstream.sync(config);
                install_for_update(
                    config,
                    &upstream,
                    state,
                    IntentUpdate::Replace(local_stable.intent.clone()),
                    Changes::default(),
                    options,
                )
            } else {
                // Already on the newest stable, which does not mean there is nothing to do: the
                // channel's own components may have moved.
                update_installed_channel(config, &local_stable, state, options)
            }
        },
        Some(UserChannel::Version(version)) => {
            let installation = state
                .get(version)
                .cloned()
                .context(format!("ERROR: No installed channel found with version {version}"))?;

            println!("syncing channel updates for {}", installation.channel);
            update_installed_channel(config, &installation, state, options)
        },
        None => {
            // Update everything installed. Cloned up front because each update writes state.
            for installation in state.installations.clone() {
                println!("syncing channel updates for {}", installation.channel);
                update_installed_channel(config, &installation, state, options)?;
            }
            Ok(())
        },
        Some(UserChannel::Nightly) => todo!(),
        Some(UserChannel::Other(_)) => todo!(),
    }
}

/// What an update has to do beyond re-resolving intent.
#[derive(Debug, Default, Clone)]
struct Changes {
    /// Components whose files must be re-acquired rather than carried forward.
    stale: Vec<String>,
    /// Components the update policy declined to touch, at the definition they are installed with.
    held_back: Vec<Component>,
}

fn update_installed_channel(
    config: &Config,
    installation: &Installation,
    state: &mut LocalState,
    options: &UpdateOptions,
) -> anyhow::Result<()> {
    let local_channel = installation.as_channel();
    let Some(upstream) = local_channel.find_upstream_counterpart(config) else {
        // A bit of an edge case. The channel is installed but absent upstream, so it is either a
        // developer toolchain or something that was withdrawn; either way there is nothing to
        // reconcile it against.
        return Ok(());
    };

    println!("upstream last updated on {}", config.manifest.last_updated());

    match migration_of(&upstream, installation) {
        Some(old_channel) => migrate(config, installation, &upstream.channel, state, options)
            .with_context(|| format!("failed to migrate channel {old_channel}")),
        None => {
            let Some(changes) = classify(installation, &upstream.channel, options)? else {
                println!(
                    "Aborting update of {} due to user input/configuration",
                    installation.channel
                );
                return Ok(());
            };

            install_for_update(
                config,
                &upstream.channel,
                state,
                IntentUpdate::Preserve,
                changes,
                options,
            )
        },
    }
}

/// The channel this one supersedes, if this really is a migration.
///
/// A channel that has already been migrated into is not migrated again; without this check, an
/// upstream channel declaring `migrates_from` would re-migrate itself on every update.
/// See <https://github.com/0xMiden/midenup/issues/193>.
fn migration_of(
    upstream: &UpstreamChannel,
    installation: &Installation,
) -> Option<semver::Version> {
    match &upstream.upstream_match {
        UpstreamMatch::UpstreamCounterpart => None,
        UpstreamMatch::Migrated(MigrationStrategy::NameChange { old_channel }) => {
            (upstream.channel.name != installation.channel).then(|| old_channel.clone())
        },
    }
}

/// Carries an installation to the channel that supersedes it.
///
/// The new channel is installed first and the old one removed only afterwards, so an interruption
/// leaves both -- which the next `midenup update` finishes -- rather than neither.
fn migrate(
    config: &Config,
    installation: &Installation,
    upstream: &Channel,
    state: &mut LocalState,
    options: &UpdateOptions,
) -> anyhow::Result<()> {
    println!(
        "{}: migrating {} to {}",
        "warning".yellow().bold(),
        installation.channel,
        upstream.name
    );

    install_for_update(
        config,
        upstream,
        state,
        // Intent transfers verbatim and is resolved against the new channel.
        IntentUpdate::Replace(installation.intent.clone()),
        Changes::default(),
        options,
    )?;

    // The sole exception to "nothing touches `var/`": the user's data follows the toolchain it
    // belongs to, rather than being stranded under a channel name that no longer exists. A rename,
    // so it is atomic and cannot half-happen.
    carry_var_to(config, &installation.channel, &upstream.name)?;

    let old_channel = installation.as_channel();
    // Not purged: the data has already been renamed to the new channel, and purging would delete a
    // directory that no longer belongs to the channel being removed.
    commands::uninstall(config, &old_channel, state, false)
}

/// Decides which components have to be re-acquired, applying the path-update policy.
///
/// `None` means the user cancelled.
fn classify(
    installation: &Installation,
    upstream: &Channel,
    options: &UpdateOptions,
) -> anyhow::Result<Option<Changes>> {
    let mut changes = Changes::default();

    for installed in &installation.components {
        // Absent upstream: there is nothing to re-acquire. Whether it stays installed is decided
        // by re-resolving intent, not here.
        let Some(upstream_component) = upstream.get_component(&installed.name) else {
            continue;
        };
        if installed.is_up_to_date(upstream_component) {
            continue;
        }

        match update_decision(installed, options)? {
            ComponentUpdateDecision::Abort => return Ok(None),
            ComponentUpdateDecision::Keep => changes.held_back.push(installed.clone()),
            ComponentUpdateDecision::Update => changes.stale.push(installed.name.to_string()),
        }
    }

    Ok(Some(changes))
}

/// Runs the install, unless there is demonstrably nothing to do.
///
/// The idempotency check is deliberately made against the *resolved* set rather than against a
/// hand-built channel: an update with no changed components can still have work to do, because
/// re-resolving the same intent against a new upstream channel can add or drop components.
fn install_for_update(
    config: &Config,
    upstream: &Channel,
    state: &mut LocalState,
    intent_update: IntentUpdate,
    changes: Changes,
    options: &UpdateOptions,
) -> anyhow::Result<()> {
    let install_options = InstallationOptions {
        verbose: options.verbose,
        stale: changes.stale,
        held_back: changes.held_back,
        intent_update: Some(intent_update),
        ..Default::default()
    };

    if nothing_to_do(upstream, state, &install_options)? {
        println!("Toolchain {} is up to date", upstream.name);
        return Ok(());
    }

    display_warnings(upstream, &install_options, options);

    println!("Updating toolchain {}..", upstream.name);
    commands::install(config, upstream, state, &install_options)
}

/// Whether the installed set already matches what this update would produce.
fn nothing_to_do(
    upstream: &Channel,
    state: &LocalState,
    options: &InstallationOptions,
) -> anyhow::Result<bool> {
    if !options.stale.is_empty() {
        return Ok(false);
    }

    let Some(installed) = state.get(&upstream.name) else {
        // Not installed yet -- a carried-over or migrated channel.
        return Ok(false);
    };

    let intent = commands::install::effective_intent(state, upstream, options);
    let resolved = crate::resolve::resolve(upstream, &intent)?;

    let installed_names: std::collections::BTreeSet<&str> =
        installed.components.iter().map(|component| component.name.as_ref()).collect();
    let resolved_names: std::collections::BTreeSet<&str> =
        resolved.iter().map(|component| component.name.as_ref()).collect();

    Ok(installed_names == resolved_names && installed.intent == intent)
}

/// Moves `var/<from>` to `var/<to>`, so client data follows a migrated channel.
///
/// A no-op when there is nothing to move. An existing destination is left alone: that means the new
/// channel already has data of its own, and silently merging or replacing it would be worse than
/// leaving the old directory where a user can find it.
fn carry_var_to(
    config: &Config,
    from: &semver::Version,
    to: &semver::Version,
) -> anyhow::Result<()> {
    if from == to {
        return Ok(());
    }

    let source = crate::paths::var_dir(&config.midenup_home, from);
    let destination = crate::paths::var_dir(&config.midenup_home, to);
    if !source.is_dir() || destination.exists() {
        return Ok(());
    }

    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create '{}'", parent.display()))?;
    }
    std::fs::rename(&source, &destination).with_context(|| {
        format!("failed to move '{}' to '{}'", source.display(), destination.display())
    })
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

enum ComponentUpdateDecision {
    /// Abort the update entirely
    Abort,
    /// Keep the installed version of the component
    Keep,
    /// Update the component to the version available in the channel
    Update,
}

fn update_decision(
    component: &Component,
    options: &UpdateOptions,
) -> anyhow::Result<ComponentUpdateDecision> {
    match &component.version {
        Authority::Registry { .. } | Authority::Git { .. } => Ok(ComponentUpdateDecision::Update),
        // Since uninstalling a component from the filesystem is potentially irreversible, we take
        // special precautions before uninstalling them.
        Authority::Path { .. } => match options.path_update {
            PathUpdate::Interactive => match handle_path_uninstall_interactive(component)? {
                InteractiveResult::Cancel => Ok(ComponentUpdateDecision::Abort),
                InteractiveResult::UpdateComponent => Ok(ComponentUpdateDecision::Update),
                InteractiveResult::DontUpdateComponent => Ok(ComponentUpdateDecision::Keep),
            },
            PathUpdate::All => Ok(ComponentUpdateDecision::Update),
            PathUpdate::Off => Ok(ComponentUpdateDecision::Keep),
        },
    }
}

fn display_warnings(
    upstream: &Channel,
    install_options: &InstallationOptions,
    options: &UpdateOptions,
) {
    let components_from_path: Vec<String> = upstream
        .components
        .iter()
        .filter_map(|component| match &component.version {
            Authority::Path { path, .. } => Some((path, component.crate_name()?)),
            _ => None,
        })
        .map(|(path, crate_name)| {
            format!("- {} is installed from {}.\n", crate_name.bold(), path.display())
        })
        .collect();

    if components_from_path.is_empty() {
        return;
    }

    println!(
        "\n{}: The following elements are installed from a specific path in the filesystem.",
        "WARNING".yellow().bold(),
    );

    if matches!(options.path_update, PathUpdate::Off) && !install_options.held_back.is_empty() {
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
