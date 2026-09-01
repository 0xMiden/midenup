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
    io::Read,
    process::{Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};

/// How often a running child is checked on, and its elapsed display redrawn.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long the pipe must be quiet after the direct child exits before the tail is complete.
const POST_EXIT_QUIET_PERIOD: Duration = Duration::from_millis(100);

/// The absolute limit on draining a pipe held open by a descendant after the direct child exits.
const POST_EXIT_DRAIN_LIMIT: Duration = Duration::from_secs(1);

/// Bounds captured stderr waiting in memory while the terminal writer catches up.
const READER_CHANNEL_CAPACITY: usize = 16;

/// Runs `command` to completion, showing a live elapsed line for `label` while it works.
///
/// When the line is live, the child's stderr is piped, and the poll loop writes each captured byte
/// chunk through the erase-before-write path, so errors and unterminated prompts never collide
/// with the redrawn line. The reader thread only reads: the erase bookkeeping is thread-local.
/// EOF should be immediate in the ordinary case, but a large direct-child tail can take several
/// reads to forward. Each received chunk renews a short post-exit quiet period so that tail is
/// retained; a hard deadline still wins when a background descendant inherited the pipe.
pub fn run_reporting_progress(command: &mut Command, label: &str) -> std::io::Result<ExitStatus> {
    if !crate::report::activity_is_live() {
        return command.spawn()?.wait();
    }

    command.stderr(Stdio::piped());
    let mut activity = crate::report::Activity::begin(label);
    let mut child = command.spawn()?;
    let stderr = child.stderr.take().expect("stderr was piped above");

    let (sender, output) = mpsc::sync_channel(READER_CHANNEL_CAPACITY);
    let reader = std::thread::spawn(move || {
        let mut stderr = stderr;
        let mut buffer = [0u8; 8192];
        loop {
            match stderr.read(&mut buffer) {
                Ok(0) => {
                    let _ = sender.send(ReaderEvent::Eof);
                    break;
                },
                Ok(read) => {
                    if sender.send(ReaderEvent::Output(buffer[..read].into())).is_err() {
                        break;
                    }
                },
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(err) => {
                    let _ = sender.send(ReaderEvent::Error(err));
                    break;
                },
            }
        }
    });

    let mut child_line_pending = false;
    let mut reader_finished = false;
    let mut reader_error = None;
    loop {
        drain_child_output(
            &output,
            READER_CHANNEL_CAPACITY,
            &mut child_line_pending,
            &mut reader_finished,
            &mut reader_error,
        );
        if let Some(status) = child.try_wait()? {
            // Usually Cargo's exit closes the pipe and EOF arrives immediately. A background
            // descendant may have inherited the write end, though, so bound the final drain rather
            // than waiting forever for an EOF Cargo cannot provide.
            let hard_deadline = Instant::now() + POST_EXIT_DRAIN_LIMIT;
            let mut quiet_deadline = Instant::now() + POST_EXIT_QUIET_PERIOD;
            while !reader_finished {
                let deadline = hard_deadline.min(quiet_deadline);
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match output.recv_timeout(remaining) {
                    Ok(event) => {
                        handle_reader_event(
                            event,
                            &mut child_line_pending,
                            &mut reader_finished,
                            &mut reader_error,
                        );
                        quiet_deadline = Instant::now() + POST_EXIT_QUIET_PERIOD;
                    },
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => {
                        reader_finished = true;
                        break;
                    },
                }
            }
            drain_child_output(
                &output,
                READER_CHANNEL_CAPACITY,
                &mut child_line_pending,
                &mut reader_finished,
                &mut reader_error,
            );

            // Joining is safe only once the reader has observed EOF/an error. Otherwise a
            // descendant still owns the pipe and the detached reader will finish when it closes.
            if reader_finished || reader.is_finished() {
                let _ = reader.join();
                // Close the small race where the reader sent its terminal event after the final
                // bounded drain but before is_finished() was observed.
                drain_child_output(
                    &output,
                    usize::MAX,
                    &mut child_line_pending,
                    &mut reader_finished,
                    &mut reader_error,
                );
            }
            if child_line_pending {
                crate::report::emit_child_output(b"\n");
            }
            drop(activity);
            return reader_error.map_or(Ok(status), Err);
        }
        if !child_line_pending {
            activity.tick();
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

enum ReaderEvent {
    Output(Box<[u8]>),
    Eof,
    Error(std::io::Error),
}

fn drain_child_output(
    output: &Receiver<ReaderEvent>,
    limit: usize,
    child_line_pending: &mut bool,
    reader_finished: &mut bool,
    reader_error: &mut Option<std::io::Error>,
) {
    for _ in 0..limit {
        let Ok(event) = output.try_recv() else {
            break;
        };
        handle_reader_event(event, child_line_pending, reader_finished, reader_error);
    }
}

fn handle_reader_event(
    event: ReaderEvent,
    child_line_pending: &mut bool,
    reader_finished: &mut bool,
    reader_error: &mut Option<std::io::Error>,
) {
    match event {
        ReaderEvent::Output(bytes) => {
            crate::report::emit_child_output(&bytes);
            *child_line_pending = !bytes.ends_with(b"\n");
        },
        ReaderEvent::Eof => *reader_finished = true,
        ReaderEvent::Error(err) => {
            *reader_finished = true;
            *reader_error = Some(err);
        },
    }
}

pub use self::{
    archive::ArchiveError,
    cargo::{CargoError, argv_for, build as cargo_build},
    download::{ExecError, acquire},
    extract::{ExtractError, extract, render_script},
    stage::{Realized, Seed, StageError, execute, prepare, seed, verify},
};
