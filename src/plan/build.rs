//! Turning a resolved component set into an exact, target-specific installation plan.
//!
//! Every decision is made here. Execution performs no filtering, resolves no names, and consults
//! no manifest: it walks the steps and does what each one says. That separation is what makes the
//! executor testable and keeps "what should happen" from drifting between the code that decides
//! and the code that acts.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{
    artifact::{ArtifactUri, Digest, InvalidArtifactError},
    manifest::{Channel, Component, ComponentKind, InstallationMethod, PackageInstallationMethod},
    plan::{
        ComponentInputs, Destination, DestinationError, KeyInputs, PlanKey, ResolvedAuthority,
        compute_plan_key, destination_for,
    },
    resolve::{Intent, ResolutionError, resolve},
};

#[derive(Debug, thiserror::Error)]
pub enum PlanError {
    #[error(transparent)]
    Resolution(#[from] ResolutionError),
    #[error(transparent)]
    Destination(#[from] DestinationError),
    #[error(transparent)]
    Artifact(#[from] InvalidArtifactError),
    #[error(transparent)]
    Pin(#[from] crate::plan::PinError),
    #[error(
        "component '{component}' declares {found} artifact(s), but a {kind} component using \
         '{method}' must declare {expected}"
    )]
    ArtifactCardinality {
        component: String,
        // Not `&'static str`: an unsupported kind carries a tag read from the manifest.
        kind: String,
        method: &'static str,
        expected: &'static str,
        found: usize,
    },
    #[error(
        "component '{component}' declares its artifact as '{artifact}', but installs the \
         executable '{executable}'; for an executable the artifact id must be the installed \
         filename"
    )]
    ArtifactIdMismatch {
        component: String,
        artifact: String,
        executable: String,
    },
    #[error(
        "component '{component}' has no artifact for target '{target}', and does not declare a \
         Cargo fallback"
    )]
    TargetUnsupported { component: String, target: String },
    #[error("components '{first}' and '{second}' both install to '{path}'")]
    DestinationCollision {
        first: String,
        second: String,
        path: String,
    },
    #[error(
        "components '{first}' and '{second}' both build Cargo package '{unit}'; midenup cannot \
         tell their outputs apart"
    )]
    CargoUnitConflict {
        unit: String,
        first: String,
        second: String,
    },
}

/// An `opt/` shim: `opt/<name>` pointing at `bin/<target_binary>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymlinkSpec {
    pub name: String,
    pub target_binary: String,
}

/// One thing the executor must do. Every path is final and absolute.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanStep {
    /// Fetch over HTTP(S).
    Download {
        uri: String,
        dest: PathBuf,
        mode: u32,
        owner: String,
        /// Recorded in the receipt, never verified. See [crate::artifact::Digest].
        digest: Option<Digest>,
        /// What to do instead if the transfer fails (spec section 9.3).
        ///
        /// Only a `prebuilt-with-cargo-fallback` component has one, and the planner only ever
        /// puts a [PlanStep::CargoBuild] here. It is a whole step rather than a bag of Cargo
        /// arguments so that the executor needs no second construction path -- and so a test can
        /// substitute a cheaper step to exercise the fallback itself.
        fallback: Option<Box<PlanStep>>,
    },
    /// Copy from the local filesystem.
    CopyLocal {
        src: PathBuf,
        dest: PathBuf,
        mode: u32,
        owner: String,
        /// As [PlanStep::Download::fallback]. A `file://` artifact can be missing just as a
        /// download can 404, and the declared fallback is what makes either recoverable.
        fallback: Option<Box<PlanStep>>,
    },
    /// Build with `cargo install`.
    CargoBuild {
        crate_name: String,
        /// Pinned: a branch is already reduced to the commit that will be installed.
        authority: ResolvedAuthority,
        features: Vec<String>,
        rustup_channel: Option<String>,
        /// The binary the build must produce. Passed as `--bin`, and verified afterwards.
        expect_binary: String,
        dest: PathBuf,
        owner: String,
    },
    /// Extract a Miden package from a Rust crate. See [ComponentKind::LegacyPackage].
    ExtractPackage {
        crate_name: String,
        /// Pinned: a branch is already reduced to the commit that will be installed.
        authority: ResolvedAuthority,
        features: Vec<String>,
        extractor: String,
        dest: PathBuf,
        owner: String,
    },
}

impl PlanStep {
    /// The component this step installs for.
    pub fn owner(&self) -> &str {
        match self {
            Self::Download { owner, .. }
            | Self::CopyLocal { owner, .. }
            | Self::CargoBuild { owner, .. }
            | Self::ExtractPackage { owner, .. } => owner,
        }
    }

