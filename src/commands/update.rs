use anyhow::{bail, Context};

use crate::{
    channel::{Channel, UserChannel},
    commands,
    manifest::Manifest,
    Config,
};

/// Updates installed toolchains
pub fn update(config: &Config, channel_type: Option<&UserChannel>) -> anyhow::Result<()> {
    let local_manifest_path = config.midenup_home.join("manifest").with_extension("json");
    let local_manifest_uri = format!(
        "file://{}",
        local_manifest_path.to_str().context("Couldn't convert miden directory")?,
    );

    // TODO(fabrio): This could fail either because the file doesn't exist of
    // because the json is ill formatted. There should be a destinction
    let local_manifest = Manifest::load_from(local_manifest_uri).context(
        "No installed toolchains found. To install stable, run:
midenup install stable
",
    )?;

    match channel_type {
        Some(UserChannel::Stable) => {
            let local_stable = local_manifest.get_latest_stable();
            let upstream_stable = config.manifest.get_latest_stable().expect("TODO: Remove unwrap");

            // Check if local latest stable is older than upstream's
            let local_stable = local_stable.context(
                "No stable version was found. To install it, try running:
midenup install stable
",
            )?;

            if upstream_stable.name > local_stable.name {
                commands::install(config, upstream_stable)?
            } else {
                todo!()
            }
        },
        Some(UserChannel::Version(version)) => {
            // Check if any individual component changed since the last the
            // manifest was synced
            let local_version = local_manifest
                .get_channel(&UserChannel::Version(version.clone()))
                .expect("TODO: Think what this means");

            let upstream_version = local_manifest
                .get_channel(&UserChannel::Version(version.clone()))
                .expect("TODO: Think what this means");
            let updates = local_version.components_to_update(upstream_version);

            todo!()
        },
        _ => todo!(),
    }
    todo!()
}
