mod gc;
mod init;
mod install;
mod list;
mod r#override;
mod set;
mod show;
mod uninstall;
mod update;

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use anyhow::{Context, anyhow, bail};
use clap::{ArgAction, Args, Parser, Subcommand, builder::ArgPredicate};

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
        /// The location of the Miden toolchain root
        #[arg(long, global(true), hide(true), value_name = "DIR", env = "MIDENUP_HOME")]
        midenup_home: Option<PathBuf>,
        #[arg(long, global(true), hide(true), value_name = "DIR", env = "CARGO_HOME")]
        cargo_home: Option<PathBuf>,
        /// The URI from which we should load the global toolchain manifest
        #[arg(
            long,
            global(true),
            hide(true),
            value_name = "FILE",
            env = MIDENUP_MANIFEST_URI_ENV,
            default_value = manifest::VersionedManifest::PUBLISHED_MANIFEST_URI
        )]
        manifest_uri: String,
        /// Displays `midenup`'s version information.
        #[arg(short = 'V', global(true), long, action, default_value_t = false)]
        version: bool,
        #[command(subcommand)]
        command: Option<Commands>,
    },
    /// Invoke components of the current Miden toolchain
    #[command(external_subcommand)]
    Miden(Vec<OsString>),
}

/// Configuration options for `midenup`
#[derive(Debug, Args)]
pub struct Flags {
    /// Determines whether the components are installed in debug mode. Useful for debugging and
    /// faster installations. This flag is only available to `midenup`, not `miden`.
    #[arg(long, env = "MIDENUP_DEBUG_MODE", hide = true, action(ArgAction::SetTrue))]
    pub debug: bool,
    /// Emit simple textual output: no color, no live progress decorations.
    #[arg(long, action(ArgAction::SetTrue))]
    pub plain: bool,
    /// How progress on long-running work is displayed [default: pretty]
    #[arg(
        long,
        value_enum,
        value_name = "STYLE",
        conflicts_with_all(["no_progress", "quiet"]),
        num_args(0..=1),
        require_equals(true),
        default_value_t = report::ProgressStyle::Pretty,
        default_missing_value = "pretty",
        default_value_ifs([
            ("no_progress", ArgPredicate::Equals("true".into()), Some("none")),
            ("quiet", ArgPredicate::Equals("true".into()), Some("none")),
            ("plain", ArgPredicate::Equals("true".into()), Some("plain")),
        ])
    )]
    pub progress: report::ProgressStyle,
    /// Suppress the progress display without suppressing informational output.
    #[arg(long, action(ArgAction::SetTrue))]
    pub no_progress: bool,
    /// Suppress progress and informational output. Warnings and errors are still shown.
    #[arg(short, long, action(ArgAction::SetTrue))]
    pub quiet: bool,
    /// How much to report: `debug` also shows the output of the programs midenup runs, `trace`
    /// additionally emits selected low-level filesystem, network, and subprocess details
    /// [default: info]
    #[arg(
        short = 'v',
        long = "verbose",
        value_enum,
        value_name = "LEVEL",
        conflicts_with("quiet"),
        num_args(0..=1),
        require_equals = true,
        default_value_t = report::Verbosity::Info,
        default_missing_value = "debug",
        default_value_ifs([
            ("quiet", ArgPredicate::Equals("true".into()), Some("warn")),
        ])
    )]
    pub verbosity: report::Verbosity,
    /// Whether output is colored [default: auto]
    #[arg(
        long,
        value_enum,
        value_name = "WHEN",
        conflicts_with("plain"),
        num_args(0..=1),
        require_equals = true,
        default_value_t = report::ColorChoice::Auto,
        default_missing_value = "true",
        default_value_ifs([
            ("plain", ArgPredicate::Equals("true".into()), Some("false")),
        ])
    )]
    pub color: report::ColorChoice,
}

