mod init;
mod install;
mod list;
mod r#override;
mod set;
mod show;
mod uninstall;
mod update;

use std::{ffi::OsString, path::PathBuf};

use anyhow::{Context, anyhow, bail};
use clap::{ArgAction, Args, Parser, Subcommand};

pub use self::{
    init::{init, setup_midenup},
    install::install,
    list::list,
    r#override::r#override,
    set::set,
    show::ShowCommand,
    uninstall::uninstall,
    update::{ComponentUpdate, update},
};
use crate::{channel, config, manifest, options};

pub const MIDENUP_MANIFEST_URI_ENV: &str = "MIDENUP_MANIFEST_URI";

#[derive(Debug, Parser)]
#[command(name = "midenup")]
#[command(multicall(true))]
#[command(author, about = "The Miden toolchain installer", long_about = None)]
pub struct Midenup {
    #[command(subcommand)]
    behavior: Behavior,
}

/// What set of behavior the CLI should exhibit
#[derive(Debug, Subcommand)]
enum Behavior {
    /// The Miden toolchain installer
    Midenup {
        #[command(flatten)]
        config: GlobalArgs,
        #[command(subcommand)]
        command: Option<Commands>,
    },
    /// Invoke components of the current Miden toolchain
    #[command(external_subcommand)]
    Miden(Vec<OsString>),
}

/// Global configuration options for `midenup`
#[derive(Debug, Args)]
struct GlobalArgs {
    /// The location of the Miden toolchain root
    #[arg(long, hide(true), value_name = "DIR", env = "MIDENUP_HOME")]
    pub midenup_home: Option<PathBuf>,
    #[arg(long, hide(true), value_name = "DIR", env = "CARGO_HOME")]
    pub cargo_home: Option<PathBuf>,
    /// The URI from which we should load the global toolchain manifest
    #[arg(
        long,
        hide(true),
        value_name = "FILE",
        env = MIDENUP_MANIFEST_URI_ENV,
        default_value = manifest::Manifest::PUBLISHED_MANIFEST_URI
    )]
    pub manifest_uri: String,
    /// Determines wether the components are installed in debug mode. Useful for
    /// debugging and faster installations. This flag is only avaialble to
    /// `midenup`, not `miden`.
    #[arg(env = "MIDENUP_DEBUG_MODE", action = ArgAction::Set, default_value = "false", hide = true)]
    pub debug: bool,
    /// Display verbose output, mainly used during install.
    #[arg(short, long, action, default_value_t = false)]
    pub verbose: bool,
    // This flag needed to be implemented manually in order to use the
    // `display_version` function and circumvent `clap`'s default `--version`
    // output.
    /// Displays `midenup`'s version information.
    #[arg(short = 'V', long, action, default_value_t = false)]
    pub version: bool,
}

/// All the available Midenup Commands
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
        channel: channel::UserChannel,

        #[clap(flatten)]
        options: options::InstallationOptions,
    },
    /// List all available toolchains
    List,
    /// Uninstall a Miden toolchain
    Uninstall {
        /// The channel or version to install, e.g. `stable` or `0.15.0`
        #[arg(required(true), value_name = "CHANNEL", value_parser)]
        channel: channel::UserChannel,
    },
    /// Show information about the local midenup environment.
    #[command(subcommand)]
    Show(ShowCommand),
    /// Sets the current active miden toolchain for the current project.
    /// This creates a miden-toolchain.toml file in the present working directory.
    Set {
        /// The channel or version to set, e.g. `stable` or `0.15.0`
        #[arg(required(true), value_name = "CHANNEL", value_parser)]
        channel: channel::UserChannel,
    },
    /// Sets the system's default toolchain.
    ///
    /// Unlike `rustup`, midenup does *not* have a notion of directory
    /// overrides. Instead, the `midenup set` command can be used to configure a
    /// directory-specific toolchain.
    Override {
        /// The channel or version to set, e.g. `stable` or `0.15.0`
        #[arg(required(true), value_name = "CHANNEL", value_parser)]
        channel: channel::UserChannel,
    },
    /// Update your installed Miden toolchains.
    Update {
        /// `midenup update`'s behavior differs depending on the specified [CHANNEL]
        /// - If provided, updates only the specified channel.
        /// - If left blank, then midenup will check for updates in all the downloaded toolchains.
        /// - If [CHANNEL] = stable, then it will look for the newest available toolchain and set
        ///   that to be stable.
        #[clap(verbatim_doc_comment)]
        #[arg(value_name = "CHANNEL", value_parser)]
        channel: Option<channel::UserChannel>,

        #[clap(flatten)]
        options: options::UpdateOptions,
    },
}

