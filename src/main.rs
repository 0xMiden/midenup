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

    use crate::version::Authority;

    type LocalManifest = Manifest;
    type MidenupHome = PathBuf;
    use crate::{channel::*, manifest::*, *};

    /// Simple auxiliary function to setup a midenup directory environment in
    /// tests.
    /// It returns a LocalManifest, and a Config based on the manifest uri. It
    /// also sets up a temporary directory where the installation will take
    /// place. The path to this temporary directory is also returned
    fn test_setup(manifest_uri: &str) -> (LocalManifest, Config, MidenupHome) {
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();
        let midenup_home = tmp_home_path.join("midenup");

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

        (local_manifest, config, midenup_home)
    }

    #[test]
    /// This tests serves as basic check that the install and uninstall
    /// functionalities of midenup work correctly.
    fn install_uninstall_test() {
        const FILE: &str = "file://tests/data/install_uninstall_test/channel-manifest.json";
        let (mut local_manifest, config, midenup_home) = test_setup(FILE);
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
    /// Tries to install the "stable" toolchain from the present manifest.
    fn integration_install_stable() {
        const FILE: &str = "file://manifest/channel-manifest.json";

        let (mut local_manifest, config, midenup_home) = test_setup(FILE);

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
    }

    #[test]
    /// First, use a manifest file to install the stable toolchain under version
    /// 0.14.0. Then, update said manifest and try to update stable to the newer
    /// version
    fn integration_update_stable() {
        // NOTE: Currentlty "update stable" maintains the old stable toolchain
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();

        let midenup_home = tmp_home_path.join("midenup");

        const FILE_PRE_UPDATE: &str = "file://tests/data/update-stable/manifest-pre-update.json";

        let (mut local_manifest, config, midenup_home) = test_setup(FILE_PRE_UPDATE);

        let install = Commands::Install { channel: UserChannel::Stable };
        install.execute(&config, &mut local_manifest).expect("Failed to install stable");
        // After install is executed, the local manifest should be present
        let manifest = midenup_home.join("manifest").with_extension("json");
        assert!(manifest.exists());
        let stable_dir = midenup_home.join("toolchains").join("stable");
        assert!(stable_dir.exists());
        assert!(stable_dir.is_symlink());

        const FILE_POST_UPDATE: &str = "file://tests/data/update-stable/manifest-post-update.json";

        let (mut local_manifest, config, midenup_home) = test_setup(FILE_POST_UPDATE);

        let update = Commands::Update { channel: Some(UserChannel::Stable) };
        update.execute(&config, &mut local_manifest).expect("Failed to update stable");

        // Now there should be two channels. The old stable (no longer marked as
        // such) and the new stable channel
        assert_eq!(local_manifest.get_channels().count(), 2);
        let old_stable = local_manifest
            .get_channel(&UserChannel::Version(semver::Version::new(0, 14, 0)))
            .expect("Couldn't find old stable channel via version");
        assert_eq!(old_stable.alias, None);

        let new_stable = local_manifest
            .get_channel(&UserChannel::Version(semver::Version::new(0, 15, 0)))
            .expect("Couldn't find old stable channel via version");
        assert_eq!(new_stable.alias, Some(ChannelAlias::Stable));

        // Now we check if the structure is correclty saved in the filesystem
        let (local_manifest, _, midenup_home) = test_setup(FILE_POST_UPDATE);
        let old_stable = local_manifest
            .get_channel(&UserChannel::Version(semver::Version::new(0, 14, 0)))
            .expect("Couldn't find old stable channel via version");
        assert_eq!(old_stable.alias, None);

        let new_stable = local_manifest
            .get_channel(&UserChannel::Version(semver::Version::new(0, 15, 0)))
            .expect("Couldn't find old stable channel via version");
        assert_eq!(new_stable.alias, Some(ChannelAlias::Stable));

        let toolchain_dir = midenup_home.join("toolchains");
        let _old_stable = toolchain_dir.join("0.14.0");
        let new_stable = toolchain_dir.join("0.15.0");
        let stable_symlink = toolchain_dir.join("stable");

        assert!(stable_symlink.exists());
        assert!(stable_symlink.is_symlink());

        let stable_dir = std::fs::read_link(stable_symlink.as_path())
            .expect("Couldn't obtain directory where the stable directory is pointing to");
        assert_eq!(stable_dir, new_stable);

        tmp_home.close().expect("Couldn't delete tmp midenup home directory");
    }

    #[test]
    /// First, use a manifest file to install the version 0.14.0.  Then, use a
    /// newer manifest to display an update in the std component and a downgrade
    /// in base. After triggering an update, check if those components got
    /// updated successfully.
    fn integration_update_specific_component() {
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();

        let midenup_home = tmp_home_path.join("midenup");

        const FILE_PRE_UPDATE: &str =
            "file://tests/data/update-specific/manifest-pre-component-update.json";

        let (mut local_manifest, config, midenup_home) = test_setup(FILE_PRE_UPDATE);

        let install = Commands::Install {
            channel: UserChannel::Version(semver::Version::new(0, 14, 0)),
        };
        install.execute(&config, &mut local_manifest).expect("Failed to install 0.14.0");
        // After install is executed, the local manifest should be present
        let manifest = midenup_home.join("manifest").with_extension("json");
        assert!(manifest.exists());
        let version = semver::Version::new(0, 14, 0);
        let old_std = local_manifest
            .get_channel(&UserChannel::Version(version.clone()))
            .expect("Local manifest didn't register version 0.14.0 despite having being installed")
            .get_component("std").expect("Local manifest didn't save the std component despite being present in the upstream manifest");
        if let Authority::Cargo { version, .. } = old_std.version.clone() {
            // 0.13.0 is the version of the std library saved in FILE_PRE_UPDATE
            assert_eq!(version, semver::Version::new(0, 13, 0))
        } else {
            panic!("The old std's authority is not Cargo, despite having been installed with it");
        }

        // This is used for debugging purposes in case the test fails.
        let mut show_toolchain_dir = std::process::Command::new("tree")
            .arg(tmp_home_path)
            .stderr(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .spawn()
            .expect("Couldn't execute tree command");
        let _ = show_toolchain_dir.wait().expect("Failed to execute tree");

        const FILE_POST_UPDATE: &str =
            "file://tests/data/update-specific/manifest-post-component-update.json";

        let (mut local_manifest, config, midenup_home) = test_setup(FILE_POST_UPDATE);

        let update = Commands::Update {
            channel: Some(UserChannel::Version(semver::Version::new(0, 14, 0))),
        };
        update.execute(&config, &mut local_manifest).expect("Failed to update stable");
        let new_std = local_manifest
            .get_channel(&UserChannel::Version(version.clone()))
            .expect("Local manifest didn't register version 0.14.0 despite having being installed")
            .get_component("std").expect("Local manifest didn't save the std component despite being present in the upstream manifest");

        if let Authority::Cargo { version, .. } = new_std.version.clone() {
            // 0.14.0 is the newer version
            assert_eq!(version, semver::Version::new(0, 14, 0))
        } else {
            panic!(
                "The updated std's authority is not Cargo, despite having been installed with it"
            );
        }
    }

    #[test]
    /// Install a specific component and then try to check if midenup update
    /// registers it got rolled back
    fn integration_rollback_specific_component() {
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();
        let midenup_home = tmp_home_path.join("midenup");

        const FILE_PRE_UPDATE: &str =
            "file://tests/data/rollback-component/manifest-pre-component-rollback.json";

        let (mut local_manifest, config, midenup_home) = test_setup(FILE_PRE_UPDATE);

        let install = Commands::Install {
            channel: UserChannel::Version(semver::Version::new(0, 14, 0)),
        };
        install.execute(&config, &mut local_manifest).expect("Failed to install 0.14.0");
        // After install is executed, the local manifest should be present
        let manifest = midenup_home.join("manifest").with_extension("json");
        assert!(manifest.exists());

        let toolchain_path = midenup_home.join("toolchains").join("0.14.0");
        assert!(toolchain_path.join("installation-successful").exists());
        assert!(toolchain_path.exists());

        // This is used for debugging purposes in case the test fails.
        let mut show_toolchain_dir = std::process::Command::new("tree")
            .arg(tmp_home_path)
            .stderr(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .spawn()
            .expect("Couldn't execute tree command");

        let _ = show_toolchain_dir.wait().expect("Failed to execute tree");

        let version = semver::Version::new(0, 14, 0);
        let std = local_manifest
            .get_channel(&UserChannel::Version(version.clone()))
            .expect("Local manifest didn't register version 0.14.0 despite having being installed")
            .get_component("std").expect("Local manifest didn't save the std component despite being present in the upstream manifest");
        if let Authority::Cargo { version, .. } = std.version.clone() {
            // 0.13.0 is the version of the std library saved in FILE_PRE_UPDATE
            assert_eq!(version, semver::Version::new(0, 14, 0))
        } else {
            panic!("The old std's authority is not Cargo, despite having been installed with it");
        }

        const FILE_POST_UPDATE: &str =
            "file://tests/data/rollback-component/manifest-post-component-rollback.json";

        let (mut local_manifest, config, midenup_home) = test_setup(FILE_POST_UPDATE);

        let update = Commands::Update {
            channel: Some(UserChannel::Version(semver::Version::new(0, 14, 0))),
        };
        update.execute(&config, &mut local_manifest).expect("Failed to update stable");
        let rolled_back_std = local_manifest
            .get_channel(&UserChannel::Version(version.clone()))
            .expect("Local manifest didn't register version 0.14.0 despite having being installed")
            .get_component("std").expect("Local manifest didn't save the std component despite being present in the upstream manifest");

        if let Authority::Cargo { version, .. } = rolled_back_std.version.clone() {
            // 0.14.0 is the newer version
            assert_eq!(version, semver::Version::new(0, 13, 0))
        } else {
            panic!(
                "The updated std's authority is not Cargo, despite having been installed with it"
            );
        }
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

        let (mut local_manifest, config, midenup_home) = test_setup(FILE_PRE_UPDATE);

        let install = Commands::Install { channel: UserChannel::Stable };
        install.execute(&config, &mut local_manifest).expect("Failed to install stable");
        // After install is executed, the local manifest should be present
        let manifest = midenup_home.join("manifest").with_extension("json");
        assert!(manifest.exists());
    }
}
