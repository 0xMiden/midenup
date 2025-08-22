use anyhow::Context;

use crate::{utils, Config, DEFAULT_USER_DATA_DIR};

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
/// |- bin/
/// | |- miden --> $CARGO_INSTALL_DIR/midenup
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

    let bin_dir = config.midenup_home.join("bin");
    if !bin_dir.exists() {
        std::fs::create_dir_all(&bin_dir).with_context(|| {
            format!("failed to initialize MIDENUP_HOME subdirectory: '{}'", bin_dir.display())
        })?;
        already_initialized = false;
    }

    let opt_dir = config.midenup_home.join("opt");
    if !opt_dir.exists() {
        std::fs::create_dir_all(&opt_dir).with_context(|| {
            format!("failed to initialize MIDENUP_HOME subdirectory: '{}'", opt_dir.display())
        })?;
        already_initialized = false;
    }

    // Write the symlink for `miden` to $MIDENUP_HOME/bin
    let current_exe =
        std::env::current_exe().expect("unable to get location of current executable");
    let miden_exe = bin_dir.join("miden");
    if !miden_exe.exists() {
        utils::symlink(&miden_exe, &current_exe)?;
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
Could not find `miden` executable in the system's PATH. To enable it, add midenup's bin directory to your system's PATH. 

export MIDENUP_HOME='{midenup_home_dir}/midenup'
export PATH=${{MIDENUP_HOME}}/bin:$PATH

To your shell's profile file.
"
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
