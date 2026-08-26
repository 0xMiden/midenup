//! Local installation state.
//!
//! This is a **different document** from a channel manifest, not a second role of one. A manifest
//! describes what exists upstream; this describes what this machine installed. They share no
//! top-level key -- `manifest_version` versus `state_version` -- so neither can be mistaken for the
//! other, and no `role` discriminator is needed to tell them apart.

pub mod installation;

use std::path::Path;

use serde::{Deserialize, Serialize};

pub use self::installation::{
    Installation, Output, PublicationId, PublicationRef, RealizedMethod, Receipt,
};
use crate::manifest::version;

/// The schema version of the local state document.
pub const STATE_VERSION: semver::Version = semver::Version::new(1, 0, 0);

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("failed to read local state from '{path}': {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid local state in '{path}': {reason}")]
    Invalid { path: std::path::PathBuf, reason: String },
    #[error(
        "'{path}' is a {found} document, but a {expected} document was expected; midenup keeps \
         these separate and will not read one as the other"
    )]
    WrongDocumentType {
        path: std::path::PathBuf,
        expected: &'static str,
        found: &'static str,
    },
    #[error("local state in '{path}' declares version {found}, which requires a newer midenup")]
    RequiresNewer {
        path: std::path::PathBuf,
        found: semver::Version,
    },
    #[error("failed to write local state: {0}")]
    Write(#[from] crate::utils::atomic::WriteError),
}

/// Everything `midenup` has installed on this machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalState {
    pub state_version: semver::Version,
    #[serde(default)]
    pub installations: Vec<Installation>,
}

impl Default for LocalState {
    fn default() -> Self {
        Self {
            state_version: STATE_VERSION,
            installations: Vec::new(),
        }
    }
}

