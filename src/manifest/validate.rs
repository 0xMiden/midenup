//! Structural validation of a channel manifest.
//!
//! Deliberately **platform-neutral**: a manifest is not invalid because *this* machine cannot
//! install one of its components. Whether an artifact exists for the current target, and whether
//! the component/artifact cardinality matrix holds for a selected target, are decided when an
//! installation plan is built -- not here. Conflating the two would make a manifest "invalid" on a
//! Mac and valid on Linux.
//!
//! Every rule returns rather than short-circuits, so `update-manifest check` can report everything
//! wrong with a manifest in one pass instead of one error per run.
//!
//! # This is not run at parse time
//!
//! Loading a manifest is deliberately permissive. Validation is an *authoring* gate
//! (`update-manifest check`) and an *install-time* gate (plan construction, scoped to the channel
//! actually being installed) -- not a precondition for reading the document.
//!
//! The reason is concrete. The manifest published at the time of writing has dangling requirements
//! in channel 0.13.3: `midenc` requires `base` and `std`, which were renamed to `core` and
//! `protocol`. Failing the parse would make every `midenup` and `miden` invocation fail for every
//! user, including those on channels that are perfectly well-formed, because one stale channel in
//! the same file is broken.
//!
//! This is the same rule applied to unrecognized component kinds: a defect that is scoped to one
//! part of a manifest must not take down the rest of it. A broken channel becomes uninstallable;
//! it does not make the tool unusable.

use std::collections::{BTreeSet, HashMap};

use super::{Channel, Component, ComponentKind, Extra, Manifest};
use crate::{
    artifact::Artifact,
    plan::{destination_for, validate_artifact_id},
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ValidationError {
    #[error("duplicate channel '{0}': channel names must be unique")]
    DuplicateChannel(semver::Version),
    #[error("channel {channel}: duplicate component '{component}'")]
    DuplicateComponent {
        channel: semver::Version,
        component: String,
    },
    #[error(
        "channel {channel}: component '{component}' requires '{requires}', which is not in the \
         channel"
    )]
    UnknownRequirement {
        channel: semver::Version,
        component: String,
        requires: String,
    },
    #[error("channel {channel}: components form a requirement cycle: {}", path.join(" -> "))]
    RequirementCycle {
        channel: semver::Version,
        path: Vec<String>,
    },
    #[error("manifest timestamp must be a positive UTC epoch, got {0}")]
    InvalidTimestamp(i64),
    #[error("channel {channel}: component '{component}' has an invalid {field}: {reason}")]
    InvalidName {
        channel: semver::Version,
        component: String,
        field: &'static str,
        reason: String,
    },
    #[error("channel {channel}: components '{first}' and '{second}' both install to '{path}'")]
    DestinationCollision {
        channel: semver::Version,
        first: String,
        second: String,
        path: String,
    },
    #[error("channel {channel}: alias '{alias}' is defined by both '{first}' and '{second}'")]
    ConflictingAlias {
        channel: semver::Version,
        alias: String,
        first: String,
        second: String,
    },
    #[error(
        "channel {channel}: alias '{alias}' on component '{component}' collides with the command \
         name of component '{collides_with}'"
    )]
    AliasShadowsCommand {
        channel: semver::Version,
        alias: String,
        component: String,
        collides_with: String,
    },
    #[error(
        "channel {channel}: component '{component}' is hidden but defines no aliases, so it can \
         never be invoked"
    )]
    HiddenWithoutAliases {
        channel: semver::Version,
        component: String,
    },
    #[error(
        "channel {channel}: command component '{component}' defines neither a format nor any \
         aliases, so it can never be invoked"
    )]
    UnreachableCommand {
        channel: semver::Version,
        component: String,
    },
    #[error("network '{network}' names channel {version}, which is not in this manifest")]
    DanglingNetwork {
        network: String,
        version: semver::Version,
    },
    #[error("network name '{name}' is invalid: {reason}")]
    InvalidNetworkName { name: String, reason: String },
    #[error(
        "this manifest declares no '{0}' network, which is the channel midenup uses when nothing \
         else selects one"
    )]
    MissingDefaultNetwork(&'static str),
    #[error(
        "channel {channel}: artifact '{artifact}' of component '{component}' declares archive \
         format '{format}', which midenup cannot read; supported formats are {supported}"
    )]
    UnsupportedArchiveFormat {
        channel: semver::Version,
        component: String,
        artifact: String,
        format: String,
        supported: String,
    },
    #[error(
        "channel {channel}: artifact '{artifact}' of component '{component}' is fetched from a \
         '{format}' archive but does not declare one, so the archive itself would be installed as \
         '{artifact}'; add \"archive\": \"{format}\""
    )]
    UndeclaredArchive {
        channel: semver::Version,
        component: String,
        artifact: String,
        format: &'static str,
    },
    #[error(
        "manifest timestamp {next} does not advance past the previous manifest's {previous}; run \
         `update-manifest touch`"
    )]
    StaleTimestamp { previous: i64, next: i64 },
    #[error(
        "manifest_version moves from {previous} to {next}: a major version change requires a new \
         midenup release before the manifest can be published"
    )]
    SchemaMajorChanged {
        previous: semver::Version,
        next: semver::Version,
    },
    #[error("network '{network}' is declared by the previous manifest but not by this one")]
    NetworkRemoved { network: String },
    #[error(
        "network '{network}' moves back from {from} to {to}; pass --allow-downgrade if that is \
         intended"
    )]
    NetworkDowngraded {
        network: String,
        from: semver::Version,
        to: semver::Version,
    },
    #[error(
        "channel {channel} is named by network(s) {} in the previous manifest but is not in this \
         one, and no channel declares `migrates_from` it",
        networks.join(", ")
    )]
    TrackedChannelRemoved {
        channel: semver::Version,
        networks: Vec<String>,
    },
    #[error("channel {0} declares `migrates_from` itself")]
    SelfMigration(semver::Version),
    #[error(
        "channels {first} and {second} both declare `migrates_from` {from}; an installation of \
         {from} can only be carried to one of them"
    )]
    AmbiguousMigration {
        from: semver::Version,
        first: semver::Version,
        second: semver::Version,
    },
    #[error(
        "channel {channel} declares `migrates_from` {from}, which is newer; a channel can only \
         supersede an older one"
    )]
    BackwardMigration {
        channel: semver::Version,
        from: semver::Version,
    },
    #[error(
        "{location}: unknown field '{field}'; a misspelled field is kept but never read, and a \
         field from a newer schema cannot be published by this build"
    )]
    UnknownField { location: String, field: String },
}

