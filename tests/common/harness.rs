//! Shared harness pieces for integration tests.

use std::sync::{Mutex, MutexGuard, OnceLock};

/// Serializes integration tests that mutate shared state outside their own temp directory.
///
/// Each test gets an isolated `MIDENUP_HOME`, but installs still run `cargo install` against a
/// shared `CARGO_HOME` and the shared Cargo registry/package cache. Running several installs
/// concurrently makes them contend, and which test loses the race varies between runs, so without
/// this a nondeterministic subset of the install tests fails while each one passes in isolation.
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

use flate2::{Compression, write::GzEncoder};

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
    pub manifest_path: PathBuf,
    /// `file://`-style URI to hand to `test_setup`.
    pub manifest_uri: String,
    /// Where the fixture's artifacts and manifest live.
    pub dir: PathBuf,
    devnet: String,
    mainnet: String,
    testnet: String,
    channels: Vec<serde_json::Value>,
}

impl OfflineFixture {
    pub fn new(root: &Path) -> Self {
        let dir = root.join("offline-fixture");
        std::fs::create_dir_all(&dir).expect("failed to create fixture dir");
        let manifest_path = dir.join("channel-manifest.json");
        let manifest_uri = format!("file://{}", manifest_path.display());

        Self {
            manifest_path,
            manifest_uri,
            dir,
            devnet: String::new(),
            mainnet: String::new(),
            testnet: String::new(),
            channels: vec![],
        }
    }

    /// Builds a channel containing one of each installable shape.
    ///
    /// * `vm`     -- a prebuilt executable, so `bin/` and `opt/` are exercised
    /// * `core`   -- a prebuilt package, so `lib/` is exercised
    /// * `assets` -- an asset, so `etc/<component>/` is exercised
    ///
    /// Artifacts are target-agnostic so the fixture is not tied to the host triple.
    pub fn create(root: &Path, channel: &str) -> Self {
        Self::new(root).with_channel(channel).build()
    }

    /// Finalizes the manifest for this fixture and writes it to the fixture directory
    pub fn build(mut self) -> Self {
        let channels = core::mem::take(&mut self.channels);
        let manifest = serde_json::json!({
            "manifest_version": "3.0.0",
            "date": 1735689600,
            "networks": {"devnet": self.devnet.clone(), "mainnet": self.mainnet.clone(), "testnet": self.testnet.clone()},
            "channels": channels
        });

        std::fs::write(&self.manifest_path, serde_json::to_string_pretty(&manifest).unwrap())
            .expect("failed to write fixture manifest");

        self
    }

    /// Adds a new channel, `channel`, to the set of channels in this fixture
    pub fn with_channel(mut self, channel: &str) -> Self {
        if self.channels.is_empty() {
            self.mainnet = channel.to_string();
            self.testnet = self.mainnet.clone();
            self.devnet = self.mainnet.clone();
        }

        let dir = self.dir.join(channel);
        std::fs::create_dir_all(&dir).expect("failed to create fixture channel dir");

        // A stand-in executable that behaves well enough to be run with `--help`.
        let vm_binary = dir.join("miden-vm");
        let vm_script = format!(
            r#"#!/bin/sh

echo "miden-vm {channel}"

exit 0
"#,
            channel = channel
        );
        std::fs::write(&vm_binary, &vm_script).expect("failed to write fixture binary");
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

        self.channels.push(serde_json::json!({
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
        }));

        self
    }

