use colored::Colorize;

use crate::{config::Config, manifest::Manifest};

/// List all the available [[Channels]] presents in the upstream manifest.
pub fn list(config: &Config, local_manifest: &Manifest) {
    let upstream_channels = config.manifest.get_channels();

    let toolchains_display: Vec<String> = upstream_channels
        .map(|channel| {
            let channel_name = &channel.name;

            let installed_indicator = if local_manifest.get_channel_by_name(&channel.name).is_some()
            {
                format!(" {}", "(installed)".green())
            } else {
                String::new()
            };

            format!("{channel_name}{installed_indicator}")
        })
        .collect();

    println!("{}", "Available toolchains upstream:".bold().underline());
    for toolchain in toolchains_display {
        println!("{toolchain}");
    }
}
