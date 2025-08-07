mod channel;
mod commands;
mod config;
mod manifest;
mod miden_wrapper;
mod toolchain;
mod utils;
mod version;

use std::{ffi::OsString, path::PathBuf};

use anyhow::{Context, anyhow, bail};
use clap::{ArgAction, Args, FromArgMatches, Parser, Subcommand};

pub use self::config::Config;
use self::{
    channel::UserChannel,
    manifest::{Manifest, ManifestError},
    miden_wrapper::miden_wrapper,
};

#[derive(Debug, Parser)]
#[command(name = "midenup")]
#[command(multicall(true))]
#[command(author, version, about = "The Miden toolchain installer", long_about = None)]
pub struct Midenup {
    #[command(subcommand)]
    behavior: Behavior,
}

#[derive(Debug, Subcommand)]
enum Behavior {
    /// The Miden toolchain installer
    Midenup {
        #[command(flatten)]
        config: GlobalArgs,
        #[command(subcommand)]
        command: Commands,
    },
    /// Invoke components of the current Miden toolchain
    #[command(external_subcommand)]
    Miden(Vec<OsString>),
}

#[derive(Debug, Subcommand)]
/// All the available Midenup Commands
enum Commands {
    /// Bootstrap the `midenup` environment.
    ///
    /// This initializes the `MIDEN_HOME` directory layout and configuration.
    Init,
    /// Install a Miden toolchain
    Install {
        /// The channel or version to install, e.g. `stable` or `0.15.0`
        #[arg(required(true), value_name = "CHANNEL", value_parser)]
        channel: UserChannel,
    },
    /// Uninstall a Miden toolchain
    Uninstall {
        /// The channel or version to install, e.g. `stable` or `0.15.0`
        #[arg(required(true), value_name = "CHANNEL", value_parser)]
        channel: UserChannel,
    },
    /// Show information about the midenup environment
    #[command(subcommand)]
    Show(commands::ShowCommand),
    /// Sets the current active miden toolchain for the current project.
    /// This creates a miden-toolchain.toml file in the present working directory.
    Set {
        /// The channel or version to set, e.g. `stable` or `0.15.0`
        #[arg(required(true), value_name = "CHANNEL", value_parser)]
        channel: UserChannel,
    },
    /// Sets the system's default toolchain.
    ///
    /// Unlike `rustup`, midenup does *not* have a notion of directory
    /// overrides. Instead, the `midenup set` command can be used to configure a
    /// directory-specific toolchain.
    Override {
        /// The channel or version to set, e.g. `stable` or `0.15.0`
        #[arg(required(true), value_name = "CHANNEL", value_parser)]
        channel: UserChannel,
    },
    /// Update your installed Miden toolchains.
    Update {
        /// `midenup update`'s behavior differs depending on the specified [CHANNEL]
        /// - If provided, updates only the specified channel.
        /// - If left blank, then midenup will check for updates in all the downloaded toolchains.
        /// - If [CHANNEL] = stable, then it will look for the newest available toolchain and set
        ///   that to be stable.
        #[arg(value_name = "CHANNEL", value_parser)]
        channel: Option<UserChannel>,
    },
}

const DEFAULT_USER_DATA_DIR: &str = "XDG_DATA_HOME";

const MIDENUP_MANIFEST_URI_ENV: &str = "MIDENUP_MANIFEST_URI";
/// Global configuration options for `midenup`
#[derive(Debug, Args)]
struct GlobalArgs {
    /// The location of the Miden toolchain root
    #[arg(long, hide(true), value_name = "DIR", env = "MIDENUP_HOME")]
    midenup_home: Option<PathBuf>,
    /// The URI from which we should load the global toolchain manifest
    #[arg(
        long,
        hide(true),
        value_name = "FILE",
        env = MIDENUP_MANIFEST_URI_ENV,
        default_value = manifest::Manifest::PUBLISHED_MANIFEST_URI
    )]
    manifest_uri: String,

    /// Determines wether the components are installed in debug mode. Useful for
    /// debugging and faster installations. This flag is only avaialble to
    /// `midenup`, not `miden`.
    #[clap(env = "MIDENUP_DEBUG_MODE", action = ArgAction::Set, default_value = "false", hide = true)]
    debug: bool,
}

