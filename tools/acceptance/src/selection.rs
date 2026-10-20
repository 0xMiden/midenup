//! Which toolchains to verify, and which scenarios and coverage apply to each.

use std::collections::{BTreeMap, BTreeSet};

use midenup::manifest::{
    Channel, Manifest,
    diff::{self, Change},
};

use crate::scenarios::Scenario;

/// A toolchain to verify and the components whose change put it here.
#[derive(Debug)]
pub struct Target<'m> {
    pub toolchain: &'m Channel,
    pub changed: BTreeSet<String>,
}

/// The toolchains changed since `previous`, or every network's toolchain without one.
pub fn targets<'m>(manifest: &'m Manifest, previous: Option<&Manifest>) -> Vec<Target<'m>> {
    let Some(previous) = previous else {
        let toolchains: BTreeSet<&semver::Version> = manifest
            .network_names()
            .filter_map(|network| manifest.network_version(network))
            .collect();
        return toolchains
            .into_iter()
            .filter_map(|version| manifest.get_channel_by_name(version))
            .map(|toolchain| Target {
                toolchain,
                changed: component_names(toolchain),
            })
            .collect();
    };

    let changes = diff::diff(previous, manifest);
    let mut targets: BTreeMap<&semver::Version, Target<'m>> = BTreeMap::new();
    for version in diff::affected_toolchains(&changes) {
        if let Some(toolchain) = manifest.get_channel_by_name(version) {
            targets.insert(&toolchain.name, Target { toolchain, changed: BTreeSet::new() });
        }
    }
    for change in &changes {
        match change {
            Change::ChannelAdded { channel: version, .. } => {
                if let Some(target) = targets.get_mut(version) {
                    target.changed = component_names(target.toolchain);
                }
            },
            Change::ComponentAdded { channel, component }
            | Change::ComponentChanged { channel, component, .. } => {
                if let Some(target) = targets.get_mut(channel) {
                    target.changed.insert(component.clone());
                }
            },
            _ => {},
        }
    }
    targets.into_values().collect()
}

/// Components of `target`'s toolchain that no scenario covers.
pub fn uncovered(target: &Target, scenarios: &[Scenario]) -> Vec<String> {
    component_names(target.toolchain)
        .into_iter()
        .filter(|component| {
            !scenarios.iter().any(|scenario| {
                scenario.toolchain.matches(&target.toolchain.name)
                    && scenario.requires.contains(component)
            })
        })
        .collect()
}

/// Scenarios for `target`'s toolchain that require one of its changed components.
pub fn selected<'s>(target: &Target, scenarios: &'s [Scenario]) -> Vec<&'s Scenario> {
    scenarios
        .iter()
        .filter(|scenario| {
            scenario.toolchain.matches(&target.toolchain.name)
                && scenario.requires.iter().any(|component| target.changed.contains(component))
        })
        .collect()
}

fn component_names(toolchain: &Channel) -> BTreeSet<String> {
    toolchain
        .components
        .iter()
        .map(|component| component.name.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(json: serde_json::Value) -> Manifest {
        midenup::manifest::VersionedManifest::parse_str(&json.to_string()).unwrap()
    }

    fn component(name: &str, version: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "version": {"kind": "registry", "version": version},
            "kind": "executable",
            "installation_method": {"kind": "cargo", "crate_name": name},
            "installed-executable": name,
        })
    }

    fn with_channels(channels: serde_json::Value) -> Manifest {
        manifest(serde_json::json!({
            "manifest_version": "3.0.0",
            "min_client_version": "1.0.0",
            "date": 1,
            "networks": {"mainnet": "0.17.0"},
            "channels": channels,
        }))
    }

    fn scenario(toolchain: &str, requires: &[&str]) -> Scenario {
        toml::from_str(&format!(
            "title = 't'\ntoolchain = '{toolchain}'\nrequires = {requires:?}\nkind = \
             'script'\nscript = 'true'"
        ))
        .unwrap()
    }

    #[test]
    fn without_a_previous_manifest_every_network_toolchain_is_verified_in_full() {
        let m = with_channels(serde_json::json!([
            {"name": "0.16.0", "components": [component("vm", "0.16.0")]},
            {"name": "0.17.0", "components": [component("vm", "0.17.0"), component("midenc", "0.17.0")]},
        ]));
        let targets = targets(&m, None);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].toolchain.name, semver::Version::new(0, 17, 0));
        assert_eq!(targets[0].changed, BTreeSet::from(["midenc".into(), "vm".into()]));
    }

    #[test]
    fn a_changed_component_selects_only_scenarios_that_require_it() {
        let previous = with_channels(serde_json::json!([
            {"name": "0.17.0", "components": [component("vm", "0.17.0"), component("midenc", "0.17.0")]},
        ]));
        let next = with_channels(serde_json::json!([
            {"name": "0.17.0", "components": [component("vm", "0.17.1"), component("midenc", "0.17.0")]},
        ]));
        let targets = targets(&next, Some(&previous));
        assert_eq!(targets[0].changed, BTreeSet::from(["vm".into()]));

        let scenarios = [
            scenario("^0.17.0", &["vm", "midenc"]),
            scenario("^0.17.0", &["midenc"]),
            scenario("^0.16.0", &["vm"]),
        ];
        let selected = selected(&targets[0], &scenarios);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].requires, ["vm", "midenc"]);
        assert!(uncovered(&targets[0], &scenarios).is_empty());
    }

    #[test]
    fn an_added_toolchain_is_verified_in_full_and_reports_missing_coverage() {
        let previous = with_channels(serde_json::json!([
            {"name": "0.17.0", "components": [component("vm", "0.17.0")]},
        ]));
        let next = with_channels(serde_json::json!([
            {"name": "0.17.0", "components": [component("vm", "0.17.0")]},
            {"name": "0.18.0", "components": [component("vm", "0.18.0")]},
        ]));
        let targets = targets(&next, Some(&previous));
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].changed, BTreeSet::from(["vm".into()]));
        assert_eq!(uncovered(&targets[0], &[scenario("^0.17.0", &["vm"])]), ["vm"]);
    }
}
