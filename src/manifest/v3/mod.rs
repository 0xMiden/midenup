mod channel;
mod component;
pub mod unknown;

use std::{collections::BTreeMap, path::Path};

use serde::{Deserialize, Serialize};

pub use self::{channel::*, component::*, unknown::*};
use super::ManifestError;

pub const MANIFEST_VERSION: semver::Version = semver::Version::new(3, 0, 0);

/// The global manifest of all known channels and their toolchains
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Manifest {
    /// The schema version this document declares.
    ///
    /// Deserialized normally and verified after parsing. It was previously `skip_deserializing`
    /// with a default, which meant the in-memory value was *always* the current version regardless
    /// of what the file said -- making every version check downstream a tautology.
    pub(super) manifest_version: semver::Version,
    /// The UTC timestamp at which this manifest was generated
    pub(super) date: i64,
    /// Which channel each release network currently runs.
    ///
    /// A network is a *moving name* for a channel: `mainnet` names whichever toolchain is deployed
    /// to mainnet today, and `update-manifest promote` is what moves it. Several networks may name
    /// one channel, which is the normal state once a testnet toolchain is promoted to mainnet --
    /// the per-channel `alias` field this replaces could not express that at all.
    ///
    /// Deliberately not derived. Which toolchain a network runs is a deployment fact; mainnet may
    /// lag testnet by several releases, and a hotfix may put it ahead. No ordering over version
    /// numbers can express that.
    ///
    /// `#[serde(default)]` because parsing does not validate. That a manifest declares `mainnet`
    /// is a rule in `validate::validate_manifest`, not a precondition for reading the
    /// document.
    #[serde(default)]
    pub(super) networks: BTreeMap<String, semver::Version>,
    /// The channels described in this manifest
    pub(super) channels: Vec<Channel>,
    /// Fields declared by a newer schema that this build does not recognize.
    ///
    /// Safe to derive here: `Manifest` has no other flattened field, so the catch-all captures
    /// only genuinely unknown keys.
    #[serde(flatten)]
    pub(super) extra: Extra,
}

impl Manifest {
    /// Loads a [Manifest] from the given file path.
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Self, ManifestError> {
        let path = path.as_ref();
        let manifest_contents = std::fs::read_to_string(path)
            .map_err(|_| ManifestError::Missing(path.display().to_string()))?;
        // This could potentially be valid if we are parsing the local manifest
        if manifest_contents.is_empty() {
            return Err(ManifestError::Empty);
        }

        Self::parse_str(&manifest_contents)
    }

    /// Parses a v3 [Manifest] from `content`, and returns it in canonical form.
    pub fn parse_str(content: &str) -> Result<Self, ManifestError> {
        let mut manifest = serde_json::from_str::<Self>(content)
            .map_err(|err| ManifestError::Invalid(format!("failed to parse manifest: {err}")))?;

        // Verify the literal declared version rather than trusting a serde default to have
        // supplied it. Callers dispatch on a separately-read header, so a mismatch here means the
        // document disagrees with itself.
        if manifest.manifest_version.major != MANIFEST_VERSION.major {
            return Err(ManifestError::UnsupportedVersion(manifest.manifest_version));
        }

        // Sort channels by version, in ascending order
        if !manifest.channels.is_sorted_by_key(|channel| &channel.name) {
            manifest.channels.sort_by_key(|channel| channel.name.clone());
        };

        // Sort the components of each channel by name
        for channel in manifest.channels.iter_mut() {
            if !channel.components.is_sorted_by_key(|c| c.name.as_ref()) {
                channel.components.sort_by_key(|c| c.name.clone());
            }
        }

        Ok(manifest)
    }
}

impl Default for Manifest {
    fn default() -> Self {
        let date = chrono::Utc::now().timestamp();
        Self {
            manifest_version: MANIFEST_VERSION,
            date,
            networks: BTreeMap::new(),
            channels: vec![],
            extra: Extra::new(),
        }
    }
}

impl Manifest {
    #[inline(always)]
    pub fn manifest_version(&self) -> &semver::Version {
        &self.manifest_version
    }
}
