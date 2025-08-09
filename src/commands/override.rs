// This function is called r#override because "override" is a reserved keyword.
// Source: https://doc.rust-lang.org/reference/keywords.html#r-lex.keywords.reserved

use anyhow::Context;

use crate::{Config, channel::UserChannel, commands, utils};

/// This functions sets the system's default toolchain. This is handled
/// similarly to how we handle the `stable`. We create a symlink called
/// `default` that points to the desired toolchain directory.
pub fn r#override(config: &Config, channel: &UserChannel) -> anyhow::Result<()> {
    commands::setup_midenup(config)?;

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

    println!("Setting {channel} as the new default toolchain");
    utils::symlink(&default_path, &channel_dir)?;

    Ok(())
}
