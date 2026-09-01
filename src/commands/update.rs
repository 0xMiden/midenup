//! Bringing an installed channel up to date with upstream.
//!
//! Update owns exactly two decisions: *which components have to be re-acquired*, and *which intent
//! the result is recorded under*. Everything else -- what the installed set should be, what to
//! stage, what to publish -- belongs to [`commands::install`], which re-resolves the persisted
//! intent against the new upstream channel. That is a deliberate narrowing.

use std::{
    io::{BufRead, Write},
    path::Path,
};

use anyhow::Context;
use colored::Colorize;

use crate::{
    channel::{Channel, UpstreamChannel, UpstreamMatch, UserChannel},
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
        Some(UserChannel::Named(name)) => update_network(config, name, state, options),
        Some(UserChannel::Version(version)) => {
            let installation = state
                .get(version)
                .cloned()
                .context(format!("ERROR: No installed channel found with version {version}"))?;

            update_installed_channel(config, &installation, state, options)
        },
        None => {
            if state.installations.is_empty() {
                crate::info!("nothing to update: no toolchains are installed");
                return Ok(());
            }
            // Update everything installed. Cloned up front because each update writes state.
            for installation in state.installations.clone() {
                update_installed_channel(config, &installation, state, options)?;
            }
            Ok(())
        },
    }
}

/// Brings a network to the channel it now names.
///
/// A network is a moving name, so what has to be reconciled is *the pointer*, not the channel: if
/// `networks[name]` has moved, the installation is carried to wherever it now points. This is
/// deliberately not `migrates_from` lineage. That describes a relationship between two channels and
/// is what someone tracking a pinned version follows; a user tracking mainnet asked for mainnet.
///
/// Only the pointer and the installation move. The network's `var/` is keyed by the network, not by
/// the channel ([`crate::paths::var_dir`]), so it is already where the new channel will look for it
/// and there is nothing here to carry.
///
/// The comparison is inequality, not "is newer". The pointer is authoritative in both directions:
/// a rollback is rare, and `update-manifest promote` refuses to author one without an explicit
/// flag, but once one is published, following it is what tracking the network means.
fn update_network(
    config: &Config,
    name: &str,
    state: &mut LocalState,
    options: &UpdateOptions,
) -> anyhow::Result<()> {
    let manifest = config.upstream_manifest()?;
    let target = manifest.network_version(name).cloned().with_context(|| {
        format!(
            "unknown channel '{name}'; known networks are {}",
            manifest.network_names().collect::<Vec<_>>().join(", ")
        )
    })?;

    let user_channel = UserChannel::Named(name.to_string().into());
    let installed = config.local_channel(&user_channel).with_context(|| {
        format!("{name} is not installed. To install it, run:\n    midenup install {name}\n")
    })?;

    let installation = state.get(&installed).cloned().with_context(|| {
        format!(
            "toolchains/{name} names {installed}, which is not in local state; reinstall with: \
             midenup install {name}"
        )
    })?;

    crate::info!("{name} is now {target} (installed as {installed})");

    if installed == target {
        // The pointer has not moved, which does not mean there is nothing to do: the channel's own
        // components may have.
        return update_installed_channel(config, &installation, state, options);
    }

    let mut upstream = manifest.get_channel_by_name(&target).cloned().with_context(|| {
        format!("network '{name}' names channel {target}, which is not upstream")
    })?;

    if target < installed {
        crate::warn!("{name} has moved back from {installed} to {target}.");
    }

    upstream.sync(config);
    install_for_update(
        config,
        &upstream,
        state,
        // Intent transfers verbatim and is re-resolved against the channel now being tracked, so
        // it gains components that did not exist there before.
        IntentUpdate::Replace(installation.intent.clone()),
        Changes::default(),
        options,
    )?;

    // DERIVE writes this link too, and writing it twice is a rename onto the same target -- but
    // DERIVE runs only inside `commands::install`, and an update whose target is already installed
    // with the same intent comes back as `Work::Nothing` without going there. That is the rollback
    // case, where moving the pointer is the entire operation, so it is done unconditionally here.
    // Nothing has to precede or follow it: a network's `var/` is keyed by the network and does not
    // move with the pointer.
    let link = crate::paths::network_link(&config.midenup_home, name);
    crate::utils::fs::replace_symlink(&link, Path::new(&target.to_string()))
        .with_context(|| format!("failed to point '{name}' at {target}"))
}

