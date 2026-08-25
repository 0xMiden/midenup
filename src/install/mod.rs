//! Executing an [crate::plan::InstallationPlan].
//!
//! Everything here is deliberately decision-free: the plan says what to do, and these modules do
//! exactly that. No filtering, no name resolution, no manifest access.

pub mod archive;
pub mod cargo;
pub mod download;
pub mod extract;
pub mod stage;

use std::{
    process::{Command, ExitStatus},
    time::Duration,
};

/// How often a running child is checked on, and its elapsed display redrawn.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Runs `command` to completion, showing a live elapsed line for `label` while it works.
///
/// Polled rather than driven by a ticker thread: the erase-before-write bookkeeping is
/// thread-local, and the executor is sequential.
pub fn run_reporting_progress(command: &mut Command, label: &str) -> std::io::Result<ExitStatus> {
    let mut activity = crate::report::Activity::begin(label);
    let mut child = command.spawn()?;

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        activity.tick();
        std::thread::sleep(POLL_INTERVAL);
    }
}

pub use self::{
    archive::ArchiveError,
    cargo::{CargoError, argv_for, build as cargo_build},
    download::{ExecError, acquire},
    extract::{ExtractError, extract, render_script},
    stage::{Realized, Seed, StageError, execute, prepare, seed, verify},
};