    /// The exact path this step produces.
    pub fn dest(&self) -> &Path {
        match self {
            Self::Download { dest, .. }
            | Self::CopyLocal { dest, .. }
            | Self::CargoBuild { dest, .. }
            | Self::ExtractPackage { dest, .. } => dest,
        }
    }

    /// The step to run instead if this one fails, if any.
    pub fn fallback(&self) -> Option<&PlanStep> {
        match self {
            Self::Download { fallback, .. } | Self::CopyLocal { fallback, .. } => {
                fallback.as_deref()
            },
            Self::CargoBuild { .. } | Self::ExtractPackage { .. } => None,
        }
    }

    /// The mode the produced file must end up with.
    pub fn mode(&self) -> u32 {
        match self {
            Self::Download { mode, .. } | Self::CopyLocal { mode, .. } => *mode,
            // Anything Cargo produces is a program.
            Self::CargoBuild { .. } => crate::plan::MODE_EXECUTABLE,
            Self::ExtractPackage { .. } => crate::plan::MODE_DATA,
        }
    }
}

/// A fully resolved, target-specific description of an installation.
#[derive(Debug, Clone)]
pub struct InstallationPlan {
    pub target: String,
    pub channel: semver::Version,
    /// Dependency order: a component's requirements appear before it.
    pub steps: Vec<PlanStep>,
    pub symlinks: Vec<SymlinkSpec>,
    pub key: PlanKey,
}

/// Builds the plan for installing `intent` from `channel` onto `target`, rooted at `sysroot`.
pub fn build(
    channel: &Channel,
    intent: &Intent,
    target: &str,
    sysroot: &Path,
) -> Result<InstallationPlan, PlanError> {
    build_in(channel, intent, target, sysroot, Path::new("."))
}

/// As [build], with an explicit working directory for resolving relative path authorities.
pub fn build_in(
    channel: &Channel,
    intent: &Intent,
    target: &str,
    sysroot: &Path,
    cwd: &Path,
) -> Result<InstallationPlan, PlanError> {
    let components = resolve(channel, intent)?;

    let mut steps = Vec::new();
    let mut symlinks = Vec::new();
    let mut key_components = Vec::with_capacity(components.len());
    let mut claimed: HashMap<PathBuf, String> = HashMap::new();
    let mut cargo_units: HashMap<String, String> = HashMap::new();

    for component in components {
        // Pin before anything is recorded. A key computed over "the tip of main" identifies
        // nothing: it would compare equal across two installs producing different binaries.
        let authority = crate::plan::pin(&component.version, cwd)?;

        let mut inputs = ComponentInputs {
            name: component.name.to_string(),
            authority: authority.identity(),
            kind: component.kind().tag().to_string(),
            ..Default::default()
        };

        plan_component(
            component,
            &authority,
            target,
            sysroot,
            &mut steps,
            &mut symlinks,
            &mut inputs,
            &mut cargo_units,
        )?;

        // Claim destinations after the component is planned, so a component colliding with
        // *itself* is reported the same way as one colliding with another.
        for step in steps.iter().filter(|s| s.owner() == component.name.as_ref()) {
            if let Some(previous) =
                claimed.insert(step.dest().to_path_buf(), component.name.to_string())
                && previous != component.name.as_ref()
            {
                return Err(PlanError::DestinationCollision {
                    first: previous,
                    second: component.name.to_string(),
                    path: step.dest().display().to_string(),
                });
            }
        }

        key_components.push(inputs);
    }

    let key = compute_plan_key(&KeyInputs {
        target: target.to_string(),
        components: key_components,
    });

    Ok(InstallationPlan {
        target: target.to_string(),
        channel: channel.name.clone(),
        steps,
        symlinks,
        key,
    })
}

/// The plan key contribution of one component, computed in isolation.
///
/// "A component needs reinstallation iff its contribution to the plan key changed" (spec section
/// 11.1) is only a usable rule if that contribution can be computed for one component at a time.
/// This runs the same planning logic the full plan does -- pinning the authority, resolving
/// artifacts for the target, computing destinations and modes -- and hashes the result.
///
/// The sysroot is notional: destinations enter the key relative to it, so the answer does not
/// depend on where the toolchain happens to live.
pub fn component_key(
    component: &Component,
    target: &str,
    cwd: &Path,
) -> Result<PlanKey, PlanError> {
    const NOTIONAL_SYSROOT: &str = "/sysroot";

    let authority = crate::plan::pin(&component.version, cwd)?;
    let mut inputs = ComponentInputs {
        name: component.name.to_string(),
        authority: authority.identity(),
        kind: component.kind().tag().to_string(),
        ..Default::default()
    };

    plan_component(
        component,
        &authority,
        target,
        Path::new(NOTIONAL_SYSROOT),
        &mut Vec::new(),
        &mut Vec::new(),
        &mut inputs,
        &mut HashMap::new(),
    )?;

    Ok(compute_plan_key(&KeyInputs {
        target: target.to_string(),
        components: vec![inputs],
    }))
}

