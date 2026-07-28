//! Turning a resolved component set into an exact, target-specific installation plan.

use std::path::Path;

use crate::manifest::{Channel, ComponentKind, InstallationMethod, PackageInstallationMethod};

pub mod destination;
pub mod key;

pub use self::{
    destination::{
        Destination, DestinationError, InvalidArtifactId, MODE_DATA, MODE_EXECUTABLE,
        destination_for, validate_artifact_id,
    },
    key::{ComponentInputs, KeyInputs, PlanKey, compute as compute_plan_key},
};

/// Derives [KeyInputs] for `channel` on `target`.
///
/// This is the bridge from manifest data to the canonical key. Note what it deliberately does not
/// read: profiles, aliases, call formats, subcommands, `initialization`, or the channel alias.
/// None of those change a byte on disk -- they are resolved at dispatch time from local state.
/// `opt/` symlinks are files, so they are included.
pub fn key_inputs_for_channel(
    channel: &Channel,
    target: &str,
    sysroot: &Path,
) -> anyhow::Result<KeyInputs> {
    let mut components = Vec::with_capacity(channel.components.len());

    for component in channel.components.iter() {
        if !component.is_supported() {
            // Unsupported components are never installed, so they contribute nothing.
            continue;
        }

        let (crate_name, features, rustup_channel, method) = match component.kind() {
            ComponentKind::Executable { installation_method, .. }
            | ComponentKind::CargoExtension { installation_method, .. } => {
                match installation_method {
                    InstallationMethod::Prebuilt => (None, None, None, "prebuilt"),
                    InstallationMethod::PrebuiltWithCargoFallback {
                        crate_name,
                        rustup_channel,
                        features,
                    } => (
                        Some(crate_name.clone()),
                        Some(features.clone()),
                        rustup_channel.clone(),
                        "prebuilt-with-cargo-fallback",
                    ),
                    InstallationMethod::Cargo { crate_name, rustup_channel, features } => (
                        Some(crate_name.clone()),
                        Some(features.clone()),
                        rustup_channel.clone(),
                        "cargo",
                    ),
                }
            },
            ComponentKind::LegacyPackage { installation_method, .. } => match installation_method {
                PackageInstallationMethod::Prebuilt => (None, None, None, "prebuilt"),
                PackageInstallationMethod::Cargo { crate_name, features, extractor } => {
                    // The extractor is source code compiled into the install script, so it is
                    // every bit as material as a crate version.
                    (
                        Some(format!("{crate_name}#{extractor}")),
                        Some(features.clone()),
                        None,
                        "cargo",
                    )
                },
            },
            _ => (None, None, None, "n/a"),
        };

        let artifacts: Vec<(String, String)> = component
            .artifacts
            .get_default_artifacts_for_target(target, component)?
            .into_iter()
            .map(|(id, uri)| (id.to_string(), uri.to_string()))
            .collect();

        let mut destinations = Vec::with_capacity(artifacts.len());
        for (id, _) in artifacts.iter() {
            let destination = destination_for(component, id, sysroot)?;
            destinations.push((destination.path.display().to_string(), destination.mode));
        }

        let symlinks = match (component.get_symlink_name(), component.kind()) {
            (
                Some(name),
                ComponentKind::Executable { spec, .. } | ComponentKind::CargoExtension { spec, .. },
            ) => vec![(name, spec.installed_executable.clone())],
            _ => vec![],
        };

        components.push(ComponentInputs {
            name: component.name.to_string(),
            authority: component.version.to_string(),
            kind: component.kind().tag().to_string(),
            installation_method: method.to_string(),
            artifacts,
            destinations,
            crate_name,
            features,
            rustup_channel,
            symlinks,
        });
    }

    Ok(KeyInputs { target: target.to_string(), components })
}
