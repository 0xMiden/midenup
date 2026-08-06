use anyhow::{Context, bail};
use colored::Colorize;

use crate::{
    channel::UserChannel,
    commands,
    config::Config,
    state::LocalState,
    toolchain::{Toolchain, ToolchainJustification},
    utils,
};

/// Sets the system-wide default toolchain.
///
/// A network name is recorded as a link to the network's own link, rather than to the toolchain it
/// names today, so that `default` follows the network as it moves. A version is recorded as the
/// toolchain directory, because that is exactly what pinning a version means.
// This function requires raw identifier syntax because "override" is a reserved keyword.
// Source: https://doc.rust-lang.org/reference/keywords.html#r-lex.keywords.reserved
pub fn r#override(
    config: &Config,
    _state: &LocalState,
    channel: &UserChannel,
) -> anyhow::Result<()> {
    commands::setup_midenup(config)?;

    // We check which toolchain is active in order to inform the user in case the `override` command
    // won't take effect.
    let (active, justification) = Toolchain::current(config)?;

    let toolchains_dir = config.midenup_home.join("toolchains");
    let channel_dir = match channel {
        // A network name is indirected through its own symlink rather than resolved to a toolchain
        // directory, so that `default` keeps following the network as it moves.
        UserChannel::Named(name) => {
            let link = crate::paths::network_link(&config.midenup_home, name.as_ref());

            // Validated before it is written: unchecked, a typo would point `default` at a
            // `toolchains/` link that does not exist, and the command would report success.
            match config.upstream_manifest() {
                Ok(manifest) => {
                    if !manifest.network_names().any(|network| network == name.as_ref()) {
                        bail!(
                            "unknown channel '{name}'; known networks are {}",
                            manifest.network_names().collect::<Vec<_>>().join(", ")
                        );
                    }
                },
                // Upstream may be unreachable, and a network this machine has already acted on is
                // recorded by its own link, which is enough to accept the name offline.
                Err(err) if link.symlink_metadata().is_err() => {
                    return Err(err.context(format!(
                        "cannot check whether '{name}' is a known network, and no \
                         toolchains/{name} link exists locally"
                    )));
                },
                Err(_) => {},
            }

            link
        },
        UserChannel::Version(_) => {
            let inner_channel = config.upstream_manifest()?.get_channel(channel).context(
                "failed to set {channel} as the system default. Try installing it:
        midenup install {channel}",
            )?;
            inner_channel.get_channel_dir(config)
        },
    };

    let default_path = toolchains_dir.join("default");
    // `symlink_metadata`, not `exists`: the latter follows the link, so a dangling `default` would
    // never be removed here and every later override would fail with `EEXIST`.
    if std::fs::symlink_metadata(&default_path).is_ok() {
        std::fs::remove_file(&default_path)
            .context("failed to remove 'default' toolchain symlink")?;
    }

    println!("{}: setting {channel} as the new default toolchain\n", "info".white().bold());
    if let ToolchainJustification::MidenToolchainFile { path } = justification {
        println!(
            "{}: there is a toolchain file present in {}, which sets the current active toolchain \
             to be {}.
This will take prescedence over the configuration done by `midenup override`.",
            "warn".yellow(),
            path.display(),
            active.channel
        );
    };
    utils::fs::symlink(&default_path, &channel_dir)?;

    Ok(())
}
