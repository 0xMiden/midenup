use anyhow::Context;
use colored::Colorize;

use crate::{
    channel::UserChannel,
    commands,
    config::Config,
    manifest::Manifest,
    toolchain::{Toolchain, ToolchainJustification},
    utils,
};

/// This functions sets the system's default toolchain. This is handled similarly to how we handle
/// the `stable`. We create a symlink called `default` that points to the desired toolchain
/// directory.
// This function requires raw identifier syntax because "override" is a reserved keyword.
// Source: https://doc.rust-lang.org/reference/keywords.html#r-lex.keywords.reserved
pub fn r#override(
    config: &Config,
    local_manifest: &Manifest,
    channel: &UserChannel,
) -> anyhow::Result<()> {
    commands::setup_midenup(config, local_manifest)?;

    // We check which toolchain is active in order to inform the user in case the `override` command
    // won't take effect.
    let (active, justification) = Toolchain::current(config)?;

    let toolchains_dir = config.midenup_home.join("toolchains");
    let channel_dir = match channel {
        // If a user sets `stable` to be the default; then we need to point to the `stable` symlink
        // itself and *not* the underlying toolchain directory. In effect, this allows the user to
        // always be using the stable toolchain, even after updates occur.
        UserChannel::Stable => toolchains_dir.join("stable"),
        _ => {
            let inner_channel = config.manifest.get_channel(channel).context(
                "failed to set {channel} as the system default. Try installing it:
        midenup install {channel}",
            )?;
            inner_channel.get_channel_dir(config)
        },
    };

    let default_path = toolchains_dir.join("default");
    if default_path.exists() {
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
