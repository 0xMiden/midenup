use std::{
    collections::HashSet,
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, bail};

use crate::{
    artifact::TargetTriple,
    channel::{Channel, ChannelAlias, InstalledFile},
    commands,
    config::Config,
    manifest::Manifest,
    options::InstallationOptions,
    utils,
    version::{Authority, GitTarget},
};

/// Installs a specified toolchain by channel or version.
pub fn install(
    config: &Config,
    channel: &Channel,
    local_manifest: &mut Manifest,
    options: &InstallationOptions,
) -> anyhow::Result<()> {
    commands::setup_midenup(config, local_manifest)?;

    let toolchains_dir = config.midenup_home.join("toolchains");
    let toolchain_dir = toolchains_dir.join(format!("{}", &channel.name));

    let installed_toolchains_dir = config.midenup_home.join("installed_toolchains");
    let install_dir_name = format!("{}-{}", &channel.name, channel.content_hash());
    let install_dir = installed_toolchains_dir.join(&install_dir_name);

    // Relative path to the newly installed channel directory.
    let relative_install_target =
        PathBuf::from("..").join("installed_toolchains").join(&install_dir_name);

    // If the install directory already exists; then that means we are re-issuing
    // an install. That's probably because the installation got interrumpted
    // mid way through.
    if !install_dir.exists() {
        std::fs::create_dir_all(&install_dir).with_context(|| {
            format!("failed to create install directory: '{}'", install_dir.display())
        })?;
        // If a previous install of this channel exists, reuse the components.
        // For more context behind this, see the [[update_channel]] function
        // documentation.
        if toolchain_dir.exists() {
            utils::fs::copy_dir_recursive(&toolchain_dir, &install_dir, &[]).with_context(
                || {
                    format!(
                        "failed to seed install directory '{}' from previous install at '{}'",
                        install_dir.display(),
                        toolchain_dir.display()
                    )
                },
            )?;

            commands::uninstall::uninstall_components(
                &install_dir,
                &options.components_to_uninstall,
            )?;
        }
    }

    let bin_dir = install_dir.join("bin");
    if !bin_dir.exists() {
        std::fs::create_dir_all(&bin_dir).with_context(|| {
            format!("failed to create toolchain directory: '{}'", bin_dir.display())
        })?;
    }

    // `lib/` directory which holds MASP libraries.
    let lib_dir = install_dir.join("lib");
    if !lib_dir.exists() {
        std::fs::create_dir_all(&lib_dir).with_context(|| {
            format!("failed to create toolchain directory: '{}'", lib_dir.display())
        })?;
    }

    // `opt/` directory which holds symlinks to binaries in `bin/`.
    //
    // These are used in order to preserve a "midenup" compatible interface. This relies on the fact
    // that clap uses argv[0] in order to display executable names names. These symlinks have the
    // following format: `miden <component name>`
    //
    // Then, when `miden` is invoked, it uses these symlinks to execute the underlying binary. With
    // this setup, `clap` displays the name as: `miden <component name>` instead of just
    // `binary_name` when displaying help messages.
    let opt_dir = install_dir.join("opt");
    if !opt_dir.exists() {
        std::fs::create_dir_all(&opt_dir).with_context(|| {
            format!("failed to create toolchain directory: '{}'", opt_dir.display())
        })?;
    }

    // NOTE: Even when performing an update, we still need to re-generate the install script.
    // This is because, the versions that will be installed are written directly into the file; so
    // the file can't be "re-used".
    let install_file_path = install_dir.join("install").with_extension("rs");
    let mut install_file = std::fs::File::create(&install_file_path).with_context(|| {
        format!("failed to create file for install script at '{}'", install_file_path.display())
    })?;

    let install_script_contents = generate_install_script(config, channel, options, &install_dir);
    install_file.write_all(&install_script_contents.into_bytes()).with_context(|| {
        format!("failed to write install script at '{}'", install_file_path.display())
    })?;

    let mut child = std::process::Command::new("cargo")
        .env("MIDEN_SYSROOT", &install_dir)
        // HACK(pauls): This is for the benefit of the compiler, until it moves to using
        // MIDEN_SYSROOT instead.
        .env("MIDENC_SYSROOT", &install_dir)
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
            "midenup failed to install toolchain from channel {} with status {}",
            channel.name,
            status.code().unwrap_or(1)
        )
    }

    let temp_symlink = installed_toolchains_dir.join(format!("{}.new", &channel.name));
    if std::fs::symlink_metadata(&temp_symlink).is_ok() {
        std::fs::remove_file(&temp_symlink).with_context(|| {
            format!("failed to remove stale temp symlink '{}'", temp_symlink.display())
        })?;
    }

    // ======================== Installation finalized  ===========================

    // tmp_link is a symlink file that points to relative_install_target. Even
    // if tmp_link file is moved, it will still point to relative_install_target.
    // For further reference on atomic directory updates, see:
    // https://axialcorps.wordpress.com/2013/07/03/atomically-replacing-files-and-directories/
    utils::fs::symlink(&temp_symlink, &relative_install_target)?;

    // We now rename tmp_link to toolchain_dir. When renamed, it will still be
    // pointing to relative_install_target. If the channel directory existed, it
    // will overwrite the file. This is what marks the install as completed.
    std::fs::rename(&temp_symlink, &toolchain_dir).with_context(|| {
        format!(
            "failed to publish toolchain symlink '{}' -> '{}'",
            toolchain_dir.display(),
            relative_install_target.display()
        )
    })?;

    let is_latest_stable = config.manifest.is_latest_stable(channel);

    // If this channel is the new stable, we update the symlink
    if is_latest_stable {
        let stable_dir = toolchains_dir.join("stable");
        if stable_dir.exists() {
            std::fs::remove_file(&stable_dir).context("Couldn't remove stable symlink")?;
        }
        let relative_channel_target = PathBuf::from(format!("{}", &channel.name));
        utils::fs::symlink(&stable_dir, &relative_channel_target)
            .expect("Couldn't create stable dir");
    }

    // Update local manifest
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

        // We determine how the component got installed.
        // A component could have been installed either by cargo install (i.e. "from
        // source") or via a pre-compiled miden-provided binary artifact.
        // We can only *truly* determine how it got installed after the fact.
        let cargo_installed_binaries = get_installed_cargo_binaries(toolchain_dir)?;

        for component in channel_to_save.components.iter_mut() {
            match &component.version {
                #[allow(clippy::collapsible_match)]
                Authority::Git { repository_url, crate_name, target } => {
                    #[allow(clippy::single_match)]
                    match target {
                        // If a component was installed with --branch, then
                        // write down the current commit.  This is used on
                        // updates to check if any new commits were pushed since
                        // installation.
                        GitTarget::Branch { name, latest_revision: _ } => {
                            // If, for whatever reason, we fail to find the latest hash, we simply
                            // leave it empty. That does mean that an
                            // update will be triggered even if the component
                            // does not need it.
                            let revision_hash =
                                utils::git::find_latest_hash(repository_url, name).ok();

                            component.version = Authority::Git {
                                repository_url: repository_url.clone(),
                                crate_name: crate_name.clone(),
                                target: GitTarget::Branch {
                                    name: name.clone(),
                                    latest_revision: revision_hash,
                                },
                            }
                        },
                        _ => {},
                    }
                },
                Authority::Path { path, crate_name, last_modification: _ } => {
                    // If a component was installed with --path, then write down the latest
                    // modification time found inside the directory (or the current time as a
                    // fallback). This is used on updates to check if anything changed.
                    let latest_time = utils::fs::latest_modification(path)
                        .ok()
                        .map(|(latest_modification, _)| latest_modification)
                        .unwrap_or(SystemTime::now());
                    component.version = Authority::Path {
                        path: path.clone(),
                        crate_name: crate_name.clone(),
                        last_modification: Some(latest_time),
                    }
                },
                Authority::Cargo { package, .. } => {
                    // If a component is marked with Cargo as an authority and
                    // also has artifacts listed as available, determine which
                    // got used for the installation.
                    //
                    // Currently, by convention, if a component has an artifacts
                    // field listed on the *LOCAL* manifest, then that means
                    // that artifacts were used.
                    if component.get_artifact_uri(&config.target).is_none() {
                        continue;
                    }

                    let package = package.as_deref().unwrap_or(component.name.as_ref()).to_string();

                    let installed_via_cargo = cargo_installed_binaries.contains(package.as_str());

                    // TODO (fabrio): Unify this in the local manifest, I don't
                    // believe there really is a need to store both the artifact
                    // and authority fields.  We could only store the field that
                    // was actually used.
                    if installed_via_cargo {
                        // This means that the component had an artifacts entry,
                        // yet it was not utilized. While rare, this can happen
                        // due to a number of factors, such as: no artifact for
                        // this system's triple or Github being offline (with
                        // the latter becoming more likely).
                        component.artifacts = None;
                    }
                },
            }
        }

        // Now that the channels have been updated, add them to the local manifest.
        local_manifest.add_channel(channel_to_save);
    }

    let mut local_manifest_file =
        std::fs::File::create(&local_manifest_path).with_context(|| {
            format!(
                "failed to create file for local manifest at '{}'",
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
/// `midenup/toolchains/<version>/install.rs`.
///
/// This file is then executed by `cargo -Zscript`.
fn generate_install_script(
    config: &Config,
    channel: &Channel,
    options: &InstallationOptions,
    toolchain_directory: &Path,
) -> String {
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
curl = "{{ curl_version }}"
---

// NOTE: This file was generated by midenup. Do not edit by hand

{{ install_artifact.function }}

// Utility functions
mod utility {
    #[cfg(unix)]
    pub fn symlink(from: &std::path::Path, to: &std::path::Path) {
        std::os::unix::fs::symlink(to, from).expect("could not create symlink")
    }

    #[cfg(windows)]
    pub fn symlink(from: &std::path::Path, to: &std::path::Path) {
        std::os::windows::fs::symlink_file(to, from).expect("could not create symlink")
    }
}

fn main() {
    // MIDEN_SYSROOT is set by `midenup` when invoking this script, and will contain the resolved
    // (and prepared) sysroot path to which this script will install the desired toolchain
    // components.
    let miden_sysroot_dir = std::path::Path::new(env!("MIDEN_SYSROOT"));

    let padding = "    ";

    // Install libraries
    let lib_dir = miden_sysroot_dir.join("lib");
    {
        {% for dep in dependencies %}
        println!("Installing: {{ dep.name }}.masp");

        // Write library to $MIDEN_SYSROOT/lib/dep.masp
        let lib = {{ dep.exposing_function }};
        let lib_path = lib_dir.join("{{ dep.name }}").with_extension("masp");
        // NOTE: If the file already exists, then we are running an update and we
        // don't need to update this element
        if !std::fs::exists(&lib_path).expect("Can't check existence of file") {
            let do_fetch_artifact: bool;
            let mut do_install_from_source: bool;
            let mut successfully_installed = false;
            let initial_message: String;

            if !"{{ dep.artifact.0 }}".is_empty() {
                do_fetch_artifact = true;
                do_install_from_source = false;
                initial_message = format!("{} Fetching artifact", padding);
            } else {
                do_fetch_artifact = false;
                do_install_from_source = true;
                initial_message = format!("{} No artifact found. Proceeding to install from source", padding);
            }

            println!("{initial_message}");
            if do_fetch_artifact {
                if let Err(err) = install_artifact("{{ dep.artifact.0 }}", std::path::Path::new("{{ dep.artifact.1 }}")) {
                    println!("{} {err}.", padding);
                    println!("{} Proceeding to install from source.", padding);
                    do_install_from_source = true;
                } else {
                    successfully_installed = true;
                }
            }

            if do_install_from_source {
                if let Err(err) = lib.as_ref().write_to_file(&lib_path) {
                    if {{ keep_going }} {
                            println!("{} Failed to install '{{ dep.name }}' from source because of {err}. Skipping.", padding);
                    } else {
                            panic!("Failed to install '{{ dep.name }}' from source because of {err}.");
                    }
                } else {
                    successfully_installed = true;
                }
            }

            if successfully_installed {
                println!("{} Installed!", padding);
            }
        } else {
            println!("{} Already installed", padding);
        }
        {%- endfor %}
    }


    // Install executables
    let bin_dir = miden_sysroot_dir.join("bin");
    {% for component in installable_components %}

    // Install {{ component.name }}
    println!("Installing: {{ component.name }}");
    let bin_path = bin_dir.join("{{ component.installed_file }}");
    if !std::fs::exists(&bin_path).unwrap_or(false) {
        let do_fetch_artifact: bool;
        let mut do_install_from_source: bool;
        let mut successfully_installed = false;
        let initial_message: String;

        if !"{{ component.artifact.0 }}".is_empty() {
            do_fetch_artifact = true;
            do_install_from_source = false;
            initial_message = format!("{} Fetching artifact", padding);
        } else {
            do_fetch_artifact = false;
            do_install_from_source = true;
            initial_message = format!("{} No artifact found. Proceeding to install from source", padding);
        }

        println!("{initial_message}");
        if do_fetch_artifact {
            if let Err(err) = install_artifact("{{ component.artifact.0 }}", std::path::Path::new("{{ component.artifact.1 }}")) {
                println!("{} {err}.", padding);
                println!("{} Proceeding to install from source.", padding);
                do_install_from_source = true;
            } else {
                successfully_installed = true;
            }
        }

        if do_install_from_source {
            if let Err(err) = install_from_source(
                      "{{ component.name }}",
                      "{{ component.required_toolchain_flag }}",
                      &[
                          {%- for arg in chosen_profile %}
                          "{{ arg }}",
                          {%- endfor %}
                      ],
                      "{{ verbosity.quiet_flag }}",
                      &[
                          {%- for arg in component.args %}
                          "{{ arg }}",
                          {%- endfor %}
                      ],
                      miden_sysroot_dir,
                      ) {

                if {{ keep_going }} {
                        println!("{} Failed to install '{{ component.name }}' from source because of {err}. Skipping.", padding);
                } else {
                        panic!("Failed to install '{{ component.name }}' from source because of {err}.");
                }

           } else {
                successfully_installed = true;
           }
        }

        if successfully_installed {
            println!("{} Installed!", padding);
        }
    } else {
        println!("{} Already installed", padding);
    }
    {% endfor %}

    let opt_dir = miden_sysroot_dir.join("opt");
    // We install the 'miden <name>' symlinks
    {%- for link in symlinks %}

    let new_link = opt_dir.join("{{ link.alias }}");
    let executable = std::path::Path::new("../bin").join("{{ link.binary }}");
    if std::fs::read_link(&new_link).is_err() {
         utility::symlink(&new_link, &executable);
    }

    {%- endfor %}

    // Create var directory
    let var_dir = miden_sysroot_dir.join("var");
    if !std::fs::exists(&var_dir).unwrap_or(false) {
        std::fs::create_dir(&var_dir).expect("Failed to create etc directory toolchain directory.");
    }
}
"##,
        )
        .unwrap_or_else(|err| panic!("invalid install script template: {err}"));

    // Prepare install script context with available channel components
    let mut dependencies = Vec::new();
    let mut installable_components = Vec::new();
    for component in channel.components.iter() {
        match component.get_installed_file() {
            InstalledFile::Executable { .. } => {
                let artifact_destination = {
                    component.get_artifact_uri(&config.target).map(|uri| {
                        let destination =
                            component.get_installed_file().get_path_from(toolchain_directory);
                        (uri, destination)
                    })
                };
                installable_components.push((component, artifact_destination))
            },
            InstalledFile::Library { .. } => {
                let artifact_destination = {
                    component.get_artifact_uri(&TargetTriple::MidenVM).map(|uri| {
                        let destination =
                            component.get_installed_file().get_path_from(toolchain_directory);

                        (uri, destination)
                    })
                };

                dependencies.push((component, artifact_destination))
            },
        }
    }

    // List of all the symlinks that need to be installed.
    //
    // Currently, these includes:
    //
    // - A symlink that adds the 'miden ' prefix to the corresponding executable,   done in order to
    //   "trick" clap into displaying midenup compatile messages, for more information, see: https://github.com/0xMiden/midenup/pull/73.
    let symlinks = channel
        .components
        .iter()
        .flat_map(|component| {
            let mut executables = Vec::new();

            let exe_name = component.get_installed_file();
            if let InstalledFile::Executable { ref binary_name, alias_only: _ } = exe_name {
                let miden_display = component.get_symlink_name();
                executables.push((miden_display, binary_name.clone()));
            }

            executables
        })
        .map(|(alias, binary)| {
            upon::value! {
                alias: alias,
                binary: binary,
            }
        })
        .collect::<Vec<_>>();

    // The set of cargo dependencies needed for the install script
    let dependencies = dependencies
        .into_iter()
        .map(|(component, artifact)| {
            let installed_file = component.get_installed_file();
            let library_struct = installed_file
                .get_library_struct()
                .with_context(|| {
                    format!(
                        "Component {} is marked as library, however the manifest does not contain \
                         the associated Library struct from where it will obtain the `.masp` \
                         file. \nThe manifest should contain a line like the following: \
                         \nlibrary_struct: \"miden_stdlib::MidenStdLib::default()\"",
                        component.name
                    )
                })
                .unwrap();
            let exposing_function = format!("{library_struct}::default()");
            let artifact = artifact.unwrap_or_default();
            match &component.version {
                Authority::Cargo { package, version } => {
                    let package = package.as_deref().unwrap_or(component.name.as_ref()).to_string();
                    upon::value! {
                        name: component.name.to_string(),
                        package: package,
                        version: version.to_string(),
                        git_uri: "",
                        path: "",
                        exposing_function: exposing_function,
                        artifact: artifact,
                    }
                },
                Authority::Git { repository_url, crate_name, target } => {
                    upon::value! {
                        name: component.name.to_string(),
                        package: crate_name,
                        version: "> 0.0.0",
                        git_uri: format!("{}\", {target}", repository_url.clone()),
                        path: "",
                        exposing_function: exposing_function,
                        artifact: artifact,
                    }
                },
                Authority::Path { crate_name, path, .. } => {
                    upon::value! {
                        name: component.name.to_string(),
                        package: crate_name,
                        version: "> 0.0.0",
                        git_uri: "",
                        path: path.display().to_string(),
                        exposing_function: exposing_function,
                        artifact: artifact,
                    }
                },
            }
        })
        .collect::<Vec<_>>();

    // The set of components to be installed with `cargo install`
    let installable_components = installable_components
        .into_iter()
        .map(|(component, artifact)| {
            let mut args = vec![];
            match &component.version {
                Authority::Cargo { package, version } => {
                    let package = package.as_deref().unwrap_or(component.name.as_ref());
                    args.push(package.to_string());
                    args.push("--version".to_string());
                    args.push(version.to_string());
                },
                Authority::Git { repository_url, target, crate_name } => {
                    args.push("--git".to_string());
                    args.push(repository_url.clone());
                    args.push(target.to_cargo_flag()[0].clone());
                    args.push(target.to_cargo_flag()[1].clone());
                    args.push(crate_name.clone());
                },
                Authority::Path { path, .. } => {
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

            let installed_file = component.get_installed_file().to_string();

            upon::value! {
                name: component.name.to_string(),
                installed_file: installed_file,
                required_toolchain_flag: required_toolchain_flag,
                args: args,
                artifact: artifact.unwrap_or_default(),
            }
        })
        .collect::<Vec<_>>();

    let chosen_profile = if config.debug {
        ["--profile", "dev"]
    } else {
        ["--profile", "release"]
    };

    // NOTE: We do not pass cargo's --verbose flag since it displays a *lot* of information.
    let verbosity = if !options.verbose {
        upon::value! {
            quiet_flag: "--quiet"
        }
    } else {
        upon::value! {
            quiet_flag: ""
        }
    };

    let install_artifact_function = {
        upon::value! {
            function: include_str!("../external.rs")
        }
    };

    let curl_version = env!("CURL_VERSION");

    // This determines whether to panic if a component fails to be install. In release builds, we
    // want midenup to keep going; but on debug builds we want to catch those errors.
    let install_keep_going = {
        #[cfg(debug_assertions)]
        {
            false
        }
        #[cfg(not(debug_assertions))]
        {
            true
        }
    };

    // Render the install script
    template
        .render(
            &engine,
            upon::value! {
                dependencies: dependencies,
                installable_components: installable_components,
                channel_json : serde_json::to_string_pretty(channel).unwrap(),
                symlinks: symlinks,
                chosen_profile: chosen_profile,
                verbosity: verbosity,
                install_artifact: install_artifact_function,
                curl_version: curl_version,
                keep_going: install_keep_going,
            },
        )
        .to_string()
        .unwrap_or_else(|err| panic!("install script rendering failed: {err}"))
}

type InstalledBinary = String;
/// Returns the names of all packages installed via cargo at the given root.
///
/// Runs `cargo install --list --root <root>` and parses each package header line.
pub fn get_installed_cargo_binaries(root_dir: PathBuf) -> anyhow::Result<HashSet<InstalledBinary>> {
    let output = std::process::Command::new("cargo")
        .arg("install")
        .arg("--root")
        .arg(&root_dir)
        .arg("--list")
        .output()
        .with_context(|| "Failed to obtain binaries intalled via cargo")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        bail!("Failed to obtain binaries installed via cargo {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let programs = stdout
        .lines()
        // The format of cargo install --list is as follows:
        // <crate> <version>
        //     <binary>
        //
        // e.g.:
        // ripgrep v15.1.0:
        //     rg
        // sccache v0.10.0:
        //     sccache
        .filter(|line| !line.is_empty() && !line.starts_with(char::is_whitespace))
        // The first item is the name of the crate that we have installed.
        .filter_map(|line| line.split_whitespace().next())
        .map(String::from)
        .collect();

    Ok(programs)
}
