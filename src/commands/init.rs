use std::path::PathBuf;

use anyhow::Context;
use thiserror::Error;

use crate::{config::Config, manifest::Manifest, options::DEFAULT_USER_DATA_DIR, utils};

#[derive(Error, Debug)]
pub enum InitializationError {
    #[error("Could not determine cargo bin directory. Set CARGO_HOME or HOME.")]
    CargoBinNotFound,
    #[error("Failed to create directory: '{0}'. {1}")]
    DirectoryCreationFailed(PathBuf, String),
    #[error("Failed to create file: '{0}'. {1}")]
    FileCreationFailed(PathBuf, String),
    #[error("Failed to create symlink. {0}")]
    SymlinkFailed(String),
}

pub enum InitializationState {
    AlreadyInitialized,
    Initialized,
}

/// Get the user's cargo bin directory.
///
/// If the user has `$CARGO_HOME/bin` set, then use it. If not, fallback to `$HOME/.cargo/bin`.
///
/// This relies on the behavior described [here](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-reads)
fn cargo_bin_dir() -> Result<PathBuf, InitializationError> {
    if let Some(cargo_home) = std::env::var_os("CARGO_HOME") {
        return Ok(PathBuf::from(cargo_home).join("bin"));
    }
    if let Some(home) = dirs::home_dir() {
        return Ok(home.join(".cargo").join("bin"));
    }
    Err(InitializationError::CargoBinNotFound)
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
            InitializationError::DirectoryCreationFailed(midenhome_dir.clone(), e.to_string())
        })?;
        state = InitializationState::Initialized;
    }
    let local_manifest_file = config.midenup_home.join("manifest").with_extension("json");
    if !local_manifest_file.exists() {
        std::fs::File::create(&local_manifest_file).map_err(|e| {
            InitializationError::FileCreationFailed(local_manifest_file.clone(), e.to_string())
        })?;
        state = InitializationState::Initialized;
    }

    let toolchains_dir = config.midenup_home.join("toolchains");
    if !toolchains_dir.exists() {
        std::fs::create_dir_all(&toolchains_dir).map_err(|e| {
            InitializationError::DirectoryCreationFailed(toolchains_dir.clone(), e.to_string())
        })?;
        state = InitializationState::Initialized;
    }

    let installed_toolchains_dir = config.midenup_home.join("installed_toolchains");
    if !installed_toolchains_dir.exists() {
        std::fs::create_dir_all(&installed_toolchains_dir).map_err(|e| {
            InitializationError::DirectoryCreationFailed(
                installed_toolchains_dir.clone(),
                e.to_string(),
            )
        })?;
        state = InitializationState::Initialized;
    }

    // Install the `miden` symlink.
    {
        // Write the symlink for `miden` to $CARGO_HOME/bin
        let cargo_bin = cargo_bin_dir()?;
        if !cargo_bin.exists() {
            // In most cases, this directory should already directory
            std::fs::create_dir_all(&cargo_bin).map_err(|e| {
                InitializationError::DirectoryCreationFailed(cargo_bin.clone(), e.to_string())
            })?;
        }

        let current_exe =
            std::env::current_exe().expect("unable to get location of current executable");
        let miden_exe = cargo_bin.join("miden");
        if !miden_exe.exists() {
            utils::fs::symlink(&miden_exe, &current_exe)
                .map_err(|e| InitializationError::SymlinkFailed(e.to_string()))?;
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
            let midenup_home_dir = if std::env::var(DEFAULT_USER_DATA_DIR).is_ok() {
                String::from("${{XDG_DATA_HOME}}")
            } else {
                // Some OSs, like MacOs, don't define the XDG_* family of environment variables. In
                // those cases, we fall back on data_dir
                state = InitializationState::Initialized;

                dirs::data_dir()
                    .and_then(|dir| dir.into_os_string().into_string().ok())
                    .unwrap_or(String::from("${{HOME}}/.local/share"))
            };

            println!(
                "
Could not find `miden` executable in the system's PATH.

The `miden` symlink was placed in $CARGO_HOME/bin ({cargo_bin_display}), which should already be \
                 in your PATH if you have Rust installed. If not, ensure $CARGO_HOME/bin is in \
                 your PATH.

You may also need to add midenup's opt directory for toolchain components:

export MIDENUP_HOME='{midenup_home_dir}/midenup'
export PATH=${{MIDENUP_HOME}}/opt:$PATH

To your shell's profile file.
",
                cargo_bin_display = cargo_bin.display(),
            );
        }
    }

    execute_migrations(config, local_manifest).unwrap();

    Ok(state)
}

