// This function is called r#override because "override" is a reserved keyword.
// Source: https://doc.rust-lang.org/reference/keywords.html#r-lex.keywords.reserved

use anyhow::Context;
use colored::Colorize;

use crate::{
    channel::UserChannel,
    commands,
    config::Config,
    toolchain::{Toolchain, ToolchainJustification},
    utils,
};

/// This functions sets the system's default toolchain. This is handled
/// similarly to how we handle the `stable`. We create a symlink called
/// `default` that points to the desired toolchain directory.
pub fn r#override(config: &Config, channel: &UserChannel) -> anyhow::Result<()> {
    commands::setup_midenup(config)?;

    // We check which toolchain is active in order to inform the user in case
    // the `override` command won't take effect.
    let (active, justification) = Toolchain::current(config)?;

    let toolchains_dir = config.midenup_home.join("toolchains");
    let channel_dir = match channel {
        // If a user sets `stable` to be the default; then we need to point to
        // the `stable` symlink itself and *not* the underlying toolchain
        // directory. In effect, this allows the user to always be using the
        // stable toolchain, even after updates occur.
        UserChannel::Stable => toolchains_dir.join("stable"),
        _ => {
            let inner_channel = config.manifest.get_channel(channel).context(
                "Failed to set {channel} as the system default. Try installing it:
        midenup install {channel}",
            )?;
            inner_channel.get_channel_dir(config)
        },
    };

    let default_path = toolchains_dir.join("default");
    if default_path.exists() {
        std::fs::remove_file(&default_path).context("Couldn't remove 'default' symlink")?;
    }

    println!("Setting {channel} as the new default toolchain\n");
    if let ToolchainJustification::MidenToolchainFile { path } = justification {
        std::println!("{}: There is a toolchain file present in {}, which sets the current active toolchain to be {}.
This will take prescedence over the configuration done by `midenup override`.", "WARNING".yellow(), path.display(), active.channel);
    };
    utils::fs::symlink(&default_path, &channel_dir)?;

    Ok(())
}
