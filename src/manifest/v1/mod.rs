mod authority;
pub mod channel;
pub mod component;

use serde::Deserialize;

pub use self::{channel::*, component::*};
use super::ManifestError;

pub const MANIFEST_VERSION: semver::Version = semver::Version::new(1, 0, 1);

/// The global manifest of all known channels and their toolchains
#[derive(Deserialize, Debug, Clone)]
pub struct Manifest {
    /// The UTC timestamp at which this manifest was generated
    pub(super) date: i64,
    /// The channels described in this manifest
    pub(super) channels: Vec<Channel>,
}

impl Default for Manifest {
    fn default() -> Self {
        let date = chrono::Utc::now().timestamp();
        Self { date, channels: vec![] }
    }
}

impl TryFrom<Manifest> for crate::manifest::v3::Manifest {
    type Error = ManifestError;

    fn try_from(value: Manifest) -> Result<Self, Self::Error> {
        use crate::manifest::v3;

        let mut channels = Vec::<v3::Channel>::with_capacity(value.channels.len());
        // The alias each channel carried, paired with the channel, so the network map can be built
        // once every channel is known.
        let mut aliases = Vec::new();

        for channel in value.channels {
            let mut components = Vec::<v3::Component>::with_capacity(channel.components.len());
            for component in channel.components {
                components.push(component.try_into()?);
            }
            // `Partial` is dropped: it recorded local state in a document that describes upstream,
            // and v3 derives it. `Migration` becomes the explicit field.
            let migrates_from = channel.tags.iter().find_map(|tag| match tag {
                super::v1::channel::Tags::Migration { old_channel } => Some(old_channel.clone()),
                super::v1::channel::Tags::Partial => None,
            });

            if let Some(alias) = channel.alias.clone() {
                aliases.push((alias, channel.name.clone()));
            }

            channels.push(v3::Channel {
                name: channel.name,
                alias: channel.alias,
                migrates_from,
                components,
                extra: Default::default(),
            });
        }

        let mut manifest = v3::Manifest {
            // The output of this conversion is a v3 manifest, so it declares the v3 version.
            // Stamping the v1 constant here left converted manifests claiming to be v1.0.1.
            manifest_version: v3::MANIFEST_VERSION,
            date: value.date,
            networks: Default::default(),
            channels,
            extra: Default::default(),
        };

        for (alias, channel) in aliases {
            // `crate::channel::…`, not `channel::…`: the enum still lives in `crate::channel` at
            // this point, and `v1/channel.rs`'s `use` of it is private, so naming it through the
            // v1 module is `E0603: enum import ChannelAlias is private`. T7 moves the enum here
            // and flips these four paths.
            let network = match alias {
                crate::channel::ChannelAlias::Stable => crate::channel::DEFAULT_NETWORK.to_string(),
                crate::channel::ChannelAlias::Nightly(None) => {
                    crate::channel::canonical_network("nightly").to_string()
                },
                crate::channel::ChannelAlias::Nightly(Some(suffix)) => format!("nightly-{suffix}"),
                crate::channel::ChannelAlias::Tag(tag) => {
                    crate::channel::canonical_network(&tag).to_string()
                },
            };
            manifest.promote(&network, channel);
        }

        // v1 derived `stable` as the highest channel when nothing carried the alias. Reproducing
        // that here is what makes a converted document mean what it meant before -- and it is the
        // only place such a derivation is allowed, because it describes v1's semantics rather than
        // v3's.
        if manifest.network_version(crate::channel::DEFAULT_NETWORK).is_none()
            && let Some(highest) = manifest
                .channels
                .iter()
                .map(|c| c.name.clone())
                .max_by(|a, b| a.cmp_precedence(b))
        {
            manifest.promote(crate::channel::DEFAULT_NETWORK, highest);
        }

        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use crate::manifest::{VersionedManifest, v3};

    fn v1(channels: serde_json::Value) -> String {
        serde_json::json!({
            "manifest_version": "1.0.1",
            "date": 1735689600,
            "channels": channels
        })
        .to_string()
    }

    fn channel(name: &str, alias: Option<&str>) -> serde_json::Value {
        let mut value = serde_json::json!({ "name": name, "components": [] });
        if let Some(alias) = alias {
            value["alias"] = serde_json::Value::String(alias.to_string());
        }
        value
    }

    /// v1 expressed "which channel is current" as an alias on the channel. v3 expresses it as a
    /// network, so the converter has to translate rather than drop it -- otherwise every converted
    /// manifest would silently lose which toolchain mainnet runs.
    #[test]
    fn v1_aliases_become_networks() {
        let src = v1(serde_json::json!([
            channel("0.14.0", Some("stable")),
            channel("0.15.0", Some("nightly")),
            channel("0.15.1", Some("custom-dev-build")),
        ]));

        let manifest = VersionedManifest::parse_str(&src).expect("must convert");

        assert_eq!(manifest.network_version("mainnet"), Some(&semver::Version::new(0, 14, 0)));
        assert_eq!(manifest.network_version("devnet"), Some(&semver::Version::new(0, 15, 0)));
        assert_eq!(
            manifest.network_version("custom-dev-build"),
            Some(&semver::Version::new(0, 15, 1))
        );
        assert_eq!(manifest.manifest_version(), &v3::MANIFEST_VERSION);
    }

    #[test]
    fn a_suffixed_nightly_keeps_its_full_name() {
        let src = v1(serde_json::json!([channel("0.15.0", Some("nightly-experimental"))]));
        let manifest = VersionedManifest::parse_str(&src).expect("must convert");
        assert_eq!(
            manifest.network_version("nightly-experimental"),
            Some(&semver::Version::new(0, 15, 0))
        );
    }

    /// v1 had no notion of testnet, so a channel could carry a bare `beta` tag. v3 treats `beta` as
    /// a synonym for `testnet` and rewrites it before any lookup, so the converter has to land the
    /// network under the name a lookup will actually use.
    #[test]
    fn a_beta_alias_becomes_the_testnet_network() {
        let src = v1(serde_json::json!([channel("0.15.0", Some("beta"))]));
        let manifest = VersionedManifest::parse_str(&src).expect("must convert");
        assert_eq!(manifest.network_version("testnet"), Some(&semver::Version::new(0, 15, 0)));
        assert!(manifest.network_version("beta").is_none());
    }

    /// v1 derived stable as the highest channel when nothing carried the alias. Reproducing that
    /// here -- and only here -- is what makes a converted document mean what it meant before.
    ///
    /// This is what the v1.0.1 fixtures under `tests/data/` rely on: none of them carries a stable
    /// alias, and every test that installs by name resolves through this rule.
    #[test]
    fn without_a_stable_alias_mainnet_becomes_the_highest_channel() {
        let src = v1(serde_json::json!([channel("0.14.0", None), channel("0.15.0", None)]));
        let manifest = VersionedManifest::parse_str(&src).expect("must convert");
        assert_eq!(manifest.network_version("mainnet"), Some(&semver::Version::new(0, 15, 0)));
    }
}
