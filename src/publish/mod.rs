//! Publications: an installed tree, and the receipt that says what it owns.
//!
//! A publication is written once, verified, published by repointing one symlink, and thereafter
//! never modified. Any change to the installed set produces a *new* publication.
//!
//! Two consequences shape this module:
//!
//! * **The directory name carries no meaning.** It is `<channel>-<publication-id>`, where the id is
//!   opaque and randomly generated. Naming a publication after a digest of its inputs invites
//!   treating equal names as equal bytes -- and nothing here verifies bytes ([crate::artifact]
//!   digests are recorded, never checked). An opaque id makes that mistake impossible to express.
//! * **The receipt, not the manifest, says what a publication owns.** Uninstall and update-seeding
//!   both read it. Deriving ownership from the manifest a second time would give those paths their
//!   own answer to a question the install already answered, and two answers can disagree.

pub mod journal;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub use self::journal::{JournalEntry, OperationKind};
pub use crate::paths::publication_dir;
use crate::{
    plan::{InstallationPlan, PlanStep},
    state::{Output, PublicationId, RealizedMethod, Receipt},
};

/// The receipt's filename inside a publication.
pub const RECEIPT_FILE: &str = "receipt.json";

#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error("failed to read the publication receipt '{path}': {source}")]
    ReadReceipt {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("'{path}' is not a valid publication receipt: {reason}")]
    InvalidReceipt { path: PathBuf, reason: String },
    #[error("failed to write the publication receipt: {0}")]
    WriteReceipt(#[from] crate::utils::atomic::WriteError),
    #[error("failed to access the operation journal at '{path}': {source}")]
    Journal {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("'{path}' is not a valid journal entry: {reason}")]
    InvalidJournal { path: PathBuf, reason: String },
    #[error(
        "an interrupted {operation} of channel {channel} is still recorded; run any midenup \
         command to let it finish recovering before starting another operation"
    )]
    OperationInProgress {
        operation: OperationKind,
        channel: semver::Version,
    },
    #[error("failed to publish the toolchain link '{path}': {source}")]
    Commit {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to record the operation in local state: {reason}")]
    Record { reason: String },
    #[error(
        "channel {channel} is recorded as installed, but {detail}; midenup will not guess what \
         happened. To reinstall it, run: {remediation}"
    )]
    DivergentState {
        channel: semver::Version,
        detail: String,
        remediation: String,
    },
}

/// Where a publication records what it owns.
pub fn receipt_path(publication: &Path) -> PathBuf {
    publication.join(RECEIPT_FILE)
}

/// Writes `receipt` into `publication`, refusing to commit anything that cannot be read back.
pub fn write_receipt(publication: &Path, receipt: &Receipt) -> Result<(), PublishError> {
    let path = receipt_path(publication);
    crate::utils::atomic::write_validated(&path, receipt, |written| {
        serde_json::from_str::<Receipt>(written)
            .map(|_| ())
            .map_err(|err| format!("the result would not parse as a receipt: {err}"))
    })?;
    Ok(())
}

/// Reads the receipt describing `publication`.
pub fn read_receipt(publication: &Path) -> Result<Receipt, PublishError> {
    let path = receipt_path(publication);
    let contents = std::fs::read_to_string(&path)
        .map_err(|source| PublishError::ReadReceipt { path: path.clone(), source })?;
    serde_json::from_str(&contents)
        .map_err(|err| PublishError::InvalidReceipt { path, reason: err.to_string() })
}

/// Describes what a staged tree contains, given the plan that produced it.
///
/// `realized` records how each destination was *actually* obtained on this run, which can differ
/// from what the plan declared: a `prebuilt-with-cargo-fallback` component whose download fails is
/// built from source instead. Uninstall has to match the path that was really taken, so the plan
/// alone is not a sufficient record.
///
/// Destinations that were neither run nor seeded fall back to the method the plan implies.
pub fn receipt_for(
    plan: &InstallationPlan,
    publication: &Path,
    id: &PublicationId,
    realized: &BTreeMap<PathBuf, RealizedMethod>,
    seeded_from: Option<&Receipt>,
) -> Receipt {
    let outputs = plan
        .steps
        .iter()
        .map(|step| {
            let path = relative_output(step.dest(), publication);
            let realized = realized
                .get(step.dest())
                .copied()
                .or_else(|| {
                    // Seeded from the previous publication: it was not acquired on this run, so
                    // the method that produced it is whatever produced it last time.
                    seeded_from
                        .and_then(|receipt| receipt.outputs.iter().find(|o| o.path == path))
                        .map(|output| output.realized)
                })
                .unwrap_or_else(|| declared_method(step));
            Output {
                path,
                owner: step.owner().to_string(),
                mode: step.mode(),
                realized,
                digest: match step {
                    PlanStep::Download { digest, .. } => digest.clone(),
                    _ => None,
                },
            }
        })
        .collect();

    Receipt {
        publication_id: id.clone(),
        plan_key: plan.key.clone(),
        target: plan.target.clone(),
        channel: plan.channel.clone(),
        outputs,
    }
}

