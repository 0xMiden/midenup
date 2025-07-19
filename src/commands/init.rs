use anyhow::Context;

use crate::{Config, commands, config::ensure_midenup_home_exists, manifest::Manifest};

/// This is the first command the user runs after first installing the midenup. It performs the
/// following tasks:
///
/// - Bootstrap the `midenup` environment (create directories, default config, etc.), if not already
///   done. (done in [ensure_midenup_home_exists]).
/// - Install the stable channel.
pub fn init(config: &Config, local_manifest: &mut Manifest) -> anyhow::Result<()> {
    ensure_midenup_home_exists(config)?;

    let toolchains_dir = config.midenup_home.join("toolchains");
    let stable = toolchains_dir.join("stable");
    if !stable.exists() {
        std::println!("About to install stable toolchain");

        let upstream_stable = config
            .manifest
            .get_latest_stable()
            .context("ERROR: No stable channel found in upstream")?;
        commands::install(config, upstream_stable, local_manifest)?;
    }

    println!(
        "midenup was successfully initialized in:
{}",
        config.midenup_home.as_path().display()
    );

    Ok(())
}
