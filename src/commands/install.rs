use std::{
    borrow::Cow,
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    time::SystemTime,
};

use anyhow::Context;
use serde::Deserialize;

use crate::{
    channel::Channel,
    commands,
    config::Config,
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

    // Build the plan against the real staging directory. The key was computed above from a plan
    // rooted elsewhere, which is safe because the key is sysroot-independent by construction --
    // otherwise naming the directory after the key would be circular.
    let plan = crate::plan::build_plan(channel, &selection, config.target(), &install_dir)?;

    crate::install::prepare(&install_dir)?;
    crate::install::execute(&plan, &install_dir, options.verbose, config.debug)?;

    // Structural check before anything is published: every planned file exists, is a regular
    // file, and carries the planned mode. Contents are not verified -- digests are recorded but
    // never checked -- so this asserts the plan was carried out, not what was installed.
    crate::install::verify(&plan, &install_dir)?;

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
