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

use super::{Channel, Component, ComponentKind, Manifest};
use crate::plan::{destination_for, validate_artifact_id};

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

        for id in component.artifacts.artifacts.keys() {
            if let Err(err) = validate_artifact_id(id) {
                errors.push(invalid("artifact id", err.to_string()));
            }
        }
    }
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
