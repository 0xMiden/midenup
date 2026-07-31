//! The `$MIDENUP_HOME` advisory lock.
//!
//! Mutating operations take an exclusive `flock` on `$MIDENUP_HOME/.lock`; read-only ones take
//! nothing. This is not optional: `miden <cmd>` installs the current toolchain if it is missing, so
//! two `miden` invocations in two project directories are two concurrent writers against one
//! `MIDENUP_HOME`, with no user involved in making that happen.
//!
//! `flock` is released by the kernel when the process exits, so a crashed holder cannot wedge the
//! home directory -- which is why this is a lock file rather than a pid file. It does not protect a
//! `MIDENUP_HOME` shared over a network filesystem, which is unsupported.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use colored::Colorize;
use fs4::FileExt;

/// How long to wait before telling the user why nothing is happening.
const NOTIFY_AFTER: Duration = Duration::from_secs(1);

/// How long to wait in total. A real operation can take minutes -- a `cargo install` of several
/// components is not quick -- so the cap is generous; it exists to fail rather than hang forever.
const TIMEOUT: Duration = Duration::from_secs(600);

/// How often to retry. Short enough to be imperceptible, long enough not to spin.
const POLL: Duration = Duration::from_millis(50);

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("failed to open the lock file '{path}': {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "timed out after {} minutes waiting for another midenup operation (pid {holder}) to \
         finish. If that process is gone, the lock is already released and retrying will succeed.",
        TIMEOUT.as_secs() / 60
    )]
    Timeout { holder: String },
}

/// An exclusive hold on `$MIDENUP_HOME`, released when dropped or when the process exits.
#[derive(Debug)]
pub struct HomeLock {
    file: std::fs::File,
}

impl Drop for HomeLock {
    fn drop(&mut self) {
        // Unlocking explicitly is not strictly required -- closing the descriptor releases the
        // lock -- but doing it here keeps the release at a point we control.
        let _ = FileExt::unlock(&self.file);
    }
}

/// Where the lock lives.
pub fn lock_path(home: &Path) -> PathBuf {
    home.join(".lock")
}

/// Takes the exclusive lock, waiting for whoever holds it.
///
/// Blocks in a poll loop rather than a blocking `flock` call so that a wait can be explained: a
/// user who runs `miden` in a second terminal deserves to know it is waiting on the first, not to
/// watch it hang silently.
pub fn acquire(home: &Path) -> Result<HomeLock, LockError> {
    let path = lock_path(home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| LockError::Open { path: path.clone(), source })?;
    }

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|source| LockError::Open { path: path.clone(), source })?;

    let start = Instant::now();
    let mut notified = false;

    loop {
        if FileExt::try_lock(&file).is_ok() {
            // Record who holds it, so a waiter can name the process it is waiting on. Purely
            // diagnostic: the lock itself is the `flock`, never the file's contents.
            let _ = file.set_len(0);
            let _ = write!(file, "{}", std::process::id());
            let _ = file.flush();
            return Ok(HomeLock { file });
        }

        if start.elapsed() >= TIMEOUT {
            return Err(LockError::Timeout { holder: holder_of(&path) });
        }

        if !notified && start.elapsed() >= NOTIFY_AFTER {
            println!(
                "{}: waiting for another midenup operation to finish...",
                "info".white().bold()
            );
            notified = true;
        }

        std::thread::sleep(POLL);
    }
}

/// The pid recorded by the current holder, for diagnostics only.
fn holder_of(path: &Path) -> String {
    let mut contents = String::new();
    std::fs::File::open(path)
        .and_then(|mut file| file.read_to_string(&mut contents))
        .ok()
        .map(|_| contents.trim().to_string())
        .filter(|contents| !contents.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_is_exclusive_and_released_on_drop() {
        let temp = tempdir::TempDir::new("lock").unwrap();
        let home = temp.path();

        let held = acquire(home).expect("should acquire");

        // A second *open* of the same file in this process contends exactly as another process
        // would: `flock` is per open file description, not per process.
        let other = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(lock_path(home))
            .unwrap();
        assert!(FileExt::try_lock(&other).is_err(), "the lock must exclude a second holder");

        drop(held);
        assert!(FileExt::try_lock(&other).is_ok(), "dropping the lock must release it");
    }

    #[test]
    fn the_holder_records_its_pid_for_diagnostics() {
        let temp = tempdir::TempDir::new("lock-pid").unwrap();
        let _held = acquire(temp.path()).expect("should acquire");
        assert_eq!(holder_of(&lock_path(temp.path())), std::process::id().to_string());
    }

    /// Acquiring after a previous holder has gone must work, and must not be confused by the pid
    /// that holder left behind.
    #[test]
    fn a_released_lock_can_be_retaken() {
        let temp = tempdir::TempDir::new("lock-retake").unwrap();
        drop(acquire(temp.path()).unwrap());
        let _again = acquire(temp.path()).expect("must be retakeable");
        assert_eq!(holder_of(&lock_path(temp.path())), std::process::id().to_string());
    }
}
