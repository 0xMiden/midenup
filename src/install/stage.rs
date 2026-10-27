//! Building a staged installation tree and checking it before it is published.

use std::{
    collections::{BTreeMap, BTreeSet},
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

impl StageError {
    /// Whether fetched bytes did not match their declared digest.
    fn is_integrity_failure(&self) -> bool {
        matches!(self, Self::Acquire(ExecError::DigestMismatch { .. }))
    }
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
/// Deliberately *not* a directory copy. Copying the old toolchain tree wholesale would carry
/// forward anything that had ever landed there -- files of components since removed upstream,
/// leftovers from an interrupted install, Cargo's own bookkeeping -- and leave no way to tell
/// inherited content from installed content. The receipt is the ownership record, so it is the
/// only thing consulted here.
pub fn seed(plan: &InstallationPlan, into: &Path, from: &Seed<'_>) -> Result<(), StageError> {
    let wanted: BTreeMap<&Path, &str> =
        plan.steps.iter().map(|step| (step.dest(), step.owner())).collect();

    for output in &from.receipt.outputs {
        if from.stale.iter().any(|name| name == &output.owner) {
            continue;
        }

        let dest = into.join(&output.path);
        if wanted.get(dest.as_path()).copied() != Some(output.owner.as_str()) {
            // A destination may be reused by a different component. Its previous contents
            // belong to the old owner, so both the destination and owner must still match.
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
        crate::trace!("seeding {} from the previous publication", dest.display());
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
/// The check is against the step's *exact* destination, never a containing directory: `lib/`
/// exists as soon as the tree is prepared, so testing it would be true from the start and skip
/// every package download.
///
/// Returns how each destination was actually produced, which is not always what the plan declared:
/// see [PlanStep::fallback].
pub fn execute(
    plan: &InstallationPlan,
    staging_root: &Path,
    build_cache: &Path,
    verbose: bool,
    debug: bool,
) -> Result<Realized, StageError> {
    let mut pending_extractions = Vec::new();
    let mut realized = Realized::new();
    let mut announced = BTreeSet::new();
    let mut acquisitions = crate::install::download::Acquisitions::new(&plan.steps);

    let planned = planned_announcements(plan);
    announce_summary(&planned);
    let mut progress = Progress { done: 0, total: planned.len() };

    for step in &plan.steps {
        if step.dest().exists() {
            crate::trace!(
                "carried forward from the previous publication: {}",
                step.dest().display()
            );
            continue;
        }

        // Collected so that every package sharing a crate is extracted by one script.
        if matches!(step, PlanStep::ExtractPackage { .. }) {
            pending_extractions.push(step.clone());
            continue;
        }

        announce(step, &mut announced, Some(&mut progress));

        let method = match run(step, &mut acquisitions, staging_root, build_cache, verbose, debug) {
            Ok(method) => method,
            Err(err) => {
                // A declared Cargo fallback is what makes an unavailable artifact recoverable
                // rather than fatal. Only one retry: if the build fails too, the original
                // acquisition failure is not the interesting one. Bytes that fail their digest
                // are not unavailable, they are wrong, and that is never recoverable.
                let Some(fallback) = step.fallback().filter(|_| !err.is_integrity_failure()) else {
                    return Err(err);
                };
                crate::warn!("could not acquire {}: {err}", step.owner().bold());
                announce(fallback, &mut announced, None);
                run(fallback, &mut acquisitions, staging_root, build_cache, verbose, debug)?
            },
        };

        realized.insert(step.dest().to_path_buf(), method);
    }

    if !pending_extractions.is_empty() {
        for step in &pending_extractions {
            announce(step, &mut announced, Some(&mut progress));
        }
        let script = staging_root.join("extract").with_extension("rs");
        crate::install::extract(&pending_extractions, &script, verbose)?;
        // The script is a build artefact, not part of the toolchain.
        let _ = std::fs::remove_file(&script);
        for step in &pending_extractions {
            realized.insert(step.dest().to_path_buf(), RealizedMethod::Extracted);
        }
    }

    create_symlinks(plan, staging_root)?;

    Ok(realized)
}

/// The kind of work a step performs, as the announcements name it.
fn action_of(step: &PlanStep) -> &'static str {
    match step {
        PlanStep::Download { .. } => "downloading",
        PlanStep::CopyLocal { .. } => "copying",
        PlanStep::CargoBuild { .. } => "building",
        PlanStep::ExtractPackage { .. } => "extracting",
    }
}

/// How far through the announcements the install is. A position in a list, not an estimate of
/// time remaining: the steps are wildly unequal.
struct Progress {
    done: usize,
    total: usize,
}

/// Every announcement this plan will make, deduplicated and filtered exactly as [announce] is, so
/// the counter always reaches its total.
fn planned_announcements(plan: &InstallationPlan) -> Vec<(String, &'static str)> {
    let mut seen = BTreeSet::new();
    let mut planned = Vec::new();

    for step in &plan.steps {
        if step.dest().exists() {
            continue;
        }
        let entry = (step.owner().to_string(), action_of(step));
        if seen.insert(entry.clone()) {
            planned.push(entry);
        }
    }

    planned
}

/// States what kind of work is ahead before any of it starts, chiefly so a multi-minute source
/// build is known about before the pause it causes.
fn announce_summary(planned: &[(String, &'static str)]) {
    if let Some(summary) = summarize(planned) {
        crate::info!("{summary}");
    }
}

fn summarize(planned: &[(String, &'static str)]) -> Option<String> {
    if planned.is_empty() {
        return None;
    }

    // A fixed order, so the same plan reads the same way every time.
    let described: Vec<String> = ["downloading", "copying", "building", "extracting"]
        .into_iter()
        .filter_map(|action| {
            let count = planned.iter().filter(|(_, kind)| *kind == action).count();
            if count == 0 {
                return None;
            }
            let (singular, plural) = match action {
                "downloading" => ("download", "downloads"),
                "copying" => ("copy", "copies"),
                "building" => ("source build", "source builds"),
                _ => ("extraction", "extractions"),
            };
            Some(format!("{count} {}", if count == 1 { singular } else { plural }))
        })
        .collect();

    let steps = if planned.len() == 1 { "step" } else { "steps" };
    Some(format!("{} {steps}: {}", planned.len(), described.join(", ")))
}

/// Announces the work a step is about to do, once per component and kind of work.
///
/// `progress` is absent for an unplanned step (a taken fallback), which is left un-numbered rather
/// than pushed past a total fixed before the run began.
fn announce(
    step: &PlanStep,
    announced: &mut BTreeSet<(String, &'static str)>,
    progress: Option<&mut Progress>,
) {
    let action = action_of(step);
    if !announced.insert((step.owner().to_string(), action)) {
        return;
    }

    // Where a source build is the one thing a user is most likely to want to know about, because
    // it is the one that takes minutes rather than seconds.
    let source = if matches!(step, PlanStep::CargoBuild { .. }) {
        " from source"
    } else {
        ""
    };

    match progress {
        Some(progress) => {
            progress.done += 1;
            crate::info!(
                "[{}/{}] {action} component '{}'{source}",
                progress.done,
                progress.total,
                step.owner().bold()
            );
        },
        None => crate::info!("{action} component '{}'{source}", step.owner().bold()),
    }
}

/// Performs one step, reporting how its output was produced.
fn run(
    step: &PlanStep,
    acquisitions: &mut crate::install::download::Acquisitions<'_>,
    staging_root: &Path,
    build_cache: &Path,
    verbose: bool,
    debug: bool,
) -> Result<RealizedMethod, StageError> {
    match step {
        PlanStep::Download { .. } | PlanStep::CopyLocal { .. } => {
            acquisitions.acquire(step)?;
            Ok(RealizedMethod::Prebuilt)
        },
        PlanStep::CargoBuild { .. } => {
            crate::install::cargo_build(step, staging_root, build_cache, verbose, debug)?;
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
        crate::trace!("linking {} -> {}", link.display(), target.display());
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
/// mode. Content is checked against declared digests as it is acquired, not here: a digest
/// describes the bytes fetched, which for an archive are not the file installed.
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

    fn shared_archive_plan(uri: &str, staging: &Path) -> InstallationPlan {
        let component: Component = serde_json::from_value(serde_json::json!({
            "name": "node", "kind": "asset",
            "version": {"kind": "registry", "version": "1.0.0"},
            "profiles": ["minimal"],
            "artifacts": {
                "compose.yml": {"uri": uri, "archive": "tar.gz"},
                "prometheus.yml": {"uri": uri, "archive": "tar.gz"}
            }
        }))
        .unwrap();
        build_plan(
            &Channel::new(semver::Version::new(0, 15, 0), vec![component]),
            &Intent::new(&[Profile::Minimal], &[]),
            TARGET,
            staging,
        )
        .unwrap()
    }

    fn config_archive() -> Vec<u8> {
        use std::io::Write;
        let mut tar = tar::Builder::new(Vec::new());
        for (name, contents) in
            [("compose.yml", "compose"), ("prometheus.yml", "metrics"), ("README", "ignored")]
        {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o600);
            header.set_cksum();
            tar.append_data(&mut header, name, contents.as_bytes()).unwrap();
        }
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&tar.into_inner().unwrap()).unwrap();
        gzip.finish().unwrap()
    }

    fn serve_archive_once(status: u16) -> (String, std::thread::JoinHandle<()>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let uri = format!("http://{}/config.tar.gz", server.server_addr());
        let handle = std::thread::spawn(move || {
            let request = server.recv_timeout(std::time::Duration::from_secs(10)).unwrap().unwrap();
            request
                .respond(tiny_http::Response::from_data(config_archive()).with_status_code(status))
                .unwrap();
        });
        (uri, handle)
    }

    fn copy_fallback(src: &Path, dest: &Path, owner: &str) -> Box<PlanStep> {
        Box::new(PlanStep::CopyLocal {
            src: src.to_path_buf(),
            dest: dest.to_path_buf(),
            owner: owner.to_string(),
            mode: MODE_DATA,
            digest: None,
            archive: None,
            fallback: None,
        })
    }

    #[test]
    fn shared_archive_members_are_downloaded_once_and_receipted_as_individual_files() {
        let temp = tempdir::TempDir::new("stage-shared-archive").unwrap();
        let staging = temp.path().join("staging");
        // The server closes after one response. Any second fetch fails rather than hanging.
        let (uri, handle) = serve_archive_once(200);
        let plan = shared_archive_plan(&uri, &staging);
        prepare(&staging).unwrap();

        let realized = execute(&plan, &staging, &temp.path().join("cache"), false, true).unwrap();
        handle.join().unwrap();
        verify(&plan, &staging).unwrap();

        let etc = staging.join("etc/node");
        assert_eq!(std::fs::read(etc.join("compose.yml")).unwrap(), b"compose");
        assert_eq!(std::fs::read(etc.join("prometheus.yml")).unwrap(), b"metrics");
        assert_eq!(std::fs::read_dir(&etc).unwrap().count(), 2);
        assert_eq!(realized.len(), 2);
        let receipt = crate::publish::receipt_for(
            &plan,
            &staging,
            &crate::state::PublicationId::generate(),
            &realized,
            None,
        );
        assert_eq!(receipt.outputs.len(), 2);
        assert!(receipt.outputs.iter().all(|o| o.owner == "node" && o.mode == 0o644));
        assert_eq!(receipt.outputs[0].path, Path::new("etc/node/compose.yml"));
        assert_eq!(receipt.outputs[1].path, Path::new("etc/node/prometheus.yml"));
    }

    #[test]
    fn one_archive_member_can_supply_different_components_and_modes() {
        let temp = tempdir::TempDir::new("stage-shared-owners").unwrap();
        let staging = temp.path().join("staging");
        let (uri, handle) = serve_archive_once(200);
        let mut plan = shared_archive_plan(&uri, &staging);
        let mut executable = plan.steps[0].clone();
        if let PlanStep::Download { dest, owner, mode, .. } = &mut executable {
            *dest = staging.join("bin/compose.yml");
            *owner = "runner".into();
            *mode = MODE_EXECUTABLE;
        }
        plan.steps.push(executable);
        prepare(&staging).unwrap();

        let realized = execute(&plan, &staging, &temp.path().join("cache"), false, true).unwrap();
        handle.join().unwrap();
        verify(&plan, &staging).unwrap();
        assert_eq!(std::fs::read(staging.join("bin/compose.yml")).unwrap(), b"compose");
        let receipt = crate::publish::receipt_for(
            &plan,
            &staging,
            &crate::state::PublicationId::generate(),
            &realized,
            None,
        );
        assert_eq!(receipt.outputs[0].owner, "node");
        assert_eq!(receipt.outputs[0].mode, MODE_DATA);
        assert_eq!(receipt.outputs[2].owner, "runner");
        assert_eq!(receipt.outputs[2].mode, MODE_EXECUTABLE);
    }

    #[test]
    fn seeded_archive_members_are_preserved_while_missing_members_are_reacquired() {
        let temp = tempdir::TempDir::new("stage-shared-seed").unwrap();
        let staging = temp.path().join("staging");
        let (uri, handle) = serve_archive_once(200);
        let plan = shared_archive_plan(&uri, &staging);
        let previous = temp.path().join("previous");
        std::fs::create_dir_all(previous.join("etc/node")).unwrap();
        std::fs::write(previous.join("etc/node/compose.yml"), b"seeded").unwrap();
        let mut receipt = crate::publish::receipt_for(
            &plan,
            &staging,
            &crate::state::PublicationId::generate(),
            &Realized::new(),
            None,
        );
        receipt.outputs.truncate(1);
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

        let realized = execute(&plan, &staging, &temp.path().join("cache"), false, true).unwrap();
        handle.join().unwrap();
        verify(&plan, &staging).unwrap();
        assert_eq!(std::fs::read(staging.join("etc/node/compose.yml")).unwrap(), b"seeded");
        assert_eq!(std::fs::read(staging.join("etc/node/prometheus.yml")).unwrap(), b"metrics");
        assert_eq!(realized.len(), 1);
        // With every output present, execution succeeds even though the server is gone.
        assert!(
            execute(&plan, &staging, &temp.path().join("cache"), false, true)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_missing_archive_member_uses_its_fallback_without_refetching_valid_peers() {
        let temp = tempdir::TempDir::new("stage-shared-fallback").unwrap();
        let staging = temp.path().join("staging");
        let (uri, handle) = serve_archive_once(200);
        let mut plan = shared_archive_plan(&uri, &staging);
        let replacement = temp.path().join("replacement");
        std::fs::write(&replacement, b"fallback").unwrap();
        if let PlanStep::Download { archive, dest, owner, fallback, .. } = &mut plan.steps[0] {
            archive.as_mut().unwrap().member = "missing.yml".into();
            *dest = staging.join("etc/node/missing.yml");
            *fallback = Some(copy_fallback(&replacement, dest, owner));
        }
        prepare(&staging).unwrap();

        let realized = execute(&plan, &staging, &temp.path().join("cache"), false, true).unwrap();
        handle.join().unwrap();
        verify(&plan, &staging).unwrap();
        assert_eq!(std::fs::read(staging.join("etc/node/missing.yml")).unwrap(), b"fallback");
        assert_eq!(std::fs::read(staging.join("etc/node/prometheus.yml")).unwrap(), b"metrics");
        assert_eq!(realized.len(), 2);
    }

    #[test]
    fn a_shared_archive_transfer_failure_is_fetched_once_and_each_step_can_fall_back() {
        use std::sync::Arc;
        let temp = tempdir::TempDir::new("stage-shared-failed-transfer").unwrap();
        let staging = temp.path().join("staging");
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").unwrap());
        let uri = format!("http://{}/config.tar.gz", server.server_addr());
        let responding = Arc::clone(&server);
        let handle = std::thread::spawn(move || {
            let mut requests = 0;
            while let Ok(request) = responding.recv() {
                requests += 1;
                request.respond(tiny_http::Response::empty(404)).unwrap();
            }
            requests
        });
        let mut plan = shared_archive_plan(&uri, &staging);
        let replacement = temp.path().join("replacement");
        std::fs::write(&replacement, b"fallback").unwrap();
        for step in &mut plan.steps {
            if let PlanStep::Download { dest, owner, fallback, .. } = step {
                *fallback = Some(copy_fallback(&replacement, dest, owner));
            }
        }
        prepare(&staging).unwrap();

        let result = execute(&plan, &staging, &temp.path().join("cache"), false, true);
        server.unblock();
        assert_eq!(handle.join().unwrap(), 1, "a failed source is fetched only once");
        result.unwrap();
        verify(&plan, &staging).unwrap();
        assert_eq!(std::fs::read(staging.join("etc/node/compose.yml")).unwrap(), b"fallback");
        assert_eq!(std::fs::read(staging.join("etc/node/prometheus.yml")).unwrap(), b"fallback");
    }

    #[test]
    fn an_archive_selector_keeps_the_artifact_keys_relative_path() {
        use std::io::Write;
        let temp = tempdir::TempDir::new("stage-shared-nested").unwrap();
        let staging = temp.path().join("staging");
        let mut tar = tar::Builder::new(Vec::new());
        for (name, data) in [("config.yml", "wrong"), ("nested/config.yml", "right")] {
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(data.len() as u64);
            header.set_cksum();
            tar.append_data(&mut header, name, data.as_bytes()).unwrap();
        }
        let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gzip.write_all(&tar.into_inner().unwrap()).unwrap();
        let archive_path = temp.path().join("config.tar.gz");
        std::fs::write(&archive_path, gzip.finish().unwrap()).unwrap();
        // Build the nested artifact through the planner rather than deriving a selector from dest.
        let component: Component = serde_json::from_value(serde_json::json!({
            "name": "node", "kind": "asset", "version": {"kind": "registry", "version": "1.0.0"},
            "profiles": ["minimal"],
            "artifacts": {"nested/config.yml": {"uri": format!("file://{}", archive_path.display()), "archive": "tar.gz"}}
        })).unwrap();
        let plan = build_plan(
            &Channel::new(semver::Version::new(0, 15, 0), vec![component]),
            &Intent::new(&[Profile::Minimal], &[]),
            TARGET,
            &staging,
        )
        .unwrap();
        prepare(&staging).unwrap();

        execute(&plan, &staging, &temp.path().join("cache"), false, true).unwrap();
        verify(&plan, &staging).unwrap();
        assert_eq!(std::fs::read(staging.join("etc/node/nested/config.yml")).unwrap(), b"right");
        assert!(!staging.join("etc/node/config.yml").exists());
    }

    #[test]
    fn a_one_step_summary_is_singular() {
        let planned = [("vm".to_string(), "building")];
        assert_eq!(summarize(&planned).as_deref(), Some("1 step: 1 source build"));
    }

    /// A channel with one prebuilt executable and one package, both sourced from local files.
    fn fixture(root: &Path) -> (Channel, PathBuf) {
        let sources = root.join("sources");
        std::fs::create_dir_all(&sources).unwrap();
        std::fs::write(sources.join("miden-vm"), b"binary").unwrap();
        std::fs::write(sources.join("core.masp"), b"package").unwrap();

        let uri = |name: &str| format!("file://{}", sources.join(name).display());
        let artifact = |name: &str| Artifact::TargetAgnostic {
            uri: uri(name),
            digest: None,
            archive: None,
            extra: Default::default(),
        };

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
        execute(&plan, &staging, &temp.path().join("cache"), false, true).expect("should stage");
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
            execute(&plan, &staging, &temp.path().join("cache"), false, true).unwrap();

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
        execute(&plan, &staging, &temp.path().join("cache"), false, true).unwrap();

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
        execute(&plan, &staging, &temp.path().join("cache"), false, true).unwrap();

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
            execute(&plan, &staging, &temp.path().join("cache"), false, true).unwrap();

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
        execute(&plan, &staging, &temp.path().join("cache"), false, true).unwrap();

        std::fs::remove_file(staging.join("opt").join("miden vm")).unwrap();
        let err = verify(&plan, &staging).expect_err("must fail");
        assert!(matches!(err, StageError::MissingSymlink { .. }), "{err}");
    }

    /// A directory where a file was planned must not pass: verification is against the exact
    /// destination, not a containing directory that exists as soon as the tree is prepared.
    #[test]
    fn a_directory_where_a_file_was_planned_fails_verification() {
        let temp = tempdir::TempDir::new("stage-dir").unwrap();
        let (plan, staging) = staged(temp.path());
        prepare(&staging).unwrap();
        execute(&plan, &staging, &temp.path().join("cache"), false, true).unwrap();

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
        execute(&plan, &staging, &temp.path().join("cache"), false, true).unwrap();

        // Mark the file so a re-download would be detectable.
        let marked = staging.join("bin").join("miden-vm");
        std::fs::write(&marked, b"marked").unwrap();

        execute(&plan, &staging, &temp.path().join("cache"), false, true)
            .expect("re-staging must succeed");
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

    /// Seeding copies what the previous receipt owns and nothing else, so an unowned file sitting
    /// in the old publication -- a leftover from an interrupted install, a file of a component
    /// since removed -- is not inherited by the new one.
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

    #[test]
    fn reusing_a_destination_for_a_different_component_reacquires_its_contents() {
        let temp = tempdir::TempDir::new("stage-seed-owner").unwrap();
        let (previous, mut receipt) = previous_publication(temp.path());
        receipt.outputs[0].owner = "retired-vm".into();
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
        execute(&plan, &staging, &temp.path().join("cache"), false, true).unwrap();
        assert_eq!(std::fs::read(staging.join("bin/miden-vm")).unwrap(), b"binary");
        assert_eq!(std::fs::read(staging.join("lib/core.masp")).unwrap(), b"old-package");
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
        execute(&plan, &staging, &temp.path().join("cache"), false, true).unwrap();

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
        execute(&plan, &staging, &temp.path().join("cache"), false, true).unwrap();

        assert!(!staging.join("var").exists());
    }

    /// Every destination the run produced is reported, so the receipt can record how each file was
    /// really obtained rather than how the plan said it would be.
    #[test]
    fn execution_reports_how_each_destination_was_produced() {
        let temp = tempdir::TempDir::new("stage-realized").unwrap();
        let (plan, staging) = staged(temp.path());
        prepare(&staging).unwrap();

        let realized = execute(&plan, &staging, &temp.path().join("cache"), false, true).unwrap();
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
            digest: None,
            archive: None,
            fallback: None,
        };

        assert!(execute(&plan, &staging, &temp.path().join("cache"), false, true).is_err());
    }

    /// Bytes that fail their digest are never replaced by the fallback, even one that would
    /// succeed.
    #[test]
    fn a_digest_mismatch_does_not_take_the_fallback() {
        let temp = tempdir::TempDir::new("stage-digest-mismatch").unwrap();
        let (plan, staging) = staged(temp.path());
        prepare(&staging).unwrap();

        let fetched = temp.path().join("fetched");
        std::fs::write(&fetched, b"tampered").unwrap();
        let replacement = temp.path().join("replacement");
        std::fs::write(&replacement, b"from-the-fallback").unwrap();

        let mut plan = plan;
        let dest = staging.join("bin").join("miden-vm");
        plan.steps[0] = PlanStep::CopyLocal {
            src: fetched,
            dest: dest.clone(),
            mode: MODE_EXECUTABLE,
            owner: "vm".to_string(),
            digest: Some(crate::artifact::Digest::sha256(b"binary")),
            archive: None,
            fallback: Some(Box::new(PlanStep::CopyLocal {
                src: replacement,
                dest: dest.clone(),
                mode: MODE_EXECUTABLE,
                owner: "vm".to_string(),
                digest: None,
                archive: None,
                fallback: None,
            })),
        };

        let err = execute(&plan, &staging, &temp.path().join("cache"), false, true)
            .expect_err("a mismatch must be fatal");
        assert!(err.is_integrity_failure(), "{err}");
        assert!(!dest.exists(), "neither the fetched bytes nor the fallback may be installed");
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
            digest: None,
            archive: None,
            fallback: Some(Box::new(PlanStep::CopyLocal {
                src: replacement,
                dest: dest.clone(),
                mode: MODE_EXECUTABLE,
                owner: "vm".to_string(),
                digest: None,
                archive: None,
                fallback: None,
            })),
        };

        let realized = execute(&plan, &staging, &temp.path().join("cache"), false, true)
            .expect("the fallback must be taken");
        assert_eq!(std::fs::read(&dest).unwrap(), b"from-the-fallback");
        assert_eq!(realized.get(&dest), Some(&RealizedMethod::Prebuilt));
    }
}
