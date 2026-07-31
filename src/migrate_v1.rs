//! Migrating a v1.0.1 local manifest to `state.json`.
//!
//! # What is carried forward, and what is not
//!
//! **The selection only**: for each installed channel, its version and the names of its installed
//! components. Installed filenames, aliases, call formats, artifact destinations, Cargo
//! bookkeeping -- all discarded, because upstream is authoritative for every one of them and
//! carrying a stale copy forward would mean reconciling two answers later.
//!
//! The result is ordinary native intent, roots-only:
//!
//! ```json
//! { "channel": "0.15.0", "intent": { "profiles": [], "roots": ["vm", "client", "core"] } }
//! ```
//!
//! which is why there is no `Frozen` intent variant. Roots-only already has exactly the semantics a
//! frozen migration needs -- new dependencies of the roots are picked up, unrelated new profile
//! members are not -- using the same resolver as everything else.
//!
//! # Why it runs first
//!
//! Before recovery, and before any upstream fetch. A user whose network is down, or whose upstream
//! manifest has moved, must still end up with a readable `state.json`; making migration depend on a
//! successful fetch would strand exactly the people who most need their installation to keep
//! working.
//!
//! # Why the temporary file is validated before the rename
//!
//! Every failure before the rename leaves the v1 document byte-for-byte intact, and after it
//! `state.json` is known to parse. Validating only *after* replacing the original is too late to
//! preserve anything.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::{
    paths,
    state::{Installation, LocalState, PublicationRef},
};

/// The oldest local document this build can migrate.
pub const MIGRATION_FLOOR: semver::Version = semver::Version::new(1, 0, 1);

