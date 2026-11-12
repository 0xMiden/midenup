//! `midenup self`: managing the midenup installation itself.
//!
//! These commands run on a startup path of their own. They never check the home format, read
//! local state, recover an interrupted publication, or fetch the upstream manifest, because a
//! midenup that cannot understand its home -- one written by a newer release, or left corrupt --
//! must still be able to update or uninstall itself.

mod uninstall;
mod update;

use std::path::PathBuf;

use anyhow::{Context, bail};
use clap::{ArgAction, Subcommand};

pub use self::update::MIDENUP_RELEASE_URI_ENV;
use super::Flags;
use crate::config;

/// Commands that manage the midenup installation itself.
#[derive(Debug, Subcommand)]
pub enum SelfCommand {
    /// Update midenup to the latest release.
    Update {
        /// General configuration flags
        #[clap(flatten)]
        flags: Flags,
    },
    /// Uninstall midenup, removing every installed toolchain and all of its data.
    Uninstall {
        /// Do not ask for confirmation.
        #[arg(short, long, action(ArgAction::SetTrue))]
        force: bool,
        /// General configuration flags
        #[clap(flatten)]
        flags: Flags,
    },
}

impl SelfCommand {
    pub(super) fn flags(&self) -> &Flags {
        match self {
            Self::Update { flags } | Self::Uninstall { flags, .. } => flags,
        }
    }

    /// Runs the command. The caller must not have run any of the normal startup sequence.
    pub fn execute(&self, config: &config::Config) -> anyhow::Result<()> {
        ensure_supported_platform(self)?;

        match self {
            Self::Update { .. } => update::update(config),
            Self::Uninstall { force, .. } => uninstall::uninstall(config, *force),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Update { .. } => "self update",
            Self::Uninstall { .. } => "self uninstall",
        }
    }
}

/// The executable actually running, with every symlink resolved: an update replaces the file a
/// symlink points to and leaves the link in place, and an uninstall removes that file.
fn running_executable() -> anyhow::Result<PathBuf> {
    let path = std::env::current_exe().context("failed to locate the running midenup")?;
    path.canonicalize()
        .with_context(|| format!("failed to resolve the running midenup at '{}'", path.display()))
}

/// Fails before anything is locked, downloaded or removed on a platform without a backend.
///
/// macOS and Linux, including WSL2, share one backend: atomic replacement of the running
/// executable and removal of the home while it is in use. Native Windows needs a different one.
fn ensure_supported_platform(command: &SelfCommand) -> anyhow::Result<()> {
    if cfg!(any(target_os = "macos", target_os = "linux")) {
        return Ok(());
    }
    bail!(
        "`midenup {}` is not supported on this platform yet; reinstall or remove midenup manually",
        command.name()
    )
}
