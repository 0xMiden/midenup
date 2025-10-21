use std::{io::Write, time::SystemTime};

use anyhow::{Context, bail};

use crate::{
    Config, InstallationOptions,
    channel::{Channel, ChannelAlias, InstalledFile},
    commands,
    manifest::Manifest,
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
    commands::setup_midenup(config)?;

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

    // We create the opt/ directory where the aliases are going to be stored.
    let opt_dir = toolchain_dir.join("opt");
    if !opt_dir.exists() {
        std::fs::create_dir_all(&opt_dir).with_context(|| {
            format!("failed to create toolchain directory: '{}'", opt_dir.display())
        })?;
    }

    let install_file_path = toolchain_dir.join("install").with_extension("rs");
    // NOTE: Even when performing an update, we still need to re-generate the
    // install script.  This is because, the versions that will be installed are
    // written directly into the file; so the file can't be "re-used".
    let mut install_file = std::fs::File::create(&install_file_path).with_context(|| {
        format!("failed to create file for install script at '{}'", install_file_path.display())
    })?;

    let install_script_contents = generate_install_script(config, channel, options);
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
            "midenup failed to install toolchain from channel {} with status {}",
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
        utils::fs::symlink(&stable_dir, &toolchain_dir).expect("Couldn't create stable dir");
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

        for component in channel_to_save.components.iter_mut() {
            match &component.version {
                // If a component was installed with --branch, then write down the
                // current commit. This is used on updates to check if any new commits
                // were pushed since installation.
                Authority::Git {
                    repository_url,
                    crate_name,
                    target: GitTarget::Branch { name, latest_revision: _ },
                } => {
                    // If, for whatever reason, we fail to find the latest hash, we
                    // simply leave it empty. That does mean that an update will be
                    // triggered even if the component does not need it.
                    let revision_hash = utils::git::find_latest_hash(repository_url, name).ok();

                    component.version = Authority::Git {
                        repository_url: repository_url.clone(),
                        crate_name: crate_name.clone(),
                        target: GitTarget::Branch {
                            name: name.clone(),
                            latest_revision: revision_hash,
                        },
                    };
                },
                Authority::Path { path, crate_name, last_modification: _ } => {
                    // If a component was installed with --path, then write down
                    // the latest modification time found inside the directory
                    // (or the current time as a fallback). This is used on
                    // updates to check if anything changed.
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
                _ => (),
            }

            if let Some(init_command) = component.get_initialization() {
                // The component could be already initialized if this is an update.
                let already_initialized = local_manifest
                    .get_channel_by_name(&channel.name)
                    .and_then(|ch| ch.get_component(&component.name))
                    .map(|comp| comp.already_initialized())
                    .unwrap_or(false);
                if already_initialized {
                    continue;
                }

                let commands = resolve_command(init_command, channel, component, config)?;
                // SAFETY: Safe under the assumption that every command has at
                // least one associated command.
                let target_exe = commands.first().cloned().unwrap();
                let prefix_args: Vec<String> = commands.into_iter().skip(1).collect();

                let toolchain_bin = config
                    .midenup_home
                    .join("toolchains")
                    .join(channel.name.to_string())
                    .join("opt");

                let path = match std::env::var_os("PATH") {
                    Some(prev_path) => {
                        let mut path = OsString::from(format!("{}:", toolchain_bin.display()));
                        path.push(prev_path);
                        path
                    },
                    None => toolchain_bin.into_os_string(),
                };

                let command = std::process::Command::new(target_exe)
                    .env("MIDENUP_HOME", &config.midenup_home)
                    .env("PATH", path)
                    .args(prefix_args)
                    .stderr(std::process::Stdio::inherit())
                    .stdout(std::process::Stdio::inherit())
                    .spawn();
                let Ok(mut command) = command else {
                    continue;
                };

                let status = command.wait();
                let Ok(_) = status else {
                    continue;
                };
                component.mark_as_initialized()?;
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
/// `midenup/toolchains/<version>/install.rs`. This file is then executed by
/// `cargo -Zscript`.
fn generate_install_script(
    config: &Config,
    channel: &Channel,
    options: &InstallationOptions,
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
---

// NOTE: This file was generated by midenup. Do not edit by hand

use std::process::Command;
use std::io::{Write};
use std::fs::{OpenOptions, rename};

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


    // We save the state the channel was in when installed. This is used when uninstalling.
    {
        let channel_json = r#"{{ channel_json }}"#;
        let channel_json_path = miden_sysroot_dir.join(".installed_channel.json");
        let mut installed_json = std::fs::File::create(channel_json_path).expect("failed to create installation in progress file");
        installed_json.write_all(&channel_json.as_bytes()).unwrap();
    }


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
            lib.as_ref()
                .write_to_file(&lib_path)
                .expect("failed to install {{ dep.name }} library component");
            println!("{} Installed!", padding);
        } else {
            println!("{} Already installed", padding);
        }
        writeln!(progress_file, "{{ dep.name }}").expect("Failed to write component name to progress file");
        {%- endfor %}
    }


    // Install executables
    let bin_dir = miden_sysroot_dir.join("bin");
    {% for component in installable_components %}

    // Install {{ component.name }}
    println!("Installing: {{ component.name }}");
    let bin_path = bin_dir.join("{{ component.installed_file }}");
    if !std::fs::exists(&bin_path).unwrap_or(false) {
        let mut child = Command::new("cargo")
            .arg(
            "{{ component.required_toolchain_flag }}",
            )
            .arg("install")
            .arg("--locked")
            .args([
            {%- for arg in chosen_profile %}
            "{{ arg }}",
            {%- endfor %}
            ])
            {%- if verbosity.quiet_flag %}
            .arg("{{ verbosity.quiet_flag }}")
            {%- endif %}
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
            panic!(
                "midenup failed to install '{{ component.name }}'"
            );
        }
        println!("{} Installed!", padding);
    } else {
        println!("{} Already installed", padding);
    }
    writeln!(progress_file, "{{component.name}}").expect("Failed to write component name to progress file");

    {% endfor %}

    let opt_dir = miden_sysroot_dir.join("opt");
    // We install the symlinks associated with the aliases
    {%- for link in symlinks %}

    let new_link = opt_dir.join("{{ link.alias }}");
    let executable = bin_dir.join("{{ link.binary }}");
    if std::fs::read_link(&new_link).is_err() {
         utility::symlink(&new_link, &executable);
    }

    {%- endfor %}


    // Now that installation finished, we rename the file to indicate that
    // installation finished successfully.
    let checkpoint_path = miden_sysroot_dir.join("installation-successful");
    rename(progress_path, checkpoint_path).expect("Couldn't rename .installation-in-progress to installation-successful");

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
            InstalledFile::Executable { .. } => installable_components.push(component),
            InstalledFile::Library { .. } => dependencies.push(component),
        }
    }

    // List of all the symlinks that need to be installed.
    // Currently, these includes:
    // - A symlink that adds the 'miden ' prefix to the corresponding executable,
    //   done in order to "trick" clap into displaying midenup compatile messages,
    //   for more information, see: https://github.com/0xMiden/midenup/pull/73.
    // - A symlink from all the aliases to the the corresponding executable

    let symlinks = channel
        .components
        .iter()
        .flat_map(|component| {
            let mut executables = Vec::new();

            let aliases = component.aliases.keys();
            let exe_name = component.get_installed_file();
            if let InstalledFile::Executable { ref binary_name } = exe_name {
                let miden_display = component.get_cli_display();
                for alias in aliases {
                    executables.push((alias.clone(), binary_name.clone()));
                }
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
        .map(|component| {
            let installed_file = component
                .get_installed_file();
            let library_struct = installed_file
                .get_library_struct()
                .with_context(|| format!("Component {} is marked as library, \
                                          however the manifest does not contain the associated Library struct \
                                          from where it will obtain the `.masp` file. \n\
                                          The manifest should contain a line like the following: \n\
                                          library_struct: \"miden_stdlib::MidenStdLib::default()\""
                                         , component.name)).unwrap();
            let exposing_function = format!("{library_struct}::default()");
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
                    }
                },
            }
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
            }
        })
        .collect::<Vec<_>>();

    let chosen_profile = if config.debug {
        ["--profile", "dev"]
    } else {
        ["--profile", "release"]
    };

    // NOTE: We do not pass cargo's --verbose flag since it displays a *lot* of
    // information.
    let verbosity = if !options.verbose {
        upon::value! {
            quiet_flag: "--quiet"
        }
    } else {
        upon::value! {
            quiet_flag: ""
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
            },
        )
        .to_string()
        .unwrap_or_else(|err| panic!("install script rendering failed: {err}"))
}