/// All the available Midenup Commands
#[derive(Debug, Subcommand)]
enum Commands {
    /// Bootstrap the `midenup` environment.
    ///
    /// This initializes the `MIDEN_HOME` directory layout and configuration.
    Init {
        /// General configuration flags
        #[clap(flatten)]
        flags: Flags,
    },
    /// Install a Miden toolchain
    Install {
        /// The channel or version to install, e.g. `stable` or `0.15.0`
        #[arg(required(true), value_name = "CHANNEL", value_parser)]
        channel: channel::UserChannel,
        /// General configuration flags
        #[clap(flatten)]
        flags: Flags,
        #[clap(flatten)]
        options: options::InstallationOptions,
    },
    /// Reclaim disk space from toolchain installations nothing refers to any more.
    ///
    /// Every change to an installed channel publishes a new copy and leaves the previous one in
    /// place, because another process may still be running out of it. This removes those.
    Gc {
        /// General configuration flags
        #[clap(flatten)]
        flags: Flags,
    },
    /// List all available toolchains
    List {
        /// General configuration flags
        #[clap(flatten)]
        flags: Flags,
    },
    /// Uninstall a Miden toolchain
    Uninstall {
        /// The channel or version to install, e.g. `stable` or `0.15.0`
        #[arg(required(true), value_name = "CHANNEL", value_parser)]
        channel: channel::UserChannel,
        /// General configuration flags
        #[clap(flatten)]
        flags: Flags,
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
        /// General configuration flags
        #[clap(flatten)]
        flags: Flags,
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
        /// General configuration flags
        #[clap(flatten)]
        flags: Flags,
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
        /// General configuration flags
        #[clap(flatten)]
        flags: Flags,
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
            Self::Init { .. }
            | Self::Install { .. }
            | Self::Uninstall { .. }
            | Self::Update { .. }
            | Self::Gc { .. } => true,
            // Writes `toolchains/default`.
            Self::Override { .. } => true,
            // Writes `miden-toolchain.toml` in the working directory, not `$MIDENUP_HOME`.
            Self::Set { .. } => false,
            Self::List { .. } | Self::Show(_) => false,
        }
    }

    fn flags(&self) -> Option<&Flags> {
        match self {
            Self::Init { flags }
            | Self::Install { flags, .. }
            | Self::Uninstall { flags, .. }
            | Self::Update { flags, .. }
            | Self::Gc { flags, .. }
            | Self::Override { flags, .. }
            | Self::Set { flags, .. }
            | Self::List { flags, .. }
            | Self::Show(ShowCommand::Current { flags } | ShowCommand::List { flags }) => {
                Some(flags)
            },
            Self::Show(ShowCommand::Home) => None,
        }
    }

    /// Execute the requested subcommand
    pub fn execute(
        &self,
        config: &config::Config,
        state: &mut crate::state::LocalState,
    ) -> anyhow::Result<()> {
        match &self {
            Self::Init { flags: _ } => {
                init(config)?;
                Ok(())
            },
            Self::Gc { flags: _ } => gc(config, state),
            Self::List { flags: _ } => list(config, state),
            Self::Install { channel, options, flags: _ } => {
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
                crate::info!("installing {target}");

                install(config, channel, state, options)
            },
            // Deliberately not resolved against upstream: a channel that has been withdrawn is
            // exactly one a user needs to be able to uninstall (spec section 12.3).
            Self::Uninstall { channel, purge, flags: _ } => {
                uninstall(config, channel, state, *purge)
            },
            Self::Update { channel, options, flags: _ } => {
                update(config, channel.as_ref(), state, options)
            },
            Self::Show(cmd) => cmd.execute(config, state),
            Self::Set { channel, flags: _ } => set(config, channel),
            Self::Override { channel, flags: _ } => r#override(config, state, channel),
        }
    }
}

