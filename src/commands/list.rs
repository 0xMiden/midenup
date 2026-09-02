use colored::Colorize;

use crate::{config::Config, state::LocalState};

/// List all the available [[Channels]] presents in the upstream manifest.
pub fn list(config: &Config, state: &LocalState) -> anyhow::Result<()> {
    let upstream_channels = config.upstream_manifest()?.get_channels();
    crate::report::prepare_stdout_color();

    let toolchains_display: Vec<String> = upstream_channels
        .map(|channel| {
            let channel_name = &channel.name;

            // Update status is *derived*, never recorded (spec section 8.6): an installation has
            // an update exactly when re-resolving its recorded intent against upstream would
            // change it. A stored flag would be a second answer to a question the manifest and
            // the component set already answer, and the two would drift.
            let installed_indicator = match state.get(&channel.name) {
                Some(installation) if super::update::needs_update(installation, channel) => {
                    format!(" {}", "(update available)".yellow())
                },
                Some(_) => format!(" {}", "(installed)".green()),
                // A channel that supersedes an installed one (spec section 11.4): updating the
                // predecessor migrates to it.
                None if channel
                    .migrates_from
                    .as_ref()
                    .is_some_and(|old| state.get(old).is_some()) =>
                {
                    format!(" {}", "(update available)".yellow())
                },
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
