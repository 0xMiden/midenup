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
    io::{BufRead, BufReader},
    process::{Command, ExitStatus, Stdio},
    sync::mpsc,
    time::Duration,
};

/// How often a running child is checked on, and its elapsed display redrawn.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Runs `command` to completion, showing a live elapsed line for `label` while it works.
///
/// When the line is live, the child's stderr is piped, and the poll loop prints each captured
/// line through the erase-before-write path, so the child's errors never collide with the
/// redrawn line. The reader thread only reads: the erase bookkeeping is thread-local.
pub fn run_reporting_progress(command: &mut Command, label: &str) -> std::io::Result<ExitStatus> {
    if !crate::report::activity_is_live() {
        return command.spawn()?.wait();
    }

    command.stderr(Stdio::piped());
    let mut activity = crate::report::Activity::begin(label);
    let mut child = command.spawn()?;
    let stderr = child.stderr.take().expect("stderr was piped above");

    let (sender, lines) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    loop {
        for line in lines.try_iter() {
            crate::report::emit_child_line(&line);
        }
        if let Some(status) = child.try_wait()? {
            // Exit closed the pipe, which ends the reader; drain what it sent since the last tick.
            let _ = reader.join();
            for line in lines.try_iter() {
                crate::report::emit_child_line(&line);
            }
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
