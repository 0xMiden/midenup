use anyhow::Context;

use crate::{
    Config, MIDENUP_PARENT_DEFAULT_DIR, commands, config::ensure_midenup_home_exists,
    manifest::Manifest,
};

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
    let miden_is_accessible = std::process::Command::new("miden").arg("--version").output().is_ok();

    if !miden_is_accessible {
        let midenup_home_dir = match std::env::var(MIDENUP_PARENT_DEFAULT_DIR) {
            Ok(_) => String::from("${{XDG_DATA_HOME}}"),
            // Some OSs, like MacOs, don't define the XDG_* family of
            // environment variables. In those cases, we fall back on data_dir
            Err(_) => dirs::data_dir()
                .and_then(|dir| dir.into_os_string().into_string().ok())
                .unwrap_or(String::from("${{HOME}}/.local/share")),
        };

        std::println!(
            "
Could not find `miden` executable in the system's PATH. To enable it, add midenup's bin directory to your system's PATH. 

export MIDENUP_HOME={midenup_home_dir}/midenup
export PATH=${{MIDENUP_HOME}}/bin:$PATH

To your shell's profile file.
"
        );
    }

    Ok(())
}
