//! What changed between two manifests.

use std::collections::BTreeSet;

use super::{Channel, Component, Extra, Manifest, UnknownFields, validate::is_empty_value};
use crate::{artifact::Artifact, version::Authority};

/// A field whose value differs; a side is `None` when the field is absent or empty there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldChange {
    pub field: String,
    pub from: Option<serde_json::Value>,
    pub to: Option<serde_json::Value>,
}

/// One difference between two manifests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    ManifestVersionChanged {
        from: semver::Version,
        to: semver::Version,
    },
    /// Unknown top-level fields that differ.
    ManifestFieldsChanged(Vec<FieldChange>),
    MinClientVersionChanged {
        from: semver::Version,
        to: semver::Version,
    },
    NetworkAdded {
        network: String,
        channel: semver::Version,
    },
    NetworkRemoved {
        network: String,
        channel: semver::Version,
    },
    NetworkMoved {
        network: String,
        from: semver::Version,
        to: semver::Version,
    },
    ChannelAdded {
        channel: semver::Version,
        components: Vec<String>,
    },
    ChannelRemoved(semver::Version),
    /// Unknown fields of the channel that differ.
    ChannelFieldsChanged {
        channel: semver::Version,
        fields: Vec<FieldChange>,
    },
    MigrationChanged {
        channel: semver::Version,
        from: Option<semver::Version>,
        to: Option<semver::Version>,
    },
    ComponentAdded {
        channel: semver::Version,
        component: String,
    },
    ComponentRemoved {
        channel: semver::Version,
        component: String,
    },
    /// `fields` holds every top-level field that differs, by its schema name.
    ComponentChanged {
        channel: semver::Version,
        component: String,
        fields: Vec<FieldChange>,
        /// Set when both sides are registry versions and they differ.
        version: Option<(semver::Version, semver::Version)>,
    },
}

/// Lists every change from `previous` to `next`, ignoring the timestamp.
pub fn diff(previous: &Manifest, next: &Manifest) -> Vec<Change> {
    let mut changes = Vec::new();

    if previous.manifest_version != next.manifest_version {
        changes.push(Change::ManifestVersionChanged {
            from: previous.manifest_version.clone(),
            to: next.manifest_version.clone(),
        });
    }
    let fields = changed_values(&previous.extra, &next.extra);
    if !fields.is_empty() {
        changes.push(Change::ManifestFieldsChanged(fields));
    }

    if previous.min_client_version != next.min_client_version {
        changes.push(Change::MinClientVersionChanged {
            from: previous.min_client_version.clone(),
            to: next.min_client_version.clone(),
        });
    }

    for (network, from) in previous.networks.iter() {
        match next.networks.get(network) {
            None => changes.push(Change::NetworkRemoved {
                network: network.clone(),
                channel: from.clone(),
            }),
            Some(to) if to != from => changes.push(Change::NetworkMoved {
                network: network.clone(),
                from: from.clone(),
                to: to.clone(),
            }),
            Some(_) => {},
        }
    }
    for (network, channel) in next.networks.iter() {
        if !previous.networks.contains_key(network) {
            changes.push(Change::NetworkAdded {
                network: network.clone(),
                channel: channel.clone(),
            });
        }
    }

    for old in previous.channels.iter() {
        match next.get_channel_by_name(&old.name) {
            None => changes.push(Change::ChannelRemoved(old.name.clone())),
            Some(new) => diff_channel(old, new, &mut changes),
        }
    }
    for new in next.channels.iter() {
        if previous.get_channel_by_name(&new.name).is_none() {
            changes.push(Change::ChannelAdded {
                channel: new.name.clone(),
                components: new.components.iter().map(|c| c.name.to_string()).collect(),
            });
        }
    }

    changes
}

