//! The journal: one commit point, and recovery from either side of it.
//!
//! Publishing touches many objects -- a staged tree, a symlink, `state.json`, derived symlinks,
//! the previous publication -- and no filesystem operation makes all of that atomic. So it is not
//! made atomic; it is made **recoverable**, by writing down what is about to happen and defining
//! exactly one point after which the operation is considered to have occurred:
//!
//! ```text
//! 1. PREPARE   write journal/<op-id>.json
//! 2. STAGE     build the new publication
//! 3. VERIFY    structural check; write receipt.json
//! 4. COMMIT    repoint toolchains/<channel>          <- THE COMMIT POINT
//! 5. RECORD    commit state.json
//! 6. DERIVE    rebuild the network links and opt
//! 7. CLEAN     release the old publication; delete the journal
//! ```
//!
//! Before step 4 the operation never happened: the staged publication is discarded and prior state
//! stands. After step 4 it did happen: steps 5--7 are completed from the journal. `state.json` is
//! the authority before the commit point; the journal is the authority after it. Nothing infers
//! which side it is on from anything other than the symlink itself.
//!
//! Uninstall replaces step 4 with an atomic replacement of the symlink by a **tombstone**, so that
//! a removed toolchain is distinguishable from one someone deleted by hand.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    paths,
    publish::PublishError,
    state::{Installation, LocalState, PublicationId, PublicationRef},
    utils,
};

/// What `toolchains/<channel>` is replaced by when a channel is uninstalled.
///
/// A dangling symlink rather than a marker file: the commit stays a `rename` of a symlink onto a
/// symlink, which is atomic, and every reader that resolves the link sees "not installed" during
/// the window between the commit and the state record.
const TOMBSTONE: &str = ".uninstalled";

/// The kind of physical operation a journal entry describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OperationKind {
    Install,
    Uninstall,
    ChannelMigrate,
}

impl std::fmt::Display for OperationKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Install => "install",
            Self::Uninstall => "uninstall",
            Self::ChannelMigrate => "channel migration",
        })
    }
}

/// What an in-flight operation intends to do, written down before it starts.
///
/// `target_installation` is the whole record the operation means to commit, not just the intent
/// and plan key: recovery has to be able to complete step 5 without re-resolving anything, and
/// re-resolving would consult an upstream manifest that may have moved on since.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: String,
    pub kind: OperationKind,
    pub channel: semver::Version,
    /// The publication being replaced, to be removed once the new state record is committed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_publication: Option<PublicationId>,
    /// The publication being published. Absent for an uninstall.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_publication: Option<PublicationId>,
    /// The state record to commit. Absent for an uninstall, which removes one instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_installation: Option<Installation>,
}

impl JournalEntry {
    /// An install or republication of `channel`.
    pub fn install(
        channel: semver::Version,
        old_publication: Option<PublicationId>,
        new_publication: PublicationId,
        target_installation: Installation,
    ) -> Self {
        Self {
            id: utils::opaque_id(),
            kind: OperationKind::Install,
            channel,
            old_publication,
            new_publication: Some(new_publication),
            target_installation: Some(target_installation),
        }
    }

    /// Removal of `channel`, whose publication is `publication`.
    pub fn uninstall(channel: semver::Version, publication: Option<PublicationId>) -> Self {
        Self {
            id: utils::opaque_id(),
            kind: OperationKind::Uninstall,
            channel,
            old_publication: publication,
            new_publication: None,
            target_installation: None,
        }
    }
}

/// Where an entry is written.
pub fn entry_path(home: &Path, id: &str) -> PathBuf {
    paths::journal_dir(home).join(id).with_extension("json")
}

