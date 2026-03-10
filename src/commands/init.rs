use std::path::PathBuf;

use anyhow::Context;

use crate::{Config, DEFAULT_USER_DATA_DIR, utils};

/// Get the user's cargo bin directory. If the user has '$CARGO_HOME/bin' set,
/// then use it. If not, fallback to '$HOME/.cargo/bin'.
/// This relies on the behavior described here:
/// https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-reads
fn cargo_bin_dir() -> anyhow::Result<PathBuf> {
    if let Some(cargo_home) = std::env::var_os("CARGO_HOME") {
        return Ok(PathBuf::from(cargo_home).join("bin"));
    }
    if let Some(home) = dirs::home_dir() {
        return Ok(home.join(".cargo").join("bin"));
    }
    anyhow::bail!("Could not determine cargo bin directory. Set CARGO_HOME or HOME.")
}

/// This functions bootstrap the `midenup` environment (creates basic directory
/// structure, creates the miden executable symlink, etc.), if not already
/// initialized. The boolean represents whether midenup had already been
/// initalized or not.
/// NOTE: An environment is considered to be "uninitialized" if *at least* one element
/// (be it a file, directory, etc) is missing,
///
/// The following is a sketch of the directory tree and contents
///
/// $MIDENUP_HOME
/// |- opt/
/// | |- symlinks
/// |- toolchains
/// | |- stable/ --> <channel>/
/// | |- <channel>/
/// | | |- bin/
/// | | |- lib/
/// | | | |- std.masp
/// |- config.toml
/// |- manifest.json
///
/// Additionally, a `miden` symlink is created in `$CARGO_HOME/bin/` pointing
/// to the midenup executable.
pub fn setup_midenup(config: &Config) -> anyhow::Result<bool> {
    let mut already_initialized = true;

    let midenhome_dir = &config.midenup_home;
    if !midenhome_dir.exists() {
        std::fs::create_dir_all(midenhome_dir).with_context(|| {
            format!("failed to initialize MIDENUP_HOME directory: '{}'", midenhome_dir.display())
        })?;
        already_initialized = false;
    }
    let local_manifest_file = config.midenup_home.join("manifest").with_extension("json");
    if !local_manifest_file.exists() {
        std::fs::File::create(&local_manifest_file).with_context(|| {
            format!(
                "failed to create local manifest.json file in: '{}'",
                local_manifest_file.display()
            )
        })?;
        already_initialized = false;
    }

    // Write the symlink for `miden` to $CARGO_HOME/bin
    let cargo_bin = cargo_bin_dir()?;
    if !cargo_bin.exists() {
        // In most cases, this directory should already directory
        std::fs::create_dir_all(&cargo_bin).with_context(|| {
            format!("failed to create cargo bin directory: '{}'", cargo_bin.display())
        })?;
    }
    let current_exe =
        std::env::current_exe().expect("unable to get location of current executable");
    let miden_exe = cargo_bin.join("miden");
    if !miden_exe.exists() {
        utils::fs::symlink(&miden_exe, &current_exe)?;
        already_initialized = false;
    }

    let toolchains_dir = config.midenup_home.join("toolchains");
    if !toolchains_dir.exists() {
        std::fs::create_dir_all(&toolchains_dir).with_context(|| {
            format!(
                "failed to initialize MIDENUP_HOME subdirectory: '{}'",
                toolchains_dir.display()
            )
        })?;
        already_initialized = false;
    }

    // We check if the `miden` executable is accessible via the $PATH. This is
    // most certainly not going to be the case the first time `midenup` is
    // initialized.
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
            // Some OSs, like MacOs, don't define the XDG_* family of
            // environment variables. In those cases, we fall back on data_dir
            already_initialized = false;

            dirs::data_dir()
                .and_then(|dir| dir.into_os_string().into_string().ok())
                .unwrap_or(String::from("${{HOME}}/.local/share"))
        };

        println!(
            "
Could not find `miden` executable in the system's PATH.

The `miden` symlink was placed in $CARGO_HOME/bin ({cargo_bin_display}), which should already be \
in your PATH if you have Rust installed. If not, ensure $CARGO_HOME/bin is in your PATH.

You may also need to add midenup's opt directory for toolchain components:

export MIDENUP_HOME='{midenup_home_dir}/midenup'
export PATH=${{MIDENUP_HOME}}/opt:$PATH

To your shell's profile file.
",
            cargo_bin_display = cargo_bin.display(),
        );
    }

    Ok(already_initialized)
}

pub fn init(config: &Config) -> anyhow::Result<()> {
    let already_initialized = setup_midenup(config)?;

    if !already_initialized {
        println!(
            "midenup was successfully initialized in:
{}",
            config.midenup_home.as_path().display()
        );
    } else {
        println!(
            "midenup already initialized in:
{}",
            config.midenup_home.as_path().display()
        );
    }

    Ok(())
}
