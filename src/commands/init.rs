use anyhow::{bail, Context};

use crate::Config;

/// This is the first command the user runs after first installing the midenup. It performs the
/// following tasks:
///
/// - Bootstrap the `midenup` environment (create directories, default config, etc.), if not already
///   done.
/// - Install the stable channel
pub fn init(config: &Config) -> anyhow::Result<()> {
    // Create the data directory layout.
    //
    // The following is a sketch of the directory tree and contents
    //
    // $MIDENUP_HOME
    // |- bin/
    // | |- miden --> $CARGO_INSTALL_DIR/midenup
    // |- toolchains
    // | |- <channel>/
    // | | |- bin/
    // | | |- lib/
    // | | | |- std.masp
    // |- config.toml
    // |- manifest.json

    let midenhome_dir = &config.midenup_home;
    if !midenhome_dir.exists() {
        std::fs::create_dir_all(&midenhome_dir).with_context(|| {
            format!("failed to initialize MIDENUP_HOME directory: '{}'", midenhome_dir.display())
        })?;
    }

    // The local manifest contains the installed toolchains with the
    // corresponding components
    let local_manifest_file = config.midenup_home.join("manifest").with_extension("json");
    std::fs::File::create(&local_manifest_file).with_context(|| {
        format!(
            "failed to create local manifest.json file in: '{}'",
            local_manifest_file.display()
        )
    })?;

    let bin_dir = config.midenup_home.join("bin");
    if !bin_dir.exists() {
        std::fs::create_dir_all(&bin_dir).with_context(|| {
            format!("failed to initialize MIDENUP_HOME subdirectory: '{}'", bin_dir.display())
        })?;
    }

    // Write the symlink for `miden` to $MIDENUP_HOME/bin
    let current_exe =
        std::env::current_exe().expect("unable to get location of current executable");
    let miden_exe = bin_dir.join("miden");
    if !miden_exe.exists() {
        symlink(&miden_exe, &current_exe)?;
    }

    let toolchains_dir = config.midenup_home.join("toolchains");
    if !toolchains_dir.exists() {
        std::fs::create_dir_all(&toolchains_dir).with_context(|| {
            format!(
                "failed to initialize MIDENUP_HOME subdirectory: '{}'",
                toolchains_dir.display()
            )
        })?;
    }

    let default_toolchain_dir = toolchains_dir.join("stable");
    if default_toolchain_dir.exists() {
        bail!("midenup has already been initialized");
    }

    Ok(())
}

#[cfg(unix)]
fn symlink(from: &std::path::Path, to: &std::path::Path) -> anyhow::Result<()> {
    std::os::unix::fs::symlink(to, from).context("could not create symlink")
}

#[cfg(windows)]
fn symlink(from: &std::path::Path, to: &std::path::Path) -> anyhow::Result<()> {
    std::os::windows::fs::symlink_file(to, from).context("could not create symlink")
}