/// Toolchains whose contents changed and that still exist, from the output of [diff].
pub fn affected_toolchains(changes: &[Change]) -> BTreeSet<&semver::Version> {
    changes
        .iter()
        .filter_map(|change| match change {
            Change::ChannelAdded { channel, .. }
            | Change::ChannelFieldsChanged { channel, .. }
            | Change::MigrationChanged { channel, .. }
            | Change::ComponentAdded { channel, .. }
            | Change::ComponentRemoved { channel, .. }
            | Change::ComponentChanged { channel, .. } => Some(channel),
            Change::ManifestVersionChanged { .. }
            | Change::ManifestFieldsChanged(_)
            | Change::MinClientVersionChanged { .. }
            | Change::NetworkAdded { .. }
            | Change::NetworkRemoved { .. }
            | Change::NetworkMoved { .. }
            | Change::ChannelRemoved(_) => None,
        })
        .collect()
}

fn diff_channel(old: &Channel, new: &Channel, changes: &mut Vec<Change>) {
    let fields = changed_values(&old.extra, &new.extra);
    if !fields.is_empty() {
        changes.push(Change::ChannelFieldsChanged { channel: old.name.clone(), fields });
    }
    if old.migrates_from != new.migrates_from {
        changes.push(Change::MigrationChanged {
            channel: old.name.clone(),
            from: old.migrates_from.clone(),
            to: new.migrates_from.clone(),
        });
    }

    for old_component in old.components.iter() {
        match new.get_component(&old_component.name) {
            None => changes.push(Change::ComponentRemoved {
                channel: old.name.clone(),
                component: old_component.name.to_string(),
            }),
            Some(new_component) => {
                let fields = changed_fields(old_component, new_component);
                if !fields.is_empty() {
                    changes.push(Change::ComponentChanged {
                        channel: old.name.clone(),
                        component: old_component.name.to_string(),
                        fields,
                        version: registry_versions(old_component, new_component),
                    });
                }
            },
        }
    }
    for new_component in new.components.iter() {
        if old.get_component(&new_component.name).is_none() {
            changes.push(Change::ComponentAdded {
                channel: new.name.clone(),
                component: new_component.name.to_string(),
            });
        }
    }
}

/// Top-level fields whose serialized values differ.
fn changed_fields(old: &Component, new: &Component) -> Vec<FieldChange> {
    changed_values(&component_fields(old), &component_fields(new))
}

/// Empty omitted fields can be preserved in extras at any known schema object. Normalize only
/// those containers: an empty entry in a map such as artifact targets still carries meaning,
/// and the structure of an unknown field's nonempty value is opaque to this build.
fn component_fields(component: &Component) -> Extra {
    fn discard_empty(extra: &mut Extra) {
        extra.retain(|_, value| !is_empty_value(value));
    }

    fn discard_nested_empty(unknown: &mut UnknownFields) {
        discard_empty(&mut unknown.fields);
        for inner in unknown.nested.values_mut() {
            discard_nested_empty(inner);
        }
    }

    let mut component = component.clone();
    discard_nested_empty(&mut component.extra);
    for artifact in component.artifacts.artifacts.values_mut() {
        let (extra, archive) = match artifact {
            Artifact::TargetSpecific {
                substitutions, targets, archive, extra, ..
            } => {
                for substitutions in substitutions.iter_mut().chain(targets.values_mut()) {
                    discard_empty(&mut substitutions.extra);
                }
                (extra, archive)
            },
            Artifact::TargetAgnostic { archive, extra, .. } => (extra, archive),
        };
        discard_empty(extra);
        if let Some(archive) = archive {
            discard_empty(&mut archive.extra);
        }
    }

    match serde_json::to_value(component) {
        Ok(serde_json::Value::Object(fields)) => fields,
        _ => unreachable!("a component serializes to an object"),
    }
}