impl Commands {
    /// Execute the requested subcommand
    fn execute(&self, config: &Config, local_manifest: &mut Manifest) -> anyhow::Result<()> {
        match &self {
            Self::Init => commands::init(config),
            Self::Install { channel, .. } => {
                let Some(channel) = config.manifest.get_channel(channel) else {
                    bail!("channel '{}' doesn't exist or is unavailable", channel);
                };
                commands::install(config, channel, local_manifest)
            },
            Self::Uninstall { channel, .. } => commands::uninstall(config, channel, local_manifest),
            Self::Update { channel } => commands::update(config, channel.as_ref(), local_manifest),
            Self::Show(cmd) => cmd.execute(config, local_manifest),
            Self::Set { channel } => commands::set(config, channel),
            Self::Override { channel } => commands::r#override(config, channel),
        }
    }
}

fn main() -> anyhow::Result<()> {
    curl::init();

    let cli = <Midenup as clap::CommandFactory>::command();
    let matches = cli.get_matches();
    let cli = Midenup::from_arg_matches(&matches).map_err(|err| err.exit()).unwrap();

    let config = match cli.behavior {
        Behavior::Miden(_) => {
            // Always respect XDG dirs if set
            let midenup_home = std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .map(|dir| dir.join("midenup"))
                .or_else(|| dirs::data_dir().map(|dir| dir.join("midenup")))
                // If for whatever reason, we can't access the data dir, we fall
                // back to .local/share
                .or_else(|| {
                    std::env::home_dir()
                        .map(|home| home.join(".local").join("share"))
                })
                .ok_or_else(||
                            anyhow!("Failed to set midenup directory.\
                                     Consider setting a value for XDG_DATA_HOME in your shell's profile"
                            )
                )?;

            let manifest_uri = std::env::var(MIDENUP_MANIFEST_URI_ENV)
                .unwrap_or(manifest::Manifest::PUBLISHED_MANIFEST_URI.to_string());
            Config::init(midenup_home, manifest_uri, false)?
        },
        Behavior::Midenup { ref config, .. } => {
            let midenup_home = config
                .midenup_home
                .clone()
                .or_else(|| {
                    // Always respect XDG dirs if set
                    std::env::var_os("XDG_DATA_HOME")
                        .map(PathBuf::from)
                        .map(|dir| dir.join("midenup"))
                })
                .or_else(|| dirs::data_dir().map(|dir| dir.join("midenup")))
                // If for whatever reason, we can't access the data dir, we fall
                // back to .local/share
                .or_else(|| {
                    std::env::home_dir()
                        .map(|home| home.join(".local").join("share"))
                })
                .ok_or_else(||
                            anyhow!("Failed to set midenup directory.\
                                     Consider setting a value for XDG_DATA_HOME in your shell's profile"
                            )
                )?;

            Config::init(midenup_home, &config.manifest_uri, config.debug)?
        },
    };

    // Manifest that stores locally installed toolchains
    let mut local_manifest = {
        let local_manifest_path = config.midenup_home.join("manifest").with_extension("json");
        let local_manifest_uri = format!(
            "file://{}",
            local_manifest_path.to_str().context("Couldn't convert miden directory")?,
        );
        match Manifest::load_from(local_manifest_uri) {
            Ok(manifest) => Ok(manifest),
            Err(ManifestError::Empty | ManifestError::Missing(_)) => Ok(Manifest::default()),
            Err(err) => Err(err),
        }
        .context("Error parsing local manifest")
    }?;

    match cli.behavior {
        Behavior::Miden(argv) => miden_wrapper(argv, &config, &mut local_manifest),
        Behavior::Midenup { command: subcommand, .. } => {
            subcommand.execute(&config, &mut local_manifest)
        },
    }
}

#[cfg(test)]
mod tests {

    use std::path::Path;

    use crate::version::Authority;
    type LocalManifest = Manifest;
    use crate::{channel::*, manifest::*, *};