/// Enforces the component/artifact matrix for one component and emits its steps.
#[allow(clippy::too_many_arguments)]
fn plan_component(
    component: &Component,
    authority: &ResolvedAuthority,
    target: &str,
    sysroot: &Path,
    steps: &mut Vec<PlanStep>,
    symlinks: &mut Vec<SymlinkSpec>,
    inputs: &mut ComponentInputs,
    cargo_units: &mut HashMap<String, String>,
) -> Result<(), PlanError> {
    let name = component.name.to_string();
    let declared = component.artifacts.artifacts.len();
    let available: Vec<(String, ArtifactUri)> = component
        .artifacts
        .get_artifacts_for_target(target, component)?
        .into_iter()
        .map(|(id, uri)| (id.to_string(), uri))
        .collect();

    match component.kind() {
        ComponentKind::Executable { installation_method, spec }
        | ComponentKind::CargoExtension { installation_method, spec } => {
            let kind_tag = component.kind().tag().to_string();
            let executable = spec.installed_executable.as_str();

            // An executable installs exactly one file, so its artifact must be named for it.
            if declared > 0 {
                if declared > 1 {
                    return Err(PlanError::ArtifactCardinality {
                        component: name,
                        kind: kind_tag.clone(),
                        method: method_tag(installation_method),
                        expected: "at most one",
                        found: declared,
                    });
                }
                let id = component.artifacts.artifacts.keys().next().expect("declared > 0");
                if id != executable {
                    return Err(PlanError::ArtifactIdMismatch {
                        component: name,
                        artifact: id.clone(),
                        executable: executable.to_string(),
                    });
                }
            }

            let destination = destination_for(component, executable, sysroot)?;
            inputs.destinations.push(key_destination(&destination, sysroot));

            match installation_method {
                InstallationMethod::Cargo { crate_name, rustup_channel, features } => {
                    if declared > 0 {
                        return Err(PlanError::ArtifactCardinality {
                            component: name,
                            kind: kind_tag.clone(),
                            method: "cargo",
                            expected: "none",
                            found: declared,
                        });
                    }
                    push_cargo_build(
                        component,
                        authority,
                        crate_name,
                        features,
                        rustup_channel,
                        executable,
                        destination,
                        steps,
                        inputs,
                        cargo_units,
                    )?;
                },
                InstallationMethod::Prebuilt => {
                    let Some((id, uri)) = available.into_iter().next() else {
                        // A prebuilt component with no artifact for this target cannot be
                        // installed at all -- previously this produced an empty artifact list and
                        // an install that reported success while placing no files.
                        return Err(PlanError::TargetUnsupported {
                            component: name,
                            target: target.to_string(),
                        });
                    };
                    push_transfer(component, &id, uri, destination, steps, inputs);
                },
                InstallationMethod::PrebuiltWithCargoFallback {
                    crate_name,
                    rustup_channel,
                    features,
                } => match available.into_iter().next() {
                    // The artifact is what will be installed, but the declared fallback travels
                    // with the step: an artifact that 404s at execution time is exactly the
                    // situation the fallback exists for.
                    Some((id, uri)) => {
                        let fallback = PlanStep::CargoBuild {
                            crate_name: crate_name.clone(),
                            authority: authority.clone(),
                            features: features.clone(),
                            rustup_channel: rustup_channel.clone(),
                            expect_binary: executable.to_string(),
                            dest: destination.path.clone(),
                            owner: component.name.to_string(),
                        };
                        push_transfer_with_fallback(
                            component,
                            &id,
                            uri,
                            destination,
                            steps,
                            inputs,
                            Some(fallback),
                        )
                    },
                    // The declared fallback is exactly what makes missing target support
                    // recoverable rather than fatal.
                    None => push_cargo_build(
                        component,
                        authority,
                        crate_name,
                        features,
                        rustup_channel,
                        executable,
                        destination,
                        steps,
                        inputs,
                        cargo_units,
                    )?,
                },
            }

            if let Some(symlink) = component.get_symlink_name() {
                symlinks.push(SymlinkSpec {
                    name: symlink.clone(),
                    target_binary: executable.to_string(),
                });
                inputs.symlinks.push((symlink, executable.to_string()));
            }
            inputs.installation_method = method_tag(installation_method).to_string();
        },

        ComponentKind::Package | ComponentKind::Asset => {
            let kind_tag = component.kind().tag().to_string();
            if declared == 0 {
                return Err(PlanError::ArtifactCardinality {
                    component: name,
                    kind: kind_tag.clone(),
                    method: "prebuilt",
                    expected: "at least one",
                    found: 0,
                });
            }
            // Every declared artifact is required: a partial install is not a success.
            if available.len() != declared {
                return Err(PlanError::TargetUnsupported {
                    component: name,
                    target: target.to_string(),
                });
            }
            for (id, uri) in available {
                let destination = destination_for(component, &id, sysroot)?;
                inputs.destinations.push(key_destination(&destination, sysroot));
                push_transfer(component, &id, uri, destination, steps, inputs);
            }
            inputs.installation_method = "prebuilt".to_string();
        },

        ComponentKind::Command { .. } => {
            // A command may install nothing at all; it is still a real component.
            for (id, uri) in available {
                let destination = destination_for(component, &id, sysroot)?;
                inputs.destinations.push(key_destination(&destination, sysroot));
                push_transfer(component, &id, uri, destination, steps, inputs);
            }
            inputs.installation_method = "n/a".to_string();
        },

        ComponentKind::LegacyPackage { installation_method, .. } => {
            if declared > 0 {
                return Err(PlanError::ArtifactCardinality {
                    component: name,
                    kind: "legacy-package".to_string(),
                    method: "cargo",
                    expected: "none -- a package with artifacts is a 'package'",
                    found: declared,
                });
            }
            let installed = component
                .installed_package_name()
                .expect("legacy packages always resolve an installed filename");
            let destination = destination_for(component, &installed, sysroot)?;
            inputs.destinations.push(key_destination(&destination, sysroot));

            match installation_method {
                PackageInstallationMethod::Cargo { crate_name, features, extractor } => {
                    claim_cargo_unit(authority, crate_name, &name, cargo_units)?;
                    inputs.crate_name = Some(format!("{crate_name}#{extractor}"));
                    inputs.features = Some(features.clone());
                    steps.push(PlanStep::ExtractPackage {
                        crate_name: crate_name.clone(),
                        authority: authority.clone(),
                        features: features.clone(),
                        extractor: extractor.clone(),
                        dest: destination.path,
                        owner: name,
                    });
                },
                PackageInstallationMethod::Prebuilt => {
                    // A prebuilt package is a `package`; this shape cannot install anything.
                    return Err(PlanError::ArtifactCardinality {
                        component: name,
                        kind: "legacy-package".to_string(),
                        method: "prebuilt",
                        expected: "artifacts, in which case declare it as a 'package'",
                        found: 0,
                    });
                },
            }
            inputs.installation_method = "cargo".to_string();
        },

        ComponentKind::Unsupported { .. } => {
            unreachable!("the resolver rejects unsupported components before planning")
        },
    }

    Ok(())
}

