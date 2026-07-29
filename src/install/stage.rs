//! Building a staged installation tree and checking it before it is published.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use colored::Colorize;

use crate::{
    install::{CargoError, ExecError, ExtractError},
    plan::{InstallationPlan, PlanStep},
    state::{RealizedMethod, Receipt},
    utils,
};

#[derive(Debug, thiserror::Error)]
pub enum StageError {
    #[error(transparent)]
    Acquire(#[from] ExecError),
    #[error(transparent)]
    Cargo(#[from] CargoError),
    #[error(transparent)]
    Extract(#[from] ExtractError),
    #[error("failed to create '{path}': {source}")]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("component '{owner}' was installed, but '{path}' is missing")]
    Missing { owner: String, path: PathBuf },
    #[error("component '{owner}' installed '{path}', but it is not a regular file")]
    NotAFile { owner: String, path: PathBuf },
    #[error("component '{owner}' installed '{path}' with mode {found:o}, expected {expected:o}")]
    WrongMode {
        owner: String,
        path: PathBuf,
        expected: u32,
        found: u32,
    },
    #[error("the shim '{path}' was not created")]
    MissingSymlink { path: PathBuf },
    #[error("failed to carry '{path}' forward from the previous installation: {source}")]
    Seed {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("component '{owner}' declares a fallback that cannot be executed as one")]
    UnsupportedFallback { owner: String },
}

/// How each destination in a staged tree was actually produced.
pub type Realized = BTreeMap<PathBuf, RealizedMethod>;

/// The publication a new one is being built from.
///
/// Seeding is what makes an update cheap -- unchanged files are copied rather than re-fetched --
/// but it is also how an update can silently carry stale content forward, so it is scoped
/// deliberately: only files the previous receipt *owns*, only those the new plan still wants, and
/// never those of a component known to have changed.
pub struct Seed<'a> {
    /// The previous publication's directory.
    pub publication: &'a Path,
    /// What that publication owns. Nothing outside it is carried forward, whatever is on disk.
    pub receipt: &'a Receipt,
    /// Components whose files must be re-acquired rather than reused.
    pub stale: &'a [String],
}

/// The subdirectories every staged tree has.
///
/// Created up front so a step never has to reason about whether its parent exists.
const LAYOUT: &[&str] = &["bin", "lib", "etc", "opt"];

/// Prepares an empty staged tree at `into`.
pub fn prepare(into: &Path) -> Result<(), StageError> {
    for dir in std::iter::once(into.to_path_buf()).chain(LAYOUT.iter().map(|name| into.join(name)))
    {
        std::fs::create_dir_all(&dir).map_err(|source| StageError::Create { path: dir, source })?;
    }
    Ok(())
}

/// Copies forward everything `from` owns that the new plan still wants.
///
/// Deliberately *not* a directory copy. The previous implementation copied the old toolchain tree
/// wholesale, which carried forward anything that had ever landed there -- files from components
/// since removed upstream, leftovers from an interrupted install, Cargo's own bookkeeping -- and
/// left no way to tell inherited content from installed content. The receipt is the ownership
/// record, so it is the only thing consulted here.
pub fn seed(plan: &InstallationPlan, into: &Path, from: &Seed<'_>) -> Result<(), StageError> {
    let wanted: HashSet<&Path> = plan.steps.iter().map(|step| step.dest()).collect();

    for output in &from.receipt.outputs {
        if from.stale.iter().any(|name| name == &output.owner) {
            continue;
        }

        let dest = into.join(&output.path);
        if !wanted.contains(dest.as_path()) {
            // Not part of the new installation. Omitting it here is how components removed
            // upstream stop being installed.
            continue;
        }

        let src = from.publication.join(&output.path);
        // A missing source is not a failure: the step below simply acquires it instead. Refusing
        // to proceed would turn a partially cleaned-up publication into an unrecoverable one.
        if !src.is_file() {
            continue;
        }

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| StageError::Seed { path: dest.clone(), source })?;
        }
        std::fs::copy(&src, &dest)
            .map_err(|source| StageError::Seed { path: dest.clone(), source })?;
    }