/// Publication directories no `state.json` record refers to and no journal names.
///
/// Because a replaced publication is left on disk rather than deleted -- another process may be
/// executing out of it (§3.1) -- this is what accumulates, and reclaiming it is `midenup gc`'s
/// whole job.
///
/// Two things are deliberately *not* treated as garbage: a publication an in-flight operation
/// names, which is either about to be published or about to be replaced; and anything that is not a
/// directory, since nothing here created it.
pub fn unreferenced(
    home: &Path,
    state: &crate::state::LocalState,
) -> Result<Vec<PathBuf>, PublishError> {
    let mut referenced: std::collections::HashSet<PathBuf> = state
        .installations
        .iter()
        .filter_map(|installation| match &installation.publication {
            crate::state::PublicationRef::Managed { id, .. } => {
                Some(publication_dir(home, &installation.channel, id))
            },
            crate::state::PublicationRef::NeedsReinstall => None,
        })
        .collect();

    if let Some(entry) = journal::read(home)? {
        for id in [&entry.old_publication, &entry.new_publication].into_iter().flatten() {
            referenced.insert(publication_dir(home, &entry.channel, id));
        }
    }

    let dir = crate::paths::publications_dir(home);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(PublishError::Journal { path: dir, source }),
    };

    let mut orphans: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && !referenced.contains(path))
        .collect();
    orphans.sort();

    Ok(orphans)
}

/// The method a step implies when nothing observed it running.
pub fn declared_method(step: &PlanStep) -> RealizedMethod {
    match step {
        PlanStep::Download { .. } | PlanStep::CopyLocal { .. } => RealizedMethod::Prebuilt,
        PlanStep::CargoBuild { .. } => RealizedMethod::Cargo,
        PlanStep::ExtractPackage { .. } => RealizedMethod::Extracted,
    }
}