    /// Simple auxiliary function to setup a midneup directory environment in
    /// tests.
    fn test_setup(midenup_home: &Path, manifest_uri: &str) -> (LocalManifest, Config) {
        let local_manifest = {
            let local_manifest_path = midenup_home.join("manifest").with_extension("json");
            let local_manifest_uri = format!(
                "file://{}",
                local_manifest_path.to_str().expect("Couldn't convert miden directory"),
            );

            match Manifest::load_from(local_manifest_uri) {
                Ok(manifest) => Ok(manifest),
                Err(ManifestError::Empty | ManifestError::Missing(_)) => Ok(Manifest::default()),
                Err(err) => Err(err),
            }
            .unwrap_or_else(|_| {
                panic!("Failed to parse manifest {}", local_manifest_path.display())
            })
        };

        let config = Config::init(midenup_home.to_path_buf().clone(), manifest_uri, true)
            .unwrap_or_else(|err| {
                panic!(
                    "Failed to construct config from manifest {} and midenup_home at {}.
Error: {}",
                    manifest_uri,
                    midenup_home.display(),
                    err,
                )
            });

        (local_manifest, config)
    }

    fn get_full_command(argv: Vec<OsString>) -> String {
        argv.clone()
            .iter()
            .map(|arg| format!("{} ", arg.clone().into_string().unwrap()))
            .collect::<String>()
    }

    #[test]
    /// This tests serves as basic check that the install and uninstall
    /// functionalities of midenup work correctly.
    fn integration_install_uninstall_test() {
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();
        let midenup_home = tmp_home_path.join("midenup");

        const FILE: &str =
            "file://tests/data/integration_install_uninstall_test/channel-manifest.json";
        let (mut local_manifest, config) = test_setup(&midenup_home, FILE);
        let toolchain_dir = midenup_home.join("toolchains");

        // We begin by initializing the midenup directory
        let command = Midenup::try_parse_from(["midenup", "init"]).unwrap();
        let Behavior::Midenup { command, .. } = command.behavior else {
            panic!("Error while parsing test command. Expected Midneup Behavior, got Miden");
        };
        command.execute(&config, &mut local_manifest).expect("Failed to install stable");

        // We check that the basic midenup directory structure is present
        assert!(midenup_home.exists());
        assert!(midenup_home.join("bin").exists());
        assert!(toolchain_dir.exists());

        // Now, we install stable
        let command = Midenup::try_parse_from(["midenup", "install", "stable"]).unwrap();
        let Behavior::Midenup { command, .. } = command.behavior else {
            panic!("Error while parsing test command. Expected Midneup Behavior, got Miden");
        };

        // This should install version 0.16.0, since it's the latest available
        // stable toolchain present in FILE
        command.execute(&config, &mut local_manifest).expect("Failed to install stable");

        let latest_toolchain = toolchain_dir.join("0.16.0");
        assert!(latest_toolchain.exists());

        // Besides it should create the `stable` symlink
        let stable_dir = toolchain_dir.join("stable");
        assert!(stable_dir.exists());
        assert!(stable_dir.is_symlink());

        // Stable should point to 0.16.0
        let stable_toolchain =
            std::fs::read_link(&stable_dir).expect("Failed to read stable symlink");
        assert_eq!(stable_toolchain.file_name(), latest_toolchain.file_name());

        // Now we install a separate toolchain.
        let command = Midenup::try_parse_from(["midenup", "install", "0.15.0"]).unwrap();
        let Behavior::Midenup { command, .. } = command.behavior else {
            panic!("Error while parsing test command. Expected Midneup Behavior, got Miden");
        };
        command.execute(&config, &mut local_manifest).expect("Failed to install stable");

        // This should install toolchain version 0.15.0.

        let older_toolchain = toolchain_dir.join("0.15.0");
        assert!(older_toolchain.exists());

        // Besides this new toolchain, all the other directories should still
        // exists.
        assert!(stable_dir.exists());
        assert!(latest_toolchain.exists());

        let installed_toolchains = ["0.15.0", "0.16.0"].iter().map(|version| {
            semver::Version::parse(version)
                .unwrap_or_else(|_| panic!("Failed to turn {version} into semver::Version"))
        });

        // Besides creating the various directories, the local manifest should
        // also reflect this structure
        local_manifest
            .get_channels()
            .map(|channel| channel.name.clone())
            .eq(installed_toolchains);

        // Now, we'll uninstall 0.16.0.
        let command = Midenup::try_parse_from(["midenup", "uninstall", "0.16.0"]).unwrap();
        let Behavior::Midenup { command, .. } = command.behavior else {
            panic!("Error while parsing test command. Expected Midneup Behavior, got Miden");
        };

        command.execute(&config, &mut local_manifest).expect("Failed to install stable");

        // Afterwards, both the 0.16.0 directory and the `stable` symlink should
        // be deleted. But, 0.15.0 should still remain
        assert!(!latest_toolchain.exists());
        assert!(!stable_dir.exists());
        assert!(older_toolchain.exists());

        // Similarly, the local manifest should now also reflect the that the
        // older toolchain got uninstalled
        let installed_toolchains = ["0.15.0"].iter().map(|version| {
            semver::Version::parse(version)
                .unwrap_or_else(|_| panic!("Failed to turn {version} into semver::Version"))
        });

        // Besides creating the various directories, the local manifest should
        // also reflect this structure
        local_manifest
            .get_channels()
            .map(|channel| channel.name.clone())
            .eq(installed_toolchains);
    }

