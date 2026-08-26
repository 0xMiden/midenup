//! What `midenup` says while it works, and how much of it (spec section 14.4).
//!
//! Everything here goes to stderr; stdout carries results only. Verbose and above also display
//! cargo's output; debug additionally traces midenup's own actions. The level comes from the
//! `-q`/`-v` flags.
//!
//! Note that an install triggered by `miden` command always runs at the default level.

use std::{
    cell::RefCell,
    fmt,
    io::{IsTerminal, Write},
    sync::atomic::{AtomicU8, Ordering},
    time::{Duration, Instant},
};

use colored::Colorize;

/// How much `midenup` says about what it is doing.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verbosity {
    /// Warnings and errors only. No progress, no announcements.
    Quiet,
    /// One line per component as it is acquired, plus a live transfer display on a terminal.
    #[default]
    Normal,
    /// The above, and the output of spawned programs (e.g. cargo) is no longer suppressed.
    Verbose,
    /// The above, and every action `midenup` takes is traced.
    Debug,
}

impl Verbosity {
    /// Resolves the level in effect from the `-q`/`-v` flags.
    pub fn resolve(quiet: bool, verbose: u8) -> Self {
        if quiet {
            return Self::Quiet;
        }
        match verbose {
            0 => Self::Normal,
            1 => Self::Verbose,
            _ => Self::Debug,
        }
    }
}

/// The level in effect, as a [Verbosity] discriminant.
///
/// Process-global because the things that report are not all reachable from a [crate::config]:
/// [crate::lock] is handed a path, and the executors are handed a plan.
static LEVEL: AtomicU8 = AtomicU8::new(Verbosity::Normal as u8);

/// Installs the level for the rest of the process. Called once, from [crate::commands::Midenup].
pub fn set(verbosity: Verbosity) {
    LEVEL.store(verbosity as u8, Ordering::Relaxed);
}

/// The level in effect.
pub fn verbosity() -> Verbosity {
    match LEVEL.load(Ordering::Relaxed) {
        0 => Verbosity::Quiet,
        1 => Verbosity::Normal,
        2 => Verbosity::Verbose,
        _ => Verbosity::Debug,
    }
}

/// Whether the output of spawned programs is shown rather than suppressed.
pub fn subprocess_output_visible() -> bool {
    verbosity() >= Verbosity::Verbose
}

/// Whether a live, redrawn transfer display is appropriate.
/// level check. Without it the announcement lines remain, which is the whole report in that case.
pub fn transfers_are_live() -> bool {
    // skip if this is not a terminal, we don't need transfer data for file/CI logs.
    verbosity() >= Verbosity::Normal && std::io::stderr().is_terminal()
}

/// Whether a live, redrawn line for a long-running child process is appropriate.
///
/// Only at exactly [Verbosity::Normal]: above it the child's own output is shown, and a line
/// redrawn underneath would interleave with it.
pub fn activity_is_live() -> bool {
    verbosity() == Verbosity::Normal && std::io::stderr().is_terminal()
}

/// Emits a labelled line at [Verbosity::Normal] and above. Prefer the [crate::info] macro.
pub fn emit_info(args: fmt::Arguments) {
    if verbosity() >= Verbosity::Normal {
        write_line(format_args!("{}: {args}", "info".bold()));
    }
}

/// Emits an unlabelled line at [Verbosity::Normal] and above. Prefer the [crate::note] macro.
pub fn emit_note(args: fmt::Arguments) {
    if verbosity() >= Verbosity::Normal {
        write_line(args);
    }
}

/// Emits a labelled line at every level. Prefer the [crate::warn] macro.
pub fn emit_warning(args: fmt::Arguments) {
    write_line(format_args!("{}: {args}", "warning".yellow().bold()));
}

/// Emits at [Verbosity::Debug] only, labelled. Prefer the [crate::trace] macro.
pub fn emit_trace(args: fmt::Arguments) {
    if verbosity() >= Verbosity::Debug {
        write_line(format_args!("{}: {args}", "debug".magenta().bold()));
    }
}

/// Writes one line to stderr, erasing a live transfer display first so the two never collide.
fn write_line(args: fmt::Arguments) {
    let mut stderr = std::io::stderr().lock();
    Transfer::erase(&mut stderr);
    let _ = writeln!(stderr, "{args}");
}

/// An `info:` line, at [Verbosity::Normal] and above.
///
/// The label belongs to the level, not to the call site: spelling it per-message is how a codebase
/// ends up emitting `info`, `warning`, `WARNING` and `warn` for two levels.
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::report::emit_info(format_args!($($arg)*)) };
}

/// An unlabelled line at the same level as [crate::info].
///
/// For continuation lines under a labelled one -- the members of a list the `info:` line above
/// introduced -- where a second label would be noise.
#[macro_export]
macro_rules! note {
    ($($arg:tt)*) => { $crate::report::emit_note(format_args!($($arg)*)) };
}

/// A `warning:` line, at every level including [Verbosity::Quiet].
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::report::emit_warning(format_args!($($arg)*)) };
}

/// An action trace, at [Verbosity::Debug] only.
#[macro_export]
macro_rules! trace {
    ($($arg:tt)*) => { $crate::report::emit_trace(format_args!($($arg)*)) };
}

/// How often the transfer display is redrawn.
///
/// Redrawing per callback would spend more time formatting than transferring; curl calls the
/// progress function on every internal read.
const REDRAW_INTERVAL: Duration = Duration::from_millis(100);

// Whether a transient line is currently on screen, and so must be erased before anything else is
// written to stderr.
//
// Thread-local rather than global: a transfer is drawn by the thread performing it, and the
// executor is sequential. A future concurrent executor cannot share one line anyway.
thread_local! {
    static LINE_PENDING: RefCell<bool> = const { RefCell::new(false) };
}

