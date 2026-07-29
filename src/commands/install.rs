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

/// Installs `channel` as `options` describes it.
///
/// **`channel` is always the full upstream channel**, and what gets installed is decided here, by
/// resolving the effective intent against it. Every operation -- a direct install, a toolchain-file
/// activation, an update, a channel migration -- differs only in how that intent is derived, and
/// they all arrive here. Previously each caller hand-built a *narrowed* channel and passed
/// `--profile complete`, so "what should be installed" was decided in three places that disagreed:
/// activation could not add a component another project had asked for, and update suppressed every
/// new component of a partially installed channel.
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

    // The single decision this whole function turns on.
    let intent = carry_migrated_intent(state, channel, effective_intent(state, channel, options));
    let plan = crate::plan::build_plan(channel, &intent, config.target(), &publication)?;

    // What the publication being replaced owns, if this build published it. Only a receipt can
    // say; a directory listing cannot distinguish installed content from anything else that
    // happens to be there.
    let previous = previous_publication(config, state, &channel.name);
    let stale = options.stale.clone();

    // 1. PREPARE. The record this operation intends to commit is written down *before* any of it
    // happens, so that a crash anywhere after this point can be completed or discarded rather than
    // reconstructed by inspection.
    let mut entry = crate::publish::JournalEntry::install(
        channel.name.clone(),
        previous_publication_id(state, &channel.name),
        publication_id.clone(),
        target_installation(config, channel, &intent, options, &publication_id, &plan)?,
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
        let relative_channel_target = PathBuf::from(format!("{}", channel.name));
        utils::fs::replace_symlink(&stable_dir, &relative_channel_target)
            .context("failed to point 'stable' at the newly installed channel")?;
    }
    fault::fail_at(fault::FaultPoint::PostDerive)?;

    // 7. CLEAN.
    crate::publish::journal::clean(home, &entry)?;

    Ok(())
}

/// What this operation wants installed.
///
/// Installing and *recording what the user wants* are separate concerns, and this is where they
/// meet: the effective intent is both what gets resolved into a plan and what gets persisted. A
/// toolchain-file activation may only add to the record, so it unions; a direct install restates
/// it, and is allowed to shrink; an update re-resolves what is already recorded rather than
/// restating it.
pub(crate) fn effective_intent(
    state: &LocalState,
    channel: &Channel,
    options: &InstallationOptions,
) -> Intent {
    // What the caller asked for on this invocation.
    let requested = Intent {
        profiles: [options.profile].into_iter().collect(),
        roots: options.components.iter().cloned().collect(),
    };
    let previous = state.get(&channel.name).map(|installation| installation.intent.clone());

    match options.intent_update.clone() {
        // A direct `midenup install`: record exactly what the command line asked for.
        None => requested,
        Some(IntentUpdate::Replace(intent)) => intent,
        Some(IntentUpdate::Union(intent)) => {
            let mut merged = previous.unwrap_or_default();
            merged.union_with(&intent);
            merged
        },
        Some(IntentUpdate::Preserve) => previous.unwrap_or(requested),
    }
}

/// Carries a migrated selection into the install that replaces it, dropping what upstream no longer
/// has.
///
/// Two rules, both from spec section 12, and both scoped to a record that is still
/// `NeedsReinstall`:
///
/// **The migrated selection is carried, not replaced.** A migrated record exists because the
/// toolchain *was* installed; the install that resolves it is a continuation of that, not a fresh
/// choice, so `midenup install <channel>` reinstalls what the user had rather than silently
/// reducing it to the default profile. Once the record is managed, `install` replaces intent as
/// usual (section 8.1) -- which is also how a user deliberately shrinks it.
///
/// **Roots upstream no longer has are dropped with a warning.** Section 11.3 blocks an update when
/// an explicit root disappears, because the user chose that root deliberately. A migrated root was
/// not chosen in those terms -- it was inferred from a v1 record -- so blocking would strand every
/// v1 user whose channel happened to drop a component, with no way forward but deleting their state
/// by hand.
///
/// Both are one-time by construction rather than by a flag: the install they are part of replaces
/// the migrated record with a managed one.
fn carry_migrated_intent(state: &LocalState, channel: &Channel, intent: Intent) -> Intent {
    use colored::Colorize;

    let Some(migrated) = state.get(&channel.name).filter(|installation| !installation.is_managed())
    else {
        return intent;
    };

    let mut intent = intent;
    intent.union_with(&migrated.intent);

    let (kept, dropped): (Vec<String>, Vec<String>) = intent
        .roots
        .iter()
        .cloned()
        .partition(|root| channel.get_component(root).is_some());

    if dropped.is_empty() {
        return intent;
    }

    println!(
        "{}: these components are no longer part of channel {} and have been dropped from your \
         selection:",
        "warning".yellow().bold(),
        channel.name
    );
    for root in &dropped {
        println!("- {}", root.white().bold());
    }

    Intent {
        profiles: intent.profiles,
        roots: kept.into_iter().collect(),
    }
}

/// The state record this install intends to commit.
///
/// Built before anything is staged, because the journal carries it: recovery has to be able to
/// complete the operation without re-resolving it against an upstream manifest that may have moved
/// on in the meantime.
///
/// The component snapshot is the **resolved** set, pinned, rather than the whole channel: `miden`
/// dispatch reads it offline to decide what is available, and update needs to know what was
/// *actually* installed. Recording every component in the channel made a `--profile minimal`
/// installation claim to have everything, so activation never noticed a missing component.
fn target_installation(
    config: &Config,
    channel: &Channel,
    intent: &Intent,
    options: &InstallationOptions,
    publication_id: &PublicationId,
    plan: &crate::plan::InstallationPlan,
) -> anyhow::Result<Installation> {
    let installed_components = {
        let mut installed_components: Vec<crate::manifest::Component> =
            crate::resolve::resolve(channel, intent)?.into_iter().cloned().collect();

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

        // A component the update policy declined to touch keeps the definition it was installed
        // with. Recording the upstream one instead would mark it up to date without having
        // rebuilt it, and the next update would stop offering.
        for component in installed_components.iter_mut() {
            if let Some(held) = options.held_back.iter().find(|held| held.name == component.name) {
                *component = held.clone();
            }
        }

        installed_components
    };

    Ok(Installation {
        channel: channel.name.clone(),
        intent: intent.clone(),
        components: installed_components,
        publication: PublicationRef::Managed {
            id: publication_id.clone(),
            plan_key: plan.key.clone(),
            target: config.target().to_string(),
        },
        installed_at: chrono::Utc::now().timestamp(),
    })
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
