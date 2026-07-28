//! Where an artifact is installed, and with what permissions.
//!
//! Destinations are *computed*, never declared: there is exactly one rule per component kind (see
//! the table in [destination_for]). Letting a manifest declare a destination directly would make
//! every path a potential traversal, and would let two components disagree about who owns a file.

use std::path::{Path, PathBuf};

use crate::manifest::{Component, ComponentKind};

/// An artifact id is invalid as a filename.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InvalidArtifactId {
    #[error("invalid artifact id: must not be empty")]
    Empty,
    #[error("invalid artifact id '{0}': must be a single path segment, not a path")]
    NotASegment(String),
    #[error("invalid artifact id '{0}': '.' and '..' are not valid filenames")]
    RelativeSegment(String),
    #[error("invalid artifact id '{0}': must not begin with '-', which reads as a CLI flag")]
    LeadingDash(String),
    #[error("invalid artifact id '{0}': must not contain a NUL byte")]
    InteriorNul(String),
}

/// A component kind that this build cannot compute a destination for.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DestinationError {
    #[error(transparent)]
    InvalidId(#[from] InvalidArtifactId),
    #[error(
        "cannot compute a destination for component '{component}': its kind '{tag}' is not \
         supported by this version of midenup"
    )]
    UnsupportedKind { component: String, tag: String },
}

/// The exact path an artifact installs to, and the mode it is installed with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Destination {
    pub path: PathBuf,
    pub mode: u32,
}

/// Mode for anything meant to be executed.
pub const MODE_EXECUTABLE: u32 = 0o755;
/// Mode for everything else. Packages and assets are data, not programs.
pub const MODE_DATA: u32 = 0o644;

/// Validates that `id` is usable as a single installed filename.
///
/// The id is both the artifact's identity in the manifest and the name it is written to disk as, so
/// anything that is not a plain filename is rejected here rather than being sanitized later.
pub fn validate_artifact_id(id: &str) -> Result<(), InvalidArtifactId> {
    if id.is_empty() {
        return Err(InvalidArtifactId::Empty);
    }
    if id.contains('\0') {
        return Err(InvalidArtifactId::InteriorNul(id.to_string()));
    }
    if id == "." || id == ".." {
        return Err(InvalidArtifactId::RelativeSegment(id.to_string()));
    }
    if id.contains('/') || id.contains('\\') {
        return Err(InvalidArtifactId::NotASegment(id.to_string()));
    }
    if id.starts_with('-') {
        return Err(InvalidArtifactId::LeadingDash(id.to_string()));
    }
    Ok(())
}

