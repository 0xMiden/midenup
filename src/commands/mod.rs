mod gc;
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
    gc::gc,
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
#[command(
    name = "midenup",
    author,
    about = "The Miden toolchain installer",
    long_about = None,
    multicall(true),
    disable_version_flag(true)
)]
pub struct Midenup {
    #[command(subcommand)]
    behavior: Behavior,
}

/// What set of behavior the CLI should exhibit
#[derive(Debug, Subcommand)]
// Boxing here would mean boxing a clap parse root that is constructed exactly once per process,
// trading a real ergonomic cost for no measurable benefit.
#[allow(clippy::large_enum_variant)]
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
        default_value = manifest::VersionedManifest::PUBLISHED_MANIFEST_URI
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
    /// Reclaim disk space from toolchain installations nothing refers to any more.
    ///
    /// Every change to an installed channel publishes a new copy and leaves the previous one in
    /// place, because another process may still be running out of it. This removes those.
    Gc,
    /// List all available toolchains
    List,
    /// Uninstall a Miden toolchain
    Uninstall {
        /// The channel or version to install, e.g. `stable` or `0.15.0`
        #[arg(required(true), value_name = "CHANNEL", value_parser)]
        channel: channel::UserChannel,

        /// Also delete this channel's mutable data (`var/<channel>`), such as the client's
        /// database. Without this flag it is kept, and you are told where it lives.
        #[arg(long, action, default_value_t = false)]
        purge: bool,
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
    /// Whether this subcommand writes to `$MIDENUP_HOME`.
    ///
    /// Only mutating operations take the advisory lock (spec section 9.9). Making `list` or `show`
    /// wait behind a long install would be a regression in a tool people run to find out *why*
    /// something is taking so long.
    fn is_mutating(&self) -> bool {
        match self {
            Self::Init
            | Self::Install { .. }
            | Self::Uninstall { .. }
            | Self::Update { .. }
            | Self::Gc => true,
            // Writes `toolchains/default`.
            Self::Override { .. } => true,
            // Writes `miden-toolchain.toml` in the working directory, not `$MIDENUP_HOME`.
            Self::Set { .. } => false,
            Self::List | Self::Show(_) => false,
        }
    }

    /// Execute the requested subcommand
    pub fn execute(
        &self,
        config: &config::Config,
        state: &mut crate::state::LocalState,
    ) -> anyhow::Result<()> {
        match &self {
            Self::Init => {
                init(config)?;
                Ok(())
            },
            Self::Gc => gc(config, state),
            Self::List => {
                list(config, state);
                Ok(())
            },
            Self::Install { channel, options } => {
                let Some(channel) = config.manifest.get_channel(channel) else {
                    bail!("channel '{}' doesn't exist or is unavailable", channel);
                };
                install(config, channel, state, options)
            },
            Self::Uninstall { channel, purge } => {
                let Some(channel) = config.manifest.get_channel(channel) else {
                    bail!("channel '{}' doesn't exist or is unavailable", channel);
                };
                uninstall(config, channel, state, *purge)
            },
            Self::Update { channel, options } => update(config, channel.as_ref(), state, options),
            Self::Show(cmd) => cmd.execute(config, state),
            Self::Set { channel } => set(config, channel),
            Self::Override { channel } => r#override(config, state, channel),
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
                    .unwrap_or(manifest::VersionedManifest::PUBLISHED_MANIFEST_URI.to_string());
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
        let mut state = config.local_state()?;
        self.execute_with_state(config, &mut state)
    }

    /// Execute this session with the provided configuration and local manifest
    pub fn execute_with_state(
        &self,
        config: &config::Config,
        state: &mut crate::state::LocalState,
    ) -> anyhow::Result<()> {
        use crate::miden_wrapper;

        recover(config, state)?;

        match &self.behavior {
            Behavior::Miden(argv) => {
                // No lock: dispatch is read-only until it discovers the toolchain is missing, and
                // it takes the lock itself at that point (`ensure_current_is_installed`).
                miden_wrapper::miden_wrapper(argv, config, state)
                    .with_context(|| format!("failed to execute '{}'", get_full_command(argv)))?;
            },
            Behavior::Midenup { config: global_args, command: subcommand } => {
                if global_args.version {
                    println!("{}", miden_wrapper::display_version(config));
                } else if let Some(subcommand) = subcommand {
                    let _lock = if subcommand.is_mutating() {
                        let lock = crate::lock::acquire(&config.midenup_home)?;
                        // Whoever held the lock may have changed what is installed, so nothing may
                        // be planned against the state read before waiting for it.
                        *state = config.local_state()?;
                        Some(lock)
                    } else {
                        None
                    };

                    subcommand.execute(config, state)?;
                } else {
                    bail!("no subcommand provided. Run `midenup --help` for usage information.")
                }
            },
        }

        // After execution we check if need to update the midenup/opt symlink
        // This is done *after* execution because some commands change what the active toolchain
        // (update, set) and some remove the directory entirely (uninstall)
        config.update_opt_symlinks(config)?;

        Ok(())
    }
}

/// Completes or discards whatever the previous run left behind, before anything else happens.
///
/// A new operation must never be planned against a half-published `MIDENUP_HOME`, so this runs
/// ahead of every command, including `miden` dispatch.
///
/// Divergence -- a state record whose publication is not on disk -- is *reported* here rather than
/// being fatal. It is a genuine error (spec section 14.3) and is never guessed at or silently
/// repaired, but the remediation it names is itself a midenup command: making it fatal at startup
/// would leave the user with a diagnostic they cannot act on. The operation that actually needs
/// the missing files fails on its own terms.
fn recover(config: &config::Config, state: &mut crate::state::LocalState) -> anyhow::Result<()> {
    use colored::Colorize;

    // Recovery mutates, so it takes the lock -- but only when there is something to recover.
    // Taking it unconditionally would put every `miden` invocation behind it, and the read-only
    // dispatch path is required to stay lock-free.
    let _lock = match crate::publish::journal::read(&config.midenup_home)? {
        Some(_) => {
            let lock = crate::lock::acquire(&config.midenup_home)?;
            // Whoever held the lock may already have completed this recovery.
            *state = config.local_state()?;
            Some(lock)
        },
        None => None,
    };

    match crate::publish::journal::recover(&config.midenup_home, state) {
        Ok(None) => {},
        Ok(Some(operation)) => {
            println!("{}: recovered an interrupted {operation}", "info".white().bold());
        },
        Err(err @ crate::publish::PublishError::DivergentState { .. }) => {
            eprintln!("{}: {err}", "warning".yellow().bold());
        },
        Err(err) => return Err(err.into()),
    }

    Ok(())
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