    // Note what is *not* here: `var/`. It lives at `$MIDENUP_HOME/var/<channel>`, outside every
    // publication, so there is nothing to carry forward and no way for staging to disturb it.
    Ok(())
}

/// Runs every step of `plan` into `staging_root`, then creates the `opt/` shims.
///
/// A step whose destination already exists is skipped: it was seeded from the previous
/// publication, and [seed] only carries forward what is still current.
///
/// The check is against the step's *exact* destination. Testing a containing directory instead is
/// what previously caused every package download to be skipped -- `lib/` exists as soon as the
/// tree is prepared, so the check was always true.
///
/// Returns how each destination was actually produced, which is not always what the plan declared:
/// see [PlanStep::fallback].
pub fn execute(
    plan: &InstallationPlan,
    staging_root: &Path,
    verbose: bool,
    debug: bool,
) -> Result<Realized, StageError> {
    let mut pending_extractions = Vec::new();
    let mut realized = Realized::new();

    for step in &plan.steps {
        if step.dest().exists() {
            continue;
        }

        // Collected so that every package sharing a crate is extracted by one script.
        if matches!(step, PlanStep::ExtractPackage { .. }) {
            pending_extractions.push(step.clone());
            continue;
        }

        let method = match run(step, staging_root, verbose, debug) {
            Ok(method) => method,
            Err(err) => {
                // A declared Cargo fallback is what makes an unavailable artifact recoverable
                // rather than fatal. Only one retry: if the build fails too, the original
                // acquisition failure is not the interesting one.
                let Some(fallback) = step.fallback() else {
                    return Err(err);
                };
                println!(
                    "{}: could not acquire {}: {err}",
                    "warning".yellow().bold(),
                    step.owner().white().bold(),
                );
                println!("building {} from source instead", step.owner().white().bold());
                run(fallback, staging_root, verbose, debug)?
            },
        };

        realized.insert(step.dest().to_path_buf(), method);
    }

    if !pending_extractions.is_empty() {
        let script = staging_root.join("extract").with_extension("rs");
        crate::install::extract(&pending_extractions, &script)?;
        // The script is a build artefact, not part of the toolchain.
        let _ = std::fs::remove_file(&script);
        for step in &pending_extractions {
            realized.insert(step.dest().to_path_buf(), RealizedMethod::Extracted);
        }
    }

    create_symlinks(plan, staging_root)?;

    Ok(realized)
}

/// Performs one step, reporting how its output was produced.
fn run(
    step: &PlanStep,
    staging_root: &Path,
    verbose: bool,
    debug: bool,
) -> Result<RealizedMethod, StageError> {
    match step {
        PlanStep::Download { .. } | PlanStep::CopyLocal { .. } => {
            crate::install::acquire(step)?;
            Ok(RealizedMethod::Prebuilt)
        },
        PlanStep::CargoBuild { .. } => {
            crate::install::cargo_build(step, staging_root, verbose, debug)?;
            Ok(RealizedMethod::Cargo)
        },
        // Extractions are batched into a single generated script, so one cannot be run alone --
        // which also means one can never be a fallback.
        PlanStep::ExtractPackage { .. } => {
            Err(StageError::UnsupportedFallback { owner: step.owner().to_string() })
        },
    }
}

/// Creates the `opt/` shims described by the plan.
fn create_symlinks(plan: &InstallationPlan, staging_root: &Path) -> Result<(), StageError> {
    let opt = staging_root.join("opt");
    for symlink in &plan.symlinks {
        let link = opt.join(&symlink.name);
        if std::fs::symlink_metadata(&link).is_ok() {
            continue;
        }
        // Relative, so the shim keeps working when the publication directory is renamed or
        // reached through a different path.
        let target = Path::new("..").join("bin").join(&symlink.target_binary);
        utils::fs::symlink(&link, &target).map_err(|err| StageError::Create {
            path: link,
            source: std::io::Error::other(err.to_string()),
        })?;
    }
    Ok(())
}

