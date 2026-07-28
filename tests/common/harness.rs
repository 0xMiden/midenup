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
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

use std::path::{Path, PathBuf};

/// A fully offline channel fixture: no network, no `cargo install`.
///
/// Most install tests assert on *layout and state* -- which files landed where, what the symlinks
/// point at, what was recorded. None of that needs a real toolchain, but building one dominates
/// their runtime: a single `midenup install stable` compiles several crates from source and pulls
/// real release binaries over the network.
///
/// This writes a manifest whose artifacts are `file://` paths to tiny local stand-ins, so those
/// tests exercise exactly the same code paths in milliseconds. Tests that genuinely need a real
/// binary -- running it, or checking Cargo/git/path authority handling -- should keep using the
/// real manifest.
pub struct OfflineFixture {
    /// `file://`-style URI to hand to `test_setup`.
    pub manifest_uri: String,
    /// Where the fixture's artifacts and manifest live.
    pub dir: PathBuf,
}

impl OfflineFixture {
    /// Builds a channel containing one of each installable shape.
    ///
    /// * `vm`     -- a prebuilt executable, so `bin/` and `opt/` are exercised
    /// * `core`   -- a prebuilt package, so `lib/` is exercised
    /// * `assets` -- an asset, so `etc/<component>/` is exercised
    ///
    /// Artifacts are target-agnostic so the fixture is not tied to the host triple.
    pub fn build(root: &Path, channel: &str) -> Self {
        let dir = root.join("offline-fixture");
        std::fs::create_dir_all(&dir).expect("failed to create fixture dir");

        // A stand-in executable that behaves well enough to be run with `--help`.
        let vm_binary = dir.join("miden-vm");
        std::fs::write(&vm_binary, "#!/bin/sh\nexit 0\n").expect("failed to write fixture binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&vm_binary, std::fs::Permissions::from_mode(0o755))
                .expect("failed to chmod fixture binary");
        }

        let core_package = dir.join("core.masp");
        std::fs::write(&core_package, b"fixture-package").expect("failed to write fixture package");

        let asset = dir.join("config.yml");
        std::fs::write(&asset, b"fixture: true\n").expect("failed to write fixture asset");

        let uri = |path: &Path| format!("file://{}", path.display());

        let manifest = serde_json::json!({
            "manifest_version": "2.0.0",
            "date": 1735689600,
            "channels": [{
                "name": channel,
                "components": [
                    {
                        "name": "vm",
                        "version": {"kind": "registry", "version": "0.1.0"},
                        "kind": "executable",
                        "installation_method": {"kind": "prebuilt"},
                        "installed-executable": "miden-vm",
                        "profiles": ["minimal"],
                        "artifacts": {"miden-vm": {"uri": uri(&vm_binary)}}
                    },
                    {
                        "name": "core",
                        "version": {"kind": "registry", "version": "0.1.0"},
                        "kind": "package",
                        "profiles": ["minimal"],
                        "artifacts": {"core.masp": {"uri": uri(&core_package)}}
                    },
                    {
                        "name": "assets",
                        "version": {"kind": "registry", "version": "0.1.0"},
                        "kind": "asset",
                        "profiles": ["complete"],
                        "artifacts": {"config.yml": {"uri": uri(&asset)}}
                    }
                ]
            }]
        });

        let manifest_path = dir.join("channel-manifest.json");
        std::fs::write(&manifest_path, serde_json::to_string_pretty(&manifest).unwrap())
            .expect("failed to write fixture manifest");

        Self {
            manifest_uri: format!("file://{}", manifest_path.display()),
            dir,
        }
    }
}