/// Step 1: records what is about to happen.
///
/// Refuses to start while another entry is present. Two concurrent physical operations against one
/// `MIDENUP_HOME` cannot both be recovered -- the surviving journal would describe one of them and
/// silently discard the other -- so the condition is reported rather than tolerated. The advisory
/// lock makes this unreachable in practice; it fires only when a previous run was interrupted and
/// recovery has not been given a chance to run.
pub fn prepare(home: &Path, entry: &JournalEntry) -> Result<(), PublishError> {
    let dir = paths::journal_dir(home);
    std::fs::create_dir_all(&dir)
        .map_err(|source| PublishError::Journal { path: dir.clone(), source })?;

    if let Some(existing) = read(home)?
        && existing.id != entry.id
    {
        return Err(PublishError::OperationInProgress {
            operation: existing.kind,
            channel: existing.channel,
        });
    }

    let path = entry_path(home, &entry.id);
    crate::utils::atomic::write_validated(&path, entry, |written| {
        serde_json::from_str::<JournalEntry>(written)
            .map(|_| ())
            .map_err(|err| format!("the result would not parse as a journal entry: {err}"))
    })?;
    Ok(())
}

/// Step 4, the commit point: repoints `toolchains/<channel>`, or tombstones it.
///
/// The swap is a `rename` of a temporary symlink onto the channel link, which is atomic: a reader
/// sees either the old publication or the new one, never a partially built path.
pub fn commit_symlink(home: &Path, entry: &JournalEntry) -> Result<(), PublishError> {
    let link = paths::toolchain_link(home, &entry.channel);
    let target = match (&entry.kind, &entry.new_publication) {
        (OperationKind::Uninstall, _) | (_, None) => PathBuf::from(TOMBSTONE),
        (_, Some(id)) => {
            PathBuf::from("..").join("publications").join(format!("{}-{id}", entry.channel))
        },
    };

    crate::trace!("committing {} to {}", link.display(), target.display());
    utils::fs::replace_symlink(&link, &target).map_err(|err| PublishError::Commit {
        path: link,
        source: std::io::Error::other(err.to_string()),
    })
}

/// Step 5: commits the state record the entry describes.
///
/// The state record is committed before anything is cleaned up. Until it lands, the journal is the
/// only thing that knows the operation happened, so reclaiming or deleting anything earlier could
/// lose it entirely.
pub fn record(
    home: &Path,
    entry: &JournalEntry,
    state: &mut LocalState,
) -> Result<(), PublishError> {
    match entry.kind {
        OperationKind::Install | OperationKind::ChannelMigrate => {
            if let Some(installation) = entry.target_installation.clone() {
                state.upsert(installation);
            }
        },
        OperationKind::Uninstall => state.remove(&entry.channel),
    }

    state
        .save(&paths::state_path(home))
        .map_err(|err| PublishError::Record { reason: err.to_string() })
}

/// Step 7: releases the publication this operation replaced, and deletes the journal.
///
/// **A publication that was merely *replaced* is not deleted here.** Another process may be
/// executing a component out of it right now -- `miden vm ...` in one terminal while the other
/// installs -- and pulling the directory out from under a running program is fatal: macOS kills it
/// with SIGKILL, and on Linux the interpreter of a shell script fails to open it. Since the
/// toolchain link now points at the new publication, nothing can *start* using the old one, so it
/// is simply unreferenced, and unreferenced publications are what `midenup gc` reclaims (spec
/// section 11.6).
///
/// An *uninstall* does delete it, because that is what was asked for.
///
/// Everything here is best effort except deleting the journal. A state record that is already
/// committed must not be undone because a directory could not be removed. The journal is
/// different: while it exists, recovery will run this operation again.
pub fn clean(home: &Path, entry: &JournalEntry) -> Result<(), PublishError> {
    if let Some(old) = &entry.old_publication
        && matches!(entry.kind, OperationKind::Uninstall)
    {
        let publication = paths::publication_dir(home, &entry.channel, old);
        crate::trace!("removing {}", publication.display());
        let _ = std::fs::remove_dir_all(publication);
    }

    if matches!(entry.kind, OperationKind::Uninstall) {
        let link = paths::toolchain_link(home, &entry.channel);
        if is_tombstone(&link) {
            let _ = std::fs::remove_file(&link);
        }
    }

    let path = entry_path(home, &entry.id);
    std::fs::remove_file(&path).map_err(|source| PublishError::Journal { path, source })
}