fn execute_migrations(config: &Config, local_manifest: &Manifest) -> anyhow::Result<()> {
    // Versions with known migrations
    const ATOMIC_INSTALLATION: semver::Version = semver::Version::new(1, 0, 1);

    let latest_local_version = &local_manifest.manifest_version;
    if latest_local_version < &ATOMIC_INSTALLATION {
        const OBSOLETE_FILES: [&str; 2] = [".installed_channel.json", "installation-successful"];

        let toolchains_dir = config.midenup_home.join("toolchains");
        let installed_toolchains_dir = config.midenup_home.join("installed_toolchains");

        for channel in local_manifest.get_channels() {
            let hash = channel.content_hash();
            let old_toolchain_dir = toolchains_dir.join(channel.name.to_string());
            let is_real_dir = old_toolchain_dir
                .symlink_metadata()
                .map(|metadata| metadata.file_type().is_dir())
                .unwrap_or(false);
            if !is_real_dir {
                continue;
            }

            let install_dir_name = format!("{}-{}", &channel.name, hash);
            let install_dir = installed_toolchains_dir.join(&install_dir_name);

            std::fs::create_dir_all(&install_dir).with_context(|| {
                format!("failed to create install directory: '{}'", install_dir.display())
            })?;

            utils::fs::copy_dir_recursive(&old_toolchain_dir, &install_dir, &OBSOLETE_FILES)
                .with_context(|| {
                    format!(
                        "failed to migrate toolchain from '{}' to '{}'",
                        old_toolchain_dir.display(),
                        install_dir.display()
                    )
                })?;

            let opt_dir = install_dir.join("opt");
            if opt_dir.exists() {
                for entry in std::fs::read_dir(&opt_dir)
                    .with_context(|| format!("failed to read directory '{}'", opt_dir.display()))?
                {
                    let entry = entry.with_context(|| {
                        format!("failed to read entry in '{}'", opt_dir.display())
                    })?;

                    let link = entry.path();
                    let old_target = std::fs::read_link(&link)
                        .with_context(|| format!("failed to read symlink '{}'", link.display()))?;
                    let binary = old_target.file_name().with_context(|| {
                        format!("symlink target has no file name: '{}'", old_target.display())
                    })?;
                    let relative_target = std::path::Path::new("..").join("bin").join(binary);

                    std::fs::remove_file(&link).with_context(|| {
                        format!("failed to remove symlink '{}'", link.display())
                    })?;
                    utils::fs::symlink(&link, &relative_target).with_context(|| {
                        format!(
                            "failed to recreate symlink '{}' -> '{}'",
                            link.display(),
                            relative_target.display()
                        )
                    })?;
                }
            }

            let relative_install_target =
                PathBuf::from("..").join("installed_toolchains").join(&install_dir_name);

            std::fs::remove_dir_all(dbg!(&old_toolchain_dir)).with_context(|| {
                format!(
                    "failed to remove old toolchain directory: '{}'",
                    old_toolchain_dir.display()
                )
            })?;
            utils::fs::symlink(&old_toolchain_dir, &relative_install_target).with_context(
                || {
                    format!(
                        "failed to symlink '{}' -> '{}'",
                        old_toolchain_dir.display(),
                        relative_install_target.display()
                    )
                },
            )?;
        }

        let stable_symlink = toolchains_dir.join("stable");
        let stable_is_symlink = stable_symlink
            .symlink_metadata()
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false);
        if stable_is_symlink {
            let old_target = std::fs::read_link(&stable_symlink).with_context(|| {
                format!("failed to read symlink '{}'", stable_symlink.display())
            })?;
            if old_target.is_absolute() {
                let channel_name = old_target.file_name().with_context(|| {
                    format!("symlink target has no file name: '{}'", old_target.display())
                })?;
                let relative_target = std::path::Path::new(channel_name);

                std::fs::remove_file(&stable_symlink).with_context(|| {
                    format!("failed to remove symlink '{}'", stable_symlink.display())
                })?;
                utils::fs::symlink(&stable_symlink, relative_target).with_context(|| {
                    format!(
                        "failed to recreate symlink '{}' -> '{}'",
                        stable_symlink.display(),
                        relative_target.display()
                    )
                })?;
            }
        }
    }

    Ok(())
}

pub fn init(config: &Config, local_manifest: &Manifest) -> Result<(), InitializationError> {
    let state = setup_midenup(config, local_manifest)?;

    match state {
        InitializationState::Initialized => println!(
            "midenup was successfully initialized in:\n{}",
            config.midenup_home.as_path().display()
        ),
        InitializationState::AlreadyInitialized => {
            println!("midenup already initialized in:\n{}", config.midenup_home.as_path().display())
        },
    }

    Ok(())
}
