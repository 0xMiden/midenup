use std::path::PathBuf;

use thiserror::Error;

use crate::{config::Config, manifest::Manifest, migration, options::DEFAULT_USER_DATA_DIR, utils};

#[derive(Error, Debug)]
pub enum InitializationError {
    #[error("Failed to create directory: '{0}'. {1}")]
    DirectoryCreation(PathBuf, String),
    #[error("Failed to create file: '{0}'. {1}")]
    FileCreation(PathBuf, String),
    #[error("Failed to create symlink. {0}")]
    Symlink(String),
    #[error(transparent)]
    Migration(anyhow::Error),
}

pub enum InitializationState {
    AlreadyInitialized,
    Initialized,
}

pub fn init(config: &Config, local_manifest: &Manifest) -> Result<(), InitializationError> {
    let state = setup_midenup(config, local_manifest)?;

    match state {
        InitializationState::Initialized => println!(
            "midenup was successfully initialized in: {}",
            config.midenup_home.as_path().display()
        ),
        InitializationState::AlreadyInitialized => {
            println!("midenup already initialized in: {}", config.midenup_home.as_path().display())
        },
    }

    Ok(())
}

/// This functions bootstrap the `midenup` environment, if not already initialized.
///
/// Initialization is comprised of:
///
/// * Create `MIDENUP_HOME` directory structure
/// * Create the `miden` executable symlink
///
/// NOTE: An environment is considered to be "uninitialized" if *at least* one element (be it a
/// file, directory, etc) is missing,
///
/// The following is a sketch of the directory tree and contents:
///
/// ```text,ignore
/// $MIDENUP_HOME
/// |- toolchains/
/// | |- stable     --> <channel>
/// | |- <channel>  --> ../installed_toolchains/<channel>-<hash>
/// |- installed_toolchains/
/// | |- <channel>-<hash>/
/// | | |- bin/
/// | | |- lib/
/// | | | |- std.masp
/// | | |- opt/
/// | | |- var/
/// |- config.toml
/// |- manifest.json
/// ```
///
/// Additionally, a `miden` symlink is created in `$CARGO_HOME/bin/` pointing to the midenup
/// executable.
pub fn setup_midenup(
    config: &Config,
    local_manifest: &Manifest,
) -> Result<InitializationState, InitializationError> {
    let mut state = InitializationState::AlreadyInitialized;

    let midenhome_dir = &config.midenup_home;
    if !midenhome_dir.exists() {
        std::fs::create_dir_all(midenhome_dir).map_err(|e| {
            InitializationError::DirectoryCreation(midenhome_dir.clone(), e.to_string())
        })?;
        state = InitializationState::Initialized;
    }
    let local_manifest_file = config.midenup_home.join("manifest").with_extension("json");
    if !local_manifest_file.exists() {
        std::fs::File::create(&local_manifest_file).map_err(|e| {
            InitializationError::FileCreation(local_manifest_file.clone(), e.to_string())
        })?;
        state = InitializationState::Initialized;
    }

    let toolchains_dir = config.midenup_home.join("toolchains");
    if !toolchains_dir.exists() {
        std::fs::create_dir_all(&toolchains_dir).map_err(|e| {
            InitializationError::DirectoryCreation(toolchains_dir.clone(), e.to_string())
        })?;
        state = InitializationState::Initialized;
    }

    let installed_toolchains_dir = config.midenup_home.join("installed_toolchains");
    if !installed_toolchains_dir.exists() {
        std::fs::create_dir_all(&installed_toolchains_dir).map_err(|e| {
            InitializationError::DirectoryCreation(installed_toolchains_dir.clone(), e.to_string())
        })?;
        state = InitializationState::Initialized;
    }

    // Install the `miden` symlink.
    {
        // Write the symlink for `miden` to $CARGO_HOME/bin
        let cargo_bin = config.cargo_home.join("bin");
        if !cargo_bin.exists() {
            // In most cases, this directory should already directory
            std::fs::create_dir_all(&cargo_bin).map_err(|e| {
                InitializationError::DirectoryCreation(cargo_bin.clone(), e.to_string())
            })?;
        }

        let current_exe =
            std::env::current_exe().expect("unable to get location of current executable");
        let miden_exe = cargo_bin.join("miden");
        if !miden_exe.exists() {
            utils::fs::symlink(&miden_exe, &current_exe)
                .map_err(|e| InitializationError::Symlink(e.to_string()))?;
            state = InitializationState::Initialized;
        }

        // We check if the `miden` executable is accessible via the $PATH. This is most certainly
        // not going to be the case the first time `midenup` is initialized.
        let miden_is_accessible = std::process::Command::new("miden")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .arg("--version")
            .output()
            .is_ok();

        if !miden_is_accessible {
            if std::env::var(DEFAULT_USER_DATA_DIR).is_err() {
                // Some OSs, like MacOs, don't define the XDG_* family of environment variables. In
                // those cases, we mark the environment as initialized so the updated guidance
                // below is surfaced on first-run.
                state = InitializationState::Initialized;
            }

            println!(
                "
Could not find `miden` executable in the system's PATH.

The `miden` symlink was placed in $CARGO_HOME/bin ({cargo_bin_display}), which should already be \
                 in your PATH if you have Rust installed. If not, ensure $CARGO_HOME/bin is in \
                 your PATH.

Add the directory containing the `miden` symlink to your shell's profile file. For the default
Rust installation this is usually:

export PATH=\"{cargo_bin_display}:$PATH\"

On macOS with zsh, add that line to ~/.zprofile (create the file first if it does not exist),
then start a new shell or run:

source ~/.zprofile
",
                cargo_bin_display = cargo_bin.display(),
            );
        }
    }

    migration::run_toolchain_migration(config, local_manifest)
        .map_err(InitializationError::Migration)?;

    Ok(state)
}
