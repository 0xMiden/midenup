use std::{borrow::Cow, fmt};

use anyhow::Context;
use serde::Serialize;

use crate::config::Config;
pub use crate::manifest::Channel;

#[derive(Debug, Clone)]
pub enum UpstreamMatch {
    /// The remote Channel is this Channel's upstream equivalent.
    UpstreamCounterpart,
    /// The remote channel supersedes this one, and declares so with `migrates_from`.
    Migrated { old_channel: semver::Version },
}

#[derive(Debug, Clone)]
pub struct UpstreamChannel {
    pub channel: Channel,
    pub upstream_match: UpstreamMatch,
}

impl UpstreamChannel {
    pub fn new(channel: Channel, upstream_match: UpstreamMatch, config: &Config) -> Self {
        let mut synced_channel = channel.clone();
        synced_channel.sync(config);
        UpstreamChannel { channel: synced_channel, upstream_match }
    }
}

/// The network `midenup` uses when nothing else selects a channel.
pub const DEFAULT_NETWORK: &str = "mainnet";

/// Traditional release-train names, accepted as input and rewritten to the network they mean.
///
/// Hardcoded rather than manifest-declared on purpose. These are about user vocabulary, not
/// deployment, and they do not change. Expressing them as data would mean either `promote` moving
/// two keys in lockstep, or letting a map value hold an indirection -- with the cycle detection
/// that implies, and the ability for a manifest author to make `stable` mean anything.
const SYNONYMS: &[(&str, &str)] =
    &[("stable", DEFAULT_NETWORK), ("beta", "testnet"), ("nightly", "devnet")];

/// Rewrites a traditional name to the network it means. Any other name is returned unchanged.
pub fn canonical_network(name: &str) -> &str {
    SYNONYMS
        .iter()
        .find(|(synonym, _)| *synonym == name)
        .map(|(_, network)| *network)
        .unwrap_or(name)
}

/// User-facing channel reference: either a specific toolchain, or a name that moves.
///
/// A name is resolved against the manifest's `networks` map, so which names exist is data rather
/// than code and a new network needs no release of `midenup`. The cost is that an unknown name
/// parses and fails at lookup, which is why the lookup's diagnostic lists what is declared.
///
/// A selector is also the key `var/` is stored under, so its `Display` form is joined onto a path.
/// A `Version` is safe by semver's own grammar, which admits no path separator; a `Named` is safe
/// because [`FromStr`](core::str::FromStr) rejects any name that is not a single path segment.
#[derive(Debug, Clone)]
pub enum UserChannel {
    Version(semver::Version),
    Named(Cow<'static, str>),
}

impl Default for UserChannel {
    fn default() -> Self {
        Self::Named(Cow::Borrowed(DEFAULT_NETWORK))
    }
}

impl fmt::Display for UserChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(version) => write!(f, "{version}"),
            Self::Named(name) => f.write_str(name),
        }
    }
}

impl Serialize for UserChannel {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::de::Deserialize<'de> for UserChannel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Unexpected;
        use serde_untagged::UntaggedEnumVisitor;

        UntaggedEnumVisitor::new()
            .string(|s| {
                s.parse::<UserChannel>().map_err(|err| {
                    serde::de::Error::invalid_value(Unexpected::Str(s), &err.to_string().as_str())
                })
            })
            .deserialize(deserializer)
    }
}

impl core::str::FromStr for UserChannel {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            anyhow::bail!("a channel must be named: either a version like '0.15.0', or a network");
        }
        if let Ok(version) = semver::Version::parse(s) {
            return Ok(Self::Version(version));
        }

        // A name becomes a single path segment: `var/<name>` and `toolchains/<name>`. A name can
        // reach here from a project's `miden-toolchain.toml`, which is untrusted input, so the same
        // gate the manifest's network names pass through applies here -- there is then no way to
        // hold a `Named` selector that is not a safe segment.
        let name = canonical_network(s);
        crate::plan::validate_artifact_id(name)
            .with_context(|| format!("'{name}' cannot name a channel"))?;

        Ok(Self::Named(Cow::Owned(name.to_string())))
    }
}

#[cfg(test)]
mod tests {
    use core::str::FromStr;

    use super::*;

    #[test]
    fn a_semantic_version_is_a_version_and_anything_else_is_a_name() {
        assert!(matches!(
            UserChannel::from_str("0.15.0").unwrap(),
            UserChannel::Version(v) if v == semver::Version::new(0, 15, 0)
        ));
        assert!(matches!(
            UserChannel::from_str("mainnet").unwrap(),
            UserChannel::Named(name) if name == "mainnet"
        ));
    }

    /// The traditional names are user vocabulary, and canonicalizing them at parse time means
    /// nothing downstream -- display, symlinks, state, diagnostics -- ever sees them.
    #[test]
    fn the_traditional_names_canonicalize_to_networks() {
        for (typed, meant) in [("stable", "mainnet"), ("beta", "testnet"), ("nightly", "devnet")] {
            assert_eq!(UserChannel::from_str(typed).unwrap().to_string(), meant);
        }
    }

    /// An unknown name parses. It has to: which names exist is manifest data, not a fixed set, so
    /// the diagnostic belongs where the manifest is available.
    #[test]
    fn an_unknown_name_parses_and_is_not_rewritten() {
        assert_eq!(UserChannel::from_str("mainet").unwrap().to_string(), "mainet");
    }

    #[test]
    fn the_empty_string_is_not_a_channel() {
        assert!(UserChannel::from_str("").is_err());
    }

    /// A selector is joined onto `$MIDENUP_HOME` as a single segment -- `var/<selector>` and
    /// `toolchains/<selector>` -- and one can arrive from a project's `miden-toolchain.toml`.
    /// Rejecting it here is what makes every such join safe by construction.
    #[test]
    fn a_name_that_is_not_a_single_path_segment_is_not_a_channel() {
        for bad in ["../../evil", "a/b", ".", "..", "a\\b", "-flag", "with\0nul"] {
            assert!(UserChannel::from_str(bad).is_err(), "must reject {bad:?}");
        }
        for good in ["mainnet", "testnet", "devnet", "some-future-net", "0.15.0"] {
            assert!(UserChannel::from_str(good).is_ok(), "must accept {good:?}");
        }
    }

    /// The rejection has to survive deserialization too: `miden-toolchain.toml` is read through
    /// `Deserialize`, never through `FromStr` directly.
    #[test]
    fn a_toolchain_file_cannot_name_a_traversal() {
        assert!(serde_json::from_str::<UserChannel>(r#""../../evil""#).is_err());
    }

    #[test]
    fn the_default_channel_is_mainnet() {
        assert_eq!(UserChannel::default().to_string(), DEFAULT_NETWORK);
    }

    /// `midenup set` writes this into `miden-toolchain.toml`, so it must round-trip as the plain
    /// string a user would have typed.
    #[test]
    fn a_channel_serializes_as_the_string_it_came_from() {
        let named = UserChannel::from_str("mainnet").unwrap();
        assert_eq!(serde_json::to_string(&named).unwrap(), r#""mainnet""#);
        let pinned = UserChannel::from_str("0.15.0").unwrap();
        assert_eq!(serde_json::to_string(&pinned).unwrap(), r#""0.15.0""#);
    }

    /// A toolchain file written before networks existed keeps working, and means mainnet.
    #[test]
    fn a_toolchain_file_saying_stable_means_mainnet() {
        let parsed: UserChannel = serde_json::from_str(r#""stable""#).unwrap();
        assert_eq!(parsed.to_string(), "mainnet");
    }
}