/// Validates a whole manifest, returning every problem found.
pub fn validate_manifest(manifest: &Manifest) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    if manifest.date <= 0 {
        errors.push(ValidationError::InvalidTimestamp(manifest.date));
    }

    let mut seen_channels = BTreeSet::new();
    for channel in manifest.channels.iter() {
        if !seen_channels.insert(channel.name.clone()) {
            errors.push(ValidationError::DuplicateChannel(channel.name.clone()));
        }
        validate_channel(channel, &mut errors);
    }

    validate_networks(manifest, &mut errors);
    validate_migrations(manifest, &mut errors);
    validate_unknown_fields(manifest, &mut errors);

    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

/// Rules over fields the parser preserved without recognizing.
///
/// Reading tolerates them so that a newer manifest does not break an older `midenup`; publishing
/// must not, because in the checked-in manifest an unknown key is a typo that would be kept and
/// never read. An empty value is exempt: a field the schema omits when empty is filed under the
/// extras too when it is written out explicitly.
fn validate_unknown_fields(manifest: &Manifest, errors: &mut Vec<ValidationError>) {
    fn is_empty_value(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Null => true,
            serde_json::Value::Bool(set) => !set,
            serde_json::Value::Array(items) => items.is_empty(),
            serde_json::Value::Object(members) => members.is_empty(),
            _ => false,
        }
    }

    fn report(location: &str, extra: &Extra, errors: &mut Vec<ValidationError>) {
        for (field, _) in extra.iter().filter(|(_, value)| !is_empty_value(value)) {
            errors.push(ValidationError::UnknownField {
                location: location.to_string(),
                field: field.clone(),
            });
        }
    }

    report("manifest", &manifest.extra, errors);
    for channel in manifest.channels.iter() {
        report(&format!("channel {}", channel.name), &channel.extra, errors);
        for component in channel.components.iter() {
            let at = format!("channel {}: component '{}'", channel.name, component.name);
            report(&at, &component.extra, errors);
            for (id, artifact) in component.artifacts.artifacts.iter() {
                let at = format!("{at}, artifact '{id}'");
                match artifact {
                    Artifact::TargetSpecific {
                        substitutions, targets, archive, extra, ..
                    } => {
                        report(&at, extra, errors);
                        if let Some(substitutions) = substitutions {
                            report(&format!("{at} substitutions"), &substitutions.extra, errors);
                        }
                        for (target, substitutions) in targets {
                            report(
                                &format!("{at} target '{target}'"),
                                &substitutions.extra,
                                errors,
                            );
                        }
                        if let Some(archive) = archive {
                            report(&format!("{at} archive"), &archive.extra, errors);
                        }
                    },
                    Artifact::TargetAgnostic { archive, extra, .. } => {
                        report(&at, extra, errors);
                        if let Some(archive) = archive {
                            report(&format!("{at} archive"), &archive.extra, errors);
                        }
                    },
                }
            }
        }
    }
}

/// Rules over `migrates_from`, which `update` follows to find a removed channel's successor.
///
/// The named channel need not exist: it is usually the one that was removed. What must hold is
/// that following the declaration has exactly one answer.
fn validate_migrations(manifest: &Manifest, errors: &mut Vec<ValidationError>) {
    let mut successors: HashMap<&semver::Version, &semver::Version> = HashMap::new();
    for channel in manifest.channels.iter() {
        let Some(from) = channel.migrates_from.as_ref() else {
            continue;
        };
        if from == &channel.name {
            errors.push(ValidationError::SelfMigration(channel.name.clone()));
            continue;
        }
        if from > &channel.name {
            errors.push(ValidationError::BackwardMigration {
                channel: channel.name.clone(),
                from: from.clone(),
            });
        }
        if let Some(previous) = successors.insert(from, &channel.name) {
            errors.push(ValidationError::AmbiguousMigration {
                from: from.clone(),
                first: previous.clone(),
                second: channel.name.clone(),
            });
        }
    }
}

/// Validates `next` as a replacement for `previous`, returning every problem found.
///
/// Every rule here needs two documents: it is about what a change does to users of the previous
/// manifest, which no single document can say.
pub fn validate_against(
    previous: &Manifest,
    next: &Manifest,
    allow_downgrade: bool,
) -> Result<(), Vec<ValidationError>> {
    // An unchanged manifest replaces the previous one trivially. Checked first so that a pull
    // request leaving the manifest alone is not failed for not advancing its timestamp.
    if previous == next {
        return Ok(());
    }

    let mut errors = Vec::new();

    if next.date <= previous.date {
        errors.push(ValidationError::StaleTimestamp { previous: previous.date, next: next.date });
    }

    if next.manifest_version().major != previous.manifest_version().major {
        errors.push(ValidationError::SchemaMajorChanged {
            previous: previous.manifest_version().clone(),
            next: next.manifest_version().clone(),
        });
    }

    for (network, tracked) in previous.networks.iter() {
        match next.network_version(network) {
            None => errors.push(ValidationError::NetworkRemoved { network: network.clone() }),
            Some(now) if now < tracked && !allow_downgrade => {
                errors.push(ValidationError::NetworkDowngraded {
                    network: network.clone(),
                    from: tracked.clone(),
                    to: now.clone(),
                });
            },
            Some(_) => {},
        }
    }

    // A channel users track may disappear only if update can carry them somewhere: `update`
    // follows a channel declaring `migrates_from` the one that vanished.
    let tracked: BTreeSet<&semver::Version> = previous.networks.values().collect();
    for channel in tracked {
        let still_present = next.get_channel_by_name(channel).is_some();
        let superseded = next.channels.iter().any(|c| c.migrates_from.as_ref() == Some(channel));
        if !still_present && !superseded {
            errors.push(ValidationError::TrackedChannelRemoved {
                channel: channel.clone(),
                networks: previous.networks_for(channel).map(str::to_string).collect(),
            });
        }
    }

    if errors.is_empty() { Ok(()) } else { Err(errors) }
}

