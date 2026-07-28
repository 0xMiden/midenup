use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use thiserror::Error;

use crate::{
    artifact::InvalidArtifactError,
    channel::Channel,
    config::Config,
    manifest::{Component, ComponentKind, InstallationMethod, PackageInstallationMethod},
    state::LocalState,
};

#[derive(Error, Debug)]
pub enum UninstallError {
    #[error("Couldn't delete file at: {0}. {1}")]
    FailedToDeleteFile(PathBuf, String),
    #[error("Failed to uninstall package: {0}, with status: {1}. {2}")]
    FailedToUninstallPackage(String, i32, String),
    #[error("Internal cargo error: {0}")]
    InternalCargoError(String),
    #[error(
        "midenup failed to delete the install directory with error {0}.
         However, manual removal should be safe. The install directory's PATH is the following:
{1}"
    )]
    FailedToRemoveToolchainDirectory(String, PathBuf),
    #[error(transparent)]
    ArtifactError(#[from] InvalidArtifactError),
}

pub fn uninstall(
    config: &Config,
    upstream_channel: &Channel,
    state: &mut LocalState,
) -> anyhow::Result<()> {
    let Some(local_channel) = state.get(&upstream_channel.name).map(|i| i.as_channel()) else {
        bail!("channel {} is not installed, nothing to uninstall", upstream_channel.name);
    };

    let toolchains_dir = config.midenup_home.join("toolchains");
    let toolchain_symlink = toolchains_dir.join(format!("{}", local_channel.name));

    let installed_channel_dir = toolchain_symlink.canonicalize();

    // We begin by removing the stable symlink. If uninstallation is
    // stopped before removing the channel symlink, re-running
    // `midenup install <channel>` will restore the file.
    {
        let stable_symlink = toolchains_dir.join("stable");

        // Only remove the stable symlink if it actually points to the toolchain being uninstalled.
        // This prevents removing a symlink that was just created for a migrated channel.
        let symlink_points_to_this_channel = stable_symlink
            .canonicalize()
            .ok()
            .zip(toolchain_symlink.canonicalize().ok())
            .map(|(a, b)| a == b)
            .unwrap_or(false);

        if symlink_points_to_this_channel
            // If it doesn't exist, that probably means that there was a previous
            // uninstallation attempt that got interrumpted.
            && stable_symlink.exists()
        {
            std::fs::remove_file(stable_symlink).context("Couldn't remove symlink")?;
        }
    }

    // If cleanup is interrumpted, then `midenup clean` can be used to clean
    // stale files.
    if let Ok(installed_channel_dir) = installed_channel_dir {
        uninstall_components(&installed_channel_dir, &local_channel.components, config)?;

        // We now remove the install directory with all the remaining files.
        std::fs::remove_dir_all(&installed_channel_dir).map_err(|e| {
            UninstallError::FailedToRemoveToolchainDirectory(
                e.to_string(),
                installed_channel_dir.to_path_buf(),
            )
        })?;
    }

    // We remove the symlink, thus making the channel unaccesible.
    if toolchain_symlink.exists() {
        std::fs::remove_file(&toolchain_symlink)?;
    }

    // Removing the state record is what *really* marks the channel as uninstalled.
    {
        state.remove(&local_channel.name);
        config.write_local_state(state)?;
    }

    Ok(())
}