    #[test]
    /// This tests checks that the `miden` utility installs the current active
    /// toolchain, if not present in the system.
    fn integration_midenup_unprompted_test() {
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();
        let midenup_home = tmp_home_path.join("midenup");

        // SIDENOTE: This tests uses toolchain with version number 0.14.0. This
        // is simply used for testing purposes and is not a toolchain meant to
        // be used.
        const FILE: &str =
            "file://tests/data/integration_midenup_unprompted_test/channel-manifest.json";
        let (mut local_manifest, config) = test_setup(&midenup_home, FILE);
        let toolchain_dir = midenup_home.join("toolchains");

        // By default, the active toolchain is the latest stable version. In the
        // case of the manifest present in FILE, that is version 0.16.0.
        let command = Midenup::try_parse_from(["miden", "client", "--version"]).unwrap();
        let Behavior::Miden(argv) = command.behavior else {
            panic!("Error while parsing test command. Expected Midne Behavior, got Midenup");
        };

        miden_wrapper::miden_wrapper(argv.clone(), &config, &mut local_manifest)
            .unwrap_or_else(|err| panic!("Failed to run: {} Error: {err}", get_full_command(argv)));

        // After this, `midenup` should:
        // 1. Recognize that the user wants to run a component
        // 2. Recognize that the active toolchain is not installed, and thus trigger an installation
        // 3. Before issuing the install, it should recognize that midenup hasn't been initialized
        //    and thus needs to be initialized.

        // midenup initialized check
        assert!(midenup_home.exists());
        assert!(midenup_home.join("bin").exists());
        assert!(toolchain_dir.exists());

        // Stable toolchain installed check
        let latest_toolchain = toolchain_dir.join("0.16.0");
        assert!(latest_toolchain.exists());

        // Symlink check
        let stable_dir = toolchain_dir.join("stable");
        assert!(stable_dir.exists());
        assert!(stable_dir.is_symlink());

        // Global default

        // Now, we set a global default toolchain. This should change the
        // current active toolchain to 0.15.0.
        let command = Midenup::try_parse_from(["midenup", "override", "0.15.0"]).unwrap();
        let Behavior::Midenup { command, .. } = command.behavior else {
            panic!("Error while parsing test command. Expected Midneup Behavior, got Miden");
        };
        command.execute(&config, &mut local_manifest).expect("Failed to install stable");

        // This should also trigger an install, since toolchain 0.15.0 is
        // missing and is now the active toolchain.
        let command = Midenup::try_parse_from(["miden", "client", "--version"]).unwrap();
        let Behavior::Miden(argv) = command.behavior else {
            panic!("Error while parsing test command. Expected Midne Behavior, got Midenup");
        };

        miden_wrapper::miden_wrapper(argv.clone(), &config, &mut local_manifest)
            .unwrap_or_else(|err| panic!("Failed to run: {} Error: {err}", get_full_command(argv)));

        let older_toolchain = toolchain_dir.join("0.15.0");
        assert!(older_toolchain.exists());

        // Directory only toolchain

        // Now, we'll create a `miden-toolchain.toml` file. This will change the
        // current active toolchain.
        // By default, the active toolchain is the latest stable version. In the
        // case of the manifest present in FILE, that is version 0.16.0.
        let command = Midenup::try_parse_from(["midenup", "set", "0.14.0"]).unwrap();
        let Behavior::Midenup { command, .. } = command.behavior else {
            panic!("Error while parsing test command. Expected Midneup Behavior, got Miden");
        };
        command.execute(&config, &mut local_manifest).expect("Failed to install stable");

        // This should also trigger an install, since toolchain 0.14.0 is now
        // missing
        let command = Midenup::try_parse_from(["miden", "client", "--version"]).unwrap();
        let Behavior::Miden(argv) = command.behavior else {
            panic!("Error while parsing test command. Expected Midne Behavior, got Midenup");
        };

        miden_wrapper::miden_wrapper(argv.clone(), &config, &mut local_manifest)
            .unwrap_or_else(|err| panic!("Failed to run: {} Error: {err}", get_full_command(argv)));

        let oldest_toolchain = toolchain_dir.join("0.14.0");
        assert!(oldest_toolchain.exists());

        // Afterwards, all of the newly installed toolchains should be present
        // in the local manifest.
        let installed_toolchains = ["0.14.0", "0.15.0", "0.16.0"].iter().map(|version| {
            semver::Version::parse(version)
                .unwrap_or_else(|_| panic!("Failed to turn {version} into semver::Version"))
        });

        // Besides creating the various directories, the local manifest should
        // also reflect this structure
        local_manifest
            .get_channels()
            .map(|channel| channel.name.clone())
            .eq(installed_toolchains);
    }