/// Computes where `artifact_id` of `component` installs within `sysroot`.
///
/// | Kind                          | Destination                          | Mode   |
/// |-------------------------------|--------------------------------------|--------|
/// | `executable`, `cargo-extension` | `bin/<installed-executable>`       | `0755` |
/// | `package`                     | `lib/<artifact-id>`                  | `0644` |
/// | `legacy-package`              | `lib/<installed-package>`            | `0644` |
/// | `asset`, `command`            | `etc/<component>/<artifact-id>`      | `0644` |
///
/// Note that executables ignore `artifact_id` for naming: the installed name comes from
/// `installed-executable`, and the matrix requires the two to agree.
pub fn destination_for(
    component: &Component,
    artifact_id: &str,
    sysroot: &Path,
) -> Result<Destination, DestinationError> {
    validate_artifact_id(artifact_id)?;

    let destination = match component.kind() {
        ComponentKind::Executable { spec, .. } | ComponentKind::CargoExtension { spec, .. } => {
            validate_artifact_id(&spec.installed_executable)?;
            Destination {
                path: sysroot.join("bin").join(&spec.installed_executable),
                mode: MODE_EXECUTABLE,
            }
        },
        ComponentKind::Package => Destination {
            path: sysroot.join("lib").join(artifact_id),
            mode: MODE_DATA,
        },
        ComponentKind::LegacyPackage { .. } => {
            // Spec 7.4 gives this kind an explicit `installed-package` filename, because a
            // crate-extracted package has no artifact to take a name from. That schema field does
            // not exist yet -- adding it touches the v1 converter and every shipped 0.9-0.15
            // channel -- so callers pass the intended filename as `artifact_id` for now. Tracked
            // as follow-up F3; it lands with the matrix enforcement in M4-T1.
            Destination {
                path: sysroot.join("lib").join(artifact_id),
                mode: MODE_DATA,
            }
        },
        ComponentKind::Asset | ComponentKind::Command { .. } => Destination {
            path: sysroot.join("etc").join(component.name.as_ref()).join(artifact_id),
            mode: MODE_DATA,
        },
        ComponentKind::Unsupported { tag, .. } => {
            return Err(DestinationError::UnsupportedKind {
                component: component.name.to_string(),
                tag: tag.clone(),
            });
        },
    };

    Ok(destination)
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::*;
    use crate::{
        artifact::Artifacts,
        exec::Executable,
        manifest::{ExecutableComponent, InstallationMethod},
        profile::Profile,
        version::Authority,
    };

    fn component(name: &'static str, kind: ComponentKind) -> Component {
        Component {
            name: Cow::Borrowed(name),
            version: Authority::Registry { version: semver::Version::new(0, 1, 0) },
            kind,
            profiles: vec![Profile::Minimal],
            requires: vec![],
            artifacts: Artifacts::default(),
            extra: Default::default(),
        }
    }

    fn executable(name: &'static str, installed: &str) -> Component {
        component(
            name,
            ComponentKind::Executable {
                installation_method: InstallationMethod::Prebuilt,
                spec: ExecutableComponent {
                    installed_executable: installed.to_string(),
                    ..Default::default()
                },
            },
        )
    }

    #[test]
    fn rejects_unsafe_artifact_ids() {
        for bad in ["", ".", "..", "a/b", "a\\b", "-leading", "with\0nul"] {
            assert!(validate_artifact_id(bad).is_err(), "should reject {bad:?}");
        }
        for good in ["core.masp", "miden-vm", "docker-compose.yml", "a.b.c"] {
            assert!(validate_artifact_id(good).is_ok(), "should accept {good:?}");
        }
    }

    /// A traversal attempt must be rejected outright, not normalized into something plausible.
    #[test]
    fn a_traversal_id_never_produces_a_path_outside_the_sysroot() {
        let pkg = component("core", ComponentKind::Package);
        let err = destination_for(&pkg, "../../etc/passwd", Path::new("/sysroot")).unwrap_err();
        assert!(matches!(err, DestinationError::InvalidId(InvalidArtifactId::NotASegment(_))));
    }

    #[test]
    fn destinations_and_modes_match_the_spec() {
        let root = Path::new("/sysroot");

        let exe = executable("vm", "miden-vm");
        let d = destination_for(&exe, "miden-vm", root).unwrap();
        assert_eq!(d.path, root.join("bin").join("miden-vm"));
        assert_eq!(d.mode, MODE_EXECUTABLE);

        let pkg = component("core", ComponentKind::Package);
        let d = destination_for(&pkg, "core.masp", root).unwrap();
        assert_eq!(d.path, root.join("lib").join("core.masp"));
        assert_eq!(d.mode, MODE_DATA, "packages are data, not programs");

        let asset = component("node", ComponentKind::Asset);
        let d = destination_for(&asset, "docker-compose.yml", root).unwrap();
        assert_eq!(d.path, root.join("etc").join("node").join("docker-compose.yml"));
        assert_eq!(d.mode, MODE_DATA);

        let cmd = component(
            "node",
            ComponentKind::Command {
                command_name: None,
                format: Executable::default(),
                subcommands: Default::default(),
                aliases: Default::default(),
            },
        );
        let d = destination_for(&cmd, "telemetry.yml", root).unwrap();
        assert_eq!(d.path, root.join("etc").join("node").join("telemetry.yml"));
        assert_eq!(d.mode, MODE_DATA);
    }

    /// The executable's installed name comes from `installed-executable`, never from the URI.
    #[test]
    fn an_executable_is_named_by_its_installed_executable() {
        let exe = executable("vm", "miden-vm");
        let d = destination_for(&exe, "miden-vm", Path::new("/s")).unwrap();
        assert_eq!(d.path.file_name().unwrap(), "miden-vm");
    }

    #[test]
    fn an_unsupported_kind_has_no_destination() {
        let unknown = component(
            "futurething",
            ComponentKind::Unsupported {
                tag: "wasm-module".to_string(),
                body: crate::manifest::OpaqueBody(Default::default()),
            },
        );
        let err = destination_for(&unknown, "x", Path::new("/s")).unwrap_err();
        assert!(matches!(err, DestinationError::UnsupportedKind { .. }));
    }
}