    /// Adds a `packages` component whose artifact is published inside a tarball.
    ///
    /// Built for real rather than stubbed: only genuine gzip bytes exercise the acquisition path an
    /// archived artifact actually takes. Nested under a directory, as release tarballs are, so the
    /// directory entry must not be mistaken for the artifact.
    pub fn with_archived_component(mut self) -> Self {
        use std::io::Write;

        let channel = self
            .channels
            .last_mut()
            .expect("with_archived_component needs a channel to add to");
        let name = channel["name"].as_str().expect("a channel has a name").to_string();

        let mut tar = tar::Builder::new(Vec::new());
        let mut dir = tar::Header::new_gnu();
        dir.set_entry_type(tar::EntryType::Directory);
        dir.set_size(0);
        dir.set_mode(0o755);
        dir.set_path("packages-1.0/").unwrap();
        dir.set_cksum();
        tar.append(&dir, std::io::empty()).expect("failed to add directory");

        let contents = b"archived-package";
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "packages-1.0/archived.masp", &contents[..])
            .expect("failed to add member");

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&tar.into_inner().unwrap())
            .expect("failed to compress fixture tarball");

        let tarball = self.dir.join(&name).join("archived.masp.tar.gz");
        std::fs::write(&tarball, encoder.finish().unwrap()).expect("failed to write tarball");

        channel["components"].as_array_mut().expect("a channel has components").push(
            serde_json::json!({
                "name": "packages",
                "version": {"kind": "registry", "version": "1.0.0"},
                "kind": "package",
                "profiles": ["minimal"],
                "artifacts": {
                    "archived.masp": {
                        "uri": format!("file://{}", tarball.display()),
                        "archive": "tar.gz"
                    }
                }
            }),
        );

        self
    }
}

/// Local `path`- and `git`-sourced crates, for exercising those authorities cheaply.
///
/// Installing from a path or a git revision is inherently a `cargo install`, so these tests cannot
/// avoid a build. What they *can* avoid is building something real: the behaviour under test is
/// whether midenup records a path's modification time and a git revision, and re-triggers an
/// install when either changes. A dependency-free crate proves that just as well as cloning an
/// entire component repository, and does it in about a second.
pub struct SourceFixture {
    /// A crate on disk, for `Authority::Path`.
    pub path_crate: PathBuf,
    /// A git repository, for `Authority::Git`.
    pub git_repo: PathBuf,
    /// Two commits in `git_repo`, oldest first, so an update can be triggered by moving between
    /// them.
    pub revisions: Vec<String>,
}

impl SourceFixture {
    pub fn build(root: &Path) -> Self {
        let path_crate = root.join("path-source");
        write_trivial_crate(&path_crate, "fixture-vm", "miden-vm");

        let git_repo = root.join("git-source");
        write_trivial_crate(&git_repo, "fixture-client", "miden-client");
        let revisions = init_repo_with_two_commits(&git_repo);

        Self { path_crate, git_repo, revisions }
    }

    /// A `file://` URL for the git repository, which cargo and `git` both accept.
    pub fn git_url(&self) -> String {
        format!("file://{}", self.git_repo.display())
    }
}

/// Writes a dependency-free crate producing a single binary.
fn write_trivial_crate(dir: &Path, package: &str, binary: &str) {
    std::fs::create_dir_all(dir.join("src")).expect("failed to create fixture crate dir");
    std::fs::write(
        dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{package}\"\nversion = \"0.1.0\"\nedition = \
             \"2021\"\n\n[[bin]]\nname = \"{binary}\"\npath = \"src/main.rs\"\n"
        ),
    )
    .expect("failed to write fixture Cargo.toml");
    std::fs::write(dir.join("src").join("main.rs"), "fn main() {}\n")
        .expect("failed to write fixture main.rs");

    // midenup installs with `--locked`, which requires a lockfile to be present.
    let status = std::process::Command::new("cargo")
        .arg("generate-lockfile")
        .arg("--manifest-path")
        .arg(dir.join("Cargo.toml"))
        .status()
        .expect("failed to run cargo generate-lockfile");
    assert!(status.success(), "cargo generate-lockfile failed for {}", dir.display());
}

/// Initializes a git repo with two commits, returning both revisions oldest-first.
fn init_repo_with_two_commits(dir: &Path) -> Vec<String> {
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|err| panic!("failed to run git {args:?}: {err}"));
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };

    git(&["init", "--quiet", "--initial-branch=main"]);
    // Set identity locally so the fixture does not depend on the developer's global git config.
    git(&["config", "user.email", "fixture@example.invalid"]);
    git(&["config", "user.name", "Fixture"]);

    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "first"]);
    let first = git(&["rev-parse", "HEAD"]);

    std::fs::write(dir.join("CHANGES"), b"second\n").expect("failed to write fixture change");
    git(&["add", "."]);
    git(&["commit", "--quiet", "-m", "second"]);
    let second = git(&["rev-parse", "HEAD"]);

    vec![first, second]
}

