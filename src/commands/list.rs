use colored::Colorize;

use crate::{config::Config, state::LocalState};

/// List all the available [[Channels]] presents in the upstream manifest.
pub fn list(config: &Config, state: &LocalState) -> anyhow::Result<()> {
    let upstream_channels = config.upstream_manifest()?.get_channels();

    let toolchains_display: Vec<String> = upstream_channels
        .map(|channel| {
            let channel_name = &channel.name;

            // Partial status is *derived*, never recorded (spec section 8.6): an installation is
            // partial exactly when it holds fewer components than the channel offers. A stored
            // flag would be a second answer to a question the component set already answers, and
            // the two would drift.
            let installed_indicator = match state.get(&channel.name) {
                Some(installation) if installation.as_channel().is_partially_installed(channel) => {
                    format!(" {}", "(partially installed)".yellow())
                },
                Some(_) => format!(" {}", "(installed)".green()),
                None => String::new(),
            };

            format!("{channel_name}{installed_indicator}")
        })
        .collect();

    println!("{}", "Available toolchains upstream:".bold().underline());
    for toolchain in toolchains_display {
        println!("{toolchain}");
    }

    Ok(())
}