/// A destination as the plan key should see it: relative to the sysroot.
///
/// The key identifies *what* is installed, not where the toolchain happens to live. Including the
/// absolute prefix would make an otherwise identical installation compare differently purely
/// because `MIDENUP_HOME` moved.
fn key_destination(destination: &Destination, sysroot: &Path) -> (String, u32) {
    let relative = destination.path.strip_prefix(sysroot).unwrap_or(&destination.path);
    (relative.display().to_string(), destination.mode)
}

fn method_tag(method: &InstallationMethod) -> &'static str {
    match method {
        InstallationMethod::Prebuilt => "prebuilt",
        InstallationMethod::PrebuiltWithCargoFallback { .. } => "prebuilt-with-cargo-fallback",
        InstallationMethod::Cargo { .. } => "cargo",
    }
}

fn push_transfer(
    component: &Component,
    id: &str,
    uri: ArtifactUri,
    destination: Destination,
    steps: &mut Vec<PlanStep>,
    inputs: &mut ComponentInputs,
) {
    push_transfer_with_fallback(component, id, uri, destination, steps, inputs, None)
}

/// As [push_transfer], with a step to run if the transfer fails.
///
/// The fallback deliberately contributes nothing to the plan key. It describes what would happen
/// in a failure that has not occurred; the installed bytes are the same either way, and letting a
/// latent alternative change the key would reclassify every existing `prebuilt-with-cargo-fallback`
/// installation as changed.
fn push_transfer_with_fallback(
    component: &Component,
    id: &str,
    uri: ArtifactUri,
    destination: Destination,
    steps: &mut Vec<PlanStep>,
    inputs: &mut ComponentInputs,
    fallback: Option<PlanStep>,
) {
    let digest = component.artifacts.artifacts.get(id).and_then(|a| a.digest().cloned());
    let fallback = fallback.map(Box::new);
    inputs.artifacts.push((id.to_string(), uri.to_string()));

    steps.push(match uri {
        ArtifactUri::File(src) => PlanStep::CopyLocal {
            src,
            dest: destination.path,
            mode: destination.mode,
            owner: component.name.to_string(),
            fallback,
        },
        ArtifactUri::Http(uri) => PlanStep::Download {
            uri,
            dest: destination.path,
            mode: destination.mode,
            owner: component.name.to_string(),
            digest,
            fallback,
        },
    });
}

