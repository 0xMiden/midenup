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
    update::update,
};
use crate::{channel, config, manifest, options, report};

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
    /// Determines whether the components are installed in debug mode. Useful for debugging and
    /// faster installations. This flag is only available to `midenup`, not `miden`.
    #[arg(long, env = "MIDENUP_DEBUG_MODE", hide = true)]
    pub debug: bool,
    /// Suppress progress and informational output. Warnings and errors are still shown.
    #[arg(
        short,
        long,
        global = true,
        action,
        default_value_t = false,
        conflicts_with = "verbose"
    )]
    pub quiet: bool,
    /// Report more: `-v` also shows the output of the programs midenup runs, `-vv` traces every
    /// action it takes.
    #[arg(short, long, global = true, action = ArgAction::Count)]
    pub verbose: u8,
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
        /// - If left blank, then midenup will check for updates in all the downloaded toolchains.
        /// - If [CHANNEL] is a version, updates that toolchain against the channel upstream now
        ///   publishes under that name.
        /// - If [CHANNEL] is a network (mainnet, testnet, devnet), follows the network to whatever
        ///   channel it now names: installing it if needed, carrying your component selection
        ///   across, and moving your data under var/ with it.
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
            Self::List => list(config, state),
            Self::Install { channel, options } => {
                // Said before the fetch it describes, which is what can hang on a slow network.
                // The whole manifest is synced, so no channel is named here.
                crate::info!("syncing channel updates from upstream");
                let manifest = config.upstream_manifest()?;
                let requested = channel;
                let Some(channel) = manifest.get_channel(channel) else {
                    // Which names exist is manifest data now, so a typo has to be answerable with
                    // what was actually declared rather than "doesn't exist or is unavailable".
                    match channel {
                        channel::UserChannel::Named(name) => bail!(
                            "unknown channel '{name}'; known networks are {}",
                            manifest.network_names().collect::<Vec<_>>().join(", ")
                        ),
                        channel::UserChannel::Version(version) => {
                            bail!("there is no toolchain {version} in the channel manifest")
                        },
                    }
                };

                // A network resolves to a version, and both halves are worth stating; a version
                // requested directly is only worth stating once.
                let target = if requested.to_string() == channel.name.to_string() {
                    channel.name.to_string()
                } else {
                    format!("{requested} ({})", channel.name)
                };
                crate::info!("upstream last updated on {}", manifest.last_updated());
                crate::info!("installing {target}");

                install(config, channel, state, options)
            },
            // Deliberately not resolved against upstream: a channel that has been withdrawn is
            // exactly one a user needs to be able to uninstall (spec section 12.3).
            Self::Uninstall { channel, purge } => uninstall(config, channel, state, *purge),
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
        // Before anything that could report: this governs the first message emitted, and the
        // migrations below are reached before any command runs.
        crate::report::set(self.verbosity());

        let working_directory =
            std::env::current_dir().context("unable to read current directory")?;
        match &self.behavior {
            Behavior::Miden(_) => {
                // Respect an explicit MIDENUP_HOME override first - `midenup` itself honors it
                // (and exports it to child processes), so `miden` must resolve to the same home -
                // then fall back to the XDG dirs.
                let midenup_home = std::env::var_os("MIDENUP_HOME")
                    .map(PathBuf::from)
                    .or_else(|| {
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

                // Before the upstream fetch below: an unreachable upstream must not be able to
                // prevent a local migration (spec section 12.2).
                crate::migrate_v1::migrate_if_needed(&midenup_home)?;
                crate::migrate_networks::migrate_if_needed(&midenup_home)
                    .context("failed to migrate the toolchains directory to the network layout")?;

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

                // See above: migration precedes any upstream fetch.
                crate::migrate_v1::migrate_if_needed(&midenup_home)?;
                crate::migrate_networks::migrate_if_needed(&midenup_home)
                    .context("failed to migrate the toolchains directory to the network layout")?;

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

    /// The verbosity in effect for this session.
    ///
    /// `miden` takes no flags of its own -- everything after it belongs to the component being
    /// dispatched to -- so an install it triggers always runs at the default level.
    fn verbosity(&self) -> report::Verbosity {
        match &self.behavior {
            Behavior::Miden(_) => report::Verbosity::default(),
            Behavior::Midenup { config, .. } => {
                report::Verbosity::resolve(config.quiet, config.verbose)
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

        // Migration first, before recovery and before anything reads local state. `config()`
        // already ran this ahead of the upstream fetch on the CLI path; it is idempotent and costs
        // one `stat`, and running it here too means a caller that built its own `Config` -- every
        // in-process test -- gets the same startup sequence rather than a subtly different one.
        if let crate::migrate_v1::MigrationOutcome::Migrated { channels } =
            crate::migrate_v1::migrate_if_needed(&config.midenup_home)?
        {
            report_migration(&channels);
            *state = config.local_state()?;
        }
        crate::migrate_networks::migrate_if_needed(&config.midenup_home)
            .context("failed to migrate the toolchains directory to the network layout")?;

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
        config.update_opt_symlinks()?;

        Ok(())
    }
}

fn report_migration(channels: &[semver::Version]) {
    crate::info!(
        "migrated {} installed toolchain(s) to the new local state format",
        channels.len()
    );
    for channel in channels {
        crate::note!(
            "- {channel} will be reinstalled the next time it is used, so that midenup knows \
             exactly what it owns"
        );
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
            crate::info!("recovered an interrupted {operation}");
        },
        Err(err @ crate::publish::PublishError::DivergentState { .. }) => {
            crate::warn!("{err}");
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

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    #[test]
    fn a_mistyped_subcommand_names_the_word_that_was_mistyped() {
        let err = Midenup::try_parse_from(["midenup", "instal", "stable"])
            .expect_err("a mistyped subcommand must not parse");
        let rendered = err.to_string();
        assert!(
            rendered.contains("instal"),
            "the typo itself must be named, not the word after it: {rendered}"
        );
    }
}
