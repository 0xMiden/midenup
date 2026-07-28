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
    fault,
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

    // Every install produces a *new* publication, named opaquely. Nothing may infer identity from
    // the name: a name derived from the plan key would invite treating equal keys as equal bytes,
    // which nothing verifies.
    let publication_id = PublicationId::generate();
    let publication = paths::publication_dir(home, &channel.name, &publication_id);

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

    // 1. PREPARE. The record this operation intends to commit is written down *before* any of it
    // happens, so that a crash anywhere after this point can be completed or discarded rather than
    // reconstructed by inspection.
    let mut entry = crate::publish::JournalEntry::install(
        channel.name.clone(),
        previous_publication_id(state, &channel.name),
        publication_id.clone(),
        target_installation(config, channel, state, options, &publication_id, &plan),
    );
    crate::publish::journal::prepare(home, &entry)?;
    fault::fail_at(fault::FaultPoint::PostPrepare)?;

    // 2. STAGE.
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
    fault::fail_at(fault::FaultPoint::PostStage)?;

    // 3. VERIFY. Structural check before anything is published: every planned file exists, is a
    // regular file, and carries the planned mode. Contents are not verified -- digests are
    // recorded but never checked -- so this asserts the plan was carried out, not what was
    // installed.
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

    // A `cargo install --path` build can touch its own source tree, so a `path` component's
    // modification time is only knowable once the build is done. Refreshing it here and amending
    // the journal -- atomically, and still before the commit point -- keeps the recorded time
    // equal to what the *next* run will observe before building. Recording the pre-build time
    // instead makes every subsequent update believe the source changed.
    if let Some(installation) = entry.target_installation.as_mut() {
        refresh_path_modification_times(config, &mut installation.components);
    }
    crate::publish::journal::prepare(home, &entry)?;
    fault::fail_at(fault::FaultPoint::PostVerify)?;

    // ======================== 4. COMMIT — the commit point ======================
    //
    // A single atomic rename of a symlink. Before it, this operation never happened and recovery
    // discards it; after it, recovery completes it. Nothing else distinguishes the two.
    crate::publish::journal::commit_symlink(home, &entry)?;
    fault::fail_at(fault::FaultPoint::PostCommit)?;

    // 5. RECORD.
    crate::publish::journal::record(home, &entry, state)?;
    fault::fail_at(fault::FaultPoint::PostRecord)?;

    // 6. DERIVE. `stable` is a property of the upstream manifest, recomputed from it rather than
    // remembered, so a stale local copy can never disagree with upstream about which channel it
    // names.
    if config.manifest.is_latest_stable(channel) {
        let stable_dir = toolchains_dir.join("stable");
        if std::fs::symlink_metadata(&stable_dir).is_ok() {
            std::fs::remove_file(&stable_dir).context("Couldn't remove stable symlink")?;
        }
        let relative_channel_target = PathBuf::from(format!("{}", channel.name));
        utils::fs::symlink(&stable_dir, &relative_channel_target)
            .expect("Couldn't create stable dir");
    }
    fault::fail_at(fault::FaultPoint::PostDerive)?;

    // 7. CLEAN.
    crate::publish::journal::clean(home, &entry)?;

    Ok(())
}

/// The state record this install intends to commit.
///
/// Built before anything is staged, because the journal carries it: recovery has to be able to
/// complete the operation without re-resolving it against an upstream manifest that may have moved
/// on in the meantime.
///
/// The component snapshot is pinned here rather than referencing the upstream manifest: `miden`
/// dispatch reads it offline, and update needs to know what was *actually* installed, not what
/// upstream happens to say now.
fn target_installation(
    config: &Config,
    channel: &Channel,
    state: &LocalState,
    options: &InstallationOptions,
    publication_id: &PublicationId,
    plan: &crate::plan::InstallationPlan,
) -> Installation {
    let installed_components = {
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
                Authority::Git { .. } | Authority::Path { .. } | Authority::Registry { .. } => (),
            }
        }

        // Path authorities are recorded here too, but their modification time is refreshed after
        // staging: see `refresh_path_modification_times`.
        refresh_path_modification_times(config, &mut installed_components);

        installed_components
    };

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

    Installation {
        channel: channel.name.clone(),
        intent,
        components: installed_components,
        publication: PublicationRef::Managed {
            id: publication_id.clone(),
            plan_key: plan.key.clone(),
            target: config.target().to_string(),
        },
        installed_at: chrono::Utc::now().timestamp(),
    }
}

/// Records each `path` component's source tree modification time, canonicalizing the path.
///
/// Called once while assembling the record and again after staging, because a build can modify its
/// own source tree -- Cargo writes into it -- and what matters is that the recorded time matches
/// what the next run will see *before* it builds. Anything else makes every subsequent update
/// believe the source changed.
fn refresh_path_modification_times(config: &Config, components: &mut [crate::manifest::Component]) {
    for component in components.iter_mut() {
        let Authority::Path { path, .. } = &component.version else {
            continue;
        };

        let path = if path.is_absolute() {
            Cow::Borrowed(path.as_path())
        } else {
            Cow::Owned(config.working_directory.join(path.as_path()))
        };
        let latest_time = utils::fs::latest_modification(&path)
            .ok()
            .map(|(latest_modification, _)| latest_modification)
            .unwrap_or_else(SystemTime::now);

        component.version = Authority::Path {
            path: path.to_path_buf(),
            last_modification: Some(latest_time),
        };
    }
}

/// The publication currently recorded for `channel`, if this build published it.
fn previous_publication_id(state: &LocalState, channel: &semver::Version) -> Option<PublicationId> {
    match &state.get(channel)?.publication {
        PublicationRef::Managed { id, .. } => Some(id.clone()),
        PublicationRef::NeedsReinstall => None,
    }
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
