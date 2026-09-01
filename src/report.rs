//! What `midenup` says while it works, and how much of it (spec section 14.4).
//!
//! Everything here goes to stderr; stdout carries results only. Debug and above also display
//! cargo's output; trace additionally traces midenup's own actions. The level comes from the
//! `-q`/`--verbose` flags; the progress display and color are controlled separately by
//! `--progress`, `--color` and `--plain`.
//!
//! Note that an install triggered by `miden` command always runs at the default settings.

use std::{
    cell::RefCell,
    fmt,
    io::{IsTerminal, Write},
    sync::atomic::{AtomicU8, Ordering},
    time::{Duration, Instant},
};

use clap::ValueEnum;
use colored::Colorize;

/// How much `midenup` says about what it is doing.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
#[repr(u8)]
pub enum Verbosity {
    /// Warnings and errors only. No progress, no announcements.
    Warn,
    /// One line per component as it is acquired, plus a live transfer display on a terminal.
    #[default]
    Info,
    /// The above, and the output of spawned programs (e.g. cargo) is no longer suppressed.
    Debug,
    /// The above, and every action `midenup` takes is traced.
    Trace,
}

/// How progress on long-running work is displayed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[repr(u8)]
pub enum ProgressStyle {
    /// A live, redrawn line on a terminal.
    #[default]
    Pretty,
    /// The announcement lines alone; no terminal decorations.
    Plain,
    /// No progress display.
    None,
}

/// Whether output is colored.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[repr(u8)]
pub enum ColorChoice {
    /// Color when the output is a terminal.
    #[default]
    Auto,
    /// Always color.
    True,
    /// Never color.
    False,
}

impl ColorChoice {
    /// Whether stdout should be colored under this choice.
    pub fn use_color(&self) -> bool {
        match self {
            Self::Auto => auto_color(std::io::stdout().is_terminal()),
            Self::True => true,
            Self::False => false,
        }
    }

    fn for_terminal(self, terminal: bool) -> bool {
        match self {
            Self::Auto => auto_color(terminal),
            Self::True => true,
            Self::False => false,
        }
    }
}

/// Resolves the conventional color environment variables for a particular destination stream.
///
/// `colored` applies the same precedence, but its automatic terminal check is permanently tied to
/// stdout. Reporting is written to stderr, so automatic mode has to supply that terminal fact
/// itself.
fn auto_color(terminal: bool) -> bool {
    resolve_auto_color(
        terminal,
        std::env::var("CLICOLOR").ok().map(|value| value != "0"),
        std::env::var("NO_COLOR").is_ok(),
        std::env::var("CLICOLOR_FORCE").ok().map(|value| value != "0"),
    )
}

fn resolve_auto_color(
    terminal: bool,
    clicolor: Option<bool>,
    no_color: bool,
    clicolor_force: Option<bool>,
) -> bool {
    if clicolor_force == Some(true) {
        true
    } else if no_color {
        false
    } else {
        clicolor.unwrap_or(true) && terminal
    }
}

/// The level in effect, as a [Verbosity] discriminant.
///
/// Process-global because the things that report are not all reachable from a [crate::config]:
/// [crate::lock] is handed a path, and the executors are handed a plan.
static LEVEL: AtomicU8 = AtomicU8::new(Verbosity::Info as u8);

/// The progress style in effect, as a [ProgressStyle] discriminant. Global for the same reason.
static PROGRESS: AtomicU8 = AtomicU8::new(ProgressStyle::Pretty as u8);

/// The color choice in effect, as a [ColorChoice] discriminant. Global for the same reason.
static COLOR: AtomicU8 = AtomicU8::new(ColorChoice::Auto as u8);

/// Installs the output settings for the rest of the process. Called once, from
/// [crate::commands::Midenup].
pub fn set(verbosity: Verbosity, progress: ProgressStyle, color: ColorChoice) {
    LEVEL.store(verbosity as u8, Ordering::Relaxed);
    PROGRESS.store(progress as u8, Ordering::Relaxed);
    COLOR.store(color as u8, Ordering::Relaxed);
    match color {
        // Clear a forced setting from an earlier in-process invocation. Each actual output path
        // installs the stream-specific automatic answer before it renders colored values.
        ColorChoice::Auto => colored::control::unset_override(),
        ColorChoice::True => colored::control::set_override(true),
        ColorChoice::False => colored::control::set_override(false),
    }
}

fn color() -> ColorChoice {
    match COLOR.load(Ordering::Relaxed) {
        0 => ColorChoice::Auto,
        1 => ColorChoice::True,
        _ => ColorChoice::False,
    }
}

fn prepare_color(choice: ColorChoice, terminal: bool) -> bool {
    let enabled = choice.for_terminal(terminal);
    colored::control::set_override(enabled);
    enabled
}

/// Selects color for stdout immediately before rendering a command result.
pub(crate) fn prepare_stdout_color() -> bool {
    prepare_color(color(), std::io::stdout().is_terminal())
}

/// Selects color for stderr immediately before rendering a report.
///
/// Public because the exported reporting macros invoke it at their call sites, before evaluating
/// arguments that may themselves contain eagerly rendered colored strings.
#[doc(hidden)]
pub fn prepare_stderr_color() -> bool {
    prepare_color(color(), std::io::stderr().is_terminal())
}

/// The level in effect.
pub fn verbosity() -> Verbosity {
    match LEVEL.load(Ordering::Relaxed) {
        0 => Verbosity::Warn,
        1 => Verbosity::Info,
        2 => Verbosity::Debug,
        _ => Verbosity::Trace,
    }
}