/// Writes a manifest whose `vm` comes from a path and whose `client` comes from a git revision.
pub fn write_source_manifest(
    dir: &Path,
    name: &str,
    fixture: &SourceFixture,
    revision: &str,
) -> String {
    let manifest = serde_json::json!({
        "manifest_version": "3.0.0",
        "date": 1735689600,
        "networks": {"mainnet": "0.15.0"},
        "channels": [{
            "name": "0.15.0",
            "components": [
                {
                    "name": "vm",
                    "version": {"kind": "path", "path": fixture.path_crate.to_str().unwrap()},
                    "kind": "executable",
                    "installation_method": {"kind": "cargo", "crate_name": "fixture-vm"},
                    "installed-executable": "miden-vm",
                    "profiles": ["minimal"]
                },
                {
                    "name": "client",
                    "version": {
                        "kind": "git",
                        "repository_url": fixture.git_url(),
                        "revision": revision
                    },
                    "kind": "executable",
                    "installation_method": {"kind": "cargo", "crate_name": "fixture-client"},
                    "installed-executable": "miden-client",
                    "profiles": ["minimal"]
                }
            ]
        }]
    });

    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap())
        .expect("failed to write source manifest");
    format!("file://{}", path.display())
}

/// A sequence of manifests describing an evolving set of channels, backed by local files.
///
/// Update semantics -- a stable version bump, a component appearing or disappearing, a version
/// moving backwards, an authority changing kind -- are all properties of the *manifest*, not of
/// the components it names. Real components prove nothing extra and cost minutes, so every
/// artifact here is a `file://` path to a tiny local stand-in.
pub struct UpdateFixture {
    dir: PathBuf,
}

impl UpdateFixture {
    pub fn build(root: &Path) -> Self {
        let dir = root.join("update-fixture");
        std::fs::create_dir_all(&dir).expect("failed to create update fixture dir");

        // `vm`'s URI carries `%version`, so each version resolves to a distinct file and version
        // changes are observable on disk.
        for version in ["0.23.1", "0.23.2", "0.23.3", "0.23.4"] {
            std::fs::write(dir.join(format!("miden-vm-{version}")), b"#!/bin/sh\nexit 0\n")
                .expect("failed to write fixture binary");
        }
        std::fs::write(dir.join("miden-client"), b"#!/bin/sh\nexit 0\n")
            .expect("failed to write fixture binary");
        std::fs::write(dir.join("core.masp"), b"fixture-package")
            .expect("failed to write fixture package");

        Self { dir }
    }

    fn uri(&self, name: &str) -> String {
        format!("file://{}", self.dir.join(name).display())
    }

    /// An executable whose artifact is versioned, so a version change moves it to another file.
    fn vm(&self, version: &str) -> serde_json::Value {
        serde_json::json!({
            "name": "vm",
            "version": {"kind": "registry", "version": version},
            "kind": "executable",
            "installation_method": {"kind": "prebuilt"},
            "installed-executable": "miden-vm",
            "profiles": ["minimal"],
            "artifacts": {"miden-vm": {"uri": self.uri("miden-vm-%version")}}
        })
    }

