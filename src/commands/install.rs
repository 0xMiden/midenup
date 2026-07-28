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
    channel::{Channel, ChannelAlias},
    commands,
    config::Config,
    manifest::{ComponentKind, InstallationMethod, Manifest, PackageInstallationMethod},
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
    let toolchain_dir = toolchains_dir.join(format!("{}", channel.name));

    let installed_toolchains_dir = config.midenup_home.join("installed_toolchains");
    let install_dir_name = format!("{}-{}", channel.name, channel.content_hash());
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

        // Next, we determine how the component got installed.
        //
        // A component could have been installed either by cargo install (i.e. "from source") or via
        // a pre-compiled artifact.
        // We can only *truly* determine how it got installed after the fact.
        for component in channel_to_save.components.iter_mut() {
            match &component.version {
                // If a component was installed with --branch, then write down the current commit.
                //
                // This is used on updates to check if any new commits were pushed since
                // installation.
                Authority::Git {
                    repository_url,
                    subpath,
                    target: GitTarget::Branch { name, .. },
                } => {
                    // If, for whatever reason, we fail to find the latest hash, we simply leave it
                    // empty. That does mean that an update will be triggered even if the component
                    // does not need it.
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
                    // If a component was installed with --path, then write down the latest
                    // modification time found inside the directory (or the current time as a
                    // fallback). This is used on updates to check if anything changed.
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
) -> anyhow::Result<String> {
    // Prepare install script template
    let engine = upon::Engine::new();
    let template = engine
        .compile(
            r##"#!/usr/bin/env cargo
---cargo
[dependencies]
{%- for dep in dependencies %}
{{ dep.name }} = { {{ dep.version }}, {{ dep.features }} }
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
        let lib_path = _lib_dir.join("{{ extractable.component }}").with_extension("masp");
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
    let components = channel.component_graph(&options.profile)?;
    for component in components.toposort()? {
        max_component_width = core::cmp::max(max_component_width, component.name.chars().count());
        match component.kind() {
            ComponentKind::Asset { .. } | ComponentKind::Command { .. } => {
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
                        let to = toolchain_directory
                            .join("etc")
                            .join(component.name.as_ref())
                            .join(id);
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
                if let Some(symlink) = spec.symlink_name.as_ref() {
                    symlinks.push(upon::value! {
                        alias: symlink.clone(),
                        binary: spec.installed_executable.clone(),
                    });
                }
            },
            ComponentKind::Package
            | ComponentKind::LegacyPackage {
                installation_method: PackageInstallationMethod::Prebuilt,
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
            } => {
                let features = if features.is_empty() {
                    String::new()
                } else {
                    let mut feature_strings = String::new();
                    for (i, f) in features.iter().enumerate() {
                        if i > 0 {
                            feature_strings.push_str(", ");
                        }
                        feature_strings.push('"');
                        feature_strings.push_str(f.as_str());
                        feature_strings.push('"');
                    }
                    format!("default-features = false, features = [{feature_strings}]")
                };
                match &component.version {
                    Authority::Registry { version } => {
                        dependencies.push(upon::value! {
                            name: crate_name.clone(),
                            version: format!("version = \"{version}\""),
                            features: features,
                        });
                    },
                    Authority::Path { path, .. } => {
                        dependencies.push(upon::value! {
                            name: crate_name.clone(),
                            version: format!("path = \"{}\"", path.display()),
                            features: features,
                        });
                    },
                    Authority::Git { repository_url, target, .. } => {
                        dependencies.push(upon::value! {
                            name: crate_name.clone(),
                            version: format!("git = \"{repository_url}\", {target}"),
                            features: features,
                        });
                    },
                }
                installable_packages.push(upon::value! {
                    component: component.name.to_string(),
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
