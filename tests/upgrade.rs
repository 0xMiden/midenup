//! A home built by a released `midenup` keeps working once the candidate binary replaces it.
//!
//! Set `MIDENUP_UPGRADE_FROM` to the released binary; without it the test is skipped.

use std::path::{Path, PathBuf};

use midenup::{paths, state::LocalState};

mod common;

use common::*;

/// Stand-in executables and two manifests: `before` is what the release installs from, `after`
/// promotes mainnet to a new toolchain and bumps `vm` in the one mainnet named.
struct Fixture {
    before: String,
    after: String,
}

impl Fixture {
    fn build(root: &Path) -> Self {
        use std::os::unix::fs::PermissionsExt;

        let dir = root.join("fixture");
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["miden-vm-1", "miden-vm-2", "miden-client"] {
            let path = dir.join(name);
            std::fs::write(&path, format!("#!/bin/sh\necho {name}\n")).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let component = |name: &str,
                         version: &str,
                         executable: &str,
                         file: &str,
                         profiles: &[&str]| {
            serde_json::json!({
                "name": name,
                "version": {"kind": "registry", "version": version},
                "kind": "executable",
                "installation_method": {"kind": "prebuilt"},
                "installed-executable": executable,
                "profiles": profiles,
                "artifacts": {executable: {"uri": format!("file://{}", dir.join(file).display())}}
            })
        };
        let vm_1 = component("vm", "0.1.0", "miden-vm", "miden-vm-1", &["minimal"]);
        let vm_2 = component("vm", "0.2.0", "miden-vm", "miden-vm-2", &["minimal"]);
        let client = component("client", "0.1.0", "miden-client", "miden-client", &[]);
        let channel = |name: &str, vm: &serde_json::Value| serde_json::json!({"name": name, "components": [vm, client]});

        let write = |file: &str, date: i64, mainnet: &str, channels: Vec<serde_json::Value>| {
            let manifest = serde_json::json!({
                "manifest_version": "3.0.0",
                "min_client_version": "1.0.0",
                "date": date,
                "networks": {"mainnet": mainnet},
                "channels": channels
            });
            let path = dir.join(file);
            std::fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
            format!("file://{}", path.display())
        };

        Self {
            before: write(
                "before.json",
                1735689600,
                "0.16.0",
                vec![channel("0.15.0", &vm_1), channel("0.16.0", &vm_1)],
            ),
            after: write(
                "after.json",
                1735776000,
                "0.17.0",
                vec![channel("0.15.0", &vm_1), channel("0.16.0", &vm_2), channel("0.17.0", &vm_2)],
            ),
        }
    }
}

/// Runs `program` and returns its stdout, failing the test if it fails.
fn run(
    program: &Path,
    env: &TestEnvironment,
    manifest: &str,
    toolchain: Option<&str>,
    args: &[&str],
) -> String {
    let mut command = midenup_command(program, env, manifest);
    if let Some(toolchain) = toolchain {
        command.env("MIDENUP_TOOLCHAIN", toolchain);
    }
    let output = command.args(args).output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{} {}: {}",
        program.display(),
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

/// Puts `binary` where midenup is installed, as an upgrade does; `miden` links there.
fn install_binary(binary: &Path, at: &Path) {
    // A fresh file rather than an overwrite: macOS caches code signatures per inode.
    let _ = std::fs::remove_file(at);
    std::fs::copy(binary, at).unwrap();
}

#[test]
#[cfg(unix)]
fn upgrade_from_a_released_midenup_keeps_the_home() {
    let Some(released) = std::env::var_os("MIDENUP_UPGRADE_FROM").map(PathBuf::from) else {
        eprintln!("MIDENUP_UPGRADE_FROM is not set; skipping");
        return;
    };

    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("upgrade_from_a_released_midenup");
    let fixture = Fixture::build(env.tmp_dir.path());
    let bin = env.tmp_dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let midenup = bin.join("midenup");
    let miden = env.cargo_home.join("bin").join("miden");
    let home = &env.midenup_home;

    // The release builds a home: mainnet followed with an extra component, a pinned version with
    // one too, a default override, and data in both stores.
    install_binary(&released, &midenup);
    run(
        &midenup,
        &env,
        &fixture.before,
        None,
        &["install", "mainnet", "--component", "client"],
    );
    run(
        &midenup,
        &env,
        &fixture.before,
        None,
        &["install", "0.15.0", "--component", "client"],
    );
    run(&midenup, &env, &fixture.before, None, &["override", "0.15.0"]);
    for (store, contents) in [("mainnet", "network data"), ("0.15.0", "pinned data")] {
        std::fs::create_dir_all(home.join("var").join(store)).unwrap();
        std::fs::write(home.join("var").join(store).join("store"), contents).unwrap();
    }

    // The candidate replaces it, and its first command converts the home without rebuilding it.
    let publications = snapshot_tree(&home.join("publications"));
    let links = snapshot_tree(&home.join("toolchains"));
    install_binary(Path::new(env!("CARGO_BIN_EXE_midenup")), &midenup);
    assert_eq!(run(&miden, &env, &fixture.before, None, &["vm"]).trim(), "miden-vm-1");
    let envelope = std::fs::read_to_string(paths::home_format_path(home)).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&envelope).unwrap(),
        serde_json::json!({"format": 1})
    );
    assert!(snapshot_tree(&home.join("publications")) == publications, "nothing was rebuilt");
    assert!(snapshot_tree(&home.join("toolchains")) == links, "every link is kept");

    // Then it brings the home up to date with a newer manifest.
    run(&midenup, &env, &fixture.after, None, &["update"]);
    run(&midenup, &env, &fixture.after, None, &["update", "mainnet"]);

    let state = LocalState::load(&home.join("state.json")).unwrap();
    for channel in [(0, 15, 0), (0, 17, 0)] {
        let version = semver::Version::new(channel.0, channel.1, channel.2);
        let installation = state.get(&version).unwrap_or_else(|| panic!("{version} is installed"));
        assert!(installation.intent.roots.contains("client"), "{version} kept its selection");
    }
    let toolchains = home.join("toolchains");
    assert_eq!(std::fs::read_link(toolchains.join("mainnet")).unwrap(), Path::new("0.17.0"));
    assert!(std::fs::read_link(toolchains.join("default")).unwrap().ends_with("0.15.0"));
    assert!(
        snapshot_tree(&toolchains).get(Path::new("0.15.0")) == links.get(Path::new("0.15.0")),
        "0.15.0 did not change upstream and was not rebuilt"
    );
    for (store, contents) in [("mainnet", "network data"), ("0.15.0", "pinned data")] {
        let read = std::fs::read_to_string(home.join("var").join(store).join("store")).unwrap();
        assert_eq!(read, contents, "var/{store} is intact");
    }

    // Dispatch: the default is still the pinned toolchain, and mainnet runs its new `vm`.
    assert_eq!(run(&miden, &env, &fixture.after, None, &["vm"]).trim(), "miden-vm-1");
    assert_eq!(run(&miden, &env, &fixture.after, Some("mainnet"), &["vm"]).trim(), "miden-vm-2");
    assert_eq!(
        run(&miden, &env, &fixture.after, Some("mainnet"), &["client"]).trim(),
        "miden-client"
    );
}
