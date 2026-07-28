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
    paths,
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

    let home = &config.midenup_home;
    let toolchains_dir = paths::toolchains_dir(home);
    let toolchain_link = paths::toolchain_link(home, &channel.name);

    // Every install produces a *new* publication, named opaquely. Nothing may infer identity from
    // the name: a name derived from the plan key would invite treating equal keys as equal bytes,
    // which nothing verifies.
    let publication_id = PublicationId::generate();
    let publication = paths::publication_dir(home, &channel.name, &publication_id);
    let relative_publication = PathBuf::from("..")
        .join("publications")
        .join(publication.file_name().expect("named"));

    let selection = crate::resolve::Intent {
        profiles: [options.profile].into_iter().collect(),
        roots: options.components.iter().cloned().collect(),
    };
    let plan = crate::plan::build_plan(channel, &selection, config.target(), &publication)?;

    // What the publication being replaced owns, if this build published it. Only a receipt can
    // say; a directory listing cannot distinguish installed content from anything else that
    // happens to be there.
    let previous = previous_publication(config, state, &channel.name);
    let stale: Vec<String> =
        options.components_to_uninstall.iter().map(|c| c.name.to_string()).collect();

    crate::install::prepare(&publication)?;
    if let Some((previous_dir, receipt)) = &previous {
        crate::install::seed(
            &plan,
            &publication,
            &crate::install::Seed {
                publication: previous_dir,
                receipt,
                stale: &stale,
            },
        )?;
    }

    let realized = crate::install::execute(&plan, &publication, options.verbose, config.debug)?;

    // Structural check before anything is published: every planned file exists, is a regular
    // file, and carries the planned mode. Contents are not verified -- digests are recorded but
    // never checked -- so this asserts the plan was carried out, not what was installed.
    crate::install::verify(&plan, &publication)?;

    // The receipt makes the publication self-describing: from here on, what it owns is a fact
    // recorded inside it rather than something re-derived from a manifest that may have moved on.
    let receipt = crate::publish::receipt_for(
        &plan,
        &publication,
        &publication_id,
        &realized,
        previous.as_ref().map(|(_, receipt)| receipt),
    );
    crate::publish::write_receipt(&publication, &receipt)?;

    let temp_symlink = paths::publications_dir(home).join(format!("{}.new", channel.name));
    if std::fs::symlink_metadata(&temp_symlink).is_ok() {
        std::fs::remove_file(&temp_symlink).with_context(|| {
            format!("failed to remove stale temp symlink '{}'", temp_symlink.display())
        })?;
    }

    // ======================== Installation finalized  ===========================

    // tmp_link is a symlink file that points to the new publication. Even if tmp_link is moved, it
    // will still point there.
    // For further reference on atomic directory updates, see:
    // https://axialcorps.wordpress.com/2013/07/03/atomically-replacing-files-and-directories/
    utils::fs::symlink(&temp_symlink, &relative_publication)?;

    // We now rename tmp_link onto the channel's toolchain link. When renamed, it will still be
    // pointing to the new publication. If the channel link existed, it is overwritten. This is
    // what marks the install as completed.
    std::fs::rename(&temp_symlink, &toolchain_link).with_context(|| {
        format!(
            "failed to publish toolchain symlink '{}' -> '{}'",
            toolchain_link.display(),
            relative_publication.display()
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
                id: publication_id,
                plan_key: plan.key.clone(),
                target: config.target().to_string(),
            },
            installed_at: chrono::Utc::now().timestamp(),
        });
    }

    config.write_local_state(state)?;

    Ok(())
}

/// The publication this install is replacing, and what it owns.
///
/// `None` when the channel is not installed, was carried over from v1 (so nothing describes it),
/// or its receipt is unreadable. In every one of those cases the correct behaviour is the same:
/// seed nothing and install from scratch. Guessing ownership from a directory listing is what this
/// exists to avoid.
fn previous_publication(
    config: &Config,
    state: &LocalState,
    channel: &semver::Version,
) -> Option<(PathBuf, crate::state::Receipt)> {
    let installation = state.get(channel)?;
    let PublicationRef::Managed { id, .. } = &installation.publication else {
        return None;
    };
    let dir = paths::publication_dir(&config.midenup_home, channel, id);
    let receipt = crate::publish::read_receipt(&dir).ok()?;
    Some((dir, receipt))
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