impl Commands {
    /// Execute the requested subcommand
    pub fn execute(
        &self,
        config: &config::Config,
        local_manifest: &mut manifest::Manifest,
    ) -> anyhow::Result<()> {
        match &self {
            Self::Init => {
                init(config, local_manifest)?;
                Ok(())
            },
            Self::List => {
                list(config, local_manifest);
                Ok(())
            },
            Self::Install { channel, options } => {
                let Some(channel) = config.manifest.get_channel(channel) else {
                    bail!("channel '{}' doesn't exist or is unavailable", channel);
                };
                install(config, channel, local_manifest, options)
            },
            Self::Uninstall { channel, .. } => {
                let Some(channel) = config.manifest.get_channel(channel) else {
                    bail!("channel '{}' doesn't exist or is unavailable", channel);
                };
                uninstall(config, channel, local_manifest)
            },
            Self::Update { channel, options } => {
                update(config, channel.as_ref(), local_manifest, options)
            },
            Self::Show(cmd) => cmd.execute(config, local_manifest),
            Self::Set { channel } => set(config, local_manifest, channel),
            Self::Override { channel } => r#override(config, local_manifest, channel),
        }
    }
}

impl Midenup {
    /// Get the effective configuration for the current session
    pub fn config(&self) -> anyhow::Result<config::Config> {
        let working_directory =
            std::env::current_dir().context("unable to read current directory")?;
        match &self.behavior {
            Behavior::Miden(_) => {
                // Always respect XDG dirs if set
                let midenup_home = std::env::var_os("XDG_DATA_HOME")
                    .map(PathBuf::from)
                    .map(|dir| dir.join("midenup"))
                    .or_else(|| dirs::data_dir().map(|dir| dir.join("midenup")))
                    // If for whatever reason, we can't access the data dir, we fall
                    // back to .local/share
                    .or_else(|| {
                        dirs::home_dir()
                            .map(|home| home.join(".local").join("share"))
                    })
                    .ok_or_else(||
                                anyhow!("Failed to set midenup directory.\
                                        Consider setting a value for XDG_DATA_HOME in your shell's profile"
                                )
                    )?;

                let cargo_home = std::env::var_os("CARGO_HOME")
                    .map(PathBuf::from)
                    .or_else(|| dirs::home_dir().map(|home| home.join(".cargo")))
                    .ok_or_else(|| {
                        anyhow!(
                            "$CARGO_HOME and $HOME are unset, but at least one must be set in \
                             your shell's profile"
                        )
                    })?;

                let manifest_uri = std::env::var(MIDENUP_MANIFEST_URI_ENV)
                    .unwrap_or(manifest::Manifest::PUBLISHED_MANIFEST_URI.to_string());
                config::Config::init(
                    working_directory,
                    midenup_home,
                    cargo_home,
                    manifest_uri,
                    false,
                )
            },
            Behavior::Midenup { config, .. } => {
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
                        dirs::home_dir()
                            .map(|home| home.join(".local").join("share"))
                    })
                    .ok_or_else(||
                                anyhow!("Failed to set midenup directory.\
                                        Consider setting a value for XDG_DATA_HOME in your shell's profile"
                                )
                    )?;
                let cargo_home = config
                    .cargo_home
                    .clone()
                    .or_else(|| std::env::var_os("CARGO_HOME").map(PathBuf::from))
                    .or_else(|| dirs::home_dir().map(|home| home.join(".cargo")))
                    .ok_or_else(|| {
                        anyhow!(
                            "$CARGO_HOME and $HOME are unset, but at least one must be set in \
                             your shell's profile"
                        )
                    })?;

                config::Config::init(
                    working_directory,
                    midenup_home,
                    cargo_home,
                    &config.manifest_uri,
                    config.debug,
                )
            },
        }
    }

    /// Execute this session with the provided configuration.
    pub fn execute(&self, config: &config::Config) -> anyhow::Result<()> {
        let mut local_manifest = config.local_manifest()?;

        self.execute_with_manifest(config, &mut local_manifest)
    }

    /// Execute this session with the provided configuration and local manifest
    pub fn execute_with_manifest(
        &self,
        config: &config::Config,
        local_manifest: &mut manifest::Manifest,
    ) -> anyhow::Result<()> {
        use crate::miden_wrapper;

        match &self.behavior {
            Behavior::Miden(argv) => {
                miden_wrapper::miden_wrapper(argv, config, local_manifest)
                    .with_context(|| format!("failed to execute '{}'", get_full_command(argv)))?;
            },
            Behavior::Midenup { config: global_args, command: subcommand } => {
                if global_args.version {
                    println!("{}", miden_wrapper::display_version(config, &*local_manifest));
                } else if let Some(subcommand) = subcommand {
                    subcommand.execute(config, local_manifest)?;
                } else {
                    bail!("no subcommand provided. Run `midenup --help` for usage information.")
                }
            },
        }

        // After execution we check if need to update the midenup/opt symlink
        // This is done *after* execution because some commands change what the
        // active toolchain (update, set) and some remove the directory entirely
        // (uninstall)
        config.update_opt_symlinks(config, &*local_manifest)?;

        Ok(())
    }
}

fn get_full_command(argv: &[OsString]) -> String {
    use core::fmt::Write;

    let mut out = String::with_capacity(256);
    for (i, arg) in argv.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        write!(&mut out, "{}", arg.display()).unwrap();
    }
    out
}