impl Midenup {
    /// Get the effective configuration for the current session
    pub fn config(&self) -> anyhow::Result<config::Config> {
        // Before anything that could report: this governs the first message emitted, and the
        // migrations below are reached before any command runs.
        let (verbosity, progress, color) = self.output_settings();
        crate::report::set(verbosity, progress, color);

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
            Behavior::Midenup {
                midenup_home, cargo_home, manifest_uri, ..
            } => {
                let flags = self.flags();
                let midenup_home = midenup_home
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
                let cargo_home = cargo_home
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

                let debug = flags.map(|flags| flags.debug).unwrap_or(false);
                config::Config::init(
                    working_directory,
                    midenup_home,
                    cargo_home,
                    manifest_uri,
                    debug,
                )
            },
        }
    }

    fn flags(&self) -> Option<&Flags> {
        match &self.behavior {
            Behavior::Miden(_) | Behavior::Midenup { command: None, .. } => None,
            Behavior::Midenup { command: Some(cmd), .. } => cmd.flags(),
        }
    }

    /// The output settings in effect for this session.
    ///
    /// `miden` takes no flags of its own -- everything after it belongs to the component being
    /// dispatched to -- so an install it triggers always runs at the default settings.
    fn output_settings(&self) -> (report::Verbosity, report::ProgressStyle, report::ColorChoice) {
        self.flags()
            .map(|flags| (flags.verbosity, flags.progress, flags.color))
            .unwrap_or_default()
    }

    /// Execute this session with the provided configuration.
    pub fn execute(&self, config: &config::Config) -> anyhow::Result<ExitCode> {
        let mut state = config.local_state()?;
        self.execute_with_state(config, &mut state)
    }

    /// Execute this session with the provided configuration and local manifest
    pub fn execute_with_state(
        &self,
        config: &config::Config,
        state: &mut crate::state::LocalState,
    ) -> anyhow::Result<ExitCode> {
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
                let code = miden_wrapper::miden_wrapper(argv, config, state)
                    .with_context(|| format!("failed to execute '{}'", get_full_command(argv)))?;
                config.update_opt_symlinks()?;
                return Ok(code);
            },
            Behavior::Midenup { version, command: subcommand, .. } => {
                if *version {
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

        Ok(ExitCode::SUCCESS)
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
    use std::assert_matches;

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

    /// Parses an argv and resolves the output settings it asks for.
    fn output_flags(
        args: &[&str],
    ) -> (report::Verbosity, report::ProgressStyle, report::ColorChoice) {
        let midenup = Midenup::try_parse_from(args).unwrap_or_else(|err| err.exit());
        assert_matches!(
            &midenup.behavior,
            Behavior::Midenup { .. },
            "the argv must select the midenup behavior"
        );
        midenup.output_settings()
    }

    #[test]
    fn the_default_output_is_pretty_colored_info() {
        let (verbosity, progress, color) = output_flags(&["midenup", "list"]);
        assert_eq!(verbosity, report::Verbosity::Info);
        assert_eq!(progress, report::ProgressStyle::Pretty);
        assert_eq!(color, report::ColorChoice::Auto);
    }

    #[test]
    fn quiet_implies_warnings_only_and_no_progress() {
        let (verbosity, progress, _) = output_flags(&["midenup", "list", "-q"]);
        assert_eq!(verbosity, report::Verbosity::Warn);
        assert_eq!(progress, report::ProgressStyle::None);
    }

    #[test]
    fn plain_implies_plain_progress_and_no_color_without_changing_the_level() {
        let (verbosity, progress, color) = output_flags(&["midenup", "list", "--plain"]);
        assert_eq!(progress, report::ProgressStyle::Plain);
        assert_eq!(color, report::ColorChoice::False);
        assert_eq!(verbosity, report::Verbosity::Info);
    }

    #[test]
    fn no_progress_suppresses_progress_without_going_quiet() {
        let (verbosity, progress, _) = output_flags(&["midenup", "list", "--no-progress"]);
        assert_eq!(progress, report::ProgressStyle::None);
        assert_eq!(verbosity, report::Verbosity::Info);
    }

    #[test]
    fn verbosity_is_selected_by_name_and_bare_verbose_is_the_default_verbose_level() {
        let (trace, ..) = output_flags(&["midenup", "list", "--verbose=trace"]);
        assert_eq!(trace, report::Verbosity::Trace);
        let (debug, ..) = output_flags(&["midenup", "list", "--verbose=debug"]);
        assert_eq!(debug, report::Verbosity::Debug);
        let (bare, ..) = output_flags(&["midenup", "list", "-v"]);
        assert_eq!(bare, report::Verbosity::Debug);
    }

    #[test]
    fn contradictory_output_flags_are_rejected() {
        Midenup::try_parse_from(["midenup", "list", "-q", "-v"])
            .expect_err("quiet and verbose must conflict");
        Midenup::try_parse_from(["midenup", "list", "--no-progress", "--progress=pretty"])
            .expect_err("no-progress and an explicit progress style must conflict");
    }
}