/// A live, single-line display of one artifact transfer.
///
/// Transient by design: it is erased when the transfer ends, leaving only the announcement line
/// for that component. Progress is about *waiting*; once the wait is over it is not a result worth
/// keeping in the scrollback.
pub struct Transfer {
    label: String,
    started: Instant,
    last_drawn: Option<Instant>,
    live: bool,
}

impl Transfer {
    /// Begins reporting a transfer of `label`, which is a component name.
    pub fn begin(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            started: Instant::now(),
            last_drawn: None,
            live: transfers_are_live(),
        }
    }

    /// Reports that `done` of `total` bytes have arrived. A `total` of zero means the response did
    /// not say, so only what has arrived is shown.
    pub fn update(&mut self, done: u64, total: u64) {
        if !self.live || done == 0 {
            return;
        }
        let now = Instant::now();
        if self.last_drawn.is_some_and(|last| now.duration_since(last) < REDRAW_INTERVAL) {
            return;
        }
        self.last_drawn = Some(now);

        let elapsed = now.duration_since(self.started).as_secs_f64();
        let rate = if elapsed > 0.0 { done as f64 / elapsed } else { 0.0 };
        let transferred = if total > 0 {
            format!("{}/{}", bytes(done), bytes(total))
        } else {
            bytes(done)
        };

        let mut stderr = std::io::stderr().lock();
        let _ =
            write!(stderr, "\r\x1b[2K  {}  {transferred} ({}/s)", self.label, bytes(rate as u64));
        let _ = stderr.flush();
        LINE_PENDING.with_borrow_mut(|pending| *pending = true);
    }

    /// Clears a pending transient line so that a permanent one can be written over it.
    fn erase(stderr: &mut impl Write) {
        LINE_PENDING.with_borrow_mut(|pending| {
            if *pending {
                let _ = write!(stderr, "\r\x1b[2K");
                let _ = stderr.flush();
                *pending = false;
            }
        });
    }
}

impl Drop for Transfer {
    /// Erases the display when the transfer ends, however it ends. A failed transfer leaves an
    /// error on stderr, and the half-drawn line it was on must not be part of it.
    fn drop(&mut self) {
        if self.live {
            let mut stderr = std::io::stderr().lock();
            Self::erase(&mut stderr);
        }
    }
}

/// A live, single-line elapsed display for one long-running child process, e.g. a source build.
///
/// Shows only that time is passing -- deliberately not parsed out of cargo's output, which is a
/// human-facing rendering rather than an interface. Transient like [Transfer].
pub struct Activity {
    label: String,
    started: Instant,
    last_drawn: Option<Instant>,
    live: bool,
}

impl Activity {
    /// Begins reporting work on `label`, which is a component name.
    pub fn begin(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            started: Instant::now(),
            last_drawn: None,
            live: activity_is_live(),
        }
    }

    /// Redraws the elapsed time, at most once per [REDRAW_INTERVAL].
    pub fn tick(&mut self) {
        if !self.live {
            return;
        }
        let now = Instant::now();
        if self.last_drawn.is_some_and(|last| now.duration_since(last) < REDRAW_INTERVAL) {
            return;
        }
        self.last_drawn = Some(now);

        let mut stderr = std::io::stderr().lock();
        let _ = write!(
            stderr,
            "\r\x1b[2K  {}  {}",
            self.label,
            elapsed(now.duration_since(self.started))
        );
        let _ = stderr.flush();
        LINE_PENDING.with_borrow_mut(|pending| *pending = true);
    }
}

impl Drop for Activity {
    /// As [Transfer::drop]: erased however the wait ends, so an error never shares its line.
    fn drop(&mut self) {
        if self.live {
            let mut stderr = std::io::stderr().lock();
            Transfer::erase(&mut stderr);
        }
    }
}

/// Formats a wait as whole seconds, and minutes once there are any.
fn elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 60 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

/// Formats a byte count the way a transfer is usually read: two significant decimals, binary
/// units.
fn bytes(count: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = count as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{count} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_is_formatted_in_minutes_once_there_are_any() {
        assert_eq!(elapsed(Duration::from_secs(0)), "0s");
        assert_eq!(elapsed(Duration::from_secs(59)), "59s");
        assert_eq!(elapsed(Duration::from_secs(60)), "1m00s");
        assert_eq!(elapsed(Duration::from_secs(72)), "1m12s");
        assert_eq!(elapsed(Duration::from_secs(3599)), "59m59s");

        // Truncated, so the count stays monotonic.
        assert_eq!(elapsed(Duration::from_millis(1900)), "1s");
    }

    #[test]
    fn flags_ladder_upwards_and_saturate() {
        assert_eq!(Verbosity::resolve(false, 0), Verbosity::Normal);
        assert_eq!(Verbosity::resolve(false, 1), Verbosity::Verbose);
        assert_eq!(Verbosity::resolve(false, 2), Verbosity::Debug);
        assert_eq!(Verbosity::resolve(false, 9), Verbosity::Debug);
    }

    #[test]
    fn only_verbose_and_above_reveal_subprocess_output() {
        for (level, visible) in [
            (Verbosity::Quiet, false),
            (Verbosity::Normal, false),
            (Verbosity::Verbose, true),
            (Verbosity::Debug, true),
        ] {
            set(level);
            assert_eq!(subprocess_output_visible(), visible, "at {level:?}");
        }
        set(Verbosity::Normal);
    }

    #[test]
    fn byte_counts_read_as_transfers() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(10 * 1024 * 1024), "10.0 MiB");
    }
}
