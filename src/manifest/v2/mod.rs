mod channel;
mod component;

use std::path::Path;

use serde::{Deserialize, Serialize};

pub use self::{channel::*, component::*};
use super::ManifestError;

pub const MANIFEST_VERSION: semver::Version = semver::Version::new(2, 0, 0);

/// The global manifest of all known channels and their toolchains
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Manifest {
    #[serde(
        skip_deserializing,
        default = "current_manifest_version",
        serialize_with = "serialize_current_manifest_version"
    )]
    pub(super) manifest_version: semver::Version,
    /// The UTC timestamp at which this manifest was generated
    pub(super) date: i64,
    /// The channels described in this manifest
    pub(super) channels: Vec<Channel>,
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

    /// Parses a [VersionedManifest] from `content`, and returns it in canonical form
    pub fn parse_str(content: &str) -> Result<Self, ManifestError> {
        let mut manifest = serde_json::from_str::<Self>(content)
            .map_err(|err| ManifestError::Invalid(format!("failed to parse manifest: {err}")))?;

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
            channels: vec![],
        }
    }
}

const fn current_manifest_version() -> semver::Version {
    MANIFEST_VERSION
}

fn serialize_current_manifest_version<S>(
    _ignore: &semver::Version,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    semver::Version::serialize(&MANIFEST_VERSION, serializer)
}

impl Manifest {
    #[inline(always)]
    pub fn manifest_version(&self) -> &semver::Version {
        &self.manifest_version
    }
}