/// Rules over the `networks` map.
///
/// Deliberately no ordering invariant between networks. It is tempting to require that devnet be at
/// least testnet, and testnet at least mainnet, but a mainnet hotfix legitimately inverts it, and a
/// validator that has to be overridden during an incident is worse than no validator.
fn validate_networks(manifest: &Manifest, errors: &mut Vec<ValidationError>) {
    use crate::channel::{DEFAULT_NETWORK, canonical_network};

    if !manifest.networks.contains_key(DEFAULT_NETWORK) {
        errors.push(ValidationError::MissingDefaultNetwork(DEFAULT_NETWORK));
    }

    let known: BTreeSet<&semver::Version> = manifest.channels.iter().map(|c| &c.name).collect();

    for (name, version) in manifest.networks.iter() {
        let invalid =
            |reason: String| ValidationError::InvalidNetworkName { name: name.clone(), reason };

        if name.is_empty() {
            errors.push(invalid("a network must have a name".to_string()));
        } else if let Err(err) = validate_artifact_id(name) {
            // A network name is joined straight onto `toolchains/` and written with
            // `replace_symlink`, which renames over whatever is at that path. `../../../.zshrc`
            // would make an ordinary `midenup install` replace a file outside `$MIDENUP_HOME`.
            // Same rule as every other name that becomes a path segment, deliberately.
            errors.push(invalid(err.to_string()));
        } else if semver::Version::parse(name).is_ok() {
            errors.push(invalid(
                "a network may not be named like a channel, which would make 'midenup install \
                 <name>' ambiguous"
                    .to_string(),
            ));
        } else if canonical_network(name) != name {
            errors.push(invalid(format!(
                "'{name}' is rewritten to '{}' before any lookup, so a network declared under it \
                 could never be reached",
                canonical_network(name)
            )));
        }

        if !known.contains(version) {
            errors.push(ValidationError::DanglingNetwork {
                network: name.clone(),
                version: version.clone(),
            });
        }
    }
}

fn validate_channel(channel: &Channel, errors: &mut Vec<ValidationError>) {
    let mut seen: HashMap<&str, ()> = HashMap::new();
    for component in channel.components.iter() {
        if seen.insert(component.name.as_ref(), ()).is_some() {
            errors.push(ValidationError::DuplicateComponent {
                channel: channel.name.clone(),
                component: component.name.to_string(),
            });
        }
    }

    validate_requirements(channel, errors);
    validate_names(channel, errors);
    validate_destinations(channel, errors);
    validate_aliases(channel, errors);
}

fn validate_requirements(channel: &Channel, errors: &mut Vec<ValidationError>) {
    let known: BTreeSet<&str> = channel.components.iter().map(|c| c.name.as_ref()).collect();
    for component in channel.components.iter() {
        for requires in component.requires.iter() {
            if !known.contains(requires.as_str()) {
                errors.push(ValidationError::UnknownRequirement {
                    channel: channel.name.clone(),
                    component: component.name.to_string(),
                    requires: requires.clone(),
                });
            }
        }
    }

    // Only look for cycles once every edge is known to resolve; otherwise the graph is incomplete
    // and any cycle it reports is an artefact of the missing nodes.
    if errors.iter().any(|e| matches!(e, ValidationError::UnknownRequirement { .. })) {
        return;
    }

    let index: HashMap<&str, usize> = channel
        .components
        .iter()
        .enumerate()
        .map(|(i, c)| (c.name.as_ref(), i))
        .collect();
    let mut graph = petgraph::graphmap::DiGraphMap::<usize, ()>::new();
    for (i, component) in channel.components.iter().enumerate() {
        graph.add_node(i);
        for requires in component.requires.iter() {
            if let Some(&j) = index.get(requires.as_str()) {
                graph.add_edge(j, i, ());
            }
        }
    }

    for scc in petgraph::algo::tarjan_scc(&graph) {
        let is_cycle =
            scc.len() > 1 || scc.first().is_some_and(|&n| graph.neighbors(n).any(|m| m == n));
        if is_cycle {
            let mut path: Vec<String> =
                scc.iter().map(|&i| channel.components[i].name.to_string()).collect();
            path.sort();
            errors.push(ValidationError::RequirementCycle { channel: channel.name.clone(), path });
        }
    }
}

fn validate_names(channel: &Channel, errors: &mut Vec<ValidationError>) {
    for component in channel.components.iter() {
        let invalid = |field: &'static str, reason: String| ValidationError::InvalidName {
            channel: channel.name.clone(),
            component: component.name.to_string(),
            field,
            reason,
        };

        if let ComponentKind::Executable { spec, .. } | ComponentKind::CargoExtension { spec, .. } =
            component.kind()
        {
            if let Err(err) = validate_artifact_id(&spec.installed_executable) {
                errors.push(invalid("installed-executable", err.to_string()));
            }
            if let Some(symlink) = spec.symlink_name.as_deref()
                && let Err(err) = validate_artifact_id(symlink)
            {
                errors.push(invalid("symlink-name", err.to_string()));
            }
            if spec.hide && spec.aliases.is_empty() {
                errors.push(ValidationError::HiddenWithoutAliases {
                    channel: channel.name.clone(),
                    component: component.name.to_string(),
                });
            }
        }

        // A command is reachable through any of three routes: a bare `format`, a declared
        // subcommand, or an alias. The real `node` component declares only `subcommands` -- each
        // carrying its full `docker compose ...` invocation -- so requiring `format` here would
        // reject the shipped manifest.
        if let ComponentKind::Command { format, aliases, subcommands, .. } = component.kind()
            && format.is_empty()
            && aliases.is_empty()
            && subcommands.is_empty()
        {
            errors.push(ValidationError::UnreachableCommand {
                channel: channel.name.clone(),
                component: component.name.to_string(),
            });
        }

        for (id, artifact) in component.artifacts.artifacts.iter() {
            if let Err(err) = validate_artifact_id(id) {
                errors.push(invalid("artifact id", err.to_string()));
            }

            // Stricter than parsing on purpose: an unreadable format parses, so that one channel
            // cannot break every other command (spec section 4.4), but nothing should be
            // *published* that the publishing build cannot install.
            match artifact.archive() {
                Some(archive) if !archive.format.is_supported() => {
                    errors.push(ValidationError::UnsupportedArchiveFormat {
                        channel: channel.name.clone(),
                        component: component.name.to_string(),
                        artifact: id.to_string(),
                        format: archive.format.to_string(),
                        // From the format table, so adding a format updates what this reports.
                        supported: crate::artifact::ArchiveFormat::supported_spellings()
                            .collect::<Vec<_>>()
                            .join(", "),
                    });
                },
                Some(_) => {},
                None => {
                    if let Some(format) = undeclared_archive(id, artifact, component) {
                        errors.push(ValidationError::UndeclaredArchive {
                            channel: channel.name.clone(),
                            component: component.name.to_string(),
                            artifact: id.to_string(),
                            format,
                        });
                    }
                },
            }
        }
    }
}

