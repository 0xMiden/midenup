//! Shared harness pieces for integration tests.

use std::sync::{Mutex, MutexGuard, OnceLock};

/// Serializes integration tests that mutate shared state outside their own temp directory.
///
/// Each test gets an isolated `MIDENUP_HOME`, but installs still run `cargo install` against a
/// shared `CARGO_HOME` and the shared Cargo registry/package cache. Running several installs
/// concurrently makes them contend, and which test loses the race varies between runs -- the
/// observed symptom is a nondeterministic subset of the install tests failing while each one
/// passes in isolation.
///
/// `cargo test` runs a test binary's tests in a thread pool within one process, so a process-global
/// mutex is sufficient. Poisoning is deliberately ignored: one panicking test must not cascade into
/// unrelated failures.
///
/// This is a test-isolation measure only. The equivalent production hazard -- two `miden`
/// invocations in different project directories both triggering an install -- is handled by the
/// `MIDENUP_HOME` advisory lock.
pub fn mutating_test_guard() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|err| err.into_inner())
}