pub fn uninstall_components(
    install_dir: &Path,
    components: &[Component],
    config: &Config,
) -> Result<(), UninstallError> {
    for component in components {
        println!("removing previous version of component {}", component.name);
        match component.kind() {
            // Never installed by this build, so it owns no files to remove.
            ComponentKind::Unsupported { .. } => continue,
            ComponentKind::Asset | ComponentKind::Command { .. } => {
                // The artifact id is the installed filename, so uninstall must mirror install
                // exactly: `etc/<component>/<artifact-id>`. Deriving the name from the URI
                // instead can produce a path that was never written.
                let base_dir = install_dir.join("etc").join(component.name.as_ref());
                for id in component.artifacts.artifacts.keys() {
                    let file_path = base_dir.join(id);
                    if file_path.try_exists().unwrap_or(false) {
                        std::fs::remove_file(&file_path).map_err(|err| {
                            UninstallError::FailedToDeleteFile(file_path, err.to_string())
                        })?;
                    }
                }
            },
            ComponentKind::Package
            | ComponentKind::LegacyPackage {
                installation_method: PackageInstallationMethod::Prebuilt,
                ..
            } => {
                // Packages install to `lib/<artifact-id>`; the previous path omitted `lib`
                // entirely, so uninstall never removed anything.
                for (id, _uri) in
                    component.artifacts.get_artifacts_for_target(config.target(), component)?
                {
                    let file_path = install_dir.join("lib").join(id);
                    if file_path.try_exists().unwrap_or(false) {
                        std::fs::remove_file(&file_path).map_err(|err| {
                            UninstallError::FailedToDeleteFile(file_path, err.to_string())
                        })?;
                    }
                }
            },
            ComponentKind::LegacyPackage {
                installation_method: PackageInstallationMethod::Cargo { .. },
                ..
            } => {
                // Must mirror what install wrote. Deriving this from the kebab-cased crate name
                // meant uninstalling `protocol` looked for `miden-protocol.masp`.
                let file_path = install_dir.join("lib").join(
                    component
                        .installed_package_name()
                        .expect("legacy packages always resolve an installed filename"),
                );
                if file_path.try_exists().unwrap_or(false) {
                    std::fs::remove_file(&file_path).map_err(|err| {
                        UninstallError::FailedToDeleteFile(file_path, err.to_string())
                    })?;
                }
            },
            ComponentKind::CargoExtension { installation_method, spec }
            | ComponentKind::Executable { installation_method, spec } => {
                let base_dir = install_dir.join("bin");
                let opt_path = install_dir.join("opt").join(component.get_symlink_name().unwrap());
                let _ = std::fs::remove_file(&opt_path);
                let artifacts =
                    component.artifacts.get_artifacts_for_target(config.target(), component)?;
                match installation_method {
                    InstallationMethod::Prebuilt
                    | InstallationMethod::PrebuiltWithCargoFallback { .. }
                        if !artifacts.is_empty() =>
                    {
                        let file_path = base_dir.join(&spec.installed_executable);
                        if file_path.try_exists().unwrap_or(false) {
                            std::fs::remove_file(&file_path).map_err(|err| {
                                UninstallError::FailedToDeleteFile(file_path, err.to_string())
                            })?;
                        }
                    },
                    InstallationMethod::Prebuilt => (),
                    InstallationMethod::Cargo { crate_name, .. }
                    | InstallationMethod::PrebuiltWithCargoFallback { crate_name, .. } => {
                        uninstall_executable(crate_name, install_dir)?;
                    },
                }
            },
        }
    }

    Ok(())
}

pub fn uninstall_executable(name: &str, root_dir: impl AsRef<OsStr>) -> Result<(), UninstallError> {
    let output = std::process::Command::new("cargo")
        .arg("uninstall")
        .arg(name)
        .arg("--root")
        .arg(&root_dir)
        .output()
        .map_err(|err| UninstallError::InternalCargoError(err.to_string()))?;

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        // If the uninstall failed because the component is already removed, then treat it as
        // successful
        if stdout.contains(&format!("package ID specification `{name}` did not match any packages"))
        {
            return Ok(());
        }

        let mut error = String::with_capacity(stdout.len() + stderr.len());
        error.push_str("======= stdout =========\n");
        if stdout.trim().is_empty() {
            error.push_str(stdout.trim());
            error.push('\n');
        }
        error.push_str("========================\n");
        error.push_str("======= stderr =========\n");
        if stderr.trim().is_empty() {
            error.push_str(stderr.trim());
            error.push('\n');
        }
        error.push_str("========================\n");

        return Err(UninstallError::FailedToUninstallPackage(
            name.to_string(),
            output.status.code().unwrap_or(1),
            error,
        ));
    }

    Ok(())
}
