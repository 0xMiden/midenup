mod channel;
mod commands;
mod config;
mod manifest;
mod toolchain;
mod version;

use std::{ffi::OsString, path::PathBuf};

use anyhow::{anyhow, bail, Context};
use clap::{Args, FromArgMatches, Parser, Subcommand};

pub use self::config::Config;
use self::{
    channel::{CanonicalChannel, ChannelType},
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
        channel: ChannelType,
    },
    /// Show information about the midenup environment
    #[command(subcommand)]
    Show(commands::ShowCommand),
    /// Update your installed Miden toolchains
    Update {
        /// If provided, updates only the specified channel.
        #[arg(required(true), value_name = "CHANNEL", value_parser)]
        channel: Option<ChannelType>,
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
    fn execute(&self, config: &Config) -> anyhow::Result<()> {
        match &self {
            Self::Init { .. } => commands::init(config),
            Self::Install { channel, .. } => {
                let channel = CanonicalChannel::from_input(channel.clone(), &config.manifest)?;
                commands::install(config, &channel)
            },
            Self::Update { channel, .. } => commands::update(config, channel.as_ref()),
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
        Behavior::Midenup { command: subcommand, .. } => subcommand.execute(&config),
    }
}
