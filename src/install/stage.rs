//! Building a staged installation tree and checking it before it is published.

use std::path::{Path, PathBuf};

use crate::{
    install::{CargoError, ExecError, ExtractError},
    plan::{InstallationPlan, PlanStep},
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

/// Runs every step of `plan` into `staging_root`, then creates the `opt/` shims.
///
/// A step whose destination already exists is skipped. That is what makes an update cheap: the
/// tree is seeded from the previous installation and only the components that actually changed
/// were removed beforehand, so everything still present is current.
///
/// The check is against the step's *exact* destination. Testing a containing directory instead is
/// what previously caused every package download to be skipped -- `lib/` exists as soon as the
/// tree is prepared, so the check was always true.
pub fn execute(
    plan: &InstallationPlan,
    staging_root: &Path,
    verbose: bool,
    debug: bool,
) -> Result<(), StageError> {
    let mut pending_extractions = Vec::new();

    for step in &plan.steps {
        if step.dest().exists() {
            continue;
        }

        match step {
            PlanStep::Download { .. } | PlanStep::CopyLocal { .. } => {
                crate::install::acquire(step)?;
            },
            PlanStep::CargoBuild { .. } => {
                crate::install::cargo_build(step, staging_root, verbose, debug)?;
            },
            // Collected so that every package sharing a crate is extracted by one script.
            PlanStep::ExtractPackage { .. } => pending_extractions.push(step.clone()),
        }
    }

    if !pending_extractions.is_empty() {
        let script = staging_root.join("extract").with_extension("rs");
        crate::install::extract(&pending_extractions, &script)?;
        // The script is a build artefact, not part of the toolchain.
        let _ = std::fs::remove_file(&script);
    }

    create_symlinks(plan, staging_root)
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
            vec![],
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
}