/// Keys whose values differ, sorted, treating an empty value the same as an absent key.
fn changed_values(old: &Extra, new: &Extra) -> Vec<FieldChange> {
    let value = |extra: &Extra, key: &str| extra.get(key).filter(|v| !is_empty_value(v)).cloned();
    let mut keys: Vec<&String> = old.keys().chain(new.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .filter_map(|key| {
            let (from, to) = (value(old, key), value(new, key));
            (from != to).then(|| FieldChange { field: key.clone(), from, to })
        })
        .collect()
}

fn registry_versions(
    old: &Component,
    new: &Component,
) -> Option<(semver::Version, semver::Version)> {
    match (&old.version, &new.version) {
        (Authority::Registry { version: from }, Authority::Registry { version: to })
            if from != to =>
        {
            Some((from.clone(), to.clone()))
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;
    use crate::{
        artifact::Artifacts,
        manifest::{ComponentKind, ExecutableComponent, InstallationMethod},
        profile::Profile,
    };

    fn vm(version: semver::Version) -> Component {
        Component {
            name: Cow::Borrowed("vm"),
            version: Authority::Registry { version },
            kind: ComponentKind::Executable {
                installation_method: InstallationMethod::Prebuilt,
                spec: ExecutableComponent {
                    installed_executable: "miden-vm".to_string(),
                    ..Default::default()
                },
            },
            profiles: vec![Profile::Minimal],
            requires: vec![],
            artifacts: Artifacts::default(),
            extra: Default::default(),
        }
    }

    fn field(
        field: &str,
        from: Option<serde_json::Value>,
        to: Option<serde_json::Value>,
    ) -> FieldChange {
        FieldChange { field: field.to_string(), from, to }
    }

    /// `mainnet` on 0.15.0, whose only component is `vm` 0.15.0.
    fn previous() -> Manifest {
        let mut m = Manifest::default();
        m.channels.push(Channel::new(
            semver::Version::new(0, 15, 0),
            vec![vm(semver::Version::new(0, 15, 0))],
        ));
        m.promote("mainnet", semver::Version::new(0, 15, 0));
        m.date = 1000;
        m
    }

    #[test]
    fn only_a_timestamp_change_is_no_change() {
        let mut next = previous();
        next.date = 2000;
        assert_eq!(diff(&previous(), &next), []);
    }

    #[test]
    fn network_changes_are_listed() {
        let mut next = previous();
        next.channels.push(Channel::new(
            semver::Version::new(0, 16, 0),
            vec![vm(semver::Version::new(0, 16, 0))],
        ));
        next.promote("mainnet", semver::Version::new(0, 16, 0));
        next.promote("testnet", semver::Version::new(0, 16, 0));
        assert_eq!(
            diff(&previous(), &next),
            [
                Change::NetworkMoved {
                    network: "mainnet".to_string(),
                    from: semver::Version::new(0, 15, 0),
                    to: semver::Version::new(0, 16, 0),
                },
                Change::NetworkAdded {
                    network: "testnet".to_string(),
                    channel: semver::Version::new(0, 16, 0),
                },
                Change::ChannelAdded {
                    channel: semver::Version::new(0, 16, 0),
                    components: vec!["vm".to_string()],
                },
            ]
        );
    }

    #[test]
    fn a_removed_network_and_channel_are_listed() {
        let mut next = previous();
        next.networks.clear();
        next.remove_channel(semver::Version::new(0, 15, 0));
        assert_eq!(
            diff(&previous(), &next),
            [
                Change::NetworkRemoved {
                    network: "mainnet".to_string(),
                    channel: semver::Version::new(0, 15, 0),
                },
                Change::ChannelRemoved(semver::Version::new(0, 15, 0)),
            ]
        );
    }

    #[test]
    fn a_changed_component_lists_its_fields_and_versions() {
        let mut next = previous();
        let component = &mut next.channels[0].components[0];
        component.version = Authority::Registry { version: semver::Version::new(0, 14, 0) };
        component.requires = vec!["core".to_string()];
        assert_eq!(
            diff(&previous(), &next),
            [Change::ComponentChanged {
                channel: semver::Version::new(0, 15, 0),
                component: "vm".to_string(),
                fields: vec![
                    field("requires", None, Some(serde_json::json!(["core"]))),
                    field(
                        "version",
                        Some(serde_json::json!({"kind": "registry", "version": "0.15.0"})),
                        Some(serde_json::json!({"kind": "registry", "version": "0.14.0"})),
                    ),
                ],
                version: Some((semver::Version::new(0, 15, 0), semver::Version::new(0, 14, 0))),
            }]
        );
    }

    /// An emptied list is omitted when serialized, so it only shows on the previous side.
    #[test]
    fn a_field_present_on_one_side_only_is_listed() {
        let mut next = previous();
        next.channels[0].components[0].profiles.clear();
        let changes = diff(&previous(), &next);
        assert!(matches!(
            &changes[..],
            [Change::ComponentChanged { fields, version: None, .. }]
                if fields == &[field("profiles", Some(serde_json::json!(["minimal"])), None)]
        ));
    }

    #[test]
    fn an_empty_field_is_the_same_as_an_absent_one() {
        let mut next = previous();
        next.extra.insert("vendor".to_string(), serde_json::Value::Null);
        next.channels[0].components[0]
            .extra
            .fields
            .insert("requires".to_string(), serde_json::json!([]));
        assert_eq!(diff(&previous(), &next), []);
    }

    #[test]
    fn a_nonempty_unknown_object_is_kept_opaque() {
        let mut next = previous();
        next.channels[0].components[0]
            .extra
            .fields
            .insert("vendor".to_string(), serde_json::json!({"flag": null}));
        assert!(matches!(
            &diff(&previous(), &next)[..],
            [Change::ComponentChanged { fields, .. }]
                if fields == &[field("vendor", None, Some(serde_json::json!({"flag": null})))]
        ));
    }

    #[test]
    fn schema_version_and_unknown_fields_are_listed() {
        let mut next = previous();
        next.manifest_version = semver::Version::new(3, 1, 0);
        next.extra.insert("vendor".to_string(), serde_json::json!(1));
        next.channels[0].extra.insert("note".to_string(), serde_json::json!("x"));
        assert_eq!(
            diff(&previous(), &next),
            [
                Change::ManifestVersionChanged {
                    from: semver::Version::new(3, 0, 0),
                    to: semver::Version::new(3, 1, 0),
                },
                Change::ManifestFieldsChanged(vec![field(
                    "vendor",
                    None,
                    Some(serde_json::json!(1))
                )]),
                Change::ChannelFieldsChanged {
                    channel: semver::Version::new(0, 15, 0),
                    fields: vec![field("note", None, Some(serde_json::json!("x")))],
                },
            ]
        );
    }

    #[test]
    fn only_changes_inside_an_existing_toolchain_affect_it() {
        let mut next = previous();
        next.channels.push(Channel::new(semver::Version::new(0, 16, 0), vec![]));
        next.promote("mainnet", semver::Version::new(0, 16, 0));
        next.channels[0].components[0].requires = vec!["core".to_string()];
        let changes = diff(&previous(), &next);
        assert_eq!(
            affected_toolchains(&changes),
            BTreeSet::from([&semver::Version::new(0, 15, 0), &semver::Version::new(0, 16, 0)])
        );

        let mut moved_only = previous();
        moved_only.promote("testnet", semver::Version::new(0, 15, 0));
        assert!(affected_toolchains(&diff(&previous(), &moved_only)).is_empty());

        let mut removed = previous();
        removed.remove_channel(semver::Version::new(0, 15, 0));
        assert!(affected_toolchains(&diff(&previous(), &removed)).is_empty());
    }

    #[test]
    fn components_and_migrations_are_listed() {
        let mut next = previous();
        let channel = &mut next.channels[0];
        channel.migrates_from = Some(semver::Version::new(0, 14, 0));
        channel.components[0].name = Cow::Borrowed("debugger");
        assert_eq!(
            diff(&previous(), &next),
            [
                Change::MigrationChanged {
                    channel: semver::Version::new(0, 15, 0),
                    from: None,
                    to: Some(semver::Version::new(0, 14, 0)),
                },
                Change::ComponentRemoved {
                    channel: semver::Version::new(0, 15, 0),
                    component: "vm".to_string(),
                },
                Change::ComponentAdded {
                    channel: semver::Version::new(0, 15, 0),
                    component: "debugger".to_string(),
                },
            ]
        );
    }
}