/// The progress style in effect.
fn progress() -> ProgressStyle {
    match PROGRESS.load(Ordering::Relaxed) {
        0 => ProgressStyle::Pretty,
        1 => ProgressStyle::Plain,
        _ => ProgressStyle::None,
    }
}

/// Whether the output of spawned programs is shown rather than suppressed.
pub fn subprocess_output_visible() -> bool {
    verbosity() >= Verbosity::Debug
}

/// Whether a live, redrawn transfer display is appropriate.
///
/// Only the pretty style redraws, and only on a terminal; a file or CI log gets the announcement
/// lines alone, which are the whole report in that case.
pub fn transfers_are_live() -> bool {
    progress() == ProgressStyle::Pretty
        && verbosity() >= Verbosity::Info
        && std::io::stderr().is_terminal()
}

/// Whether a live, redrawn line for a long-running child process is appropriate.
///
/// Only at exactly [Verbosity::Info]: above it the child's own output is shown, and a line
/// redrawn underneath would interleave with it.
pub fn activity_is_live() -> bool {
    progress() == ProgressStyle::Pretty
        && verbosity() == Verbosity::Info
        && std::io::stderr().is_terminal()
}

/// Emits a labelled line at [Verbosity::Info] and above. Prefer the [crate::info] macro.
pub fn emit_info(args: fmt::Arguments) {
    if verbosity() >= Verbosity::Info {
        write_line(format_args!("{}: {args}", "info".bold()));
    }
}

/// Emits an unlabelled line at [Verbosity::Info] and above. Prefer the [crate::note] macro.
pub fn emit_note(args: fmt::Arguments) {
    if verbosity() >= Verbosity::Info {
        write_line(args);
    }
}

/// Emits a labelled line at every level. Prefer the [crate::warn] macro.
pub fn emit_warning(args: fmt::Arguments) {
    write_line(format_args!("{}: {args}", "warning".yellow().bold()));
}

/// Writes a chunk of a child's captured stderr, erasing the live display first.
///
/// This is deliberately byte-oriented: stderr is not guaranteed to be UTF-8, and a prompt that
/// does not end in a newline must reach the terminal before the child waits for an answer.
pub(crate) fn emit_child_output(bytes: &[u8]) {
    let mut stderr = std::io::stderr().lock();
    Transfer::erase(&mut stderr);
    let _ = stderr.write_all(bytes);
    let _ = stderr.flush();
}

/// Emits at [Verbosity::Trace] only, labelled. Prefer the [crate::trace] macro.
pub fn emit_trace(args: fmt::Arguments) {
    if verbosity() >= Verbosity::Trace {
        write_line(format_args!("{}: {args}", "trace".magenta().bold()));
    }
}

/// Writes one line to stderr, erasing a live transfer display first so the two never collide.
fn write_line(args: fmt::Arguments) {
    prepare_stderr_color();
    let mut stderr = std::io::stderr().lock();
    Transfer::erase(&mut stderr);
    let _ = writeln!(stderr, "{args}");
}

/// An `info:` line, at [Verbosity::Info] and above.
///
/// The label belongs to the level, not to the call site: spelling it per-message is how a codebase
/// ends up emitting `info`, `warning`, `WARNING` and `warn` for two levels.
#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        if $crate::report::verbosity() >= $crate::report::Verbosity::Info {
            $crate::report::prepare_stderr_color();
            $crate::report::emit_info(format_args!($($arg)*));
        }
    };
}

/// An unlabelled line at the same level as [crate::info].
///
/// For continuation lines under a labelled one -- the members of a list the `info:` line above
/// introduced -- where a second label would be noise.
#[macro_export]
macro_rules! note {
    ($($arg:tt)*) => {
        if $crate::report::verbosity() >= $crate::report::Verbosity::Info {
            $crate::report::prepare_stderr_color();
            $crate::report::emit_note(format_args!($($arg)*));
        }
    };
}

/// A `warning:` line, at every level including [Verbosity::Warn].
#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {{
        $crate::report::prepare_stderr_color();
        $crate::report::emit_warning(format_args!($($arg)*))
    }};
}

/// A `trace:` action trace, at [Verbosity::Trace] only.
#[macro_export]
macro_rules! trace {
    ($($arg:tt)*) => {
        if $crate::report::verbosity() >= $crate::report::Verbosity::Trace {
            $crate::report::prepare_stderr_color();
            $crate::report::emit_trace(format_args!($($arg)*));
        }
    };
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
    fn only_debug_and_above_reveal_subprocess_output() {
        for (level, visible) in [
            (Verbosity::Warn, false),
            (Verbosity::Info, false),
            (Verbosity::Debug, true),
            (Verbosity::Trace, true),
        ] {
            set(level, ProgressStyle::Pretty, ColorChoice::Auto);
            assert_eq!(subprocess_output_visible(), visible, "at {level:?}");
        }
        set(Verbosity::Info, ProgressStyle::Pretty, ColorChoice::Auto);
    }

    #[test]
    fn byte_counts_read_as_transfers() {
        assert_eq!(bytes(512), "512 B");
        assert_eq!(bytes(1536), "1.5 KiB");
        assert_eq!(bytes(10 * 1024 * 1024), "10.0 MiB");
    }

    #[test]
    fn automatic_color_uses_the_destination_terminal_and_standard_precedence() {
        assert!(resolve_auto_color(true, None, false, None));
        assert!(!resolve_auto_color(false, None, false, None));
        assert!(!resolve_auto_color(true, Some(false), false, None));
        assert!(!resolve_auto_color(true, None, true, None));
        assert!(resolve_auto_color(false, Some(false), true, Some(true)));
        assert!(!resolve_auto_color(false, None, false, Some(false)));
        assert!(resolve_auto_color(true, None, false, Some(false)));
    }
}
