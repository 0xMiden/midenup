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
//!
//! A second lock file, the format lease, guards the home format ([`crate::home_format`]). Every
//! command holds it shared until it finishes; a home conversion holds it exclusively. The order is
//! always format lease, then `HomeLock`, and a shared lease is never upgraded in place.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use fs4::FileExt;

/// How long to wait before telling the user why nothing is happening.
const NOTIFY_AFTER: Duration = Duration::from_secs(1);

/// How long to wait in total. A real operation can take minutes -- a `cargo install` of several
/// components is not quick -- so the cap is generous; it exists to fail rather than hang forever.
const TIMEOUT: Duration = Duration::from_secs(600);

/// How often to retry. Short enough to be imperceptible, long enough not to spin.
const POLL: Duration = Duration::from_millis(50);

/// How long to wait for the format lease. Its holders may be long-running tools, so this refuses
/// rather than waiting for them.
const LEASE_TIMEOUT: Duration = Duration::from_secs(10);

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
    #[error(
        "another midenup or miden process is using '{}'. Retry once it has finished; running \
         tools are never stopped to make way.",
        home.display()
    )]
    LeaseBusy { home: PathBuf },
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

/// A shared or exclusive hold on the home format, released when dropped or when the process exits.
#[derive(Debug)]
pub struct FormatLease {
    file: std::fs::File,
}

impl Drop for FormatLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Where the lock lives.
pub fn lock_path(home: &Path) -> PathBuf {
    home.join(".lock")
}

/// Where the format lease lives.
pub fn lease_path(home: &Path) -> PathBuf {
    home.join(".format-lease")
}

/// Opens (creating if needed) a lock file and its parent directory.
fn open(path: &Path) -> Result<std::fs::File, LockError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| LockError::Open { path: path.to_path_buf(), source })?;
    }
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|source| LockError::Open { path: path.to_path_buf(), source })
}

/// Takes the format lease shared, as every command does for as long as it runs.
pub fn shared_lease(home: &Path) -> Result<FormatLease, LockError> {
    lease(home, false)
}

/// Takes the format lease exclusively, as a home conversion does. The caller must hold no lease.
pub fn exclusive_lease(home: &Path) -> Result<FormatLease, LockError> {
    lease(home, true)
}

fn lease(home: &Path, exclusive: bool) -> Result<FormatLease, LockError> {
    let file = open(&lease_path(home))?;
    let start = Instant::now();
    let mut notified = false;

    loop {
        let taken = if exclusive {
            FileExt::try_lock(&file)
        } else {
            FileExt::try_lock_shared(&file)
        };
        if taken.is_ok() {
            return Ok(FormatLease { file });
        }

        if start.elapsed() >= LEASE_TIMEOUT {
            return Err(LockError::LeaseBusy { home: home.to_path_buf() });
        }

        if !notified && start.elapsed() >= NOTIFY_AFTER {
            crate::info!("waiting for another midenup or miden process to finish...");
            notified = true;
        }

        std::thread::sleep(POLL);
    }
}

/// Takes the exclusive lock, waiting for whoever holds it.
///
/// Blocks in a poll loop rather than a blocking `flock` call so that a wait can be explained: a
/// user who runs `miden` in a second terminal deserves to know it is waiting on the first, not to
/// watch it hang silently.
pub fn acquire(home: &Path) -> Result<HomeLock, LockError> {
    let path = lock_path(home);
    let mut file = open(&path)?;

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
            crate::info!("waiting for another midenup operation to finish...");
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

    #[test]
    fn shared_leases_coexist_and_exclude_an_exclusive_one() {
        let temp = tempdir::TempDir::new("lease").unwrap();
        let home = temp.path();

        let first = shared_lease(home).expect("should acquire");
        let second = shared_lease(home).expect("shared leases must not block each other");

        let other = std::fs::File::open(lease_path(home)).unwrap();
        assert!(FileExt::try_lock(&other).is_err(), "a shared holder must exclude a converter");

        drop((first, second));
        assert!(FileExt::try_lock(&other).is_ok(), "dropping the leases must release them");
    }

    #[test]
    fn a_busy_lease_is_refused_after_a_bounded_wait() {
        let temp = tempdir::TempDir::new("lease-busy").unwrap();
        let _held = shared_lease(temp.path()).unwrap();
        let err = exclusive_lease(temp.path()).expect_err("must refuse while a holder is active");
        assert!(matches!(err, LockError::LeaseBusy { .. }), "{err}");
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