    #[test]
    /// This tests checks that midenup's update behavior works correctly
    fn integration_update_test() {
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();
        let midenup_home = tmp_home_path.join("midenup");

        // SIDENOTE: This test uses toolchain with version number 0.14.0. This
        // is simply used for testing purposes and is not a toolchain meant to
        // be used.

        // This manifest contains toolchain version 0.14.0 as its only toolchain
        let manifest: &str = "file://tests/data/integration_update_test/channel-manifest-1.json";
        let (mut local_manifest, config) = test_setup(&midenup_home, manifest);
        let toolchain_dir = midenup_home.join("toolchains");

        // We begin by initializing the midenup directory
        let command = Midenup::try_parse_from(["midenup", "init"]).unwrap();
        let Behavior::Midenup { command, .. } = command.behavior else {
            panic!("Error while parsing test command. Expected Midneup Behavior, got Miden");
        };
        command
            .execute(&config, &mut local_manifest)
            .expect("Failed to initialize midenup");

        // Now, we install stable. That is going to be version 0.14.0
        let command = Midenup::try_parse_from(["midenup", "install", "stable"]).unwrap();
        let Behavior::Midenup { command, .. } = command.behavior else {
            panic!("Error while parsing test command. Expected Midneup Behavior, got Miden");
        };
        command.execute(&config, &mut local_manifest).expect("Failed to install stable");

        // Now, we re-generate the config with a newer manifest that contains
        // version 0.15.0. This is trying to emulate the release of a new stable
        // version
        let manifest: &str = "file://tests/data/integration_update_test/channel-manifest-2.json";
        let (_, config) = test_setup(&midenup_home, manifest);

        // Now, we update stable. The stable symlink should point to
        // version 0.15.0
        let command = Midenup::try_parse_from(["midenup", "update", "stable"]).unwrap();
        let Behavior::Midenup { command, .. } = command.behavior else {
            panic!("Error while parsing test command. Expected Midneup Behavior, got Miden");
        };
        command.execute(&config, &mut local_manifest).expect("Failed to update stable");

        // The original toolchain should still exist
        let older_toolchain = toolchain_dir.join("0.14.0");
        assert!(older_toolchain.exists());

        // The newer toolchain should also now be installed
        let newer_toolchain = toolchain_dir.join("0.15.0");
        assert!(newer_toolchain.exists());

        // We check that the stable symlink still exits.
        let stable_dir = toolchain_dir.join("stable");
        assert!(stable_dir.exists());
        assert!(stable_dir.is_symlink());
        // The stable symlink should now point to the newer toolchain
        let stable_toolchain = std::fs::read_link(stable_dir.as_path())
            .expect("Couldn't obtain directory where the stable directory is pointing to");
        assert_eq!(stable_toolchain, newer_toolchain);

        // Now, we perform a "global" update. This performs an update on every
        // *installed* toolchain. It should perform the following changes:
        // - Update 0.15.0's miden-vm.
        // - Downgrade 0.14.0's miden-vm.
        // However this should *not* update stable.
        let manifest: &str = "file://tests/data/integration_update_test/channel-manifest-3.json";
        let (_, config) = test_setup(&midenup_home, manifest);

        let command = Midenup::try_parse_from(["midenup", "update"]).unwrap();
        let Behavior::Midenup { command, .. } = command.behavior else {
            panic!("Error while parsing test command. Expected Midneup Behavior, got Miden");
        };
        command
            .execute(&config, &mut local_manifest)
            .expect("Failed to perform global update");

        // We check that the stable symlink still exits and it is still pointing to 0.15.0.
        assert!(stable_dir.exists());
        assert!(stable_dir.is_symlink());

        // The stable symlink should now point to the newer toolchain
        let stable_toolchain = std::fs::read_link(stable_dir.as_path())
            .expect("Couldn't obtain directory where the stable directory is pointing to");
        assert_eq!(stable_toolchain, newer_toolchain);

        let vm_exe_v15 = toolchain_dir.join("0.15.0").join("bin").join("miden-vm");
        let command = std::process::Command::new(vm_exe_v15).arg("--version").output().unwrap();
        assert_eq!(String::from_utf8(command.stdout).unwrap(), "miden-vm 0.16.2\n");

        let vm_exe_v14 = toolchain_dir.join("0.14.0").join("bin").join("miden");
        let command = std::process::Command::new(vm_exe_v14).arg("--version").output().unwrap();
        assert_eq!(String::from_utf8(command.stdout).unwrap(), "Miden 0.13.0\n");

        // Now, we use the same manifest that we used previously to update the
        // current stable toolchain.
        let command = Midenup::try_parse_from(["midenup", "update", "stable"]).unwrap();
        let Behavior::Midenup { command, .. } = command.behavior else {
            panic!("Error while parsing test command. Expected Midneup Behavior, got Miden");
        };
        command
            .execute(&config, &mut local_manifest)
            .expect("Failed to perform global update");

        let newest_toolchain = toolchain_dir.join("0.16.0");
        assert!(newest_toolchain.exists());

        // The stable symlink should now point to the newest toolchain
        let stable_toolchain = std::fs::read_link(stable_dir.as_path())
            .expect("Couldn't obtain directory where the stable directory is pointing to");
        assert_eq!(stable_toolchain, newest_toolchain);
    }