impl LocalState {
    /// Loads local state from `path`.
    ///
    /// A missing file is an *empty* state, not an error: nothing installed is a perfectly valid
    /// thing to have recorded, and the alternative is making every caller special-case it.
    pub fn load(path: &Path) -> Result<Self, StateError> {
        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            },
            Err(source) => return Err(StateError::Io { path: path.to_path_buf(), source }),
        };

        if contents.trim().is_empty() {
            return Ok(Self::default());
        }

        Self::parse_str(&contents, path)
    }

    pub fn parse_str(contents: &str, path: &Path) -> Result<Self, StateError> {
        // Report a manifest given where state was expected as exactly that, rather than as a pile
        // of missing-field errors.
        if version::read_version_header(contents, "manifest_version").is_ok()
            && version::read_version_header(contents, "state_version").is_err()
        {
            return Err(StateError::WrongDocumentType {
                path: path.to_path_buf(),
                expected: "local state",
                found: "channel manifest",
            });
        }

        let header = version::read_version_header(contents, "state_version").map_err(|err| {
            StateError::Invalid {
                path: path.to_path_buf(),
                reason: err.to_string(),
            }
        })?;

        match version::classify(&header.version, STATE_VERSION.major) {
            version::Compatibility::Supported => {},
            version::Compatibility::RequiresNewer { found } => {
                return Err(StateError::RequiresNewer { path: path.to_path_buf(), found });
            },
            version::Compatibility::TooOld { found } => {
                return Err(StateError::Invalid {
                    path: path.to_path_buf(),
                    reason: format!("unsupported state version {found}"),
                });
            },
        }

        serde_json::from_str(contents).map_err(|err| StateError::Invalid {
            path: path.to_path_buf(),
            reason: err.to_string(),
        })
    }

    /// Writes state to `path`, refusing to commit anything that cannot be read back.
    pub fn save(&self, path: &Path) -> Result<(), StateError> {
        crate::trace!("writing {}", path.display());
        crate::utils::atomic::write_validated(path, self, |written| {
            LocalState::parse_str(written, path)
                .map(|_| ())
                .map_err(|err| format!("the result would not parse as local state: {err}"))
        })?;
        Ok(())
    }

    pub fn get(&self, channel: &semver::Version) -> Option<&Installation> {
        self.installations.iter().find(|i| &i.channel == channel)
    }

    pub fn get_mut(&mut self, channel: &semver::Version) -> Option<&mut Installation> {
        self.installations.iter_mut().find(|i| &i.channel == channel)
    }

    /// Inserts or replaces the record for a channel, keeping the list ordered by channel.
    pub fn upsert(&mut self, installation: Installation) {
        self.remove(&installation.channel);
        self.installations.push(installation);
        self.installations.sort_by(|a, b| a.channel.cmp(&b.channel));
    }

    pub fn remove(&mut self, channel: &semver::Version) {
        self.installations.retain(|i| &i.channel != channel);
    }

    pub fn channels(&self) -> impl Iterator<Item = &semver::Version> {
        self.installations.iter().map(|i| &i.channel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{plan::PlanKey, profile::Profile, resolve::Intent};

    fn sample() -> LocalState {
        LocalState {
            state_version: STATE_VERSION,
            installations: vec![Installation {
                channel: semver::Version::new(0, 15, 0),
                intent: Intent::new(&[Profile::Minimal], &["client"]),
                components: vec![],
                publication: PublicationRef::Managed {
                    id: PublicationId::generate(),
                    plan_key: serde_json::from_str::<PlanKey>(&format!(
                        "\"pk1:{}\"",
                        "a".repeat(64)
                    ))
                    .unwrap(),
                    target: "aarch64-apple-darwin".to_string(),
                },
                installed_at: 1735689600,
            }],
        }
    }

    #[test]
    fn state_round_trips() {
        let dir = tempdir::TempDir::new("state-roundtrip").unwrap();
        let path = dir.path().join("state.json");
        let state = sample();
        state.save(&path).unwrap();
        assert_eq!(LocalState::load(&path).unwrap(), state);
    }

    #[test]
    fn a_manifest_is_rejected_where_state_is_expected() {
        let dir = tempdir::TempDir::new("state-wrongdoc").unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, r#"{"manifest_version":"2.0.0","date":1,"channels":[]}"#).unwrap();

        let err = LocalState::load(&path).expect_err("must refuse a manifest");
        assert!(
            matches!(err, StateError::WrongDocumentType { found: "channel manifest", .. }),
            "expected WrongDocumentType, got: {err}"
        );
    }

    #[test]
    fn a_missing_state_file_is_an_empty_state_not_an_error() {
        let dir = tempdir::TempDir::new("state-missing").unwrap();
        let state = LocalState::load(&dir.path().join("state.json")).unwrap();
        assert!(state.installations.is_empty());
        assert_eq!(state.state_version, STATE_VERSION);
    }

    #[test]
    fn an_empty_state_file_is_an_empty_state() {
        let dir = tempdir::TempDir::new("state-empty").unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, "").unwrap();
        assert!(LocalState::load(&path).unwrap().installations.is_empty());
    }

    #[test]
    fn a_newer_major_state_version_requires_a_newer_midenup() {
        let dir = tempdir::TempDir::new("state-newer").unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, r#"{"state_version":"2.0.0","installations":[]}"#).unwrap();
        assert!(matches!(LocalState::load(&path), Err(StateError::RequiresNewer { .. })));
    }

    #[test]
    fn upsert_replaces_and_keeps_channels_ordered() {
        let mut state = LocalState::default();
        for version in [(0, 16, 0), (0, 14, 0), (0, 15, 0)] {
            let mut installation = sample().installations.remove(0);
            installation.channel = semver::Version::new(version.0, version.1, version.2);
            state.upsert(installation);
        }
        assert_eq!(state.installations.len(), 3);
        assert!(state.channels().cloned().collect::<Vec<_>>().is_sorted());

        let mut replacement = sample().installations.remove(0);
        replacement.channel = semver::Version::new(0, 15, 0);
        replacement.installed_at = 42;
        state.upsert(replacement);

        assert_eq!(state.installations.len(), 3, "upsert must replace, not append");
        assert_eq!(state.get(&semver::Version::new(0, 15, 0)).unwrap().installed_at, 42);
    }

    #[test]
    fn remove_drops_only_the_named_channel() {
        let mut state = sample();
        state.remove(&semver::Version::new(0, 14, 0));
        assert_eq!(state.installations.len(), 1, "removing an absent channel is a no-op");
        state.remove(&semver::Version::new(0, 15, 0));
        assert!(state.installations.is_empty());
    }

    /// A plan key without a recognized algorithm prefix is unknown, not merely different.
    #[test]
    fn an_unprefixed_plan_key_is_rejected() {
        let dir = tempdir::TempDir::new("state-badkey").unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(
            &path,
            r#"{"state_version":"1.0.0","installations":[{"channel":"0.15.0",
               "intent":{},"components":[],
               "publication":{"kind":"managed","id":"abc","plan_key":"deadbeef","target":"t"},
               "installed_at":1}]}"#,
        )
        .unwrap();
        assert!(matches!(LocalState::load(&path), Err(StateError::Invalid { .. })));
    }

    /// The confusion must be named in both directions.
    #[test]
    fn state_given_to_the_manifest_parser_is_rejected_by_name() {
        let err = crate::manifest::VersionedManifest::parse_str(
            r#"{"state_version":"1.0.0","installations":[]}"#,
        )
        .expect_err("must refuse local state");
        assert!(
            matches!(
                err,
                crate::manifest::ManifestError::WrongDocumentType { found: "local state", .. }
            ),
            "expected WrongDocumentType, got: {err}"
        );
    }

    #[test]
    fn publication_ids_are_unique() {
        let ids: std::collections::HashSet<_> =
            (0..1000).map(|_| PublicationId::generate()).collect();
        assert_eq!(ids.len(), 1000, "generated ids must not collide");
    }

    #[test]
    fn a_needs_reinstall_record_is_not_managed() {
        let mut state = sample();
        state.installations[0].publication = PublicationRef::NeedsReinstall;
        assert!(!state.installations[0].is_managed());

        let dir = tempdir::TempDir::new("state-needsreinstall").unwrap();
        let path = dir.path().join("state.json");
        state.save(&path).unwrap();
        assert_eq!(LocalState::load(&path).unwrap(), state, "must round-trip");
    }
}