#[allow(clippy::too_many_arguments)]
fn push_cargo_build(
    component: &Component,
    authority: &ResolvedAuthority,
    crate_name: &str,
    features: &[String],
    rustup_channel: &Option<String>,
    executable: &str,
    destination: Destination,
    steps: &mut Vec<PlanStep>,
    inputs: &mut ComponentInputs,
    cargo_units: &mut HashMap<String, String>,
) -> Result<(), PlanError> {
    let owner = component.name.to_string();
    claim_cargo_unit(authority, crate_name, &owner, cargo_units)?;

    inputs.crate_name = Some(crate_name.to_string());
    inputs.features = Some(features.to_vec());
    inputs.rustup_channel = rustup_channel.clone();

    steps.push(PlanStep::CargoBuild {
        crate_name: crate_name.to_string(),
        authority: authority.clone(),
        features: features.to_vec(),
        rustup_channel: rustup_channel.clone(),
        expect_binary: executable.to_string(),
        dest: destination.path,
        owner,
    });
    Ok(())
}

/// Records that `owner` builds `(authority, crate_name)`, rejecting a second claimant.
///
/// `cargo install` writes shared bookkeeping (`.crates.toml`, `.crates2.json`) keyed by package,
/// so two components building the same package cannot be updated or removed independently --
/// uninstalling one would take the other's binary with it.
fn claim_cargo_unit(
    authority: &ResolvedAuthority,
    crate_name: &str,
    owner: &str,
    cargo_units: &mut HashMap<String, String>,
) -> Result<(), PlanError> {
    let unit = format!("{}#{crate_name}", authority.identity());
    match cargo_units.insert(unit.clone(), owner.to_string()) {
        Some(previous) if previous != owner => Err(PlanError::CargoUnitConflict {
            unit,
            first: previous,
            second: owner.to_string(),
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;
    use crate::{
        artifact::{Artifact, Artifacts},
        manifest::{ExecutableComponent, OpaqueBody},
        profile::Profile,
        version::{Authority, GitTarget},
    };

    const TARGET: &str = "aarch64-apple-darwin";

    fn sysroot() -> &'static Path {
        Path::new("/sysroot")
    }

    fn intent() -> Intent {
        Intent::new(&[Profile::Complete], &[])
    }

    fn agnostic(uri: &str) -> Artifact {
        Artifact::TargetAgnostic { uri: uri.to_string(), digest: None }
    }

    fn specific(uri: &str, targets: &[&str]) -> Artifact {
        Artifact::TargetSpecific {
            uri: uri.to_string(),
            substitutions: None,
            targets: targets.iter().map(|t| (t.to_string(), Default::default())).collect(),
            digest: None,
        }
    }

    fn component(
        name: &'static str,
        kind: ComponentKind,
        artifacts: &[(&str, Artifact)],
    ) -> Component {
        let mut declared = Artifacts::default();
        for (id, artifact) in artifacts {
            declared.insert(id.to_string(), artifact.clone());
        }
        Component {
            name: Cow::Borrowed(name),
            version: Authority::Registry { version: semver::Version::new(0, 1, 0) },
            kind,
            profiles: vec![Profile::Minimal],
            requires: vec![],
            artifacts: declared,
            extra: Default::default(),
        }
    }

    fn executable_kind(method: InstallationMethod, installed: &str) -> ComponentKind {
        ComponentKind::Executable {
            installation_method: method,
            spec: ExecutableComponent {
                installed_executable: installed.to_string(),
                ..Default::default()
            },
        }
    }

    fn cargo(crate_name: &str) -> InstallationMethod {
        InstallationMethod::Cargo {
            crate_name: crate_name.to_string(),
            rustup_channel: None,
            features: vec![],
        }
    }

    fn fallback(crate_name: &str) -> InstallationMethod {
        InstallationMethod::PrebuiltWithCargoFallback {
            crate_name: crate_name.to_string(),
            rustup_channel: None,
            features: vec![],
        }
    }

    fn channel(components: Vec<Component>) -> Channel {
        Channel::new(semver::Version::new(0, 15, 0), None, components)
    }

    fn plan_of(components: Vec<Component>) -> Result<InstallationPlan, PlanError> {
        build(&channel(components), &intent(), TARGET, sysroot())
    }

    #[test]
    fn a_prebuilt_executable_downloads_to_bin() {
        let plan = plan_of(vec![component(
            "vm",
            executable_kind(InstallationMethod::Prebuilt, "miden-vm"),
            &[("miden-vm", agnostic("https://example.invalid/vm"))],
        )])
        .expect("should plan");

        assert_eq!(plan.steps.len(), 1);
        assert!(matches!(&plan.steps[0], PlanStep::Download { dest, mode, .. }
            if dest == &sysroot().join("bin").join("miden-vm") && *mode == crate::plan::MODE_EXECUTABLE));
    }

    #[test]
    fn a_file_artifact_becomes_a_local_copy() {
        let plan = plan_of(vec![component(
            "vm",
            executable_kind(InstallationMethod::Prebuilt, "miden-vm"),
            &[("miden-vm", agnostic("file:///tmp/miden-vm"))],
        )])
        .expect("should plan");
        assert!(matches!(&plan.steps[0], PlanStep::CopyLocal { .. }));
    }

    /// A prebuilt component with no artifact for this target cannot be installed.
    ///
    /// Regression: this previously produced an empty artifact list and an install that reported
    /// success while placing no files.
    #[test]
    fn a_prebuilt_executable_without_target_support_fails() {
        let err = plan_of(vec![component(
            "vm",
            executable_kind(InstallationMethod::Prebuilt, "miden-vm"),
            &[(
                "miden-vm",
                specific("https://example.invalid/%target", &["x86_64-unknown-linux-gnu"]),
            )],
        )])
        .expect_err("must fail");
        assert!(matches!(err, PlanError::TargetUnsupported { .. }), "{err}");
    }

    /// ...unless a Cargo fallback is declared, which is exactly what makes it recoverable.
    #[test]
    fn a_declared_fallback_turns_missing_target_support_into_a_cargo_build() {
        let plan = plan_of(vec![component(
            "vm",
            executable_kind(fallback("miden-vm"), "miden-vm"),
            &[(
                "miden-vm",
                specific("https://example.invalid/%target", &["x86_64-unknown-linux-gnu"]),
            )],
        )])
        .expect("should plan");

        assert!(matches!(&plan.steps[0], PlanStep::CargoBuild { expect_binary, .. }
            if expect_binary == "miden-vm"));
    }

    #[test]
    fn a_fallback_prefers_the_artifact_when_the_target_is_supported() {
        let plan = plan_of(vec![component(
            "vm",
            executable_kind(fallback("miden-vm"), "miden-vm"),
            &[("miden-vm", specific("https://example.invalid/%target", &[TARGET]))],
        )])
        .expect("should plan");
        assert!(matches!(&plan.steps[0], PlanStep::Download { .. }));
    }

    /// ...but the declared fallback still travels with the transfer. An artifact that is listed
    /// for this target and then 404s at execution time is exactly what the fallback is for
    /// (spec section 9.3), and the executor cannot construct one on its own -- it holds no
    /// manifest.
    #[test]
    fn a_supported_target_still_carries_the_fallback_on_the_transfer_step() {
        let plan = plan_of(vec![component(
            "vm",
            executable_kind(fallback("miden-vm"), "miden-vm"),
            &[("miden-vm", specific("https://example.invalid/%target", &[TARGET]))],
        )])
        .expect("should plan");

        let fallback = plan.steps[0].fallback().expect("the transfer must carry its fallback");
        assert!(matches!(fallback, PlanStep::CargoBuild { expect_binary, dest, .. }
            if expect_binary == "miden-vm" && dest == plan.steps[0].dest()));
    }

    #[test]
    fn a_cargo_executable_declaring_artifacts_is_rejected() {
        let err = plan_of(vec![component(
            "vm",
            executable_kind(cargo("miden-vm"), "miden-vm"),
            &[("miden-vm", agnostic("https://example.invalid/vm"))],
        )])
        .expect_err("must fail");
        assert!(matches!(err, PlanError::ArtifactCardinality { .. }), "{err}");
    }

    /// An executable installs exactly one file, so its artifact must be named for that file --
    /// otherwise the id and the installed name describe different things.
    #[test]
    fn an_executable_artifact_must_be_named_for_the_installed_binary() {
        let err = plan_of(vec![component(
            "vm",
            executable_kind(InstallationMethod::Prebuilt, "miden-vm"),
            &[("something-else", agnostic("https://example.invalid/vm"))],
        )])
        .expect_err("must fail");
        assert!(matches!(err, PlanError::ArtifactIdMismatch { .. }), "{err}");
    }

    #[test]
    fn a_package_installs_every_artifact_to_lib() {
        let plan = plan_of(vec![component(
            "core",
            ComponentKind::Package,
            &[
                ("core.masp", agnostic("https://example.invalid/core.masp")),
                ("extra.masp", agnostic("https://example.invalid/extra.masp")),
            ],
        )])
        .expect("should plan");

        assert_eq!(plan.steps.len(), 2, "every declared artifact must be installed");
        for step in &plan.steps {
            assert!(step.dest().starts_with(sysroot().join("lib")));
            assert_eq!(step.mode(), crate::plan::MODE_DATA, "packages are data, not programs");
        }
    }

    #[test]
    fn a_package_with_no_artifacts_is_rejected() {
        let err =
            plan_of(vec![component("core", ComponentKind::Package, &[])]).expect_err("must fail");
        assert!(matches!(err, PlanError::ArtifactCardinality { .. }), "{err}");
    }

    /// A partial install is not a success: if any declared artifact is missing for this target,
    /// the plan cannot be built.
    #[test]
    fn a_package_missing_one_artifact_for_the_target_is_rejected() {
        let err = plan_of(vec![component(
            "core",
            ComponentKind::Package,
            &[
                ("core.masp", agnostic("https://example.invalid/core.masp")),
                (
                    "other.masp",
                    specific("https://example.invalid/%target", &["x86_64-unknown-linux-gnu"]),
                ),
            ],
        )])
        .expect_err("must fail");
        assert!(matches!(err, PlanError::TargetUnsupported { .. }), "{err}");
    }

    #[test]
    fn an_asset_installs_under_etc_by_component_name() {
        let plan = plan_of(vec![component(
            "node",
            ComponentKind::Asset,
            &[("docker-compose.yml", agnostic("https://example.invalid/dc.yml"))],
        )])
        .expect("should plan");

        assert_eq!(
            plan.steps[0].dest(),
            sysroot().join("etc").join("node").join("docker-compose.yml")
        );
    }

    /// A command may install nothing at all and is still a real component.
    #[test]
    fn a_command_with_no_artifacts_plans_no_steps() {
        let plan = plan_of(vec![component(
            "node",
            ComponentKind::Command {
                command_name: None,
                format: crate::exec::Executable::default_call_format(),
                subcommands: Default::default(),
                aliases: Default::default(),
            },
            &[],
        )])
        .expect("a command without artifacts is valid");
        assert!(plan.steps.is_empty());
    }

    #[test]
    fn a_legacy_package_extracts_to_its_declared_filename() {
        let plan = plan_of(vec![component(
            "protocol",
            ComponentKind::LegacyPackage {
                installation_method: PackageInstallationMethod::Cargo {
                    crate_name: "miden-protocol".to_string(),
                    features: vec![],
                    extractor: "x()".to_string(),
                },
                installed_package: Some("protocol.masp".to_string()),
            },
            &[],
        )])
        .expect("should plan");

        assert!(matches!(&plan.steps[0], PlanStep::ExtractPackage { dest, .. }
            if dest == &sysroot().join("lib").join("protocol.masp")));
    }

    #[test]
    fn a_legacy_package_declaring_artifacts_is_rejected() {
        let err = plan_of(vec![component(
            "protocol",
            ComponentKind::LegacyPackage {
                installation_method: PackageInstallationMethod::Cargo {
                    crate_name: "miden-protocol".to_string(),
                    features: vec![],
                    extractor: "x()".to_string(),
                },
                installed_package: None,
            },
            &[("protocol.masp", agnostic("https://example.invalid/p.masp"))],
        )])
        .expect_err("must fail");
        assert!(matches!(err, PlanError::ArtifactCardinality { .. }), "{err}");
    }

    /// Two components building one Cargo package cannot be updated or removed independently --
    /// uninstalling one would take the other's binary with it.
    #[test]
    fn two_components_claiming_one_cargo_unit_are_rejected() {
        let err = plan_of(vec![
            component("first", executable_kind(cargo("shared"), "first-bin"), &[]),
            component("second", executable_kind(cargo("shared"), "second-bin"), &[]),
        ])
        .expect_err("must fail");
        assert!(matches!(err, PlanError::CargoUnitConflict { .. }), "{err}");
    }

    #[test]
    fn two_components_installing_to_one_path_are_rejected() {
        let err = plan_of(vec![
            component(
                "core",
                ComponentKind::Package,
                &[("shared.masp", agnostic("https://example.invalid/a"))],
            ),
            component(
                "other",
                ComponentKind::Package,
                &[("shared.masp", agnostic("https://example.invalid/b"))],
            ),
        ])
        .expect_err("must fail");
        assert!(matches!(err, PlanError::DestinationCollision { .. }), "{err}");
    }

    #[test]
    fn steps_are_emitted_in_dependency_order() {
        let mut dependent = component(
            "top",
            executable_kind(InstallationMethod::Prebuilt, "top-bin"),
            &[("top-bin", agnostic("https://example.invalid/top"))],
        );
        dependent.requires = vec!["base".to_string()];
        let base = component(
            "base",
            ComponentKind::Package,
            &[("base.masp", agnostic("https://example.invalid/base"))],
        );

        let plan = plan_of(vec![dependent, base]).expect("should plan");
        assert_eq!(plan.steps[0].owner(), "base", "dependencies must be installed first");
        assert_eq!(plan.steps[1].owner(), "top");
    }

    #[test]
    fn an_unsupported_component_never_reaches_planning() {
        let unsupported = component(
            "futurething",
            ComponentKind::Unsupported {
                tag: "wasm-module".to_string(),
                body: OpaqueBody(Default::default()),
            },
            &[],
        );
        // Selected implicitly: excluded, so the plan succeeds with nothing to do.
        let plan = plan_of(vec![unsupported.clone()]).expect("must not panic");
        assert!(plan.steps.is_empty());

        // Named explicitly: an error from the resolver, before planning.
        let err = build(
            &channel(vec![unsupported]),
            &Intent::new(&[], &["futurething"]),
            TARGET,
            sysroot(),
        )
        .expect_err("must fail");
        assert!(matches!(err, PlanError::Resolution(_)), "{err}");
    }

    /// A branch must be pinned before the key is computed.
    ///
    /// Two installs from "the tip of main" at different commits are different installations. If
    /// the key saw the branch *name*, they would compare equal -- and an update would conclude
    /// nothing had changed.
    #[test]
    fn a_branch_authority_is_pinned_before_the_key_is_computed() {
        fn git(dir: &Path, args: &[&str]) -> String {
            let output =
                std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
            assert!(output.status.success(), "git {args:?} failed");
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }

        let temp = tempdir::TempDir::new("plan-branch").unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("f"), b"one").unwrap();
        git(&repo, &["init", "--quiet", "--initial-branch=main"]);
        git(&repo, &["config", "user.email", "f@example.invalid"]);
        git(&repo, &["config", "user.name", "F"]);
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "one"]);
        let first = git(&repo, &["rev-parse", "HEAD"]);

        let branch_component = || {
            let mut c = component("vm", executable_kind(cargo("fixture-vm"), "miden-vm"), &[]);
            c.version = Authority::Git {
                repository_url: repo.display().to_string(),
                subpath: None,
                target: GitTarget::Branch {
                    name: "main".to_string(),
                    latest_revision: None,
                },
            };
            c
        };

        let before = build(&channel(vec![branch_component()]), &intent(), TARGET, sysroot())
            .expect("should plan");

        // The step must carry the commit, not the branch name.
        match &before.steps[0] {
            PlanStep::CargoBuild { authority, .. } => {
                assert!(
                    matches!(authority, ResolvedAuthority::Git { revision, .. } if revision == &first)
                );
            },
            other => panic!("expected a cargo build, got {other:?}"),
        }

        // Move the branch. The same manifest must now produce a different key.
        std::fs::write(repo.join("f"), b"two").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "--quiet", "-m", "two"]);

        let after = build(&channel(vec![branch_component()]), &intent(), TARGET, sysroot())
            .expect("should plan");
        assert_ne!(
            before.key, after.key,
            "moving the branch must change the key; otherwise an update sees no change"
        );
    }

    /// The key identifies what is installed, not where. Moving `MIDENUP_HOME` must not change it.
    #[test]
    fn the_plan_key_is_independent_of_the_sysroot() {
        let make = |root: &Path| {
            build(
                &channel(vec![component(
                    "vm",
                    executable_kind(InstallationMethod::Prebuilt, "miden-vm"),
                    &[("miden-vm", agnostic("https://example.invalid/vm"))],
                )]),
                &intent(),
                TARGET,
                root,
            )
            .unwrap()
        };

        let a = make(Path::new("/one/place"));
        let b = make(Path::new("/somewhere/else/entirely"));
        assert_eq!(a.key, b.key);
        assert_ne!(a.steps[0].dest(), b.steps[0].dest(), "but the steps are still absolute");
    }

    #[test]
    fn the_plan_key_reflects_the_target() {
        let make = |target: &str| {
            build(
                &channel(vec![component(
                    "vm",
                    executable_kind(InstallationMethod::Prebuilt, "miden-vm"),
                    &[("miden-vm", agnostic("https://example.invalid/vm"))],
                )]),
                &intent(),
                target,
                sysroot(),
            )
            .unwrap()
            .key
        };
        assert_ne!(make(TARGET), make("x86_64-unknown-linux-gnu"));
    }
}