/// A destination as the receipt records it: relative to the publication root.
///
/// Absolute paths would tie a receipt to the `MIDENUP_HOME` it was written under, so moving the
/// directory -- or comparing two publications' receipts, which seeding does -- would fail for
/// reasons that have nothing to do with what is installed.
fn relative_output(dest: &Path, publication: &Path) -> PathBuf {
    dest.strip_prefix(publication).unwrap_or(dest).to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::PlanKey;

    fn plan_key() -> PlanKey {
        serde_json::from_str::<PlanKey>(&format!("\"pk1:{}\"", "a".repeat(64))).unwrap()
    }

    fn sample_receipt(id: PublicationId) -> Receipt {
        Receipt {
            publication_id: id,
            plan_key: plan_key(),
            target: "aarch64-apple-darwin".to_string(),
            channel: semver::Version::new(0, 15, 0),
            outputs: vec![Output {
                path: PathBuf::from("bin").join("miden-vm"),
                owner: "vm".to_string(),
                mode: 0o755,
                realized: RealizedMethod::Prebuilt,
                digest: None,
            }],
        }
    }

    /// The publication id must be independent of the plan key: equal keys never authorize reusing
    /// another publication's content, and a name derived from the key would invite exactly that.
    #[test]
    fn publication_ids_are_unique_and_not_derived_from_the_plan_key() {
        let a = PublicationId::generate();
        let b = PublicationId::generate();
        assert_ne!(a, b);

        let key = plan_key().to_string();
        assert!(
            !a.to_string().contains(&key[4..12]),
            "the id must not embed the key: {a} vs {key}"
        );
    }

    #[test]
    fn a_publication_is_named_by_its_id_not_its_contents() {
        let home = Path::new("/home");
        let id = PublicationId::generate();
        let dir = publication_dir(home, &semver::Version::new(0, 15, 0), &id);
        assert_eq!(dir, home.join("publications").join(format!("0.15.0-{id}")));
    }

    #[test]
    fn a_receipt_round_trips_through_its_publication() {
        let temp = tempdir::TempDir::new("receipt-roundtrip").unwrap();
        let receipt = sample_receipt(PublicationId::generate());
        write_receipt(temp.path(), &receipt).expect("should write");
        assert_eq!(read_receipt(temp.path()).expect("should read"), receipt);
    }

    fn plan_with(steps: Vec<PlanStep>) -> InstallationPlan {
        InstallationPlan {
            target: "aarch64-apple-darwin".to_string(),
            channel: semver::Version::new(0, 15, 0),
            steps,
            symlinks: vec![],
            key: plan_key(),
        }
    }

    fn download(dest: &Path) -> PlanStep {
        PlanStep::Download {
            uri: "https://example.invalid/miden-vm".to_string(),
            dest: dest.to_path_buf(),
            mode: 0o755,
            owner: "vm".to_string(),
            digest: None,
            archive: None,
            fallback: None,
        }
    }

    /// The receipt records how a file was *really* obtained. A `prebuilt-with-cargo-fallback`
    /// component can go either way, and uninstall has to match the path actually taken rather than
    /// the one the manifest declared.
    #[test]
    fn the_receipt_records_the_realized_method_for_a_fallback_component() {
        let publication = Path::new("/home/publications/0.15.0-abc");
        let dest = publication.join("bin").join("miden-vm");
        let plan = plan_with(vec![download(&dest)]);

        // The transfer failed and the declared Cargo fallback produced the binary instead.
        let realized = BTreeMap::from([(dest.clone(), RealizedMethod::Cargo)]);

        let receipt = receipt_for(&plan, publication, &PublicationId::generate(), &realized, None);

        assert_eq!(receipt.outputs.len(), 1);
        assert!(matches!(receipt.outputs[0].realized, RealizedMethod::Cargo));
        assert_eq!(
            receipt.outputs[0].path,
            PathBuf::from("bin").join("miden-vm"),
            "outputs are relative to the publication, so a receipt is not tied to one MIDENUP_HOME"
        );
    }

    /// A seeded file was not acquired on this run, so what produced it is what produced it last
    /// time. Re-deriving it from the plan would relabel a Cargo-built binary as prebuilt the first
    /// time an unrelated component changed.
    #[test]
    fn a_seeded_output_inherits_the_previous_receipts_method() {
        let publication = Path::new("/home/publications/0.15.0-def");
        let dest = publication.join("bin").join("miden-vm");
        let plan = plan_with(vec![download(&dest)]);

        let mut previous = sample_receipt(PublicationId::generate());
        previous.outputs[0].realized = RealizedMethod::Cargo;

        let receipt = receipt_for(
            &plan,
            publication,
            &PublicationId::generate(),
            &BTreeMap::new(),
            Some(&previous),
        );

        assert!(matches!(receipt.outputs[0].realized, RealizedMethod::Cargo));
    }

    /// With nothing observed and nothing inherited, the plan is the only evidence available.
    #[test]
    fn an_unobserved_output_falls_back_to_the_declared_method() {
        let publication = Path::new("/home/publications/0.15.0-ghi");
        let dest = publication.join("bin").join("miden-vm");
        let plan = plan_with(vec![download(&dest)]);

        let receipt =
            receipt_for(&plan, publication, &PublicationId::generate(), &BTreeMap::new(), None);
        assert!(matches!(receipt.outputs[0].realized, RealizedMethod::Prebuilt));
    }

    /// A publication a state record refers to is never garbage, whatever else is on disk.
    #[test]
    fn only_publications_nothing_refers_to_are_collectable() {
        use crate::state::{Installation, LocalState, PublicationRef};

        let temp = tempdir::TempDir::new("publish-gc").unwrap();
        let home = temp.path();
        let channel = semver::Version::new(0, 15, 0);

        let live = PublicationId::generate();
        let orphan = PublicationId::generate();
        for id in [&live, &orphan] {
            std::fs::create_dir_all(publication_dir(home, &channel, id)).unwrap();
        }
        // Not a directory, and not ours: left alone.
        std::fs::write(crate::paths::publications_dir(home).join("stray-file"), b"x").unwrap();

        let mut state = LocalState::default();
        state.upsert(Installation {
            channel: channel.clone(),
            intent: Default::default(),
            components: vec![],
            publication: PublicationRef::Managed {
                id: live.clone(),
                plan_key: plan_key(),
                target: "aarch64-apple-darwin".to_string(),
            },
            installed_at: 1735689600,
        });

        assert_eq!(
            unreferenced(home, &state).unwrap(),
            vec![publication_dir(home, &channel, &orphan)]
        );
    }

    /// An operation in flight owns both publications it names, even though neither is in
    /// `state.json` yet.
    #[test]
    fn a_publication_an_in_flight_operation_names_is_not_collectable() {
        use crate::state::{Installation, LocalState};

        let temp = tempdir::TempDir::new("publish-gc-journal").unwrap();
        let home = temp.path();
        let channel = semver::Version::new(0, 15, 0);

        let staged = PublicationId::generate();
        std::fs::create_dir_all(publication_dir(home, &channel, &staged)).unwrap();

        let state = LocalState::default();
        assert_eq!(
            unreferenced(home, &state).unwrap(),
            vec![publication_dir(home, &channel, &staged)],
            "with no journal it is simply garbage"
        );

        let entry = journal::JournalEntry::install(
            channel.clone(),
            None,
            staged.clone(),
            Installation {
                channel: channel.clone(),
                intent: Default::default(),
                components: vec![],
                publication: crate::state::PublicationRef::Managed {
                    id: staged.clone(),
                    plan_key: plan_key(),
                    target: "aarch64-apple-darwin".to_string(),
                },
                installed_at: 1735689600,
            },
        );
        journal::prepare(home, &entry).unwrap();

        assert!(
            unreferenced(home, &state).unwrap().is_empty(),
            "an in-flight publication must never be collected"
        );
    }

    #[test]
    fn a_missing_receipt_is_an_error_naming_the_path() {
        let temp = tempdir::TempDir::new("receipt-missing").unwrap();
        let err = read_receipt(temp.path()).expect_err("must fail");
        assert!(err.to_string().contains(RECEIPT_FILE), "the error must name the receipt: {err}");
    }
}