/// Steps 5 and 7 together, for a caller with no step 6 of its own to run.
///
/// Recovery uses this: rebuilding the derived symlinks (step 6) is idempotent and happens on every
/// command anyway, so completing an interrupted operation does not need to interleave with it.
pub fn finish(
    home: &Path,
    entry: &JournalEntry,
    state: &mut LocalState,
) -> Result<(), PublishError> {
    record(home, entry, state)?;
    clean(home, entry)
}

/// Runs at startup: completes or discards whatever the last run left behind.
///
/// Returns the kind of operation that was recovered, if any. A journal describing an operation
/// that never reached its commit point is discarded together with its staged publication; one that
/// passed it is completed. Which side of the commit point an operation is on is read from the
/// symlink and nothing else.
pub fn recover(home: &Path, state: &mut LocalState) -> Result<Option<OperationKind>, PublishError> {
    let Some(entry) = read(home)? else {
        // With no journal, `state.json` is the authority -- and it must agree with what is on
        // disk. A disagreement here has no recorded cause, so it is reported rather than guessed
        // at: silently reinstalling would discard a user's toolchain on the strength of a stat.
        return check_divergence(home, state).map(|_| None);
    };

    let link = paths::toolchain_link(home, &entry.channel);
    let committed = match entry.kind {
        // The tombstone *is* the commit for an uninstall.
        OperationKind::Uninstall => is_tombstone(&link),
        OperationKind::Install | OperationKind::ChannelMigrate => entry
            .new_publication
            .as_ref()
            .is_some_and(|id| points_at(&link, &format!("{}-{id}", entry.channel))),
    };

    if committed {
        crate::trace!(
            "the interrupted {} of {} was committed; completing it",
            entry.kind,
            entry.channel
        );
        finish(home, &entry, state)?;
        return Ok(Some(entry.kind));
    }

    // Not committed: the operation never happened. Discard the staged publication and leave prior
    // state exactly as it was.
    crate::trace!(
        "the interrupted {} of {} never committed; discarding it",
        entry.kind,
        entry.channel
    );
    if let Some(new) = &entry.new_publication {
        let publication = paths::publication_dir(home, &entry.channel, new);
        crate::trace!("removing {}", publication.display());
        let _ = std::fs::remove_dir_all(publication);
    }
    let path = entry_path(home, &entry.id);
    crate::trace!("removing {}", path.display());
    std::fs::remove_file(&path).map_err(|source| PublishError::Journal { path, source })?;

    Ok(Some(entry.kind))
}

/// Reads the single journal entry, if one is present.
pub fn read(home: &Path) -> Result<Option<JournalEntry>, PublishError> {
    let dir = paths::journal_dir(home);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(PublishError::Journal { path: dir, source }),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let contents = std::fs::read_to_string(&path)
            .map_err(|source| PublishError::Journal { path: path.clone(), source })?;
        let parsed = serde_json::from_str(&contents)
            .map_err(|err| PublishError::InvalidJournal { path, reason: err.to_string() })?;
        return Ok(Some(parsed));
    }

    Ok(None)
}

/// Whether `link` resolves to a publication directory named `name`.
fn points_at(link: &Path, name: &str) -> bool {
    std::fs::read_link(link)
        .ok()
        .and_then(|target| target.file_name().map(|n| n == std::ffi::OsStr::new(name)))
        .unwrap_or(false)
}

fn is_tombstone(link: &Path) -> bool {
    std::fs::read_link(link).is_ok_and(|target| target == Path::new(TOMBSTONE))
}