/// How a component changed between what is installed and what upstream now says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeClass {
    /// Its files have to be replaced: authority, kind, installation method, artifacts,
    /// destinations, modes, Cargo features, rustup channel, or symlink layout.
    ///
    /// Defined precisely as "its contribution to the plan key changed" (spec section 11.1), and
    /// computed that way rather than by enumerating fields, so the rule cannot drift from the key.
    InstallationImpacting,
    /// `requires` or `profiles`. Changes what gets *selected*, not what an unchanged component's
    /// files look like.
    GraphOnly,
    /// Aliases, call format, subcommands, `initialization`. Recorded in local state and resolved at
    /// dispatch; nothing on disk depends on them, and `initialization` is still never executed.
    RuntimeMetadataOnly,
    /// Nothing at all.
    None,
}

/// Classifies `new` against the installed `old`.
///
/// A component whose *old* definition cannot be planned -- an artifact that no longer resolves for
/// this target, say -- is reported as installation-impacting: equality could not be established,
/// and reinstalling is the safe direction to be wrong in.
pub fn classify(old: &Component, new: &Component, target: &str, cwd: &Path) -> ChangeClass {
    let key_of = |component: &Component| crate::plan::component_key(component, target, cwd).ok();

    match (key_of(old), key_of(new)) {
        (Some(old_key), Some(new_key)) if old_key == new_key => {},
        _ => return ChangeClass::InstallationImpacting,
    }

    if old.requires != new.requires || old.profiles != new.profiles {
        return ChangeClass::GraphOnly;
    }

    // Anything left is metadata: the key already accounted for everything material, and the
    // authority is part of the key, so a structural difference here cannot be a physical one.
    match (serde_json::to_value(old), serde_json::to_value(new)) {
        (Ok(old), Ok(new)) if old == new => ChangeClass::None,
        _ => ChangeClass::RuntimeMetadataOnly,
    }
}

/// What an update has to do beyond re-resolving intent.
#[derive(Debug, Default, Clone)]
struct Changes {
    /// Components whose files must be re-acquired rather than carried forward.
    stale: Vec<String>,
    /// Components the update policy declined to touch, at the definition they are installed with.
    held_back: Vec<Component>,
    /// Whether anything changed that local state records but no file reflects.
    logical_only: bool,
}