    #[test]
    /// Tries to install the "stable" toolchain from the present manifest.
    fn integration_install_stable() {
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();
        let midenup_home = tmp_home_path.join("midenup");

        const FILE: &str = "file://manifest/channel-manifest.json";

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE);

        let install = Commands::Install { channel: UserChannel::Stable };
        install.execute(&config, &mut local_manifest).expect("Failed to install stable");

        // After install is executed, the local manifest should be present
        let manifest = midenup_home.join("manifest").with_extension("json");
        assert!(manifest.exists());

        let stable_dir = midenup_home.join("toolchains").join("stable");
        assert!(stable_dir.exists());
        assert!(stable_dir.is_symlink());

        let stable_channel = local_manifest
            .get_latest_stable()
            .expect("No stable channel found; despite having installed stable");

        // We test if the in-memory representation of the local manifest
        // contains the stable alias
        assert_eq!(stable_channel.alias, Some(ChannelAlias::Stable));

        // We read the filesystem again, to check that the "stable" alias was
        // correclty saved
        assert_eq!(
            local_manifest
                .get_channels()
                .next()
                .expect(
                    "ERROR: The local_manifest in the filesystem has no alias, when it should have stable alias"
                )
                .alias.as_ref().expect("ERROR: The installed stable toolchain should be marked as stable in the local manifest"),
            &ChannelAlias::Stable
        );

        tmp_home.close().expect("Couldn't delete tmp midenup home directory");
    }

    #[test]
    #[should_panic]
    /// This 'midenc' component present in this manifest is lacking its required
    /// 'rustup_channel" and thus should fail to compile.
    fn midenup_catches_installation_failure() {
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();
        let midenup_home = tmp_home_path.join("midenup");

        const FILE_PRE_UPDATE: &str = "file://tests/data/manifest-uncompilable-midenc.json";

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE_PRE_UPDATE);

        let install = Commands::Install { channel: UserChannel::Stable };
        install.execute(&config, &mut local_manifest).expect("Failed to install stable");
        // After install is executed, the local manifest should be present
        let manifest = midenup_home.join("manifest").with_extension("json");
        assert!(manifest.exists());
    }
}