/// The archive format an artifact is evidently fetched from while declaring none.
///
/// This is the one artifact mistake nothing downstream can catch: the container arrives as a
/// regular file of the planned mode, so it installs, verifies and records as a success, and only
/// fails when something tries to run it. Judged on the *resolved* URI, so a `%extension` of
/// `tar.gz` is seen the same as a literal one, and on the path it is fetched from, so a query
/// string or a fragment cannot carry the extension out of sight.
///
/// An artifact id carrying the same extension is left alone: `bundle.tar.gz` installed as
/// `bundle.tar.gz` is asking for the archive itself, which is a legitimate thing to want.
fn undeclared_archive(
    id: &str,
    artifact: &crate::artifact::Artifact,
    component: &Component,
) -> Option<&'static str> {
    // Best effort: an artifact whose URI cannot be resolved at all has its own errors, and platform
    // -specific resolution is not this validator's business.
    let uris = artifact.get_uris_for(id, component).ok()?;

    crate::artifact::ArchiveFormat::supported_spellings().find(|spelling| {
        let suffix = format!(".{spelling}");
        !id.ends_with(&suffix) && uris.iter().any(|uri| uri.path().ends_with(&suffix))
    })
}

fn validate_destinations(channel: &Channel, errors: &mut Vec<ValidationError>) {
    // A notional root: collisions are a property of the manifest, not of where it is installed.
    let root = std::path::Path::new("/");
    let mut claimed: HashMap<std::path::PathBuf, &Component> = HashMap::new();

    for component in channel.components.iter() {
        for id in component.artifacts.artifacts.keys() {
            let Ok(destination) = destination_for(component, id, root) else {
                // Unsupported kinds and invalid ids are reported by their own rules.
                continue;
            };
            if let Some(previous) = claimed.insert(destination.path.clone(), component)
                && previous.name != component.name
            {
                errors.push(ValidationError::DestinationCollision {
                    channel: channel.name.clone(),
                    first: previous.name.to_string(),
                    second: component.name.to_string(),
                    path: destination.path.display().to_string(),
                });
            }
        }
    }
}

