mod channel;
mod commands;
mod config;
mod manifest;
mod toolchain;
mod utils;
mod version;

use std::{ffi::OsString, path::PathBuf};

use anyhow::{Context, anyhow, bail};
use clap::{Args, FromArgMatches, Parser, Subcommand};

pub use self::config::Config;
use self::{
    channel::UserChannel,
    manifest::{Manifest, ManifestError},
    toolchain::Toolchain,
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
    /// Show information about the midenup environment
    #[command(subcommand)]
    Show(commands::ShowCommand),
    /// Update your installed Miden toolchains
    Update {
        /// If provided, updates only the specified channel.
        #[arg(value_name = "CHANNEL", value_parser)]
        channel: Option<UserChannel>,
    },
}

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
        env = "MIDENUP_MANIFEST_URI",
        default_value = manifest::Manifest::PUBLISHED_MANIFEST_URI
    )]
    manifest_uri: String,
}

impl Commands {
    /// Execute the requested subcommand
    fn execute(&self, config: &Config, local_manifest: &mut Manifest) -> anyhow::Result<()> {
        match &self {
            Self::Init { .. } => commands::init(config),
            Self::Install { channel, .. } => {
                let Some(channel) = config.manifest.get_channel(channel) else {
                    bail!("channel '{}' doesn't exist or is unavailable", channel);
                };
                commands::install(config, channel, local_manifest)
            },
            Self::Update { channel } => commands::update(config, channel.as_ref(), local_manifest),
            Self::Show(cmd) => cmd.execute(config),
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
                .ok_or_else(|| {
                    anyhow!("MIDENUP_HOME is unset, and the default location is unavailable")
                })?;
            Config::init(midenup_home, "file://manifest/channel-manifest.json")?
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
                .ok_or_else(|| {
                    anyhow!("MIDENUP_HOME is unset, and the default location is unavailable")
                })?;

            Config::init(midenup_home, &config.manifest_uri)?
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
        Behavior::Miden(argv) => {
            // Extract the target binary to execute from argv[1]
            let subcommand = argv[1].to_str().expect("invalid command name");
            let (target_exe, prefix_args) = match subcommand {
                // When 'help' is invoked, we should look for the target exe in argv[1], and present
                // help accordingly
                "help" => todo!(),
                "build" => ("cargo", vec!["miden", "build"]),
                "new" => ("cargo", vec!["miden", "new"]),
                other => (other, vec![]),
            };

            // Make sure we know the current toolchain so we can modify the PATH appropriately
            let toolchain = Toolchain::current()?;

            // Compute the effective PATH for this command
            let toolchain_bin = config
                .midenup_home
                .join("toolchains")
                .join(toolchain.channel.to_string())
                .join("bin");
            let path = match std::env::var_os("PATH") {
                Some(prev_path) => {
                    let mut path = OsString::from(format!("{}:", toolchain_bin.display()));
                    path.push(prev_path);
                    path
                },
                None => toolchain_bin.into_os_string(),
            };

            let mut output = std::process::Command::new(target_exe)
                .env("MIDENUP_HOME", &config.midenup_home)
                .env("PATH", path)
                .args(prefix_args)
                .args(argv.iter().skip(2))
                .stderr(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::inherit())
                .spawn()
                .with_context(|| format!("failed to run 'miden {subcommand}'"))?;

            let status = output.wait().with_context(|| {
                format!("error occurred while waiting for 'miden {subcommand}' to finish executing")
            })?;

            if status.success() {
                Ok(())
            } else {
                bail!("'miden {}' failed with status {}", subcommand, status.code().unwrap_or(1))
            }
        },
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

        let config =
            Config::init(midenup_home.to_path_buf().clone(), manifest_uri).unwrap_or_else(|_| {
                panic!(
                    "Failed construct config from manifest {} and midenup_home at {}",
                    manifest_uri,
                    midenup_home.display(),
                )
            });

        (local_manifest, config)
    }
    #[test]
    fn install_stable() {
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();
        let midenup_home = tmp_home_path.join("midenup");

        const FILE: &str = "file://manifest/channel-manifest.json";

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE);

        let init = Commands::Init;
        init.execute(&config, &mut local_manifest).expect("Failed to init");

        let manifest = midenup_home.join("manifest").with_extension("json");
        assert!(manifest.exists());

        let install = Commands::Install { channel: UserChannel::Stable };
        install.execute(&config, &mut local_manifest).expect("Failed to install stable");

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
    fn update_stable() {
        // NOTE: Currentlty "update stable" maintains the old stable toolchain
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();

        let midenup_home = tmp_home_path.join("midenup");

        const FILE_PRE_UPDATE: &str = "file://tests/data/update-stable/manifest-pre-update.json";

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE_PRE_UPDATE);

        let init = Commands::Init;
        init.execute(&config, &mut local_manifest).expect("Failed to init");
        let manifest = midenup_home.join("manifest").with_extension("json");
        assert!(manifest.exists());

        let install = Commands::Install { channel: UserChannel::Stable };
        install.execute(&config, &mut local_manifest).expect("Failed to install stable");
        let stable_dir = midenup_home.join("toolchains").join("stable");
        assert!(stable_dir.exists());
        assert!(stable_dir.is_symlink());

        const FILE_POST_UPDATE: &str = "file://tests/data/update-stable/manifest-post-update.json";

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE_POST_UPDATE);

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
        let (local_manifest, _) = test_setup(&midenup_home, FILE_POST_UPDATE);
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
    fn update_specific_component() {
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();

        let midenup_home = tmp_home_path.join("midenup");

        const FILE_PRE_UPDATE: &str =
            "file://tests/data/update-specific/manifest-pre-component-update.json";

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE_PRE_UPDATE);

        let init = Commands::Init;
        init.execute(&config, &mut local_manifest).expect("Failed to init");
        let manifest = midenup_home.join("manifest").with_extension("json");
        assert!(manifest.exists());

        let install = Commands::Install {
            channel: UserChannel::Version(semver::Version::new(0, 14, 0)),
        };
        install.execute(&config, &mut local_manifest).expect("Failed to install 0.14.0");
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

        let mut show_toolchain_dir = std::process::Command::new("tree")
            .arg(tmp_home_path)
            .stderr(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .spawn()
            .expect("Couldn't execute tree command");
        let _ = show_toolchain_dir.wait().expect("Failed to execute tree");

        const FILE_POST_UPDATE: &str =
            "file://tests/data/update-specific/manifest-post-component-update.json";

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE_POST_UPDATE);

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
    fn rollback_specific_component() {
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();
        let midenup_home = tmp_home_path.join("midenup");

        const FILE_PRE_UPDATE: &str =
            "file://tests/data/rollback-component/manifest-pre-component-rollback.json";

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE_PRE_UPDATE);

        let init = Commands::Init;
        init.execute(&config, &mut local_manifest).expect("Failed to init");
        let manifest = midenup_home.join("manifest").with_extension("json");
        assert!(manifest.exists());

        let install = Commands::Install {
            channel: UserChannel::Version(semver::Version::new(0, 14, 0)),
        };
        install.execute(&config, &mut local_manifest).expect("Failed to install 0.14.0");
        let toolchain_path = midenup_home.join("toolchains").join("0.14.0");
        assert!(toolchain_path.join("installation-successfull").exists());
        assert!(toolchain_path.exists());

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

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE_POST_UPDATE);

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
}