/// Reports a state record whose publication is not on disk.
fn check_divergence(home: &Path, state: &LocalState) -> Result<(), PublishError> {
    for installation in &state.installations {
        // A record carried over from v1 describes no publication by design; it is handled by
        // reinstalling on next use, not by this check.
        let PublicationRef::Managed { id, .. } = &installation.publication else {
            continue;
        };

        let dir = paths::publication_dir(home, &installation.channel, id);
        if !dir.is_dir() {
            return Err(PublishError::DivergentState {
                channel: installation.channel.clone(),
                detail: format!("its publication '{}' is missing", dir.display()),
                remediation: format!("midenup install {}", installation.channel),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{plan::PlanKey, profile::Profile, resolve::Intent};

    /// A `MIDENUP_HOME` with one installed channel, published and recorded.
    struct Env {
        _temp: tempdir::TempDir,
        home: PathBuf,
    }

    impl Env {
        fn with_installed(channel: &str) -> (Self, PublicationId) {
            let temp = tempdir::TempDir::new("journal").unwrap();
            let home = temp.path().join("midenup");
            let channel = semver::Version::parse(channel).unwrap();

            let id = PublicationId::generate();
            let publication = paths::publication_dir(&home, &channel, &id);
            std::fs::create_dir_all(&publication).unwrap();
            std::fs::create_dir_all(paths::toolchains_dir(&home)).unwrap();
            utils::fs::symlink(
                &paths::toolchain_link(&home, &channel),
                &PathBuf::from("..").join("publications").join(publication.file_name().unwrap()),
            )
            .unwrap();

            let mut state = LocalState::default();
            state.upsert(installation(&channel, &id));
            state.save(&paths::state_path(&home)).unwrap();

            (Env { _temp: temp, home }, id)
        }

        fn state(&self) -> LocalState {
            LocalState::load(&paths::state_path(&self.home)).unwrap()
        }

        fn journal_is_empty(&self) -> bool {
            read(&self.home).unwrap().is_none()
        }
    }

    fn plan_key() -> PlanKey {
        serde_json::from_str(&format!("\"pk1:{}\"", "a".repeat(64))).unwrap()
    }

    fn installation(channel: &semver::Version, id: &PublicationId) -> Installation {
        Installation {
            channel: channel.clone(),
            intent: Intent::new(&[Profile::Minimal], &[]),
            components: vec![],
            publication: PublicationRef::Managed {
                id: id.clone(),
                plan_key: plan_key(),
                target: "aarch64-apple-darwin".to_string(),
            },
            installed_at: 1735689600,
        }
    }

    fn v(version: &str) -> semver::Version {
        semver::Version::parse(version).unwrap()
    }

    /// Stages a publication for `channel`, as steps 2 and 3 would.
    fn stage_publication(home: &Path, channel: &semver::Version, id: &PublicationId) {
        std::fs::create_dir_all(paths::publication_dir(home, channel, id)).unwrap();
    }

    #[test]
    fn before_the_symlink_commit_recovery_discards_and_keeps_the_old_state() {
        let (env, _) = Env::with_installed("0.15.0");
        let new = PublicationId::generate();
        let entry =
            JournalEntry::install(v("0.16.0"), None, new.clone(), installation(&v("0.16.0"), &new));

        prepare(&env.home, &entry).unwrap();
        stage_publication(&env.home, &v("0.16.0"), &new);
        // crash here

        let mut state = env.state();
        recover(&env.home, &mut state).unwrap();

        assert!(state.get(&v("0.16.0")).is_none(), "uncommitted operation must be discarded");
        assert!(state.get(&v("0.15.0")).is_some(), "prior installation must survive");
        assert!(
            !paths::publication_dir(&env.home, &v("0.16.0"), &new).exists(),
            "the staged publication must be discarded"
        );
        assert!(env.journal_is_empty());
    }

    #[test]
    fn after_the_symlink_commit_recovery_rolls_forward() {
        let (env, _) = Env::with_installed("0.15.0");
        let new = PublicationId::generate();
        let entry =
            JournalEntry::install(v("0.16.0"), None, new.clone(), installation(&v("0.16.0"), &new));

        prepare(&env.home, &entry).unwrap();
        stage_publication(&env.home, &v("0.16.0"), &new);
        commit_symlink(&env.home, &entry).unwrap();
        // crash here, before state.json was written

        let mut state = env.state();
        assert!(state.get(&v("0.16.0")).is_none(), "state must not know about it yet");

        recover(&env.home, &mut state).unwrap();

        assert!(state.get(&v("0.16.0")).is_some(), "committed operation must be completed");
        assert_eq!(
            env.state().get(&v("0.16.0")).map(|i| i.channel.clone()),
            Some(v("0.16.0")),
            "and must be persisted, not merely applied in memory"
        );
        assert!(env.journal_is_empty());
    }

    /// A publication that was replaced is left for `midenup gc`, not deleted here: another
    /// process may be executing a component out of it, and removing it under a running program is
    /// fatal.
    #[test]
    fn a_replaced_publication_is_left_for_gc() {
        let (env, old) = Env::with_installed("0.15.0");
        let new = PublicationId::generate();
        let entry = JournalEntry::install(
            v("0.15.0"),
            Some(old.clone()),
            new.clone(),
            installation(&v("0.15.0"), &new),
        );

        prepare(&env.home, &entry).unwrap();
        stage_publication(&env.home, &v("0.15.0"), &new);
        commit_symlink(&env.home, &entry).unwrap();

        let mut state = env.state();
        recover(&env.home, &mut state).unwrap();

        assert!(
            paths::publication_dir(&env.home, &v("0.15.0"), &old).exists(),
            "the replaced publication must survive; nothing may be pulled out from under a \
             running process"
        );
        assert!(paths::publication_dir(&env.home, &v("0.15.0"), &new).exists());
    }

    #[test]
    fn a_tombstoned_symlink_completes_the_uninstall() {
        let (env, old) = Env::with_installed("0.15.0");
        let entry = JournalEntry::uninstall(v("0.15.0"), Some(old.clone()));

        prepare(&env.home, &entry).unwrap();
        commit_symlink(&env.home, &entry).unwrap();

        let mut state = env.state();
        recover(&env.home, &mut state).unwrap();

        assert!(state.get(&v("0.15.0")).is_none());
        assert!(!paths::publication_dir(&env.home, &v("0.15.0"), &old).exists());
        assert!(
            std::fs::symlink_metadata(paths::toolchain_link(&env.home, &v("0.15.0"))).is_err(),
            "the tombstone must be cleaned up once the removal is recorded"
        );
        assert!(env.journal_is_empty());
    }

    /// An uninstall interrupted *before* the tombstone never happened: the channel stays installed.
    #[test]
    fn an_uncommitted_uninstall_leaves_the_channel_installed() {
        let (env, old) = Env::with_installed("0.15.0");
        let entry = JournalEntry::uninstall(v("0.15.0"), Some(old.clone()));

        prepare(&env.home, &entry).unwrap();

        let mut state = env.state();
        recover(&env.home, &mut state).unwrap();

        assert!(state.get(&v("0.15.0")).is_some());
        assert!(paths::publication_dir(&env.home, &v("0.15.0"), &old).exists());
    }

    #[test]
    fn an_absent_journal_recovers_nothing() {
        let (env, _) = Env::with_installed("0.15.0");
        let mut state = env.state();
        assert_eq!(recover(&env.home, &mut state).unwrap(), None);
    }

    /// State and filesystem disagreeing with no journal to explain it is reported, never guessed
    /// at -- and the report names the command that fixes it.
    #[test]
    fn divergence_without_a_journal_is_reported_not_guessed() {
        let (env, old) = Env::with_installed("0.15.0");
        std::fs::remove_dir_all(paths::publication_dir(&env.home, &v("0.15.0"), &old)).unwrap();

        let mut state = env.state();
        let err = recover(&env.home, &mut state).expect_err("must report");
        assert!(matches!(err, PublishError::DivergentState { .. }), "{err}");
        assert!(
            err.to_string().contains("midenup install 0.15.0"),
            "the diagnostic must name the exact recovery command: {err}"
        );
    }

    /// Two physical operations cannot share one journal: recovering would complete one and lose
    /// the other.
    #[test]
    fn a_second_operation_cannot_start_while_one_is_journalled() {
        let (env, _) = Env::with_installed("0.15.0");
        let first = JournalEntry::uninstall(v("0.15.0"), None);
        prepare(&env.home, &first).unwrap();

        let second = JournalEntry::uninstall(v("0.16.0"), None);
        assert!(matches!(
            prepare(&env.home, &second),
            Err(PublishError::OperationInProgress { .. })
        ));

        // Re-preparing the *same* entry is not a conflict: it is what a retry looks like.
        prepare(&env.home, &first).expect("re-preparing the same operation must be allowed");
    }
}