/// Where a v1 `midenup` kept its local installation record.
pub fn v1_manifest_path(home: &Path) -> PathBuf {
    home.join("manifest").with_extension("json")
}

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("failed to read '{path}': {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("'{path}' is not a manifest midenup can read: {reason}")]
    Malformed { path: PathBuf, reason: String },
    #[error(
        "'{path}' declares version {found}, which is older than the oldest version midenup can \
         migrate ({floor}). The file has been left untouched. To start over, remove $MIDENUP_HOME \
         and reinstall your toolchains."
    )]
    UnsupportedVersion {
        path: PathBuf,
        found: semver::Version,
        floor: semver::Version,
    },
    #[error("failed to write the migrated state: {0}")]
    Write(#[from] crate::utils::atomic::WriteError),
    #[error(transparent)]
    Injected(#[from] crate::fault::InjectedFault),
}

#[derive(Debug, PartialEq, Eq)]
pub enum MigrationOutcome {
    NothingToDo,
    Migrated { channels: Vec<semver::Version> },
}

/// Everything migration needs from a v1 document.
///
/// Deliberately not the full `v1::Manifest`: unknown and unreadable fields are irrelevant to a
/// selection-only migration, and refusing to migrate a document whose *names* are perfectly legible
/// because some unrelated field cannot be converted would be a bug, not caution.
#[derive(Deserialize)]
struct V1Manifest {
    #[serde(default)]
    date: Option<i64>,
    #[serde(default)]
    channels: Vec<V1Channel>,
}

#[derive(Deserialize)]
struct V1Channel {
    name: semver::Version,
    #[serde(default)]
    components: Vec<V1Component>,
}

#[derive(Deserialize)]
struct V1Component {
    name: String,
}

/// Migrates `$MIDENUP_HOME/manifest.json`, if there is one to migrate.
///
/// Idempotent and cheap when there is nothing to do -- one `stat` -- because it is called on every
/// startup, from two places: before the upstream manifest is fetched (so an unreachable upstream
/// cannot prevent it) and again before a command runs (so an in-process caller that built its own
/// `Config` still gets it).
pub fn migrate_if_needed(home: &Path) -> Result<MigrationOutcome, MigrationError> {
    let path = v1_manifest_path(home);

    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(MigrationOutcome::NothingToDo);
        },
        Err(source) => return Err(MigrationError::Read { path, source }),
    };

    let header = crate::manifest::version::read_version_header(&contents, "manifest_version")
        .map_err(|err| MigrationError::Malformed {
            path: path.clone(),
            reason: err.to_string(),
        })?;

    // Not a v1 document at all. Older builds of this branch wrote a v2 `Manifest::default()` here;
    // it is meaningless but harmless, and deleting a file this build did not write is not
    // migration's business.
    if header.version.major >= 2 {
        return Ok(MigrationOutcome::NothingToDo);
    }

    if header.version < MIGRATION_FLOOR {
        return Err(MigrationError::UnsupportedVersion {
            path,
            found: header.version,
            floor: MIGRATION_FLOOR,
        });
    }

    // A v2 state document already exists. The two cannot be reconciled -- one describes what this
    // build installed, the other what an older one did -- and `state.json` is the sole logical
    // authority, so it wins. The v1 file is left alone rather than deleted: it is not ours to
    // remove, and nothing reads it.
    let state_path = paths::state_path(home);
    if LocalState::load(&state_path).is_ok_and(|state| !state.installations.is_empty()) {
        return Ok(MigrationOutcome::NothingToDo);
    }

    let manifest: V1Manifest =
        serde_json::from_str(&contents).map_err(|err| MigrationError::Malformed {
            path: path.clone(),
            reason: err.to_string(),
        })?;

    // The manifest's own timestamp is the closest thing to an install time that a v1 document
    // records. Falling back to now would claim the installation happened during the migration.
    let installed_at = manifest.date.unwrap_or_else(|| chrono::Utc::now().timestamp());

    let mut state = LocalState::default();
    let mut channels = Vec::with_capacity(manifest.channels.len());

    for channel in manifest.channels {
        let roots = channel.components.into_iter().map(|component| component.name).collect();

        channels.push(channel.name.clone());
        state.upsert(Installation {
            channel: channel.name,
            intent: crate::resolve::Intent { profiles: Default::default(), roots },
            // Nothing describes the pre-publication tree, so there is no component snapshot to
            // record. `NeedsReinstall` is what makes that safe: the record is never executed
            // against, and the next operation touching the channel installs it properly.
            components: Vec::new(),
            publication: PublicationRef::NeedsReinstall,
            installed_at,
        });
    }

    crate::fault::fail_at(crate::fault::FaultPoint::PreMigrationCommit)?;

    // Serialize, fsync, re-read, parse the bytes that actually landed, and only then rename. Up to
    // the rename the v1 document is untouched; after it, `state.json` is known to parse.
    state.save(&state_path).map_err(|err| match err {
        crate::state::StateError::Write(err) => MigrationError::Write(err),
        other => MigrationError::Malformed {
            path: state_path.clone(),
            reason: other.to_string(),
        },
    })?;

    // Committed. From here the v1 document is redundant, and leaving it would make the next startup
    // try to migrate again over a state that already exists.
    let _ = std::fs::remove_file(&path);

    Ok(MigrationOutcome::Migrated { channels })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v1_manifest(version: &str, channels: serde_json::Value) -> String {
        serde_json::json!({
            "manifest_version": version,
            "date": 1735689600,
            "channels": channels
        })
        .to_string()
    }

    fn home_with(contents: &str) -> (tempdir::TempDir, PathBuf) {
        let temp = tempdir::TempDir::new("migrate-v1").unwrap();
        let home = temp.path().join("midenup");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(v1_manifest_path(&home), contents).unwrap();
        (temp, home)
    }

    fn installed(channel: &str, components: &[&str]) -> serde_json::Value {
        serde_json::json!([{
            "name": channel,
            "components": components.iter().map(|name| serde_json::json!({
                "name": name,
                // Whatever else a v1 component carried is irrelevant to a selection-only
                // migration, and must not be able to block one.
                "version": "0.1.0",
                "installed_executable": format!("miden-{name}"),
            })).collect::<Vec<_>>()
        }])
    }

    #[test]
    fn nothing_to_migrate_is_not_an_error() {
        let temp = tempdir::TempDir::new("migrate-none").unwrap();
        assert_eq!(migrate_if_needed(temp.path()).unwrap(), MigrationOutcome::NothingToDo);
    }

    #[test]
    fn the_installed_selection_becomes_roots_only_intent() {
        let (_temp, home) =
            home_with(&v1_manifest("1.0.1", installed("0.15.0", &["vm", "client", "core"])));

        let outcome = migrate_if_needed(&home).expect("must migrate");
        assert_eq!(
            outcome,
            MigrationOutcome::Migrated {
                channels: vec![semver::Version::new(0, 15, 0)]
            }
        );

        let state = LocalState::load(&paths::state_path(&home)).unwrap();
        let installation = state.get(&semver::Version::new(0, 15, 0)).expect("recorded");

        assert!(
            installation.intent.profiles.is_empty(),
            "roots-only, so unrelated new profile members are not pulled in"
        );
        for root in ["vm", "client", "core"] {
            assert!(installation.intent.roots.contains(root), "{root} must be carried forward");
        }
        assert!(matches!(installation.publication, PublicationRef::NeedsReinstall));
        assert!(
            installation.components.is_empty(),
            "nothing describes the pre-publication tree, so nothing is claimed about it"
        );
    }

    #[test]
    fn the_v1_document_is_removed_once_the_state_document_is_committed() {
        let (_temp, home) = home_with(&v1_manifest("1.0.1", installed("0.15.0", &["vm"])));
        migrate_if_needed(&home).unwrap();

        assert!(!v1_manifest_path(&home).exists());
        assert!(paths::state_path(&home).exists());
    }

    /// Anything older than the floor is rejected *without touching the file*: the user's only
    /// remaining option is the old binary, and destroying its record would remove that too.
    #[test]
    fn a_document_older_than_the_floor_is_rejected_and_left_alone() {
        let (_temp, home) = home_with(&v1_manifest("1.0.0", installed("0.15.0", &["vm"])));
        let before = std::fs::read(v1_manifest_path(&home)).unwrap();

        let err = migrate_if_needed(&home).expect_err("must refuse");
        assert!(matches!(err, MigrationError::UnsupportedVersion { .. }), "{err}");
        assert!(err.to_string().contains("1.0.0"), "the diagnostic must name the version: {err}");

        assert_eq!(std::fs::read(v1_manifest_path(&home)).unwrap(), before);
        assert!(!paths::state_path(&home).exists(), "and must leave no partial state document");
    }

    #[test]
    fn a_v2_document_is_not_a_migration_candidate() {
        let (_temp, home) = home_with(&v1_manifest("2.0.0", serde_json::json!([])));
        assert_eq!(migrate_if_needed(&home).unwrap(), MigrationOutcome::NothingToDo);
        assert!(v1_manifest_path(&home).exists(), "and is left where it was");
    }

    #[test]
    fn a_malformed_document_is_reported_rather_than_partially_migrated() {
        let (_temp, home) = home_with("{ this is not json");
        assert!(matches!(migrate_if_needed(&home), Err(MigrationError::Malformed { .. })));
        assert!(!paths::state_path(&home).exists());
    }

    /// Running twice must not migrate twice.
    #[test]
    fn migration_is_idempotent() {
        let (_temp, home) = home_with(&v1_manifest("1.0.1", installed("0.15.0", &["vm"])));
        migrate_if_needed(&home).unwrap();
        let after_first = std::fs::read(paths::state_path(&home)).unwrap();

        assert_eq!(migrate_if_needed(&home).unwrap(), MigrationOutcome::NothingToDo);
        assert_eq!(std::fs::read(paths::state_path(&home)).unwrap(), after_first);
    }

    /// A v1 document that reappears next to a populated `state.json` -- someone ran an older
    /// binary -- cannot be reconciled with it. `state.json` is the sole logical authority, so it
    /// wins, and the v1 file is left alone rather than deleted.
    #[test]
    fn an_existing_state_document_wins_and_the_v1_file_is_not_touched() {
        let (_temp, home) = home_with(&v1_manifest("1.0.1", installed("0.15.0", &["vm"])));
        migrate_if_needed(&home).unwrap();

        // The old binary writes its manifest again.
        std::fs::write(
            v1_manifest_path(&home),
            v1_manifest("1.0.1", installed("0.14.0", &["debug"])),
        )
        .unwrap();

        assert_eq!(migrate_if_needed(&home).unwrap(), MigrationOutcome::NothingToDo);

        let state = LocalState::load(&paths::state_path(&home)).unwrap();
        assert!(state.get(&semver::Version::new(0, 15, 0)).is_some());
        assert!(state.get(&semver::Version::new(0, 14, 0)).is_none());
        assert!(v1_manifest_path(&home).exists());
    }
}