    /// A package. `authority` lets a channel change its authority *kind*, which is one of the
    /// changes update must notice.
    fn core(&self, authority: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "name": "core",
            "version": authority,
            "kind": "package",
            "profiles": ["minimal"],
            // No `%version` here: a git authority has no semantic version to substitute.
            "artifacts": {"core.masp": {"uri": self.uri("core.masp")}}
        })
    }

    fn client(&self) -> serde_json::Value {
        serde_json::json!({
            "name": "client",
            "version": {"kind": "registry", "version": "0.9.0"},
            "kind": "executable",
            "installation_method": {"kind": "prebuilt"},
            "installed-executable": "miden-client",
            "profiles": ["minimal"],
            "artifacts": {"miden-client": {"uri": self.uri("miden-client")}}
        })
    }

    fn registry(version: &str) -> serde_json::Value {
        serde_json::json!({"kind": "registry", "version": version})
    }

    fn write(&self, name: &str, mainnet: &str, channels: serde_json::Value) -> String {
        let manifest = serde_json::json!({
            "manifest_version": "3.0.0",
            "date": 1735689600,
            "networks": {"mainnet": mainnet},
            "channels": channels
        });
        let path = self.dir.join(name);
        std::fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap())
            .expect("failed to write fixture manifest");
        format!("file://{}", path.display())
    }

    /// Only 0.14.0 exists.
    pub fn initial(&self) -> String {
        self.write(
            "manifest-1.json",
            "0.14.0",
            serde_json::json!([{
                "name": "0.14.0",
                "components": [self.vm("0.23.2"), self.core(Self::registry("0.23.2"))]
            }]),
        )
    }

    /// 0.15.0 is released, so `stable` moves.
    pub fn with_new_stable(&self) -> String {
        self.write(
            "manifest-2.json",
            "0.15.0",
            serde_json::json!([
                {
                    "name": "0.14.0",
                    "components": [self.vm("0.23.2"), self.core(Self::registry("0.23.2"))]
                },
                {
                    "name": "0.15.0",
                    "components": [self.vm("0.23.3"), self.core(Self::registry("0.23.3"))]
                }
            ]),
        )
    }

    /// Every kind of change at once:
    ///
    /// * 0.14.0's `vm` moves *backwards* to 0.23.1 (a downgrade is still a change)
    /// * 0.14.0's `core` changes authority kind, registry to git
    /// * 0.14.0 gains `client`
    /// * 0.15.0 loses `core` entirely
    /// * 0.16.0 appears but is not installed, so a global update must ignore it
    pub fn with_every_change(&self) -> String {
        self.write(
            "manifest-3.json",
            "0.16.0",
            serde_json::json!([
                {
                    "name": "0.14.0",
                    "components": [
                        self.vm("0.23.1"),
                        self.core(serde_json::json!({
                            "kind": "git",
                            "repository_url": "https://example.invalid/core.git",
                            "revision": "0000000000000000000000000000000000000000"
                        })),
                        self.client()
                    ]
                },
                {
                    "name": "0.15.0",
                    "components": [self.vm("0.23.4")]
                },
                {
                    "name": "0.16.0",
                    "components": [self.vm("0.23.4"), self.core(Self::registry("0.23.4"))]
                }
            ]),
        )
    }

    /// mainnet stays on 0.14.0 while devnet moves to 0.15.0: two networks, two channels.
    pub fn with_split_networks(&self) -> String {
        self.write_split("manifest-split.json", "0.23.3")
    }

    /// The same two networks pointing at the same two channels, with 0.15.0's `vm` bumped.
    ///
    /// Nothing a network names has moved here, so following the pointer is a no-op -- and yet the
    /// channel devnet names is not up to date. This is the only way to tell "the pointer has not
    /// moved" apart from "there is nothing to do".
    pub fn with_split_networks_and_a_bumped_component(&self) -> String {
        self.write_split("manifest-split-bumped.json", "0.23.4")
    }

    /// mainnet is promoted onto the channel devnet already names: two networks, one channel.
    ///
    /// The case that must keep the two networks' `var/` directories apart: they share a toolchain
    /// but remain distinct networks, so `var/mainnet` and `var/devnet` stay separate stores.
    pub fn with_networks_on_one_channel(&self) -> String {
        self.write_split_at("manifest-shared.json", "0.15.0", "0.23.3")
    }

    fn write_split(&self, name: &str, devnet_vm: &str) -> String {
        self.write_split_at(name, "0.14.0", devnet_vm)
    }

    fn write_split_at(&self, name: &str, mainnet: &str, devnet_vm: &str) -> String {
        let manifest = serde_json::json!({
            "manifest_version": "3.0.0",
            "date": 1735689600,
            "networks": {"devnet": "0.15.0", "mainnet": mainnet},
            "channels": [
                {
                    "name": "0.14.0",
                    "components": [self.vm("0.23.2"), self.core(Self::registry("0.23.2"))]
                },
                {
                    "name": "0.15.0",
                    "components": [self.vm(devnet_vm), self.core(Self::registry("0.23.3"))]
                }
            ]
        });
        let path = self.dir.join(name);
        std::fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap())
            .expect("failed to write fixture manifest");
        format!("file://{}", path.display())
    }
}