fn update_installed_channel(
    config: &Config,
    installation: &Installation,
    state: &mut LocalState,
    options: &UpdateOptions,
) -> anyhow::Result<()> {
    config.upstream_manifest()?;

    let local_channel = installation.as_channel();
    let Some(upstream) = local_channel.find_upstream_counterpart(config) else {
        // A bit of an edge case. The channel is installed but absent upstream, so it is either a
        // developer toolchain or something that was withdrawn; either way there is nothing to
        // reconcile it against.
        crate::info!(
            "channel {} is not in the upstream manifest; leaving it as installed",
            installation.channel
        );
        return Ok(());
    };

    match migration_of(&upstream, installation) {
        Some(old_channel) => migrate(config, installation, &upstream.channel, state, options)
            .with_context(|| format!("failed to migrate channel {old_channel}")),
        None => {
            let Some(changes) = changes_for(config, installation, &upstream.channel, options)?
            else {
                crate::info!(
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
fn migration_of(
    upstream: &UpstreamChannel,
    installation: &Installation,
) -> Option<semver::Version> {
    match &upstream.upstream_match {
        UpstreamMatch::UpstreamCounterpart => None,
        UpstreamMatch::Migrated { old_channel } => {
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
    crate::warn!("migrating {} to {}", installation.channel, upstream.name);

    install_for_update(
        config,
        upstream,
        state,
        // Intent transfers verbatim and is resolved against the new channel.
        IntentUpdate::Replace(installation.intent.clone()),
        Changes::default(),
        options,
    )?;

    // A migration is the one operation that changes what a *pinned* selector means: the channel the
    // user pinned ceases to exist and is superseded by another. `var/<pinned version>` is keyed by
    // that selector, so it has to follow, or it would be stranded under a name nothing can select
    // any more. A rename, so it is atomic and cannot half-happen.
    //
    // A network selector is unaffected: `var/<network>` is not keyed by either channel, so it is
    // neither the source nor the destination here.
    carry_var_to(&config.midenup_home, &installation.channel, &upstream.name)?;

    // Not purged: the data has already been renamed to the new channel, and purging would delete a
    // directory that no longer belongs to the channel being removed.
    commands::uninstall(
        config,
        &crate::channel::UserChannel::Version(installation.channel.clone()),
        state,
        false,
    )
}

/// Decides which components have to be re-acquired, applying the path-update policy.
///
/// `None` means the user cancelled.
fn changes_for(
    config: &Config,
    installation: &Installation,
    upstream: &Channel,
    options: &UpdateOptions,
) -> anyhow::Result<Option<Changes>> {
    let mut changes = Changes::default();
    let cwd = &config.working_directory;

    for installed in &installation.components {
        // Absent upstream: there is nothing to re-acquire. Whether it stays installed is decided
        // by re-resolving intent, not here.
        let Some(upstream_component) = upstream.get_component(&installed.name) else {
            continue;
        };

        match classify(installed, upstream_component, config.target(), cwd) {
            ChangeClass::None => continue,
            // Neither moves a byte on disk, but both change what local state records.
            ChangeClass::GraphOnly | ChangeClass::RuntimeMetadataOnly => {
                changes.logical_only = true;
            },
            ChangeClass::InstallationImpacting => match update_decision(installed, options)? {
                ComponentUpdateDecision::Abort => return Ok(None),
                ComponentUpdateDecision::Keep => changes.held_back.push(installed.clone()),
                ComponentUpdateDecision::Update => changes.stale.push(installed.name.to_string()),
            },
        }
    }

    Ok(Some(changes))
}

/// Runs the install, unless less than that is needed.
///
/// The idempotency check is deliberately made against the *resolved* set rather than against the
/// set of changed components: an update with no changed components can still have work to do,
/// because re-resolving the same intent against a new upstream channel can add or drop components.
fn install_for_update(
    config: &Config,
    upstream: &Channel,
    state: &mut LocalState,
    intent_update: IntentUpdate,
    changes: Changes,
    options: &UpdateOptions,
) -> anyhow::Result<()> {
    let logical_only = changes.logical_only;
    let install_options = InstallationOptions {
        stale: changes.stale,
        held_back: changes.held_back,
        intent_update: Some(intent_update),
        ..Default::default()
    };

    match work_for(upstream, state, &install_options, logical_only)? {
        Work::Physical => {
            display_warnings(upstream, &install_options, options);
            crate::info!("Updating toolchain {}..", upstream.name);
            commands::install(config, upstream, state, &install_options)
        },
        // Spec section 9.8: a change that touches selection or runtime metadata but no installed
        // file is committed as a single atomic `state.json` write. No journal, no staging, no new
        // publication -- republishing an identical tree to record an alias would be pure cost.
        Work::LogicalOnly => {
            crate::info!("Updating recorded metadata for toolchain {}..", upstream.name);
            record_logical_changes(config, upstream, state, &install_options)
        },
        Work::Nothing => {
            crate::info!("Toolchain {} is up to date", upstream.name);
            Ok(())
        },
    }
}

/// How much of the protocol an update actually needs.
enum Work {
    /// Stage and publish.
    Physical,
    /// Rewrite `state.json` and nothing else.
    LogicalOnly,
    Nothing,
}

fn work_for(
    upstream: &Channel,
    state: &LocalState,
    options: &InstallationOptions,
    logical_only: bool,
) -> anyhow::Result<Work> {
    if !options.stale.is_empty() {
        return Ok(Work::Physical);
    }

    let Some(installed) = state.get(&upstream.name) else {
        // Not installed yet -- a carried-over or migrated channel.
        return Ok(Work::Physical);
    };

    let intent = commands::install::effective_intent(state, upstream, options);
    let resolved = crate::resolve::resolve(upstream, &intent)?;

    let installed_names: std::collections::BTreeSet<&str> =
        installed.components.iter().map(|component| component.name.as_ref()).collect();
    let resolved_names: std::collections::BTreeSet<&str> =
        resolved.iter().map(|component| component.name.as_ref()).collect();

    if installed_names != resolved_names {
        return Ok(Work::Physical);
    }
    if logical_only || installed.intent != intent {
        return Ok(Work::LogicalOnly);
    }
    Ok(Work::Nothing)
}

/// Commits selection and metadata changes that no installed file reflects.
///
/// Each component's recorded *authority* is preserved rather than taken from upstream. Reaching
/// here means the authority is materially unchanged, and the recorded one carries what was pinned
/// at install time -- a branch's commit, a path's modification time -- which upstream does not
/// have. Overwriting it would discard the pin and make the next update believe the source moved.
fn record_logical_changes(
    config: &Config,
    upstream: &Channel,
    state: &mut LocalState,
    options: &InstallationOptions,
) -> anyhow::Result<()> {
    let intent = commands::install::effective_intent(state, upstream, options);

    let installation = state
        .get_mut(&upstream.name)
        .with_context(|| format!("channel {} is not installed", upstream.name))?;
    installation.intent = intent;

    for component in installation.components.iter_mut() {
        let Some(upstream_component) = upstream.get_component(&component.name) else {
            continue;
        };
        if options.held_back.iter().any(|held| held.name == component.name) {
            continue;
        }

        let pinned = component.version.clone();
        *component = upstream_component.clone();
        component.version = pinned;
    }

    config.write_local_state(state)
}

/// Renames `var/<from>` to `var/<to>` for a channel migration, the sole caller.
///
/// Both keys are pinned-version selectors, and migration is the only thing that retires one. See
/// the call site in [`migrate`] for why nothing else may move a `var/` directory.
///
/// An existing destination is left alone rather than merged into or replaced: the user has selected
/// `to` directly at some point and has data under it, and the old directory is more useful left
/// where they can find it than silently folded into another store.
fn carry_var_to(home: &Path, from: &semver::Version, to: &semver::Version) -> anyhow::Result<()> {
    if from == to {
        return Ok(());
    }

    let source = crate::paths::var_dir(home, &UserChannel::Version(from.clone()));
    let destination = crate::paths::var_dir(home, &UserChannel::Version(to.clone()));
    if !source.is_dir() {
        return Ok(());
    }
    if destination.exists() {
        crate::warn!(
            "var/{from} was left in place: you already have data at var/{to}, and the two are not \
             merged."
        );
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

#[derive(Debug, PartialEq, Eq)]
enum InteractiveResult {
    /// Cancel the update all together. Useful for potential miss-clicks.
    Cancel,
    UpdateComponent,
    DontUpdateComponent,
}

fn handle_path_uninstall_interactive(component: &Component) -> anyhow::Result<InteractiveResult> {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    crate::report::with_stderr_interaction(|output| {
        handle_path_uninstall_interactive_with_io(component, &mut input, output)
    })
}

fn handle_path_uninstall_interactive_with_io<R: BufRead, W: Write + ?Sized>(
    component: &Component,
    input: &mut R,
    output: &mut W,
) -> anyhow::Result<InteractiveResult> {
    let component_name = &component.name;
    writeln!(
        output,
        "Would you like to update this component? (N/y/c)
   - N: no, skip this component
   - y: yes, update this component
   - c: cancel the update all-together (no changes will be applied)"
    )
    .context("failed to write update prompt")?;
    output.flush().context("failed to flush update prompt")?;

    let mut response = String::new();
    input.read_line(&mut response).context("failed to read update response")?;
    let response = response.trim().to_ascii_lowercase();
    let result = match response.as_str() {
        "y" => {
            writeln!(output, "Updating {component_name}")
                .context("failed to write update response")?;
            InteractiveResult::UpdateComponent
        },
        "c" => {
            writeln!(output, "Cancelling update, no changes will be applied.")
                .context("failed to write update response")?;
            InteractiveResult::Cancel
        },
        _ => {
            writeln!(output, "Skipping {component_name}, it will not be updated")
                .context("failed to write update response")?;
            InteractiveResult::DontUpdateComponent
        },
    };
    output.flush().context("failed to flush update response")?;
    Ok(result)
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

    crate::report::prepare_stderr_color();
    let guidance = if matches!(options.path_update, PathUpdate::Off)
        && !install_options.held_back.is_empty()
    {
        "
To make midenup update them all, pass the '--path-update=all' flag to `midenup update`.
Alternatively, pass the '--path-update=interactive' flag to interactively select which \
         path-managed components to update."
    } else {
        ""
    };
    let listed = components_from_path.concat();
    crate::warn!(
        "the following elements are installed from a specific path in the filesystem.{guidance}
{listed}"
    );
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;
    use crate::{
        artifact::{Artifact, Artifacts},
        manifest::{ComponentKind, ExecutableComponent, InstallationMethod},
        profile::Profile,
        version::Authority,
    };

    // No test here changes `initialization`, deliberately: it lives in the same struct as the
    // aliases below and travels the same path, so it would prove nothing extra -- and naming the
    // field in a command module would trip the guard in `manifest::v3::component` that keeps it
    // from ever acquiring an execution path.

    const TARGET: &str = "aarch64-apple-darwin";

    fn cwd() -> &'static Path {
        Path::new(".")
    }

    /// A prebuilt executable with one artifact, which is the shape most components have.
    fn base() -> Component {
        let mut artifacts = Artifacts::default();
        artifacts.insert(
            "miden-vm".to_string(),
            Artifact::TargetAgnostic {
                uri: "https://example.invalid/v1/miden-vm".to_string(),
                digest: None,
                archive: None,
                extra: Default::default(),
            },
        );

        Component {
            name: Cow::Borrowed("vm"),
            version: Authority::Registry { version: semver::Version::new(0, 1, 0) },
            kind: ComponentKind::Executable {
                installation_method: InstallationMethod::Prebuilt,
                spec: ExecutableComponent {
                    installed_executable: "miden-vm".to_string(),
                    ..Default::default()
                },
            },
            profiles: vec![Profile::Minimal],
            requires: vec![],
            artifacts,
            extra: Default::default(),
        }
    }

    fn classify_pair(old: Component, new: Component) -> ChangeClass {
        classify(&old, &new, TARGET, cwd())
    }

    fn spec_of(component: &mut Component) -> &mut ExecutableComponent {
        match &mut component.kind {
            ComponentKind::Executable { spec, .. } => spec,
            _ => unreachable!("the fixture is an executable"),
        }
    }

    #[test]
    fn an_identical_component_has_not_changed() {
        assert_eq!(classify_pair(base(), base()), ChangeClass::None);
    }

    /// An artifact is a file in the publication, so a component whose artifact URI moves to a new
    /// release must be reinstalled even though nothing else about it changed.
    #[test]
    fn an_artifact_only_change_is_installation_impacting() {
        let mut new = base();
        new.artifacts.insert(
            "miden-vm".to_string(),
            Artifact::TargetAgnostic {
                uri: "https://example.invalid/v2/miden-vm".to_string(),
                digest: None,
                archive: None,
                extra: Default::default(),
            },
        );

        assert_eq!(classify_pair(base(), new), ChangeClass::InstallationImpacting);
    }

    #[test]
    fn a_version_change_is_installation_impacting() {
        let mut new = base();
        new.version = Authority::Registry { version: semver::Version::new(0, 2, 0) };
        assert_eq!(classify_pair(base(), new), ChangeClass::InstallationImpacting);
    }

    /// The `opt/` shims are real files, so their layout is material even though nothing else about
    /// the component moved.
    #[test]
    fn a_symlink_layout_change_is_installation_impacting() {
        let mut new = base();
        spec_of(&mut new).symlink_name = Some("miden-vm-next".to_string());
        assert_eq!(classify_pair(base(), new), ChangeClass::InstallationImpacting);
    }

    /// An alias is resolved at dispatch and no installed file depends on it, so adding one to an
    /// otherwise identical component must not force a reinstall.
    #[test]
    fn an_alias_only_change_is_runtime_metadata_only() {
        let mut new = base();
        spec_of(&mut new)
            .aliases
            .insert("run".to_string(), crate::exec::Executable::default_call_format());

        assert_eq!(classify_pair(base(), new), ChangeClass::RuntimeMetadataOnly);
    }

    #[test]
    fn a_requires_only_change_is_graph_only() {
        let mut new = base();
        new.requires = vec!["core".to_string()];
        assert_eq!(classify_pair(base(), new), ChangeClass::GraphOnly);
    }

    #[test]
    fn a_profiles_only_change_is_graph_only() {
        let mut new = base();
        new.profiles = vec![Profile::Complete];
        assert_eq!(classify_pair(base(), new), ChangeClass::GraphOnly);
    }

    #[test]
    fn interactive_path_updates_write_and_flush_their_ui() {
        for (input, expected, acknowledgement) in [
            ("y\n", InteractiveResult::UpdateComponent, "Updating vm"),
            ("c\n", InteractiveResult::Cancel, "Cancelling update"),
            ("n\n", InteractiveResult::DontUpdateComponent, "Skipping vm"),
        ] {
            let mut input = std::io::Cursor::new(input);
            let mut output = FlushTrackingWriter::default();
            let result =
                handle_path_uninstall_interactive_with_io(&base(), &mut input, &mut output)
                    .expect("interaction must succeed");

            assert_eq!(result, expected);
            let rendered = String::from_utf8(output.bytes).unwrap();
            assert!(rendered.contains("Would you like to update this component?"));
            assert!(rendered.contains(acknowledgement), "missing acknowledgement: {rendered}");
            assert_eq!(output.flushes, 2, "prompt and acknowledgement must both be flushed");
        }
    }

    #[derive(Default)]
    struct FlushTrackingWriter {
        bytes: Vec<u8>,
        flushes: usize,
    }

    impl std::io::Write for FlushTrackingWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }
}
