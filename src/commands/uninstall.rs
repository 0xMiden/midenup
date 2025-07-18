use std::{
    ffi::OsStr,
    fmt::Display,
    io::{Read, Write},
};

use anyhow::{Context, anyhow, bail};

use crate::{
    Config,
    channel::{Channel, Component},
    commands::install::DEPENDENCIES,
    manifest::Manifest,
    version::Authority,
};

pub fn uninstall(
    config: &Config,
    channel: &Channel,
    local_manifest: &mut Manifest,
) -> anyhow::Result<()> {
    let installed_toolchains_dir = config.midenup_home.join("toolchains");

    let toolchain_dir = installed_toolchains_dir.join(format!("{}", &channel.name));

    let installed_components_path = {
        let installed_successfully = toolchain_dir.join("installation-successful");
        let installation_in_progress = toolchain_dir.join(".installation-in-progress");

        if installed_successfully.exists() {
            Some(installed_successfully)
        } else if installation_in_progress.exists() {
            // If this file exists, it means that installation got cut off
            // before finishing.  In this case, we simply delete the components
            // that managed get installed.
            Some(installation_in_progress)
        } else {
            None
        }
    }
    .ok_or(anyhow!(
        "Neither installation-successful nor .installation-in-progress files were found in {}",
        toolchain_dir.display()
    ))?;

    // This is the channel.json at the time of installation. We use this to
    // reconstruct the Component struct and thus figure out how the component
    // was installed, i.e git, cargo, path.
    let channel_content_path = toolchain_dir.join(".installed_channel.json");
    let channel_content = std::fs::read_to_string(&channel_content_path).context(format!(
        "Couldn't read channel.json file in {}",
        channel_content_path.display()
    ))?;
    let channel = serde_json::from_str::<Channel>(&channel_content).context(format!(
        "Ill-formed channel.json in {}.
Contents: {}",
        channel_content_path.display(),
        channel_content
    ))?;

    // We check the existance above
    let components: Vec<&Component> = std::fs::read_to_string(&installed_components_path)
        .unwrap()
        .lines()
        .map(String::from)
        .map(|channel_name| channel.get_component(channel_name))
        .collect::<Option<Vec<&Component>>>()
        .expect("Couldn't find installed component in channel");

    // Right after reading the components list, we delete the file. This way, if
    // anything goes wrong during uninstallation, a user can simply re-install
    // to get back to a "stable" state.
    // NOTE: We are ignoring errors when deleting this file, since it will
    // (hopefully) get deleted at the end of this function.
    let _ = std::fs::remove_file(installed_components_path);

    // Now that the installation indicator is deleted, we can remove the
    // symlink. If anything goes wrong during this process, re-issuing the
    // installation should brink the symlink back.
    if config.manifest.is_latest_stable(&channel) {
        let stable_symlink = installed_toolchains_dir.join("stable");

        // If the symlink doesn't exist, then it probably means that
        // installation got cut off mid way through.
        if stable_symlink.exists() {
            std::fs::remove_file(stable_symlink).context("Couldn't remove symlink")?;
        }
    }
    let libs = DEPENDENCIES;
    let (installed_libraries, installed_executables): (Vec<&Component>, Vec<&Component>) =
        components.iter().partition(|c| libs.contains(&(c.name.as_ref())));

    for lib in installed_libraries {
        let lib_path = toolchain_dir.join("lib").join(lib.name.as_ref()).with_extension("masp");
        std::fs::remove_file(&lib_path)
            .context(format!("Couldn't delete {}", &lib_path.display()))?;
    }

    for exe in installed_executables {
        match &exe.version {
            Authority::Cargo { package, .. } => {
                let package_name = package.as_deref().unwrap_or(exe.name.as_ref());
                uninstall_executable(package_name, &toolchain_dir)?;
            },
            Authority::Git { crate_name, .. } => {
                uninstall_executable(crate_name, &toolchain_dir)?;
            },
            Authority::Path { crate_name, .. } => {
                uninstall_executable(crate_name, &toolchain_dir)?;
            },
        }
    }

    local_manifest.remove_channel(channel.name);

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

    // We now remove the toolchain directory with all the remaining files.
    std::fs::remove_dir_all(toolchain_dir).unwrap();
    Ok(())
}

pub fn uninstall_executable(
    name: impl AsRef<OsStr> + Display,
    root_dir: impl AsRef<OsStr>,
) -> anyhow::Result<()> {
    let mut remove_exe = std::process::Command::new("cargo")
        .arg("uninstall")
        .arg(&name)
        .arg("--root")
        .arg(&root_dir)
        .stderr(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .spawn()
        .with_context(|| format!("failed to uninstall {name} via cargo"))?;

    let status = remove_exe
        .wait()
        .context(format!("Error occurred while waiting to uninstall {name}",))?;

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

        bail!(
            "midenup failed to uninstall package {} with status {}. {}",
            name,
            status.code().unwrap_or(1),
            error_msg
        )
    }

    Ok(())
}
