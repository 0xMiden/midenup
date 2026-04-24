use std::{
    ffi::OsStr,
    fmt::Display,
    io::{Read, Write},
    path::PathBuf,
};

use anyhow::{Context, bail};
use thiserror::Error;

use crate::{
    channel::{Channel, Component, InstalledFile}, config::Config, manifest::Manifest, version::Authority
};

#[derive(Error, Debug)]
pub enum UninstallError {
    #[error("Channel {0} is not installed, nothing to uninstall.")]
    ChannelNotInstalled(semver::Version),
    #[error("Could not find installation-successful or .installation-in-progress at {0}")]
    MissingInstalledComponentsFile(PathBuf),
    #[error("Could not find channel.json file at: {0}. {1}")]
    ChannelJsonMissing(PathBuf, String),
    #[error("Ill-formed channel.json at: {0}. Contents: {1}. {2}")]
    IllFormedChannelJson(PathBuf, String, String),
    #[error("Couldn't delete file at: {0}. {1}")]
    FailedToDeleteFile(PathBuf, String),
    #[error("Couldn't delete directory at: {0}. {1}")]
    FailedToDeleteDirectory(PathBuf, std::io::Error),
    #[error(transparent)]
    Cargo(#[from] CargoError),
}

#[derive(Error, Debug)]
pub enum CargoError {
    #[error("Failed to uninstall package: {0}, with status: {1}. {2}")]
    FailedToUninstallPackage(String, i32, String),
    #[error("Internal cargo error: {0}")]
    InternalCargoError(String),
}

pub fn uninstall(
    config: &Config,
    upstream_channel: &Channel,
    local_manifest: &mut Manifest,
) -> anyhow::Result<()> {
    // TODO (fabrio): should we remove the channel from the manifest ASAP?
    let local_channel = local_manifest.get_channel_by_name(&upstream_channel.name)
        .context(format!("Channel {} is not installed, nothing to uninstall.", upstream_channel.name))?;

    // NOTE: If either of the installed components files are missing, we continue with the uninstall
    // process regardless. All the installed components and additional files are going to get
    // deleted by remove_dir_all.
    match uninstall_channel(config, &local_channel) {
        Ok(()) => (),
        Err(UninstallError::MissingInstalledComponentsFile(path)) => {
            println!(
                "WARNING: Could not find installation-successful or .installation-in-progress at \
                 {}.
Uninstallation will procede by deleting toolchain manually, instead of going through cargo.\n",
                path.display()
            )
        },
        Err(err) => bail!("Failed to uninstall {err}"),
    }

    let installed_toolchains_dir = config.midenup_home.join("toolchains");
    // Now that the installation indicator is deleted, we can remove the symlink. If anything goes
    // wrong during this process, re-issuing the installation should brink the symlink back.
    {
        let toolchain_dir = installed_toolchains_dir.join(format!("{}", &local_channel.name));
        let stable_symlink = installed_toolchains_dir.join("stable");

        // Only remove the stable symlink if it actually points to the toolchain being uninstalled.
        // This prevents removing a symlink that was just created for a migrated channel.
        let symlink_points_to_this_channel = std::fs::read_link(&stable_symlink)
            .ok()
            .map(|target| target == toolchain_dir)
            .unwrap_or(false);

        if symlink_points_to_this_channel {
            std::fs::remove_file(stable_symlink).context("Couldn't remove symlink")?;
        }
    }

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

    Ok(())
}

#[derive(Debug)]
enum InstallationMethod {
    Cargo,
    Artifact
}
// Returns how the component got installed
// NOTE: Only intended to be used for Executable
// Components that came from a Local Manifest.
fn figure_installation_method_out(comp: &Component) -> InstallationMethod {
    if !comp.artifacts.is_none() {
        InstallationMethod::Artifact
    } else {
        InstallationMethod::Cargo
    }
}

// pub fn uninstall_executable(
//     name: impl AsRef<OsStr> + Display,
//     root_dir: impl AsRef<OsStr>,
// ) -> Result<(), UninstallError> {
//     todo!()
// }

pub fn uninstall_executable(
    name: impl AsRef<OsStr> + Display,
    root_dir: impl AsRef<OsStr>,
) -> Result<(), CargoError> {
    let mut remove_exe = std::process::Command::new("cargo")
        .arg("uninstall")
        .arg(&name)
        .arg("--root")
        .arg(&root_dir)
        .stderr(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .spawn()
        .map_err(|err| CargoError::InternalCargoError(err.to_string()))?;

    let status = remove_exe
        .wait()
        .map_err(|err| CargoError::InternalCargoError(err.to_string()))?;

    if !status.success() {
        let error = remove_exe.stderr.take();

        let error_msg = if let Some(mut error) = error {
            let mut stderr_msg = String::new();
            let read_err_msg = error.read_to_string(&mut stderr_msg);

            if read_err_msg.is_err() {
                String::from("")
            } else {
                format!("The following error was raised: {stderr_msg}")
            }
        } else {
            String::from("")
        };

        return Err(CargoError::FailedToUninstallPackage(
            name.to_string(),
            status.code().unwrap_or(1),
            error_msg,
        ));
    }

    Ok(())
}

// /// Uninstalls a [Component] that was installed via an artifact
// fn uninstall_binary_artifact(config: &Config, local_channel: &Channel, component: Component) {
// }

fn uninstall_channel(config: &Config, local_channel: &Channel) -> Result<(), UninstallError> {
    let installed_toolchains_dir = config.midenup_home.join("toolchains");

    let toolchain_dir = installed_toolchains_dir.join(format!("{}", &local_channel.name));
    if !toolchain_dir.exists() {
        return Err(UninstallError::ChannelNotInstalled(local_channel.name.clone()));
    };
    let lib_dir = toolchain_dir.join("lib");
    let bin_dir = toolchain_dir.join("bin");

    for comp in local_channel.components.iter() {
        match comp.get_installed_file() {
            InstalledFile::Executable { binary_name, ..} => {
                let installation_method = figure_installation_method_out(comp);
                match installation_method {
                    InstallationMethod::Artifact => {
                        let artifact_path = bin_dir.join(binary_name);
                        std::fs::remove_file(artifact_path).unwrap();
                    },
                    InstallationMethod::Cargo => {
                        // Even if uninstallation fails, we will remove the
                        // entire directory regardless.
                        let _uninstall_result = match &comp.version {
                            Authority::Cargo { package, .. } => {
                                let package_name = package.as_deref()
                                    .unwrap_or(comp.name.as_ref());
                                uninstall_executable(package_name, &toolchain_dir)
                            },
                            Authority::Git { crate_name, .. } => {
                                uninstall_executable(crate_name, &toolchain_dir)
                            },
                            Authority::Path { crate_name, .. } => {
                                uninstall_executable(crate_name, &toolchain_dir)
                            },
                        }?;
                    },
                }
            },
            InstalledFile::Library {..} => {
                let lib_path = lib_dir.join(comp.name.as_ref()).with_extension("masp");
                std::fs::remove_file(&lib_path)
                    .map_err(|err| UninstallError::FailedToDeleteFile(lib_path, err.to_string()))?;
            }
        }
    }

    // We now remove the toolchain directory with all the remaining files.
    std::fs::remove_dir_all(&toolchain_dir)
        .map_err(|e| UninstallError::FailedToDeleteDirectory(toolchain_dir, e))?;

    // let installed_components_path = {
    //     let installed_successfully = toolchain_dir.join("installation-successful");
    //     let installation_in_progress = toolchain_dir.join(".installation-in-progress");
    //     if installed_successfully.exists() {
    //         installed_successfully
    //     } else if installation_in_progress.exists() {
    //         // If this file exists, it means that installation got cut off before finishing.
    //         // In this case, we simply delete the components that managed to get installed.
    //         installation_in_progress
    //     } else {
    //         // If neither of those files are present, then we will rely on remove_dir_all to handle
    //         // deletion
    //         return Err(UninstallError::MissingInstalledComponentsFile(
    //             toolchain_dir.to_path_buf(),
    //         ));
    //     }
    // };

    // // This is the channel.json at the time of installation. We use this to reconstruct the
    // // Component struct and thus figure out how the component was installed, i.e git, cargo, path.
    // let channel_content_path = toolchain_dir.join(".installed_channel.json");
    // let channel_content = std::fs::read_to_string(&channel_content_path).map_err(|err| {
    //     UninstallError::ChannelJsonMissing(channel_content_path.clone(), err.to_string())
    // })?;

    // let channel = serde_json::from_str::<Channel>(&channel_content).map_err(|err| {
    //     UninstallError::IllFormedChannelJson(channel_content_path, channel_content, err.to_string())
    // })?;

    // // We check the existance above
    // let components: Vec<&Component> = std::fs::read_to_string(&installed_components_path)
    //     .unwrap()
    //     .lines()
    //     .map(String::from)
    //     .map(|channel_name| channel.get_component(channel_name))
    //     .collect::<Option<Vec<&Component>>>()
    //     .expect("Couldn't find installed component in channel");

    // // Right after reading the components list, we delete the file. This way, if anything goes wrong
    // // during uninstallation, a user can simply re-install to get back to a "stable" state.
    // //
    // // NOTE: We are ignoring errors when deleting this file, since it will (hopefully) get deleted
    // // at the end of this function.
    // let _ = std::fs::remove_file(installed_components_path);

    // let (installed_libraries, installed_executables): (Vec<&Component>, Vec<&Component>) =
    //     components
    //         .iter()
    //         .partition(|c| matches!(c.get_installed_file(), InstalledFile::Library { .. }));

    // for lib in installed_libraries {
    //     let lib_path = toolchain_dir.join("lib").join(lib.name.as_ref()).with_extension("masp");
    //     std::fs::remove_file(&lib_path)
    //         .map_err(|err| UninstallError::FailedToDeleteFile(lib_path, err.to_string()))?;
    // }

    // for exe in installed_executables {
    //     match &exe.version {
    //         Authority::Cargo { package, .. } => {
    //             let package_name = package.as_deref().unwrap_or(exe.name.as_ref());
    //             uninstall_executable(package_name, toolchain_dir)?;
    //         },
    //         Authority::Git { crate_name, .. } => {
    //             uninstall_executable(crate_name, toolchain_dir)?;
    //         },
    //         Authority::Path { crate_name, .. } => {
    //             uninstall_executable(crate_name, toolchain_dir)?;
    //         },
    //     }
    // }

    Ok(())
}

