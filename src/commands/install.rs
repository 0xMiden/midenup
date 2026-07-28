use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    io::Write,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context, bail};
use serde::Deserialize;

use crate::{
    channel::Channel,
    commands,
    config::Config,
    manifest::{ComponentKind, InstallationMethod, PackageInstallationMethod},
    options::{InstallationOptions, IntentUpdate},
    resolve::Intent,
    state::{Installation, LocalState, PublicationId, PublicationRef},
    utils,
    version::{Authority, GitTarget},
};

/// Installs a specified toolchain by channel or version.
pub fn install(
    config: &Config,
    channel: &Channel,
    state: &mut LocalState,
    options: &InstallationOptions,
) -> anyhow::Result<()> {
    commands::setup_midenup(config)?;

    let toolchains_dir = config.midenup_home.join("toolchains");
    let toolchain_dir = toolchains_dir.join(format!("{}", channel.name));

    let installed_toolchains_dir = config.midenup_home.join("installed_toolchains");
    // The plan is the single description of what will be installed where; the key is derived
    // from it, so directory naming and the recorded identity cannot disagree.
    //
    // Interim directory naming: M5 replaces this with an opaque publication id. Until then the
    // name must differ whenever the installed content would differ, which is what the key
    // measures. The key is sysroot-independent, so building the plan against the toolchains
    // directory rather than the not-yet-known install directory does not affect it.
    let selection = crate::resolve::Intent {
        profiles: [options.profile].into_iter().collect(),
        roots: options.components.iter().cloned().collect(),
    };
    let plan_key =
        crate::plan::build_plan(channel, &selection, config.target(), &installed_toolchains_dir)?
            .key;
    let install_dir_name =
        format!("{}-{}", channel.name, plan_key.to_string().trim_start_matches("pk1:"));
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
                config,
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

    let install_script_contents = generate_install_script(config, channel, options, &install_dir)?;
    install_file.write_all(&install_script_contents.into_bytes()).with_context(|| {
        format!("failed to write install script at '{}'", install_file_path.display())
    })?;

    let mut child = std::process::Command::new("cargo")
        .current_dir(&config.working_directory)
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

    let temp_symlink = installed_toolchains_dir.join(format!("{}.new", channel.name));
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
        let relative_channel_target = PathBuf::from(format!("{}", channel.name));
        utils::fs::symlink(&stable_dir, &relative_channel_target)
            .expect("Couldn't create stable dir");
    }

    // Record what was installed.
    //
    // The component snapshot is pinned here rather than referencing the upstream manifest: `miden`
    // dispatch reads it offline, and update needs to know what was *actually* installed, not what
    // upstream happens to say now.
    {
        let mut installed_components = channel.components.clone();

        // How a component was really obtained can only be known after the fact.
        for component in installed_components.iter_mut() {
            match &component.version {
                // A branch is not a fixed point, so record the commit that was actually installed;
                // update compares against it to decide whether new commits have landed.
                Authority::Git {
                    repository_url,
                    subpath,
                    target: GitTarget::Branch { name, .. },
                } => {
                    // Leaving this empty on failure means an update is triggered unnecessarily,
                    // which is the safe direction to fail in.
                    let revision_hash = utils::git::find_latest_hash(repository_url, name).ok();

                    component.version = Authority::Git {
                        repository_url: repository_url.clone(),
                        subpath: subpath.clone(),
                        target: GitTarget::Branch {
                            name: name.clone(),
                            latest_revision: revision_hash,
                        },
                    }
                },
                Authority::Git { .. } => (),
                Authority::Path { path, .. } => {
                    // Record the tree's modification time so update can tell whether it changed.
                    let path = if path.is_absolute() {
                        Cow::Borrowed(path.as_path())
                    } else {
                        Cow::Owned(config.working_directory.join(path.as_path()))
                    };
                    let latest_time = utils::fs::latest_modification(&path)
                        .ok()
                        .map(|(latest_modification, _)| latest_modification)
                        .unwrap_or(SystemTime::now());
                    component.version = Authority::Path {
                        path: path.to_path_buf(),
                        last_modification: Some(latest_time),
                    }
                },
                Authority::Registry { .. } => (),
            }
        }

        // What the caller asked for on this invocation.
        let requested = Intent {
            profiles: [options.profile].into_iter().collect(),
            roots: options.components.iter().cloned().collect(),
        };
        let previous = state.get(&channel.name).map(|installation| installation.intent.clone());

        // Installing and recording what the user wants are separate concerns: activation installs
        // a narrowed set but must only add to the record, while a direct install restates it.
        let intent = match options.intent_update.clone() {
            // A direct `midenup install`: record exactly what the command line asked for.
            None => requested,
            Some(IntentUpdate::Replace(intent)) => intent,
            Some(IntentUpdate::Union(intent)) => {
                let mut merged = previous.unwrap_or_default();
                merged.union_with(&intent);
                merged
            },
            Some(IntentUpdate::Preserve) => previous.unwrap_or(requested),
        };

        state.upsert(Installation {
            channel: channel.name.clone(),
            intent,
            components: installed_components,
            publication: PublicationRef::Managed {
                id: PublicationId::generate(),
                plan_key,
                target: config.target().to_string(),
            },
            installed_at: chrono::Utc::now().timestamp(),
        });
    }

    config.write_local_state(state)?;

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
) -> anyhow::Result<String> {
    // Prepare install script template
    let engine = upon::Engine::new();
    let template = engine
        .compile(
            r##"#!/usr/bin/env cargo
---cargo
[dependencies]
{%- for dep in dependencies %}
{{ dep.name }} = { {{ dep.spec }} }
{%- endfor %}
colored = "3.0"
curl = "{{ curl_version }}"
---

// NOTE: This file was generated by midenup. Do not edit by hand

use std::{process::ExitCode, path::Path};
use colored::Colorize;

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

fn error(msg: impl core::fmt::Display) {
    print!("{}: {msg}", "error".red().bold())
}

fn info(msg: impl core::fmt::Display) {
    print!("info: {msg}")
}

fn main() -> ExitCode {
    let mut exit_status = ExitCode::SUCCESS;

    // MIDEN_SYSROOT is set by `midenup` when invoking this script, and will contain the resolved
    // (and prepared) sysroot path to which this script will install the desired toolchain
    // components.
    let miden_sysroot_dir = Path::new(env!("MIDEN_SYSROOT"));


    // Create var directory
    let var_dir = miden_sysroot_dir.join("var");
    if !std::fs::exists(&var_dir).unwrap_or(false) {
        std::fs::create_dir(&var_dir).expect("failed to create 'var' subdirectory in sysroot");
    }

    // Install downloadable components first
    {
        {% for downloadable in downloadable_components %}
        info(format!("installing {:.<width$}", "{{ downloadable.component }}".white().bold(), width = {{ max_component_width }}));

        // NOTE: If the file already exists, then we are running an update and we don't need to
        // update this element. We treat failure to detect existence as non-existence, and in cases
        // where that is due to permissions or some other issue, we let the actual install fail.
        let mut already_installed = true;
        let mut successfully_installed = true;
        {% for artifact in downloadable.artifacts %}
        let artifact_path = Path::new("{{ artifact.to }}");
        if !std::fs::exists(&artifact_path).unwrap_or(false) {
            already_installed = false;
            if let Err(err) = install_artifact("{{ artifact.from }}", artifact_path, {{ artifact.is_file }}) {
                successfully_installed = false;
                error(format!("failed to fetch artifact: {err}\n"));
                if !{{ keep_going }} {
                    return ExitCode::FAILURE;
                }
            }
        }
        {%- endfor %}

        if !successfully_installed {
            exit_status = ExitCode::FAILURE;
        } else if already_installed {
            println!("already installed");
        } else {
            println!("{}", "installed".green().bold());
        }

        {%- endfor %}
    }

    // Extract packages from crate dependencies, if applicable
    {
        let _lib_dir = miden_sysroot_dir.join("lib");
        {% for extractable in installable_packages %}
        info(format!("installing {:.<width$}", "{{ extractable.component }}".white().bold(), width = {{ max_component_width }}));

        // Write library to $MIDEN_SYSROOT/lib/dep.masp
        let lib_path = _lib_dir.join("{{ extractable.installed_package }}");
        // NOTE: If the file already exists, then we are running an update and we don't need to
        // update this element. We treat failure to detect existence as non-existence, and in cases
        // where that is due to permissions or some other issue, we let the actual install fail.
        if !std::fs::exists(&lib_path).unwrap_or(false) {
            let result = {{ extractable.extractor }}.write_to_file(&lib_path);
            if let Err(err) = result {
                println!("{}: unable to install {{ extractable.component }} from crate: {err}", "failed".red().bold());
                if !{{ keep_going }} {
                    return ExitCode::FAILURE;
                }
                exit_status = ExitCode::FAILURE;
            } else {
                println!("{}", "installed".green().bold());
            }
        } else {
            println!("already installed");
        }
        {%- endfor %}
    }

    // Install executables via Cargo
    let bin_dir = miden_sysroot_dir.join("bin");
    {% for installable in installable_components %}
    // Install {{ installable.component }}
    {
        info(format!("installing {:.<width$}", "{{ installable.component }}".white().bold(), width = {{ max_component_width }}));

        let bin_path = bin_dir.join("{{ installable.installed_file }}");
        if !std::fs::exists(&bin_path).unwrap_or(false) {
            if let Err(err) = install_from_source(
                "{{ installable.required_toolchain_flag }}",
                &[
                    {%- for arg in chosen_profile %}
                    "{{ arg }}",
                    {%- endfor %}
                ],
                "{{ verbosity.quiet_flag }}",
                &[
                    {%- for arg in installable.args %}
                    "{{ arg }}",
                    {%- endfor %}
                ],
                miden_sysroot_dir,
            ) {
                println!("{}: unable to install {{ installable.component }} from source: {err}", "failed".red().bold());
                if !{{ keep_going }} {
                    return ExitCode::FAILURE;
                }
                exit_status = ExitCode::FAILURE;
            } else {
                println!("{}", "installed".green().bold());
            }
        } else {
            println!("already installed");
        }
    }
    {% endfor %}

    // We install the 'miden <name>' symlinks
    let opt_dir = miden_sysroot_dir.join("opt");
    let symlinks: &[(&str, &str)] = &[
    {%- for link in symlinks %}
        ("{{ link.alias }}", "{{ link.binary }}"),
    {%- endfor %}
    ];
    for (alias, binary) in symlinks {
        let link = opt_dir.join(alias);
        let bin = Path::new("../bin").join(binary);
        if std::fs::read_link(&link).is_err() {
             utility::symlink(&link, &bin);
        }
    }

    exit_status
}
"##,
        )
        .unwrap_or_else(|err| panic!("invalid install script template: {err:#}"));

    let mut max_component_width = 0usize;
    // Prepare install script context with available channel components
    let mut dependencies = Vec::new();
    // The set of all components with prebuilt artifacts that can simply be downloaded
    let mut downloadable_components = Vec::new();
    // The set of components which must be installed with `cargo install`
    let mut installable_components = Vec::new();
    // The set of packages which must be installed by extracting the package from a Cargo dep
    let mut installable_packages = Vec::new();
    // List of all the symlinks that need to be installed.
    //
    // Currently, these include:
    //
    // - A symlink that adds the 'miden ' prefix to the corresponding executable, done in order to
    //   "trick" clap into displaying midenup compatile messages, for more information, see: https://github.com/0xMiden/midenup/pull/73.
    let mut symlinks = Vec::new();
    // The channel handed to us has already been narrowed to exactly what should be installed,
    // so resolve against `complete` to take all of it, in dependency order.
    let components =
        crate::resolve::resolve(channel, &crate::resolve::Intent::new(&[options.profile], &[]))?;
    for component in components {
        max_component_width = core::cmp::max(max_component_width, component.name.chars().count());
        match component.kind() {
            // Reaching here means an unknown kind was explicitly selected: this build cannot know
            // how to install it, so fail with an actionable message rather than skipping it.
            ComponentKind::Unsupported { tag, .. } => {
                bail!(
                    "unable to install component '{}': its kind '{tag}' is not supported by this \
                     version of midenup; upgrade midenup or deselect the component",
                    component.name
                );
            },
            ComponentKind::Asset | ComponentKind::Command { .. } => {
                let artifacts =
                    component.artifacts.get_artifacts_for_target(config.target(), component)?;
                if artifacts.is_empty() {
                    continue;
                }
                // The artifact id is the exact installed filename, so the destination is always a
                // file path. Passing the containing directory instead makes the installer's
                // existence check test the directory, which the install already created.
                let artifacts = artifacts
                    .into_iter()
                    .map(|(id, uri)| {
                        let to =
                            toolchain_directory.join("etc").join(component.name.as_ref()).join(id);
                        upon::value! {
                            is_file: true,
                            from: uri.to_string(),
                            to: to.display().to_string(),
                        }
                    })
                    .collect::<Vec<_>>();
                downloadable_components.push(upon::value! {
                    component: component.name.to_string(),
                    artifacts: artifacts,
                });
            },
            ComponentKind::CargoExtension { installation_method, spec }
            | ComponentKind::Executable { installation_method, spec } => {
                let artifacts = component
                    .artifacts
                    .get_default_artifacts_for_target(config.target(), component)?;
                let artifacts = artifacts
                    .into_iter()
                    .map(|(_id, uri)| upon::value! {
                        is_file: true,
                        from: uri.to_string(),
                        to: toolchain_directory.join("bin").join(&spec.installed_executable).display().to_string(),
                    })
                    .collect::<Vec<_>>();
                match installation_method {
                    InstallationMethod::Prebuilt
                    | InstallationMethod::PrebuiltWithCargoFallback { .. }
                        if !artifacts.is_empty() =>
                    {
                        downloadable_components.push(upon::value! {
                            component: component.name.to_string(),
                            artifacts: artifacts,
                        });
                    },
                    InstallationMethod::Prebuilt => {
                        bail!(
                            "unable to install component '{}': unsupported target {}",
                            component.name,
                            config.target()
                        );
                    },
                    InstallationMethod::PrebuiltWithCargoFallback {
                        crate_name,
                        rustup_channel,
                        features,
                    }
                    | InstallationMethod::Cargo { crate_name, rustup_channel, features } => {
                        let mut args = vec![];
                        match &component.version {
                            Authority::Registry { version } => {
                                args.push(crate_name.clone());
                                args.push("--version".to_string());
                                args.push(version.to_string());
                            },
                            Authority::Git { repository_url, target, subpath: _ } => {
                                args.push("--git".to_string());
                                args.push(repository_url.clone());
                                args.extend(target.to_cargo_flag());
                                args.push(crate_name.clone());
                            },
                            Authority::Path { path, .. } => {
                                args.push("--path".to_string());
                                args.push(path.display().to_string());
                            },
                        }

                        let required_toolchain_flag =
                            rustup_channel.as_ref().map(|c| format!("+{c}")).unwrap_or_default();

                        // Enable optional features, if present
                        if !features.is_empty() {
                            let features = features.join(",");
                            args.push("--features".to_string());
                            args.push(features);
                        };

                        installable_components.push(upon::value! {
                            component: component.name.to_string(),
                            installed_file: spec.installed_executable.clone(),
                            required_toolchain_flag: required_toolchain_flag,
                            args: args,
                        });
                    },
                }
                // `get_symlink_name` applies the documented default (`miden <component>`) when
                // `symlink-name` is absent, and returns `None` for hidden components -- which is
                // exactly the rule, so no separate `hide` check is needed here.
                if let Some(symlink) = component.get_symlink_name() {
                    symlinks.push(upon::value! {
                        alias: symlink,
                        binary: spec.installed_executable.clone(),
                    });
                }
            },
            ComponentKind::Package
            | ComponentKind::LegacyPackage {
                installation_method: PackageInstallationMethod::Prebuilt,
                ..
            } => {
                let artifacts = component
                    .artifacts
                    .get_default_artifacts_for_target(config.target(), component)?;
                if artifacts.is_empty() {
                    bail!(
                        "invalid spec for '{}': package components must have at least one artifact",
                        component.name
                    );
                }
                // As above: `lib/<artifact-id>`, never the bare `lib/` directory.
                let artifacts = artifacts
                    .into_iter()
                    .map(|(id, uri)| {
                        upon::value! {
                            is_file: true,
                            from: uri.to_string(),
                            to: toolchain_directory.join("lib").join(id).display().to_string(),
                        }
                    })
                    .collect::<Vec<_>>();
                downloadable_components.push(upon::value! {
                    component: component.name.to_string(),
                    artifacts: artifacts,
                });
            },
            ComponentKind::LegacyPackage {
                installation_method:
                    PackageInstallationMethod::Cargo { crate_name, features, extractor },
                ..
            } => {
                // The inline table body is assembled here rather than in the template so that an
                // absent features list simply contributes no entry. Emitting
                // `{ {{version}}, {{features}} }` unconditionally produced a trailing comma when
                // features were empty, which is not valid TOML.
                let mut spec = Vec::with_capacity(2);
                match &component.version {
                    Authority::Registry { version } => {
                        spec.push(format!("version = \"{version}\""));
                    },
                    Authority::Path { path, .. } => {
                        spec.push(format!("path = \"{}\"", path.display()));
                    },
                    Authority::Git { repository_url, target, .. } => {
                        spec.push(format!("git = \"{repository_url}\", {target}"));
                    },
                }
                if !features.is_empty() {
                    let feature_strings =
                        features.iter().map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(", ");
                    spec.push(format!("default-features = false, features = [{feature_strings}]"));
                }
                dependencies.push(upon::value! {
                    name: crate_name.clone(),
                    spec: spec.join(", "),
                });
                installable_packages.push(upon::value! {
                    component: component.name.to_string(),
                    // Resolved once, here, so uninstall can look up the same name.
                    installed_package: component
                        .installed_package_name()
                        .expect("legacy packages always resolve an installed filename"),
                    extractor: extractor.clone(),
                });
            },
        }
    }

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
                max_component_width: max_component_width + 2,
                dependencies: dependencies,
                downloadable_components: downloadable_components,
                installable_components: installable_components,
                installable_packages: installable_packages,
                symlinks: symlinks,
                chosen_profile: chosen_profile,
                verbosity: verbosity,
                install_artifact: install_artifact_function,
                curl_version: curl_version,
                keep_going: install_keep_going,
            },
        )
        .to_string()
        .context("install script rendering failed")
}

