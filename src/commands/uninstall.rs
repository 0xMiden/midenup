use std::{
    ffi::OsStr,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use thiserror::Error;

use crate::{
    channel::{Channel, Component, InstalledFile},
    config::Config,
    manifest::Manifest,
    version::Authority,
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
}

pub fn uninstall(
    config: &Config,
    upstream_channel: &Channel,
    local_manifest: &mut Manifest,
) -> anyhow::Result<()> {
    let Some(local_channel) = local_manifest.get_channel_by_name(&upstream_channel.name).cloned()
    else {
        bail!(
            "Channel {} is not in the local manifest, nothing to uninstall.",
            upstream_channel.name
        );
    };

    let toolchains_dir = config.midenup_home.join("toolchains");
    let toolchain_symlink = toolchains_dir.join(format!("{}", &local_channel.name));

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
        uninstall_components(&installed_channel_dir, &local_channel.components)?;

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

    // We remove the channel from the local manifest.
    // This is what *REALLY* marks the channel as uninstalled.
    {
        local_manifest.remove_channel(local_channel.name.clone());

        let local_manifest_path = config.midenup_home.join("manifest").with_extension("json");
        let mut local_manifest_file =
            std::fs::File::create(&local_manifest_path).with_context(|| {
                format!(
                    "failed to create file for install script at '{}'",
                    local_manifest_path.display()
                )
            })?;
        local_manifest_file
            .write_all(
                serde_json::to_string_pretty(&local_manifest)
                    .context("Couldn't serialize local manifest")?
                    .as_bytes(),
            )
            .context("Couldn't create local manifest file")?;
    }

    Ok(())
}

pub fn uninstall_components(
    install_dir: &Path,
    components: &[Component],
) -> Result<(), UninstallError> {
    let (installed_libraries, installed_executables): (Vec<&Component>, Vec<&Component>) =
        components
            .iter()
            .partition(|c| matches!(c.get_installed_file(), InstalledFile::Library { .. }));

    for lib in installed_libraries {
        println!("removing previous version of component {}", &lib.name);
        let lib_path = install_dir.join("lib").join(lib.name.as_ref()).with_extension("masp");
        // Only remove the file if it exists - treat inability to determine existence as
        // non-existent
        if lib_path.try_exists().unwrap_or(false) {
            std::fs::remove_file(&lib_path)
                .map_err(|err| UninstallError::FailedToDeleteFile(lib_path, err.to_string()))?;
        }
    }

    for exe in installed_executables {
        println!("removing previous version of component {}", &exe.name);
        let opt_path = install_dir.join("opt").join(exe.get_symlink_name());
        let _ = std::fs::remove_file(&opt_path);

        // Artifacts are only stored in the local manifest if the component was
        // *actually* installed via it.
        if exe.artifacts.is_some() {
            let bin_path = exe.get_installed_file().get_path_from(install_dir);
            // Only remove the file if it exists - treat inability to determine existence as
            // non-existent
            if bin_path.try_exists().unwrap_or(false) {
                std::fs::remove_file(&bin_path)
                    .map_err(|err| UninstallError::FailedToDeleteFile(bin_path, err.to_string()))?;
            }
        } else {
            match &exe.version {
                Authority::Cargo { package, .. } => {
                    let package_name = package.as_deref().unwrap_or(exe.name.as_ref());
                    uninstall_executable(package_name, install_dir)?;
                },
                Authority::Git { crate_name, .. } => {
                    uninstall_executable(crate_name, install_dir)?;
                },
                Authority::Path { crate_name, .. } => {
                    uninstall_executable(crate_name, install_dir)?;
                },
            }
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
