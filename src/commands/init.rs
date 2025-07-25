use std::ffi::OsStr;

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
{}
",
        config.midenup_home.as_path().display()
    );

    // We check if the `miden` executable is accessible via the $PATH. This is
    // most certainly not going to be the case the first time `midenup` is
    // initialized.

    let miden_is_accessible = std::env::var_os("PATH")
        .as_ref()
        .map(|paths| {
            std::env::split_paths(paths).any(|exe| exe.file_name() == Some(OsStr::new("miden")))
        })
        .unwrap_or(false);

    if !miden_is_accessible {
        std::println!(
            "Could not find `miden` executable in the system's PATH. To enable it, add midenup's bin directory to your system's PATH. "
        );
        match std::env::consts::OS {
            "macos" => {
                std::println!(
                    "On MacOS, you could try adding:

export MIDENUP_HOME=\"{{$HOME}}/Library/Application Support/midenup\"
export PATH=${{MIDENUP_HOME}}/bin:$PATH

To your shell's profile file.
"
                );
            },
            _ => {
                std::println!(
                    "You could try adding:

export MIDENUP_HOME=$XDG_DATA_DIR/midenup
export PATH=${{MIDENUP_HOME}}/bin:$PATH

To your shell's profile file.
"
                );
            },
        };
    }

    Ok(())
}