#[allow(unused)]
pub struct InstalledBinary {
    pub version: semver::Version,
    pub location: Authority,
    pub bins: Vec<String>,
    pub features: Vec<String>,
}

#[derive(Deserialize)]
struct CargoInstalls {
    #[serde(default)]
    installs: BTreeMap<String, InstalledCrateInfo>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum InstalledCrateInfo {
    Info {
        #[serde(default)]
        bins: Vec<String>,
        #[serde(default)]
        features: Vec<String>,
    },
    UnknownFormat,
}

/// Returns the names of all packages installed via cargo at the given root.
///
/// Runs `cargo install --list --root <root>` and parses each package header line.
#[allow(unused)]
pub fn get_installed_cargo_binaries(
    root_dir: PathBuf,
) -> anyhow::Result<HashMap<String, InstalledBinary>> {
    let crates2_json = root_dir.join(".crates2.json");
    if !crates2_json.exists() {
        return Ok(HashMap::new());
    }
    let crates2_json_file = std::fs::File::open(&crates2_json).with_context(|| {
        format!(
            "failed to obtain binaries installed via Cargo from '{}'",
            crates2_json.display()
        )
    })?;
    let installs =
        serde_json::from_reader::<_, CargoInstalls>(crates2_json_file).with_context(|| {
            format!("failed to deserialize Cargo's install manifest '{}'", crates2_json.display())
        })?;

    let mut installed = HashMap::new();

    for (crate_id, info) in installs.installs {
        let InstalledCrateInfo::Info { bins, features } = info else {
            continue;
        };
        if bins.is_empty() {
            continue;
        }
        let Some((crate_name, rest)) = crate_id.split_once(' ') else {
            continue;
        };
        let Some((crate_version, source)) = rest.split_once(' ') else {
            continue;
        };
        let crate_name = crate_name.trim();
        let Ok(crate_version) = semver::Version::parse(crate_version.trim()) else {
            continue;
        };
        let source = source.trim().trim_matches(['(', ')']);
        if let Some(path) = source.strip_prefix("path+") {
            installed.insert(
                crate_name.to_string(),
                InstalledBinary {
                    version: crate_version,
                    location: Authority::Path {
                        path: PathBuf::from(path.to_string()),
                        last_modification: None,
                    },
                    bins,
                    features,
                },
            );
        } else if source.starts_with("registry+") {
            let version = crate_version.clone();
            installed.insert(
                crate_name.to_string(),
                InstalledBinary {
                    version: crate_version,
                    location: Authority::Registry { version },
                    bins,
                    features,
                },
            );
        }
    }

    Ok(installed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        artifact::Artifacts,
        manifest::{Component, ComponentKind, PackageInstallationMethod},
        profile::Profile,
        version::{Authority, GitTarget},
    };

    fn test_config() -> Config {
        Config {
            working_directory: PathBuf::from("/tmp"),
            midenup_home: PathBuf::from("/tmp/midenup"),
            cargo_home: PathBuf::from("/tmp/cargo"),
            manifest: crate::manifest::Manifest::default(),
            debug: true,
            target: Cow::Borrowed("aarch64-apple-darwin"),
        }
    }

    fn channel_with_legacy_package(version: Authority) -> Channel {
        channel_with_legacy_package_features(version, vec![])
    }

    /// Builds a channel with a single crate-extracted legacy package.
    fn channel_with_legacy_package_features(version: Authority, features: Vec<String>) -> Channel {
        Channel::new(
            semver::Version::new(0, 14, 0),
            None,
            vec![Component {
                name: Cow::Borrowed("core"),
                version,
                kind: ComponentKind::LegacyPackage {
                    installation_method: PackageInstallationMethod::Cargo {
                        crate_name: "miden-core-lib".to_string(),
                        features,
                        extractor: "miden_core_lib::CoreLibrary::default().package()".to_string(),
                    },
                    installed_package: None,
                },
                profiles: vec![Profile::Minimal],
                requires: vec![],
                artifacts: Artifacts::default(),
                extra: Default::default(),
            }],
            vec![],
        )
    }

    /// Extracts the `---cargo ... ---` frontmatter manifest from a generated script.
    fn frontmatter(script: &str) -> String {
        let body = script.split_once("---cargo\n").expect("no frontmatter").1;
        body.split_once("\n---").expect("unterminated frontmatter").0.to_string()
    }

    /// The generated frontmatter must be valid TOML for every authority kind.
    ///
    /// Regression: the dependency line was rendered as `name = { {{version}}, {{features}} }`
    /// unconditionally. With no features declared, `features` is the empty string, so the inline
    /// table ended in a trailing comma -- which TOML rejects. This only surfaced for components
    /// with no prebuilt artifacts, since anything downloadable never emits a dependency at all.
    #[test]
    fn generated_frontmatter_is_valid_toml_without_features() {
        let config = test_config();
        let options = InstallationOptions {
            profile: Profile::Complete,
            ..Default::default()
        };

        let authorities = [
            (
                "git",
                Authority::Git {
                    repository_url: "https://github.com/0xMiden/miden-vm.git".to_string(),
                    subpath: None,
                    target: GitTarget::Revision { hash: "16a2866b1a4cb535".to_string() },
                },
            ),
            ("registry", Authority::Registry { version: semver::Version::new(0, 23, 3) }),
            (
                "path",
                Authority::Path {
                    path: PathBuf::from("/tmp/miden-core-lib"),
                    last_modification: None,
                },
            ),
        ];

        for (label, authority) in authorities {
            let channel = channel_with_legacy_package(authority);
            let script =
                generate_install_script(&config, &channel, &options, Path::new("/tmp/sysroot"))
                    .unwrap_or_else(|err| panic!("{label}: render failed: {err}"));
            let fm = frontmatter(&script);
            toml::from_str::<toml::Value>(&fm).unwrap_or_else(|err| {
                panic!("{label}: generated frontmatter is not valid TOML: {err}\n---\n{fm}\n---")
            });
        }
    }

    /// The features path must also produce valid TOML, and must actually carry the features
    /// through -- the extractor can only compile if they are enabled.
    #[test]
    fn generated_frontmatter_carries_features() {
        let config = test_config();
        let options = InstallationOptions {
            profile: Profile::Complete,
            ..Default::default()
        };
        let channel = channel_with_legacy_package_features(
            Authority::Registry { version: semver::Version::new(0, 23, 3) },
            vec!["std".to_string(), "concurrent".to_string()],
        );

        let script =
            generate_install_script(&config, &channel, &options, Path::new("/tmp/sysroot"))
                .expect("render failed");
        let fm = frontmatter(&script);
        let parsed: toml::Value = toml::from_str(&fm).expect("frontmatter is not valid TOML");

        let dep = &parsed["dependencies"]["miden-core-lib"];
        assert_eq!(dep["version"].as_str(), Some("0.23.3"));
        assert_eq!(dep["default-features"].as_bool(), Some(false));
        let features: Vec<&str> = dep["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f.as_str().unwrap())
            .collect();
        assert_eq!(features, vec!["std", "concurrent"]);
    }
}
