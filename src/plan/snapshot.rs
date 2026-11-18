//! The identity of one channel's definition, as seen from one target.
//!
//! Computed from the channel alone, so it needs no network: authority pins observed locally (a
//! branch's commit, a path's modification time) are left out, and so are the manifest date and
//! network pointers, which live outside the channel.

use std::fmt;

use sha2::Digest as _;

use crate::{manifest::Channel, version::Authority};

/// A digest over a channel's component definitions for one target.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SnapshotId(String);

impl SnapshotId {
    /// The algorithm version prefix. Bump when the input set or encoding changes.
    pub const PREFIX: &'static str = "sn1:";
}

impl serde::Serialize for SnapshotId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for SnapshotId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let raw = String::deserialize(deserializer)?;
        if !raw.starts_with(Self::PREFIX) {
            return Err(D::Error::custom(format!(
                "unrecognized snapshot id '{raw}': expected a '{}' prefix",
                Self::PREFIX
            )));
        }
        Ok(SnapshotId(raw))
    }
}

impl fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Computes the snapshot identity of `channel` for `target`.
///
/// Each component enters as its full definition, with artifacts replaced by what resolves for
/// `target` (URI, archive format and declared digest) whenever they resolve.
pub fn compute(channel: &Channel, target: &str) -> SnapshotId {
    let mut components: Vec<serde_json::Value> = channel
        .components
        .iter()
        .map(|component| {
            let mut component = component.clone();
            match &mut component.version {
                Authority::Path { last_modification, .. } => *last_modification = None,
                Authority::Git {
                    target: crate::version::GitTarget::Branch { latest_revision, .. },
                    ..
                } => *latest_revision = None,
                Authority::Git { .. } | Authority::Registry { .. } => (),
            }

            let mut value =
                serde_json::to_value(&component).expect("a manifest component serializes to JSON");
            if let Ok(resolved) = component.artifacts.get_artifacts_for_target(target, &component) {
                let artifacts: Vec<serde_json::Value> = resolved
                    .into_iter()
                    .map(|(id, resolved)| {
                        serde_json::json!({
                            "id": id,
                            "uri": resolved.uri.to_string(),
                            "archive": resolved.archive.map(|format| format.as_str()),
                            "digest": component
                                .artifacts
                                .artifacts
                                .get(id)
                                .and_then(|artifact| artifact.digest())
                                .map(ToString::to_string),
                        })
                    })
                    .collect();
                value["artifacts"] = serde_json::Value::Array(artifacts);
            }
            value
        })
        .collect();
    components.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));

    // serde_json orders object keys, so the encoding is canonical.
    let canonical = serde_json::json!({
        "channel": channel.name.to_string(),
        "target": target,
        "components": components,
    });
    let digest = sha2::Sha256::digest(canonical.to_string().as_bytes());

    let mut out = String::with_capacity(SnapshotId::PREFIX.len() + 64);
    out.push_str(SnapshotId::PREFIX);
    for byte in digest {
        use fmt::Write;
        write!(&mut out, "{byte:02x}").expect("writing to a String cannot fail");
    }
    SnapshotId(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::Component;

    const TARGET: &str = "aarch64-apple-darwin";

    fn component(name: &str, version: &str) -> Component {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "version": {"kind": "registry", "version": version},
            "kind": "executable",
            "installation_method": {"kind": "prebuilt"},
            "installed-executable": format!("miden-{name}"),
            "profiles": ["minimal"],
            "artifacts": {format!("miden-{name}"): {
                "uri": format!("https://example.invalid/{version}/miden-{name}-%target"),
                "targets": {TARGET: {}, "x86_64-unknown-linux-gnu": {}}
            }}
        }))
        .unwrap()
    }

    fn channel() -> Channel {
        Channel::new(
            semver::Version::new(0, 15, 0),
            vec![component("vm", "0.1.0"), component("client", "0.2.0")],
        )
    }

    #[test]
    fn the_identity_is_stable_under_component_reordering() {
        let mut reordered = channel();
        reordered.components.reverse();
        assert_eq!(compute(&channel(), TARGET), compute(&reordered, TARGET));
        assert!(compute(&channel(), TARGET).to_string().starts_with(SnapshotId::PREFIX));
    }

    #[test]
    fn local_authority_pins_do_not_change_the_identity() {
        let branch = |latest_revision: Option<&str>| {
            let mut channel = channel();
            channel.components[0].version = Authority::Git {
                repository_url: "https://example.invalid/vm".to_string(),
                subpath: None,
                target: crate::version::GitTarget::Branch {
                    name: "main".to_string(),
                    latest_revision: latest_revision.map(str::to_string),
                },
            };
            channel
        };
        assert_eq!(compute(&branch(None), TARGET), compute(&branch(Some("abc123")), TARGET));
    }

    #[test]
    fn every_definition_change_changes_the_identity() {
        let base = compute(&channel(), TARGET);
        type Mutation = (&'static str, fn(&mut Channel));
        let mutations: Vec<Mutation> = vec![
            ("channel", |c| c.name = semver::Version::new(0, 16, 0)),
            ("version", |c| c.components[0] = component("vm", "0.1.1")),
            ("requires", |c| c.components[0].requires = vec!["client".into()]),
            ("profiles", |c| {
                c.components[0].profiles = vec![crate::profile::Profile::Complete]
            }),
            ("component added", |c| c.components.push(component("debug", "0.1.0"))),
            ("component removed", |c| {
                c.components.pop();
            }),
        ];
        for (label, mutate) in mutations {
            let mut changed = channel();
            mutate(&mut changed);
            assert_ne!(base, compute(&changed, TARGET), "identity must change for {label}");
        }
    }

    #[test]
    fn the_identity_depends_on_the_target() {
        assert_ne!(compute(&channel(), TARGET), compute(&channel(), "x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn a_digest_change_changes_the_identity() {
        let with_digest = |digest: &str| {
            let mut channel = channel();
            channel.components[0] = serde_json::from_value(serde_json::json!({
                "name": "vm",
                "version": {"kind": "registry", "version": "0.1.0"},
                "kind": "executable",
                "installation_method": {"kind": "prebuilt"},
                "installed-executable": "miden-vm",
                "profiles": ["minimal"],
                "artifacts": {"miden-vm": {"uri": "https://example.invalid/vm", "digest": digest}}
            }))
            .unwrap();
            channel
        };
        assert_ne!(
            compute(&with_digest(&format!("sha256:{}", "a".repeat(64))), TARGET),
            compute(&with_digest(&format!("sha256:{}", "b".repeat(64))), TARGET)
        );
    }
}
