use std::io::Write;

use anyhow::Context;

use crate::{
    Config, bail,
    channel::{Channel, ChannelAlias},
    config::ensure_midenup_home_exists,
    manifest::Manifest,
    utils,
    version::{Authority, GitTarget},
};

pub const DEPENDENCIES: [&str; 2] = ["std", "base"];

pub const INSTALLABLE_COMPONENTS: [&str; 4] = ["vm", "midenc", "miden-client", "cargo-miden"];

/// Installs a specified toolchain by channel or version.
pub fn install(
    config: &Config,
    channel: &Channel,
    local_manifest: &mut Manifest,
) -> anyhow::Result<()> {
    ensure_midenup_home_exists(config)?;

    let installed_toolchains_dir = config.midenup_home.join("toolchains");
    let toolchain_dir = installed_toolchains_dir.join(format!("{}", &channel.name));

    // NOTE: The installation indicator is only created after successful
    // toolchain installation.
    let installation_indicator = toolchain_dir.join("installation-successful");
    if installation_indicator.exists() {
        bail!("the '{}' toolchain is already installed", &channel.name);
    }

    if !toolchain_dir.exists() {
        std::fs::create_dir_all(&toolchain_dir).with_context(|| {
            format!("failed to create toolchain directory: '{}'", toolchain_dir.display())
        })?;
    }

    let install_file_path = toolchain_dir.join("install").with_extension("rs");
    // NOTE: Even when performing an update, we still need to re-generate the
    // install script.  This is because, the versions that will be installed are
    // written directly into the file; so the file can't be "re-used".
    let mut install_file = std::fs::File::create(&install_file_path).with_context(|| {
        format!("failed to create file for install script at '{}'", install_file_path.display())
    })?;

    let install_script_contents = generate_install_script(channel);
    install_file.write_all(&install_script_contents.into_bytes()).with_context(|| {
        format!("failed to write install script at '{}'", install_file_path.display())
    })?;

    let mut child = std::process::Command::new("cargo")
        .env("MIDEN_SYSROOT", &toolchain_dir)
        // HACK(pauls): This is for the benefit of the compiler, until it moves to using
        // MIDEN_SYSROOT instead.
        .env("MIDENC_SYSROOT", &toolchain_dir)
        .args(["+nightly", "-Zscript"])
        .arg(&install_file_path)
        .stderr(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .spawn()
        .context("error occurred while running install script")?;

    let status = child
        .wait()
        .context(format!("Error occurred while waiting to install {}", channel.name))?;

    if !status.success() {
        bail!(
            "midenup failed to install toolchan from channel {} with status {}",
            channel.name,
            status.code().unwrap_or(1)
        )
    }

    let is_latest_stable = config.manifest.is_latest_stable(channel);

    // If this channel is the new stable, we update the symlink
    if is_latest_stable {
        // NOTE: This is an absolute file path, maybe a relative symlink would be more
        // suitable
        let stable_dir = installed_toolchains_dir.join("stable");
        if stable_dir.exists() {
            std::fs::remove_file(&stable_dir).context("Couldn't remove stable symlink")?;
        }
        utils::symlink(&stable_dir, &toolchain_dir).expect("Couldn't create stable dir");
    }

    // Update local manifest
    // -------------------------------------------------------------------------
    let local_manifest_path = config.midenup_home.join("manifest").with_extension("json");
    {
        // Check if the installed channel needs to marked as stable
        let mut channel_to_save = if is_latest_stable {
            let mut modifiable = channel.clone();
            modifiable.alias = Some(ChannelAlias::Stable);
            modifiable
        } else {
            channel.clone()
        };

        // If a component was installed with --branch, then write down the
        // current commit. This is used on updates to check if any new commits
        // were pushed since installation.
        // NOTE: To check the latest commit we're using git cli instead. Would
        // it be prefereable to use git-rs instead?
        for component in channel_to_save.components.iter_mut() {
            if let Authority::Git {
                repository_url,
                crate_name,
                // NOTE: latest_revision should be None when installing.
                target: GitTarget::Branch { name, latest_revision: _ },
            } = &component.version
            {
                // If, for whatever reason, we fail to find the latest hash, we
                // simply leave it empty. That does mean that an update will be
                // triggered even if the component does not need it.
                let revision_hash = utils::find_latest_hash(repository_url, name).ok();

                component.version = Authority::Git {
                    repository_url: repository_url.clone(),
                    crate_name: crate_name.clone(),
                    target: GitTarget::Branch {
                        name: name.clone(),
                        latest_revision: revision_hash,
                    },
                }
            }
        }

        // Now that the channels have been updated, add them to the local manifest.
        local_manifest.add_channel(channel_to_save);
    }

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

/// This function generates the install script that will later be saved in
/// `midenup/toolchains/<version>/install.rs`. This file is then executed by
/// `cargo -Zscript`.
fn generate_install_script(channel: &Channel) -> String {
    // Prepare install script template
    let engine = upon::Engine::new();
    let template = engine
        .compile(
            r##"#!/usr/bin/env cargo
---cargo
[dependencies]
{%- for dep in dependencies %}
{{ dep.package }} = { version = "{{ dep.version }}"
{%- if dep.git_uri %}, git = "{{ dep.git_uri }}"
{%- else if dep.path %}, path = "{{ dep.path }}"
{%- endif %} }
{%- endfor %}
---

// NOTE: This file was generated by midenup. Do not edit by hand

use std::process::Command;
use std::io::{Write, Read};
use std::fs::{OpenOptions, rename};

fn main() {
    // MIDEN_SYSROOT is set by `midenup` when invoking this script, and will contain the resolved
    // (and prepared) sysroot path to which this script will install the desired toolchain
    // components.
    let miden_sysroot_dir = std::path::Path::new(env!("MIDEN_SYSROOT"));
    let lib_dir = miden_sysroot_dir.join("lib");

    // We save the state the channel was in when installed. This is used when uninstalling.
    let channel_json = r#"{{ channel_json }}"#;
    let channel_json_path = miden_sysroot_dir.join(".installed_channel.json");
    let mut installed_json = std::fs::File::create(channel_json_path).expect("failed to create installation in progress file");
    installed_json.write_all(&channel_json.as_bytes()).unwrap();

    // As we install components, we write them down in this file. This is used
    // to keep track of successfully installed components in case installation
    // fails.
    let progress_path = miden_sysroot_dir.join(".installation-in-progress");
    // Done to truncate the file if it exists
    let _progress_file = std::fs::File::create(progress_path.as_path()).expect("failed to create installation in progress file");
    // We'll log which components we have successfully installed.
    let mut progress_file = OpenOptions::new()
        .append(true)
        .open(&progress_path)
        .expect("Failed to create progress file");


    // Write transaction kernel to $MIDEN_SYSROOT/lib/base.masp
    let tx = miden_lib::MidenLib::default();
    let tx_path = lib_dir.join("base").with_extension("masp");
    // NOTE: If the file already exists, then we are running an update and we
    // don't need to update this element
    if !std::fs::exists(&tx_path).expect("Can't check existence of file") {
        tx.as_ref()
            .write_to_file(&tx_path)
            .expect("failed to install Miden transaction kernel library component");
    }
    writeln!(progress_file, "base").expect("Failed to write component name to progress file");

    // Write stdlib to $MIDEN_SYSROOT/std.masp
    let stdlib = miden_stdlib::StdLibrary::default();
    let stdlib_path = lib_dir.join("std").with_extension("masp");
    if !std::fs::exists(&stdlib_path).expect("Can't check existence of file") {
        stdlib
            .as_ref()
            .write_to_file(&stdlib_path)
            .expect("failed to install Miden standard library component");
    }
    writeln!(progress_file, "std").expect("Failed to write component name to progress file");


    let bin_dir = miden_sysroot_dir.join("bin");
    {% for component in installable_components %}

    // Install {{ component.name }}
    let bin_path = bin_dir.join("{{ component.installed_file }}");
    if !std::fs::exists(&bin_path).unwrap_or(false) {
        let mut child = Command::new("cargo")
            .arg(
            "{{ component.required_toolchain_flag }}",
            )
            .arg("install")
            .args([
            {%- for arg in component.args %}
            "{{ arg }}",
            {%- endfor %}
            ])
            // Force the install target directory to be $MIDEN_SYSROOT/bin
            .arg("--root")
            .arg(&miden_sysroot_dir)
            // Spawn command
            .stderr(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .spawn()
            .expect("failed to install component '{{ component.name }}'");

        // Await results
        let status = child.wait().expect("Error occurred while waiting to install component '{{ component.name }}'");


        if !status.success() {
            let error = child.stderr.take();

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

            panic!(
                "midenup failed to uninstall '{{ component.name }}' with status {}. {}",
                status.code().unwrap_or(1),
                error_msg
            );
        }
    }
    writeln!(progress_file, "{{component.name}}").expect("Failed to write component name to progress file");

    {% endfor %}

    // Now that installation finished, we rename the file to indicate that
    // installation finished successfully.
    let checkpoint_path = miden_sysroot_dir.join("installation-successful");
    rename(progress_path, checkpoint_path).expect("Couldn't rename .installation-in-progress to installation-successful");

}
"##,
        )
        .unwrap_or_else(|err| panic!("invalid install script template: {err}"));

    // Prepare install script context with available channel components
    let mut dependencies = Vec::new();
    for dep_name in DEPENDENCIES.iter() {
        let component = channel
            .get_component(dep_name)
            .unwrap_or_else(|| panic!("{dep_name} is a required component, but isn't available"));
        dependencies.push(component);
    }

    let mut installable_components = Vec::new();
    for dep_name in INSTALLABLE_COMPONENTS.iter() {
        let component = channel
            .get_component(dep_name)
            .unwrap_or_else(|| panic!("{dep_name} is a required component, but isn't available"));
        installable_components.push(component);
    }

    // The set of cargo dependencies needed for the install script
    let dependencies = dependencies
        .into_iter()
        .map(|component| match &component.version {
            Authority::Cargo { package, version } => {
                let package = package.as_deref().unwrap_or(component.name.as_ref()).to_string();
                upon::value! {
                    package: package,
                    version: version.to_string(),
                    git_uri: "",
                    path: "",
                }
            },
            Authority::Git { repository_url, crate_name, target } => {
                upon::value! {
                    package: crate_name,
                    version: "> 0.0.0",
                    git_uri: format!("{}\", {target}", repository_url.clone()),
                    path: "",
                }
            },
            Authority::Path { crate_name, path } => {
                upon::value! {
                    package: crate_name,
                    version: "> 0.0.0",
                    git_uri: "",
                    path: path.display().to_string(),
                }
            },
        })
        .collect::<Vec<_>>();

    // The set of components to be installed with `cargo install`
    let installable_components = installable_components
        .into_iter()
        .map(|component| {
            let mut args = vec![];
            match &component.version {
                Authority::Cargo { package, version } => {
                    let package = package.as_deref().unwrap_or(component.name.as_ref());
                    args.push(package.to_string());
                    args.push("--version".to_string());
                    args.push(version.to_string());
                },
                Authority::Git{repository_url, target, crate_name} => {
                    args.push("--git".to_string());
                    args.push(repository_url.clone());
                    args.push(target.to_cargo_flag()[0].clone());
                    args.push(target.to_cargo_flag()[1].clone());
                    args.push(crate_name.clone());
                },
                Authority::Path{path, ..} => {
                    args.push("--path".to_string());
                    args.push(path.display().to_string());
                },
            }

            let required_toolchain =
                component.rustup_channel.clone().unwrap_or(String::from("stable"));

            let required_toolchain_flag = format!("+{required_toolchain}");

            // Enable optional features, if present
            if !component.features.is_empty() {
                let features = component.features.join(",");
                args.push("--features".to_string());
                args.push(features);
            };

            upon::value! {
                name: component.name.to_string(),
                installed_file: component.installed_file.clone().unwrap_or(component.name.to_string()),
                required_toolchain_flag: required_toolchain_flag,
                args: args,
            }
        })
        .collect::<Vec<_>>();

    // Render the install script
    template
        .render(
            &engine,
            upon::value! {
                dependencies: dependencies,
                installable_components: installable_components,
                channel_json : serde_json::to_string_pretty(channel).unwrap(),
            },
        )
        .to_string()
        .unwrap_or_else(|err| panic!("install script rendering failed: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{UserChannel, manifest::Manifest};

    #[test]
    fn install_script_template_from_local_manifest() {
        let manifest = Manifest::load_from("file://manifest/channel-manifest.json").unwrap();

        let channel = manifest
            .get_channel(&UserChannel::Stable)
            .expect("Could not convert UserChannel to internal channel representation");

        let script = generate_install_script(channel);

        println!("{script}");

        assert!(script.contains("// Install cargo-miden"));
    }
}