/// Checks the staged tree structurally, before anything is published.
///
/// Structural only: that each planned file exists, is a regular file, and carries the planned
/// mode. Contents are deliberately not verified -- digests are recorded but never checked -- so
/// this makes no claim about *what* was installed, only that the plan was carried out.
pub fn verify(plan: &InstallationPlan, staging_root: &Path) -> Result<(), StageError> {
    for step in &plan.steps {
        let path = step.dest();
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(_) => {
                return Err(StageError::Missing {
                    owner: step.owner().to_string(),
                    path: path.to_path_buf(),
                });
            },
        };

        if !metadata.is_file() {
            return Err(StageError::NotAFile {
                owner: step.owner().to_string(),
                path: path.to_path_buf(),
            });
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let found = metadata.permissions().mode() & 0o777;
            let expected = step.mode();
            if found != expected {
                return Err(StageError::WrongMode {
                    owner: step.owner().to_string(),
                    path: path.to_path_buf(),
                    expected,
                    found,
                });
            }
        }
    }

    let opt = staging_root.join("opt");
    for symlink in &plan.symlinks {
        let link = opt.join(&symlink.name);
        if std::fs::symlink_metadata(&link).is_err() {
            return Err(StageError::MissingSymlink { path: link });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;
    use crate::{
        artifact::{Artifact, Artifacts},
        manifest::{Channel, Component, ComponentKind, ExecutableComponent, InstallationMethod},
        plan::{MODE_DATA, MODE_EXECUTABLE, build_plan},
        profile::Profile,
        resolve::Intent,
        state::Output,
        version::Authority,
    };

    const TARGET: &str = "aarch64-apple-darwin";

    /// A channel with one prebuilt executable and one package, both sourced from local files.
    fn fixture(root: &Path) -> (Channel, PathBuf) {
        let sources = root.join("sources");
        std::fs::create_dir_all(&sources).unwrap();
        std::fs::write(sources.join("miden-vm"), b"binary").unwrap();
        std::fs::write(sources.join("core.masp"), b"package").unwrap();

        let uri = |name: &str| format!("file://{}", sources.join(name).display());
        let artifact = |name: &str| Artifact::TargetAgnostic { uri: uri(name), digest: None };

        let mut vm_artifacts = Artifacts::default();
        vm_artifacts.insert("miden-vm".to_string(), artifact("miden-vm"));
        let mut core_artifacts = Artifacts::default();
        core_artifacts.insert("core.masp".to_string(), artifact("core.masp"));

        let component = |name: &'static str, kind, artifacts| Component {
            name: Cow::Borrowed(name),
            version: Authority::Registry { version: semver::Version::new(0, 1, 0) },
            kind,
            profiles: vec![Profile::Minimal],
            requires: vec![],
            artifacts,
            extra: Default::default(),
        };

        let channel = Channel::new(
            semver::Version::new(0, 15, 0),
            None,
            vec![
                component(
                    "vm",
                    ComponentKind::Executable {
                        installation_method: InstallationMethod::Prebuilt,
                        spec: ExecutableComponent {
                            installed_executable: "miden-vm".to_string(),
                            ..Default::default()
                        },
                    },
                    vm_artifacts,
                ),
                component("core", ComponentKind::Package, core_artifacts),
            ],
        );

        (channel, sources)
    }

    fn staged(root: &Path) -> (InstallationPlan, PathBuf) {
        let (channel, _) = fixture(root);
        let staging = root.join("staging");
        let plan = build_plan(&channel, &Intent::new(&[Profile::Complete], &[]), TARGET, &staging)
            .expect("should plan");
        (plan, staging)
    }

    #[test]
    fn a_staged_tree_contains_every_planned_file() {
        let temp = tempdir::TempDir::new("stage-ok").unwrap();
        let (plan, staging) = staged(temp.path());

        prepare(&staging).unwrap();
        execute(&plan, &staging, false, true).expect("should stage");
        verify(&plan, &staging).expect("should verify");

        assert_eq!(std::fs::read(staging.join("bin").join("miden-vm")).unwrap(), b"binary");
        assert_eq!(std::fs::read(staging.join("lib").join("core.masp")).unwrap(), b"package");
    }

    #[test]
    fn the_planned_modes_are_applied() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let temp = tempdir::TempDir::new("stage-modes").unwrap();
            let (plan, staging) = staged(temp.path());
            prepare(&staging).unwrap();
            execute(&plan, &staging, false, true).unwrap();

            let mode =
                |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(staging.join("bin").join("miden-vm")), MODE_EXECUTABLE);
            assert_eq!(
                mode(staging.join("lib").join("core.masp")),
                MODE_DATA,
                "packages are data, not programs"
            );
        }
    }

    #[test]
    fn shims_are_created_and_point_into_bin() {
        let temp = tempdir::TempDir::new("stage-shims").unwrap();
        let (plan, staging) = staged(temp.path());
        prepare(&staging).unwrap();
        execute(&plan, &staging, false, true).unwrap();

        let shim = staging.join("opt").join("miden vm");
        let target = std::fs::read_link(&shim).expect("the shim must be a symlink");
        assert_eq!(
            target,
            Path::new("..").join("bin").join("miden-vm"),
            "relative, so it survives the publication directory being renamed"
        );
    }

    /// Verification must notice a file the plan promised but staging did not produce.
    #[test]
    fn a_missing_file_fails_verification() {
        let temp = tempdir::TempDir::new("stage-missing").unwrap();
        let (plan, staging) = staged(temp.path());
        prepare(&staging).unwrap();
        execute(&plan, &staging, false, true).unwrap();

        std::fs::remove_file(staging.join("lib").join("core.masp")).unwrap();
        let err = verify(&plan, &staging).expect_err("must fail");
        assert!(matches!(err, StageError::Missing { .. }), "{err}");
    }

    #[test]
    fn a_wrong_mode_fails_verification() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let temp = tempdir::TempDir::new("stage-mode").unwrap();
            let (plan, staging) = staged(temp.path());
            prepare(&staging).unwrap();
            execute(&plan, &staging, false, true).unwrap();

            std::fs::set_permissions(
                staging.join("bin").join("miden-vm"),
                std::fs::Permissions::from_mode(0o644),
            )
            .unwrap();
            let err = verify(&plan, &staging).expect_err("must fail");
            assert!(matches!(err, StageError::WrongMode { .. }), "{err}");
        }
    }

    #[test]
    fn a_missing_shim_fails_verification() {
        let temp = tempdir::TempDir::new("stage-shim").unwrap();
        let (plan, staging) = staged(temp.path());
        prepare(&staging).unwrap();
        execute(&plan, &staging, false, true).unwrap();

        std::fs::remove_file(staging.join("opt").join("miden vm")).unwrap();
        let err = verify(&plan, &staging).expect_err("must fail");
        assert!(matches!(err, StageError::MissingSymlink { .. }), "{err}");
    }

    /// A directory where a file was planned must not pass. Regression: the existence check tested
    /// a containing directory, which exists as soon as the tree is prepared.
    #[test]
    fn a_directory_where_a_file_was_planned_fails_verification() {
        let temp = tempdir::TempDir::new("stage-dir").unwrap();
        let (plan, staging) = staged(temp.path());
        prepare(&staging).unwrap();
        execute(&plan, &staging, false, true).unwrap();

        let path = staging.join("lib").join("core.masp");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();

        let err = verify(&plan, &staging).expect_err("must fail");
        assert!(matches!(err, StageError::NotAFile { .. }), "{err}");
    }

    /// Re-staging an already complete tree must be a no-op, which is what makes updates cheap.
    #[test]
    fn staging_is_idempotent() {
        let temp = tempdir::TempDir::new("stage-idempotent").unwrap();
        let (plan, staging) = staged(temp.path());
        prepare(&staging).unwrap();
        execute(&plan, &staging, false, true).unwrap();

        // Mark the file so a re-download would be detectable.
        let marked = staging.join("bin").join("miden-vm");
        std::fs::write(&marked, b"marked").unwrap();

        execute(&plan, &staging, false, true).expect("re-staging must succeed");
        assert_eq!(
            std::fs::read(&marked).unwrap(),
            b"marked",
            "an existing file must not be re-fetched"
        );
    }

    /// A previous publication with `bin/miden-vm`, `lib/core.masp` and a file it does not own.
    fn previous_publication(root: &Path) -> (PathBuf, Receipt) {
        let dir = root.join("previous");
        prepare(&dir).unwrap();
        std::fs::write(dir.join("bin").join("miden-vm"), b"old-binary").unwrap();
        std::fs::write(dir.join("lib").join("core.masp"), b"old-package").unwrap();
        std::fs::write(dir.join("lib").join("stray.masp"), b"nobody-owns-me").unwrap();

        let output = |path: PathBuf, owner: &str, mode| Output {
            path,
            owner: owner.to_string(),
            mode,
            realized: RealizedMethod::Prebuilt,
            digest: None,
        };

        let receipt = Receipt {
            publication_id: crate::state::PublicationId::generate(),
            plan_key: serde_json::from_str(&format!("\"pk1:{}\"", "a".repeat(64))).unwrap(),
            target: TARGET.to_string(),
            channel: semver::Version::new(0, 15, 0),
            outputs: vec![
                output(PathBuf::from("bin").join("miden-vm"), "vm", MODE_EXECUTABLE),
                output(PathBuf::from("lib").join("core.masp"), "core", MODE_DATA),
            ],
        };

        (dir, receipt)
    }

    /// Seeding copies what the previous receipt owns and nothing else. Regression: the old
    /// implementation copied the whole directory, so anything that had ever landed there --
    /// leftovers from an interrupted install, files of components since removed -- was inherited
    /// forever, with no way to tell it apart from installed content.
    #[test]
    fn seeding_copies_only_paths_the_old_receipt_owns() {
        let temp = tempdir::TempDir::new("stage-seed-owned").unwrap();
        let (previous, receipt) = previous_publication(temp.path());
        let (plan, staging) = staged(temp.path());

        prepare(&staging).unwrap();
        seed(
            &plan,
            &staging,
            &Seed {
                publication: &previous,
                receipt: &receipt,
                stale: &[],
            },
        )
        .expect("should seed");

        assert_eq!(
            std::fs::read(staging.join("bin").join("miden-vm")).unwrap(),
            b"old-binary",
            "an owned, still-planned file must be carried forward"
        );
        assert!(
            !staging.join("lib").join("stray.masp").exists(),
            "an unowned file must not be carried forward"
        );
    }

    /// A file the previous publication owned but the new plan does not want is simply not seeded,
    /// which is how a component removed upstream stops being installed.
    #[test]
    fn files_not_in_the_new_plan_are_not_seeded() {
        let temp = tempdir::TempDir::new("stage-seed-dropped").unwrap();
        let (previous, mut receipt) = previous_publication(temp.path());
        std::fs::write(previous.join("bin").join("miden-debug"), b"old-debug").unwrap();
        receipt.outputs.push(Output {
            path: PathBuf::from("bin").join("miden-debug"),
            owner: "debug".to_string(),
            mode: MODE_EXECUTABLE,
            realized: RealizedMethod::Prebuilt,
            digest: None,
        });

        let (plan, staging) = staged(temp.path());
        prepare(&staging).unwrap();
        seed(
            &plan,
            &staging,
            &Seed {
                publication: &previous,
                receipt: &receipt,
                stale: &[],
            },
        )
        .unwrap();

        assert!(!staging.join("bin").join("miden-debug").exists());
    }

    /// A component known to have changed must be re-acquired, not reused -- otherwise an update
    /// that keeps a destination's name would silently keep its old contents.
    #[test]
    fn a_stale_component_is_re_acquired_rather_than_seeded() {
        let temp = tempdir::TempDir::new("stage-seed-stale").unwrap();
        let (previous, receipt) = previous_publication(temp.path());
        let (plan, staging) = staged(temp.path());

        prepare(&staging).unwrap();
        seed(
            &plan,
            &staging,
            &Seed {
                publication: &previous,
                receipt: &receipt,
                stale: &["vm".to_string()],
            },
        )
        .unwrap();
        execute(&plan, &staging, false, true).unwrap();

        assert_eq!(
            std::fs::read(staging.join("bin").join("miden-vm")).unwrap(),
            b"binary",
            "a stale component must be re-acquired"
        );
        assert_eq!(
            std::fs::read(staging.join("lib").join("core.masp")).unwrap(),
            b"old-package",
            "an unchanged component must still be reused"
        );
    }

    /// Staging must never create `var/` inside a publication. It lives outside, keyed by channel,
    /// precisely so that replacing a publication cannot touch it.
    #[test]
    fn staging_never_creates_var_inside_a_publication() {
        let temp = tempdir::TempDir::new("stage-no-var").unwrap();
        let (previous, receipt) = previous_publication(temp.path());
        let (plan, staging) = staged(temp.path());

        prepare(&staging).unwrap();
        seed(
            &plan,
            &staging,
            &Seed {
                publication: &previous,
                receipt: &receipt,
                stale: &[],
            },
        )
        .unwrap();
        execute(&plan, &staging, false, true).unwrap();

        assert!(!staging.join("var").exists());
    }

    /// Every destination the run produced is reported, so the receipt can record how each file was
    /// really obtained rather than how the plan said it would be.
    #[test]
    fn execution_reports_how_each_destination_was_produced() {
        let temp = tempdir::TempDir::new("stage-realized").unwrap();
        let (plan, staging) = staged(temp.path());
        prepare(&staging).unwrap();

        let realized = execute(&plan, &staging, false, true).unwrap();
        assert_eq!(
            realized.get(&staging.join("bin").join("miden-vm")),
            Some(&RealizedMethod::Prebuilt)
        );
        assert_eq!(
            realized.get(&staging.join("lib").join("core.masp")),
            Some(&RealizedMethod::Prebuilt)
        );
    }

    /// A transfer failure with no declared fallback stays fatal.
    #[test]
    fn a_failed_transfer_without_a_fallback_is_fatal() {
        let temp = tempdir::TempDir::new("stage-nofallback").unwrap();
        let (plan, staging) = staged(temp.path());
        prepare(&staging).unwrap();

        let mut plan = plan;
        let dest = staging.join("bin").join("miden-vm");
        plan.steps[0] = PlanStep::CopyLocal {
            src: temp.path().join("does-not-exist"),
            dest,
            mode: MODE_EXECUTABLE,
            owner: "vm".to_string(),
            fallback: None,
        };

        assert!(execute(&plan, &staging, false, true).is_err());
    }

    /// Spec section 9.3: a failed transfer for a component declaring a fallback is recoverable,
    /// not fatal -- the fallback carried by the step runs in its place, and what *actually*
    /// produced the file is reported, because uninstall has to match the path really taken.
    ///
    /// The planner only ever puts a Cargo build here; this substitutes a copy so the test does not
    /// invoke Cargo, which is exactly why the field holds a whole step rather than build
    /// arguments.
    #[test]
    fn a_taken_fallback_reports_its_own_method() {
        let temp = tempdir::TempDir::new("stage-fallback-ok").unwrap();
        let (plan, staging) = staged(temp.path());
        prepare(&staging).unwrap();

        let replacement = temp.path().join("replacement");
        std::fs::write(&replacement, b"from-the-fallback").unwrap();

        let mut plan = plan;
        let dest = staging.join("bin").join("miden-vm");
        plan.steps[0] = PlanStep::CopyLocal {
            src: temp.path().join("does-not-exist"),
            dest: dest.clone(),
            mode: MODE_EXECUTABLE,
            owner: "vm".to_string(),
            fallback: Some(Box::new(PlanStep::CopyLocal {
                src: replacement,
                dest: dest.clone(),
                mode: MODE_EXECUTABLE,
                owner: "vm".to_string(),
                fallback: None,
            })),
        };

        let realized = execute(&plan, &staging, false, true).expect("the fallback must be taken");
        assert_eq!(std::fs::read(&dest).unwrap(), b"from-the-fallback");
        assert_eq!(realized.get(&dest), Some(&RealizedMethod::Prebuilt));
    }
}