fn validate_aliases(channel: &Channel, errors: &mut Vec<ValidationError>) {
    // Direct command names first: `miden <name>` for anything callable.
    let mut command_names: HashMap<String, &str> = HashMap::new();
    for component in channel.components.iter() {
        if let Some(display) = component.get_cli_display() {
            let name = display.trim_start_matches("miden ").to_string();
            command_names.insert(name, component.name.as_ref());
        }
    }

    let mut aliases: HashMap<String, &str> = HashMap::new();
    for component in channel.components.iter() {
        let declared: Vec<&String> = match component.kind() {
            ComponentKind::Executable { spec, .. } | ComponentKind::CargoExtension { spec, .. } => {
                spec.aliases.keys().collect()
            },
            ComponentKind::Command { aliases, .. } => aliases.keys().collect(),
            _ => vec![],
        };

        for alias in declared {
            if let Some(&owner) = command_names.get(alias)
                && owner != component.name.as_ref()
            {
                errors.push(ValidationError::AliasShadowsCommand {
                    channel: channel.name.clone(),
                    alias: alias.clone(),
                    component: component.name.to_string(),
                    collides_with: owner.to_string(),
                });
            }
            if let Some(previous) = aliases.insert(alias.clone(), component.name.as_ref())
                && previous != component.name.as_ref()
            {
                errors.push(ValidationError::ConflictingAlias {
                    channel: channel.name.clone(),
                    alias: alias.clone(),
                    first: previous.to_string(),
                    second: component.name.to_string(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;
    use crate::{
        artifact::{Artifact, Artifacts},
        exec::Executable,
        manifest::{ExecutableComponent, InstallationMethod},
        profile::Profile,
        version::Authority,
    };

    fn component(name: &'static str, kind: ComponentKind) -> Component {
        Component {
            name: Cow::Borrowed(name),
            version: Authority::Registry { version: semver::Version::new(0, 1, 0) },
            kind,
            profiles: vec![Profile::Minimal],
            requires: vec![],
            artifacts: Artifacts::default(),
            extra: Default::default(),
        }
    }

    fn executable(name: &'static str, installed: &str) -> Component {
        component(
            name,
            ComponentKind::Executable {
                installation_method: InstallationMethod::Prebuilt,
                spec: ExecutableComponent {
                    installed_executable: installed.to_string(),
                    ..Default::default()
                },
            },
        )
    }

    fn with_artifact(mut component: Component, id: &str) -> Component {
        component.artifacts.insert(
            id.to_string(),
            Artifact::TargetAgnostic {
                uri: "https://example.invalid/x".to_string(),
                digest: None,
                archive: None,
                extra: Default::default(),
            },
        );
        component
    }

    fn manifest(components: Vec<Component>) -> Manifest {
        let mut m = Manifest::default();
        m.channels.push(Channel::new(semver::Version::new(0, 15, 0), components));
        m
    }

    fn errors_of(m: &Manifest) -> Vec<ValidationError> {
        validate_manifest(m).err().unwrap_or_default()
    }

    fn with_mainnet(mut m: Manifest) -> Manifest {
        m.promote(crate::channel::DEFAULT_NETWORK, semver::Version::new(0, 15, 0));
        m
    }

    /// A previous manifest with `mainnet` on 0.15.0 and a 0.16.0 toolchain waiting.
    fn previous() -> Manifest {
        let mut m = with_mainnet(manifest(vec![executable("vm", "miden-vm")]));
        m.channels.push(Channel::new(semver::Version::new(0, 16, 0), vec![]));
        m.date = 1000;
        m
    }

    /// A successor to [`previous`] whose only change is an advanced timestamp.
    fn successor() -> Manifest {
        let mut m = previous();
        m.date = 2000;
        m
    }

    fn errors_against(next: &Manifest) -> Vec<ValidationError> {
        validate_against(&previous(), next, false).err().unwrap_or_default()
    }

    #[test]
    fn a_successor_that_only_advances_the_timestamp_passes() {
        assert_eq!(validate_against(&previous(), &successor(), false), Ok(()));
    }

    /// Most pull requests do not touch the manifest at all; those must not be judged.
    #[test]
    fn an_unchanged_manifest_passes_without_advancing_the_timestamp() {
        assert_eq!(validate_against(&previous(), &previous(), false), Ok(()));
    }

    #[test]
    fn a_timestamp_that_does_not_advance_is_rejected() {
        let mut m = successor();
        m.promote("mainnet", semver::Version::new(0, 16, 0));
        m.date = 1000;
        assert!(
            errors_against(&m).iter().any(|e| matches!(
                e,
                ValidationError::StaleTimestamp { previous: 1000, next: 1000 }
            ))
        );
        m.date = 999;
        assert!(
            errors_against(&m)
                .iter()
                .any(|e| matches!(e, ValidationError::StaleTimestamp { .. }))
        );
    }

    #[test]
    fn a_schema_major_change_is_rejected_but_a_minor_is_not() {
        let mut m = successor();
        m.manifest_version = semver::Version::new(4, 0, 0);
        assert!(
            errors_against(&m)
                .iter()
                .any(|e| matches!(e, ValidationError::SchemaMajorChanged { .. }))
        );

        let mut m = successor();
        m.manifest_version = semver::Version::new(3, 1, 0);
        assert!(
            !errors_against(&m)
                .iter()
                .any(|e| matches!(e, ValidationError::SchemaMajorChanged { .. }))
        );
    }

    #[test]
    fn a_removed_network_is_rejected() {
        let mut m = successor();
        m.networks.clear();
        assert!(errors_against(&m).iter().any(|e| matches!(
            e,
            ValidationError::NetworkRemoved { network } if network == "mainnet"
        )));
    }

    #[test]
    fn a_network_moving_backwards_needs_the_flag() {
        let mut ahead = previous();
        ahead.promote("mainnet", semver::Version::new(0, 16, 0));
        let mut next = successor();
        next.promote("mainnet", semver::Version::new(0, 15, 0));

        let errors = validate_against(&ahead, &next, false).err().unwrap_or_default();
        assert!(errors.iter().any(|e| matches!(
            e,
            ValidationError::NetworkDowngraded { network, .. } if network == "mainnet"
        )));
        assert_eq!(validate_against(&ahead, &next, true), Ok(()));
    }

    #[test]
    fn a_network_moving_forward_passes() {
        let mut m = successor();
        m.promote("mainnet", semver::Version::new(0, 16, 0));
        assert_eq!(validate_against(&previous(), &m, false), Ok(()));
    }

    /// The channel `mainnet` names disappears, and nothing takes over from it.
    #[test]
    fn removing_a_tracked_channel_is_rejected() {
        let mut m = successor();
        m.promote("mainnet", semver::Version::new(0, 16, 0));
        m.remove_channel(semver::Version::new(0, 15, 0));
        assert!(errors_against(&m).iter().any(|e| matches!(
            e,
            ValidationError::TrackedChannelRemoved { channel, networks }
                if *channel == semver::Version::new(0, 15, 0) && networks == &["mainnet"]
        )));
    }

    /// The same removal is fine when a channel declares `migrates_from` the removed one, because
    /// `update` follows that declaration.
    #[test]
    fn removing_a_tracked_channel_with_a_successor_passes() {
        let mut m = successor();
        m.promote("mainnet", semver::Version::new(0, 16, 0));
        m.remove_channel(semver::Version::new(0, 15, 0));
        m.get_channel_by_name_mut(&semver::Version::new(0, 16, 0))
            .unwrap()
            .migrates_from = Some(semver::Version::new(0, 15, 0));
        assert_eq!(validate_against(&previous(), &m, false), Ok(()));
    }

    #[test]
    fn an_unknown_field_is_rejected_wherever_it_appears() {
        let mut m =
            with_mainnet(manifest(vec![with_artifact(executable("vm", "miden-vm"), "miden-vm")]));
        m.extra = Extra::from_iter([("dat".to_string(), serde_json::json!(1))]);
        m.channels[0].extra =
            Extra::from_iter([("migrate_from".to_string(), serde_json::json!("0.14.0"))]);
        m.channels[0].components[0].extra =
            Extra::from_iter([("instaled-executable".to_string(), serde_json::json!("miden-vm"))]);
        if let Artifact::TargetAgnostic { extra, .. } =
            m.channels[0].components[0].artifacts.artifacts.get_mut("miden-vm").unwrap()
        {
            *extra = Extra::from_iter([("digset".to_string(), serde_json::json!("sha256:00"))]);
        }

        let fields: Vec<(String, String)> = errors_of(&m)
            .into_iter()
            .filter_map(|e| match e {
                ValidationError::UnknownField { location, field } => Some((location, field)),
                _ => None,
            })
            .collect();
        assert_eq!(fields.len(), 4, "{fields:?}");
        assert!(fields.contains(&("manifest".to_string(), "dat".to_string())));
        assert!(fields.iter().any(|(at, f)| at == "channel 0.15.0" && f == "migrate_from"));
        assert!(
            fields
                .iter()
                .any(|(at, f)| at.ends_with("component 'vm'") && f == "instaled-executable")
        );
        assert!(
            fields
                .iter()
                .any(|(at, f)| at.ends_with("artifact 'miden-vm'") && f == "digset")
        );
    }

    /// An explicitly empty field the schema omits when empty parses into the extras; it is not a
    /// typo and must not be reported.
    #[test]
    fn an_empty_unknown_value_is_not_reported() {
        let mut m = with_mainnet(manifest(vec![executable("vm", "miden-vm")]));
        m.channels[0].components[0].extra = Extra::from_iter([
            ("requires".to_string(), serde_json::json!([])),
            ("aliases".to_string(), serde_json::json!({})),
            ("hide".to_string(), serde_json::json!(false)),
            ("symlink-name".to_string(), serde_json::Value::Null),
        ]);
        assert!(!errors_of(&m).iter().any(|e| matches!(e, ValidationError::UnknownField { .. })));
    }

    #[test]
    fn a_channel_migrating_from_itself_is_rejected() {
        let mut m = with_mainnet(manifest(vec![]));
        m.channels[0].migrates_from = Some(semver::Version::new(0, 15, 0));
        assert!(errors_of(&m).iter().any(|e| matches!(
            e,
            ValidationError::SelfMigration(v) if *v == semver::Version::new(0, 15, 0)
        )));
    }

    #[test]
    fn two_channels_migrating_from_the_same_one_are_rejected() {
        let mut m = with_mainnet(manifest(vec![]));
        m.channels[0].migrates_from = Some(semver::Version::new(0, 14, 0));
        let mut other = Channel::new(semver::Version::new(0, 16, 0), vec![]);
        other.migrates_from = Some(semver::Version::new(0, 14, 0));
        m.channels.push(other);
        assert!(errors_of(&m).iter().any(|e| matches!(
            e,
            ValidationError::AmbiguousMigration { from, .. } if *from == semver::Version::new(0, 14, 0)
        )));
    }

    #[test]
    fn migrating_from_a_newer_channel_is_rejected() {
        let mut m = with_mainnet(manifest(vec![]));
        m.channels[0].migrates_from = Some(semver::Version::new(0, 16, 0));
        assert!(errors_of(&m).iter().any(|e| matches!(
            e,
            ValidationError::BackwardMigration { from, .. } if *from == semver::Version::new(0, 16, 0)
        )));
    }

    /// The predecessor is usually the channel that was removed, so it need not be present.
    #[test]
    fn migrating_from_an_absent_channel_is_valid() {
        let mut m = with_mainnet(manifest(vec![]));
        m.channels[0].migrates_from = Some(semver::Version::new(0, 14, 0));
        assert!(!errors_of(&m).iter().any(|e| matches!(
            e,
            ValidationError::SelfMigration(_) | ValidationError::AmbiguousMigration { .. }
        )));
    }

    /// Removing a channel no network names is cleanup, not a release break.
    #[test]
    fn removing_an_untracked_channel_passes() {
        let mut m = successor();
        m.remove_channel(semver::Version::new(0, 16, 0));
        assert_eq!(validate_against(&previous(), &m, false), Ok(()));
    }

    #[test]
    fn a_valid_manifest_passes() {
        let m =
            with_mainnet(manifest(vec![with_artifact(executable("vm", "miden-vm"), "miden-vm")]));
        assert_eq!(validate_manifest(&m), Ok(()));
    }

    #[test]
    fn a_network_naming_an_absent_channel_is_rejected() {
        let mut m = with_mainnet(manifest(vec![]));
        m.promote("testnet", semver::Version::new(9, 9, 9));
        assert!(errors_of(&m).iter().any(|e| matches!(
            e,
            ValidationError::DanglingNetwork { network, version }
                if network == "testnet" && *version == semver::Version::new(9, 9, 9)
        )));
    }

    /// `midenup install 0.15.0` must be unambiguous, so a network may not be named like a channel.
    #[test]
    fn a_network_named_like_a_version_is_rejected() {
        let mut m = with_mainnet(manifest(vec![]));
        m.promote("0.15.0", semver::Version::new(0, 15, 0));
        assert!(errors_of(&m).iter().any(
            |e| matches!(e, ValidationError::InvalidNetworkName { name, .. } if name == "0.15.0")
        ));
    }

    /// `stable` is rewritten to `mainnet` before any lookup happens, so a network declared under
    /// that name could never be reached. The same holds for every traditional synonym.
    #[test]
    fn a_network_named_after_a_synonym_is_rejected() {
        for synonym in ["stable", "beta", "nightly"] {
            let mut m = with_mainnet(manifest(vec![]));
            m.promote(synonym, semver::Version::new(0, 15, 0));
            assert!(
                errors_of(&m).iter().any(
                    |e| matches!(e, ValidationError::InvalidNetworkName { name, .. } if name == synonym)
                ),
                "'{synonym}' must be rejected as a network name"
            );
        }
    }

    #[test]
    fn an_empty_network_name_is_rejected() {
        let src = serde_json::json!({
            "manifest_version": "3.0.0",
            "date": 1735689600,
            "networks": {"": "0.15.0", "mainnet": "0.15.0"},
            "channels": [{"name": "0.15.0", "components": []}]
        })
        .to_string();
        let manifest = crate::manifest::VersionedManifest::parse_str(&src).expect("must parse");
        assert!(errors_of(&manifest).iter().any(
            |e| matches!(e, ValidationError::InvalidNetworkName { name, .. } if name.is_empty())
        ));
    }

    /// A network name becomes a path segment under `toolchains/`, so one that escapes it would let
    /// a manifest make an ordinary `midenup install` replace a file anywhere on the machine.
    #[test]
    fn a_network_name_that_is_not_a_single_path_segment_is_rejected() {
        for name in ["../../../.zshrc", "..", "sub/net"] {
            let src = serde_json::json!({
                "manifest_version": "3.0.0",
                "date": 1735689600,
                "networks": {name: "0.15.0", "mainnet": "0.15.0"},
                "channels": [{"name": "0.15.0", "components": []}]
            })
            .to_string();
            let manifest = crate::manifest::VersionedManifest::parse_str(&src).expect("must parse");
            assert!(
                errors_of(&manifest).iter().any(|e| matches!(
                    e,
                    ValidationError::InvalidNetworkName { name: reported, .. } if reported == name
                )),
                "'{name}' must be rejected as a network name"
            );
        }
    }

    /// Deliberately valid: a mainnet hotfix puts mainnet ahead of testnet. There is no ordering
    /// invariant between networks, and this test exists so that adding one fails.
    #[test]
    fn mainnet_ahead_of_testnet_is_valid() {
        let mut m = manifest(vec![with_artifact(executable("vm", "miden-vm"), "miden-vm")]);
        m.channels.push(Channel::new(semver::Version::new(0, 16, 0), vec![]));
        m.promote(crate::channel::DEFAULT_NETWORK, semver::Version::new(0, 16, 0));
        m.promote("testnet", semver::Version::new(0, 15, 0));
        assert_eq!(validate_manifest(&m), Ok(()));
    }

    #[test]
    fn a_manifest_without_mainnet_is_rejected() {
        assert!(
            errors_of(&manifest(vec![]))
                .iter()
                .any(|e| matches!(e, ValidationError::MissingDefaultNetwork(_)))
        );
    }

    #[test]
    fn several_networks_naming_one_channel_are_valid() {
        let mut m =
            with_mainnet(manifest(vec![with_artifact(executable("vm", "miden-vm"), "miden-vm")]));
        m.promote("testnet", semver::Version::new(0, 15, 0));
        m.promote("devnet", semver::Version::new(0, 15, 0));
        assert_eq!(validate_manifest(&m), Ok(()));
    }

    #[test]
    fn duplicate_channels_are_rejected() {
        let mut m = manifest(vec![]);
        m.channels.push(Channel::new(semver::Version::new(0, 15, 0), vec![]));
        assert!(errors_of(&m).iter().any(|e| matches!(e, ValidationError::DuplicateChannel(_))));
    }

    #[test]
    fn duplicate_components_are_rejected() {
        let m = manifest(vec![executable("vm", "miden-vm"), executable("vm", "other")]);
        assert!(
            errors_of(&m)
                .iter()
                .any(|e| matches!(e, ValidationError::DuplicateComponent { .. }))
        );
    }

    #[test]
    fn a_dangling_requirement_is_rejected() {
        let mut vm = executable("vm", "miden-vm");
        vm.requires = vec!["ghost".to_string()];
        assert!(
            errors_of(&manifest(vec![vm]))
                .iter()
                .any(|e| matches!(e, ValidationError::UnknownRequirement { .. }))
        );
    }

    #[test]
    fn a_requirement_cycle_is_rejected_with_its_members() {
        let mut a = component("a", ComponentKind::Package);
        let mut b = component("b", ComponentKind::Package);
        a.requires = vec!["b".to_string()];
        b.requires = vec!["a".to_string()];

        let found = errors_of(&manifest(vec![a, b]));
        let cycle = found.iter().find_map(|e| match e {
            ValidationError::RequirementCycle { path, .. } => Some(path.clone()),
            _ => None,
        });
        assert_eq!(cycle, Some(vec!["a".to_string(), "b".to_string()]));
    }

    /// A dangling requirement makes the graph incomplete, so any cycle it "finds" is noise.
    /// Report the real problem and stop.
    #[test]
    fn a_dangling_requirement_suppresses_cycle_noise() {
        let mut a = component("a", ComponentKind::Package);
        a.requires = vec!["ghost".to_string()];
        let found = errors_of(&manifest(vec![a]));
        assert!(!found.iter().any(|e| matches!(e, ValidationError::RequirementCycle { .. })));
    }

    #[test]
    fn a_non_positive_timestamp_is_rejected() {
        let mut m = manifest(vec![]);
        m.date = 0;
        assert!(errors_of(&m).iter().any(|e| matches!(e, ValidationError::InvalidTimestamp(0))));
    }

    #[test]
    fn an_unsafe_installed_executable_is_rejected() {
        let m = manifest(vec![executable("vm", "../../bin/sh")]);
        assert!(errors_of(&m).iter().any(|e| matches!(
            e,
            ValidationError::InvalidName { field: "installed-executable", .. }
        )));
    }

    #[test]
    fn an_unsafe_artifact_id_is_rejected() {
        let m = manifest(vec![with_artifact(component("core", ComponentKind::Package), "../evil")]);
        assert!(
            errors_of(&m)
                .iter()
                .any(|e| matches!(e, ValidationError::InvalidName { field: "artifact id", .. }))
        );
    }

    /// Parsing accepts an unreadable container so an unrelated channel stays usable; publishing one
    /// is a different matter.
    #[test]
    fn an_unreadable_archive_format_is_rejected_for_publication() {
        let mut c = component("vm", ComponentKind::Package);
        c.artifacts.insert(
            "core.masp".to_string(),
            serde_json::from_value(serde_json::json!({
                "uri": "https://example.invalid/core.zip",
                "archive": "zip"
            }))
            .expect("an unknown format must still parse"),
        );

        let errors = errors_of(&manifest(vec![c]));
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::UnsupportedArchiveFormat { .. })),
            "{errors:?}"
        );
    }

    #[test]
    fn a_readable_archive_declaration_validates() {
        let mut c = component("vm", ComponentKind::Package);
        c.artifacts.insert(
            "core.masp".to_string(),
            serde_json::from_value(serde_json::json!({
                "uri": "https://example.invalid/core.tar.gz",
                "archive": "tar.gz"
            }))
            .expect("must parse"),
        );

        assert!(validate_manifest(&with_mainnet(manifest(vec![c]))).is_ok());
    }

    /// The mistake nothing downstream can catch: without the declaration the container installs as
    /// the artifact, and only fails when something runs it.
    #[test]
    fn an_archive_uri_without_an_archive_declaration_is_rejected() {
        let mut c = component("vm", ComponentKind::Package);
        c.artifacts.insert(
            "core.masp".to_string(),
            serde_json::from_value(serde_json::json!({
                "uri": "https://example.invalid/core.masp.tar.gz"
            }))
            .expect("must parse"),
        );

        let errors = errors_of(&manifest(vec![c]));
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::UndeclaredArchive { format: "tar.gz", .. })),
            "{errors:?}"
        );
    }

    /// Judged on the resolved URI, so a per-target `%extension` is seen like a literal suffix.
    #[test]
    fn a_substituted_archive_extension_is_also_caught() {
        let mut c = component("vm", ComponentKind::Package);
        c.artifacts.insert(
            "core.masp".to_string(),
            serde_json::from_value(serde_json::json!({
                "uri": "https://example.invalid/%basename-%target.%extension",
                "extension": "tar.gz",
                "targets": {"aarch64-apple-darwin": {}}
            }))
            .expect("must parse"),
        );

        let errors = errors_of(&manifest(vec![c]));
        assert!(
            errors.iter().any(|e| matches!(e, ValidationError::UndeclaredArchive { .. })),
            "{errors:?}"
        );
    }

    /// The extension is part of what is fetched; a query string is not, and cannot be relied on
    /// to hide it. Fetching this installs a tarball named `core.masp`.
    #[test]
    fn a_query_string_does_not_hide_the_archive_extension() {
        let mut c = component("vm", ComponentKind::Package);
        c.artifacts.insert(
            "core.masp".to_string(),
            serde_json::from_value(serde_json::json!({
                "uri": "https://example.invalid/core.masp.tar.gz?download=1"
            }))
            .expect("must parse"),
        );

        let errors = errors_of(&manifest(vec![c]));
        assert!(
            errors
                .iter()
                .any(|e| matches!(e, ValidationError::UndeclaredArchive { format: "tar.gz", .. })),
            "{errors:?}"
        );
    }

    /// As for a query string: a fragment is not part of what is fetched either.
    #[test]
    fn a_fragment_does_not_hide_the_archive_extension() {
        let mut c = component("vm", ComponentKind::Package);
        c.artifacts.insert(
            "core.masp".to_string(),
            serde_json::from_value(serde_json::json!({
                "uri": "https://example.invalid/core.masp.tar.gz#sha256"
            }))
            .expect("must parse"),
        );

        let errors = errors_of(&manifest(vec![c]));
        assert!(
            errors.iter().any(|e| matches!(e, ValidationError::UndeclaredArchive { .. })),
            "{errors:?}"
        );
    }

    /// A local source is a filesystem path all the way through, `?` included.
    #[test]
    fn a_local_archive_without_a_declaration_is_rejected() {
        let mut c = component("vm", ComponentKind::Package);
        c.artifacts.insert(
            "core.masp".to_string(),
            serde_json::from_value(serde_json::json!({
                "uri": "file:///releases/core.masp.tar.gz"
            }))
            .expect("must parse"),
        );

        let errors = errors_of(&manifest(vec![c]));
        assert!(
            errors.iter().any(|e| matches!(e, ValidationError::UndeclaredArchive { .. })),
            "{errors:?}"
        );
    }

    /// Only the path names the file: a host is not a thing that is fetched.
    #[test]
    fn a_host_spelled_like_an_archive_is_left_alone() {
        let mut c = component("vm", ComponentKind::Package);
        c.artifacts.insert(
            "core.masp".to_string(),
            serde_json::from_value(serde_json::json!({
                "uri": "https://releases.tar.gz/core.masp"
            }))
            .expect("must parse"),
        );

        assert!(validate_manifest(&with_mainnet(manifest(vec![c]))).is_ok());
    }

    /// An artifact installed *as* the archive is asking for exactly that.
    #[test]
    fn an_artifact_named_for_the_archive_is_left_alone() {
        let mut c = component("assets", ComponentKind::Asset);
        c.artifacts.insert(
            "bundle.tar.gz".to_string(),
            serde_json::from_value(serde_json::json!({
                "uri": "https://example.invalid/bundle.tar.gz"
            }))
            .expect("must parse"),
        );

        assert!(validate_manifest(&with_mainnet(manifest(vec![c]))).is_ok());
    }

    #[test]
    fn two_components_installing_to_one_path_are_rejected() {
        let m = manifest(vec![
            with_artifact(component("core", ComponentKind::Package), "shared.masp"),
            with_artifact(component("other", ComponentKind::Package), "shared.masp"),
        ]);
        assert!(
            errors_of(&m)
                .iter()
                .any(|e| matches!(e, ValidationError::DestinationCollision { .. }))
        );
    }

    #[test]
    fn a_hidden_component_without_aliases_is_rejected() {
        let hidden = component(
            "cargo-miden",
            ComponentKind::CargoExtension {
                installation_method: InstallationMethod::Prebuilt,
                spec: ExecutableComponent {
                    installed_executable: "cargo-miden".to_string(),
                    hide: true,
                    ..Default::default()
                },
            },
        );
        assert!(
            errors_of(&manifest(vec![hidden]))
                .iter()
                .any(|e| matches!(e, ValidationError::HiddenWithoutAliases { .. }))
        );
    }

    /// A command reachable only through its subcommands is valid -- this is what the real `node`
    /// component looks like.
    #[test]
    fn a_command_with_only_subcommands_is_reachable() {
        let cmd = component(
            "node",
            ComponentKind::Command {
                command_name: None,
                format: Executable::default(),
                subcommands: [("up".to_string(), Executable::default_call_format())]
                    .into_iter()
                    .collect(),
                aliases: Default::default(),
            },
        );
        assert!(
            !errors_of(&manifest(vec![cmd]))
                .iter()
                .any(|e| matches!(e, ValidationError::UnreachableCommand { .. }))
        );
    }

    #[test]
    fn a_command_with_no_format_no_subcommands_and_no_aliases_is_rejected() {
        let cmd = component(
            "node",
            ComponentKind::Command {
                command_name: None,
                format: Executable::default(),
                subcommands: Default::default(),
                aliases: Default::default(),
            },
        );
        assert!(
            errors_of(&manifest(vec![cmd]))
                .iter()
                .any(|e| matches!(e, ValidationError::UnreachableCommand { .. }))
        );
    }

    #[test]
    fn two_components_declaring_the_same_alias_are_rejected() {
        let make = |name: &'static str| {
            component(
                name,
                ComponentKind::Executable {
                    installation_method: InstallationMethod::Prebuilt,
                    spec: ExecutableComponent {
                        installed_executable: name.to_string(),
                        aliases: [("shared".to_string(), Executable::default_call_format())]
                            .into_iter()
                            .collect(),
                        ..Default::default()
                    },
                },
            )
        };
        assert!(
            errors_of(&manifest(vec![make("a"), make("b")]))
                .iter()
                .any(|e| matches!(e, ValidationError::ConflictingAlias { .. }))
        );
    }

    /// An alias must not shadow another component's direct `miden <name>` command.
    #[test]
    fn an_alias_shadowing_a_command_name_is_rejected() {
        let vm = executable("vm", "miden-vm");
        let shadower = component(
            "other",
            ComponentKind::Executable {
                installation_method: InstallationMethod::Prebuilt,
                spec: ExecutableComponent {
                    installed_executable: "other".to_string(),
                    aliases: [("vm".to_string(), Executable::default_call_format())]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                },
            },
        );
        assert!(
            errors_of(&manifest(vec![vm, shadower]))
                .iter()
                .any(|e| matches!(e, ValidationError::AliasShadowsCommand { .. }))
        );
    }

    /// Loading a manifest must not run validation.
    ///
    /// Pins the decision in the module docs: the published manifest has dangling requirements in
    /// channel 0.13.3, so validating at parse time would make every command fail for every user.
    #[test]
    fn parsing_does_not_validate() {
        let src = serde_json::json!({
            "manifest_version": "3.0.0",
            "date": 1735689600,
            "channels": [{"name": "0.13.3", "components": [{
                "name": "midenc",
                "version": {"kind": "registry", "version": "0.1.0"},
                "kind": "executable",
                "installation_method": {"kind": "cargo", "crate_name": "midenc"},
                "installed-executable": "midenc",
                "requires": ["base", "std"]
            }]}]
        })
        .to_string();

        let parsed = crate::manifest::VersionedManifest::parse_str(&src)
            .expect("a channel with dangling requirements must still load");

        // ...but validation, when explicitly run, must report it.
        let errors = validate_manifest(&parsed).expect_err("validation must catch it");
        assert_eq!(
            errors
                .iter()
                .filter(|e| matches!(e, ValidationError::UnknownRequirement { .. }))
                .count(),
            2
        );
    }

    /// Every problem is reported in one pass, not one per run.
    #[test]
    fn all_errors_are_collected() {
        let mut m = manifest(vec![executable("vm", "../evil"), executable("vm", "dup")]);
        m.date = -1;
        let found = errors_of(&m);
        assert!(found.len() >= 3, "expected several errors, got {found:?}");
    }
}
