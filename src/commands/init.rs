use anyhow::Context;

use crate::{Config, utils};

/// This functions bootstrap the `midenup` environment (creates basic directory
/// structure, creates the miden executable symlink, etc.), if not already
/// initialized.
/// NOTE: An environment is considered to be "uninitialized" if *at least* one element
/// (be it a file, directory, etc) is missing,
///
/// The following is a sketch of the directory tree and contents
///
/// $MIDENUP_HOME
/// |- bin/
/// | |- miden --> $CARGO_INSTALL_DIR/midenup
/// |- toolchains
/// | |- stable/ --> <channel>/
/// | |- <channel>/
/// | | |- bin/
/// | | |- lib/
/// | | | |- std.masp
/// |- config.toml
/// |- manifest.json
pub fn init(config: &Config, display_messages: bool) -> anyhow::Result<()> {
    let mut already_exists = true;

    let midenhome_dir = &config.midenup_home;
    if !midenhome_dir.exists() {
        std::fs::create_dir_all(midenhome_dir).with_context(|| {
            format!("failed to initialize MIDENUP_HOME directory: '{}'", midenhome_dir.display())
        })?;
        already_exists = false;
    }
    let local_manifest_file = config.midenup_home.join("manifest").with_extension("json");
    if !local_manifest_file.exists() {
        std::fs::File::create(&local_manifest_file).with_context(|| {
            format!(
                "failed to create local manifest.json file in: '{}'",
                local_manifest_file.display()
            )
        })?;
        already_exists = false;
    }

    let bin_dir = config.midenup_home.join("bin");
    if !bin_dir.exists() {
        std::fs::create_dir_all(&bin_dir).with_context(|| {
            format!("failed to initialize MIDENUP_HOME subdirectory: '{}'", bin_dir.display())
        })?;
        already_exists = false;
    }

    // Write the symlink for `miden` to $MIDENUP_HOME/bin
    let current_exe =
        std::env::current_exe().expect("unable to get location of current executable");
    let miden_exe = bin_dir.join("miden");
    if !miden_exe.exists() {
        utils::symlink(&miden_exe, &current_exe)?;
        already_exists = false;
    }

    let toolchains_dir = config.midenup_home.join("toolchains");
    if !toolchains_dir.exists() {
        std::fs::create_dir_all(&toolchains_dir).with_context(|| {
            format!(
                "failed to initialize MIDENUP_HOME subdirectory: '{}'",
                toolchains_dir.display()
            )
        })?;
        already_exists = false;
    }

    if display_messages {
        if !already_exists {
            std::println!(
                "midenup was successfully initialized in:
{}",
                config.midenup_home.as_path().display()
            );
        } else {
            std::println!(
                "midenup already initialized in:
{}",
                config.midenup_home.as_path().display()
            );
        }
    }

    Ok(())
}
