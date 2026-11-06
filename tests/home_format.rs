//! The home format guard, exercised through the real binary.
//!
//! These spawn `midenup` and `miden` because the guard runs at every entry point and its lease is a
//! `flock` shared between processes.

use std::{path::Path, process::Output};

use midenup::{paths, state::LocalState};

mod common;

use common::*;

/// An upstream that cannot be reached: nothing listens here.
const UNREACHABLE_UPSTREAM: &str = "https://127.0.0.1:1/nope.json";

/// How the refusal tells the user to start over.
const CLEAN_INSTALL: &str = "MIDENUP_HOME=<new dir> midenup init";

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A `miden` symlink to the binary under test.
fn miden_exe(env: &TestEnvironment) -> std::path::PathBuf {
    let bin = env.tmp_dir.path().join("bin");
    let miden = bin.join("miden");
    if !miden.exists() {
        std::fs::create_dir_all(&bin).unwrap();
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_midenup"), &miden).unwrap();
    }
    miden
}

fn miden(env: &TestEnvironment, manifest_uri: &str, args: &[&str]) -> Output {
    midenup_command(miden_exe(env), env, manifest_uri)
        .args(args)
        .output()
        .expect("failed to run miden")
}

/// Replaces the fixture's `vm` stand-in with one that runs for `seconds`.
fn make_vm_slow(fixture: &harness::OfflineFixture, seconds: u32) {
    use std::os::unix::fs::PermissionsExt;
    let vm = fixture.dir.join("0.15.0").join("miden-vm");
    std::fs::write(&vm, format!("#!/bin/sh\nsleep {seconds}\necho \"miden-vm 0.15.0\"\n")).unwrap();
    std::fs::set_permissions(&vm, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn install(env: &TestEnvironment, fixture: &harness::OfflineFixture) {
    assert_success(&run_midenup(env, &fixture.manifest_uri, &["install", "0.15.0"]));
}

fn envelope(env: &TestEnvironment) -> Option<serde_json::Value> {
    let contents = std::fs::read_to_string(paths::home_format_path(&env.midenup_home)).ok()?;
    Some(serde_json::from_str(&contents).unwrap())
}

fn write_envelope(env: &TestEnvironment, contents: &str) {
    std::fs::write(paths::home_format_path(&env.midenup_home), contents).unwrap();
}

fn state_json(env: &TestEnvironment) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(paths::state_path(&env.midenup_home)).unwrap())
        .unwrap()
}

/// Rewinds an installed home to what a release that predates the envelope leaves: no envelope,
/// and the given state version (`1.0.0` or `2.0.0`).
fn as_released(env: &TestEnvironment, state_version: &str) {
    std::fs::remove_file(paths::home_format_path(&env.midenup_home)).unwrap();
    let mut state = state_json(env);
    state["state_version"] = state_version.into();
    if state_version == "1.0.0" {
        for installation in state["installations"].as_array_mut().unwrap() {
            let installation = installation.as_object_mut().unwrap();
            installation.remove("patches");
            installation.remove("midenup_version");
        }
    }
    std::fs::write(paths::state_path(&env.midenup_home), state.to_string()).unwrap();
}

fn publications(home: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(paths::publications_dir(home))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Whether some process holds the format lease, tested by trying to take it exclusively.
fn lease_is_held(home: &Path) -> bool {
    let file = std::fs::File::open(midenup::lock::lease_path(home)).unwrap();
    let held = fs4::FileExt::try_lock(&file).is_err();
    let _ = fs4::FileExt::unlock(&file);
    held
}

fn wait_for_lease(home: &Path) {
    let start = std::time::Instant::now();
    while !(midenup::lock::lease_path(home).exists() && lease_is_held(home)) {
        assert!(start.elapsed().as_secs() < 10, "the component never took the lease");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[test]
fn integration_init_writes_the_home_format() {
    let _guard = harness::mutating_test_guard();
    let env = environment_setup("home_format_init");

    assert_success(&run_midenup(&env, UNREACHABLE_UPSTREAM, &["init"]));
    assert_eq!(envelope(&env), Some(serde_json::json!({"format": 1})));
}

/// A home holding only the manifest cache that `list` writes is empty, not a released home.
#[test]
fn integration_a_home_with_only_a_manifest_cache_is_initialized_not_converted() {
    let _guard = harness::mutating_test_guard();
    let env = environment_setup("home_format_cache_only");
    let fixture = harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");

    let listed = run_midenup(&env, &fixture.manifest_uri, &["list"]);
    assert_success(&listed);
    assert!(paths::manifest_cache(&env.midenup_home).exists());
    assert_eq!(envelope(&env), None, "a read-only command writes no envelope");

    let initialized = run_midenup(&env, &fixture.manifest_uri, &["init"]);
    assert_success(&initialized);
    assert_eq!(envelope(&env), Some(serde_json::json!({"format": 1})));
    for output in [&listed, &initialized] {
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("converted"), "an empty home is not converted: {stderr}");
    }
}

#[test]
fn integration_released_homes_are_converted_and_keep_working_offline() {
    let _guard = harness::mutating_test_guard();
    for state_version in ["1.0.0", "2.0.0"] {
        let env = environment_setup("home_format_released");
        let fixture = harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
        install(&env, &fixture);
        let installed = LocalState::load(&paths::state_path(&env.midenup_home)).unwrap();
        let published = publications(&env.midenup_home);
        as_released(&env, state_version);

        let output = miden(&env, UNREACHABLE_UPSTREAM, &["help", "vm"]);
        assert_success(&output);
        assert!(String::from_utf8_lossy(&output.stdout).contains("miden-vm 0.15.0"));

        assert_eq!(envelope(&env), Some(serde_json::json!({"format": 1})), "{state_version}");
        assert_eq!(state_json(&env)["state_version"], "2.0.0");
        let converted = LocalState::load(&paths::state_path(&env.midenup_home)).unwrap();
        assert_eq!(converted.installations.len(), installed.installations.len());
        assert_eq!(converted.installations[0].publication, installed.installations[0].publication);
        assert_eq!(converted.installations[0].components, installed.installations[0].components);
        assert_eq!(publications(&env.midenup_home), published, "nothing may be rebuilt");
    }
}

#[test]
fn integration_unknown_or_malformed_formats_are_refused_unchanged() {
    let _guard = harness::mutating_test_guard();
    let env = environment_setup("home_format_refused");
    let fixture = harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    install(&env, &fixture);

    for contents in [
        r#"{"format": 2}"#,
        r#"{"format": "one"}"#,
        r#"{"format": 1, "conversion": "from-a-later-release"}"#,
        "{\"format\":",
    ] {
        write_envelope(&env, contents);
        let before = snapshot_tree(&env.midenup_home);

        for args in [
            &["show", "list"][..],
            &["install", "0.15.0"],
            &["update"],
            &["uninstall", "0.15.0"],
            &["gc"],
        ] {
            let output = run_midenup(&env, &fixture.manifest_uri, args);
            assert!(!output.status.success(), "{contents}: {args:?} must refuse");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains(CLEAN_INSTALL), "{contents}: {args:?}: {stderr}");
        }
        assert!(!miden(&env, &fixture.manifest_uri, &["help", "vm"]).status.success());

        // Which midenup refused the home must still be answerable.
        for args in [&["--version"][..], &["--version=plain"], &["--help"]] {
            assert_success(&run_midenup(&env, &fixture.manifest_uri, args));
        }

        assert!(snapshot_tree(&env.midenup_home) == before, "{contents}: the home changed");
    }

    write_envelope(&env, r#"{"format": 2}"#);
    let stderr = String::from_utf8_lossy(&run_midenup(&env, UNREACHABLE_UPSTREAM, &["gc"]).stderr)
        .into_owned();
    assert!(stderr.contains("format 2") && stderr.contains("format 1"), "{stderr}");
}

#[test]
fn integration_a_leftover_alpha_manifest_beside_valid_state_is_left_alone() {
    let _guard = harness::mutating_test_guard();
    let env = environment_setup("home_format_leftover");
    let fixture = harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    install(&env, &fixture);

    let leftover = env.midenup_home.join("manifest.json");
    let alpha = r#"{"manifest_version":"1.0.1","channels":[{"name":"0.14.0","components":[]}]}"#;
    std::fs::write(&leftover, alpha).unwrap();

    for released in [false, true] {
        if released {
            as_released(&env, "2.0.0");
        }
        assert_success(&miden(&env, UNREACHABLE_UPSTREAM, &["help", "vm"]));
        assert_eq!(std::fs::read_to_string(&leftover).unwrap(), alpha);
        let state = LocalState::load(&paths::state_path(&env.midenup_home)).unwrap();
        assert_eq!(state.channels().map(ToString::to_string).collect::<Vec<_>>(), ["0.15.0"]);
        assert_eq!(envelope(&env), Some(serde_json::json!({"format": 1})));
    }
}

/// Needs the `fault-injection` feature, which compiles the abort point.
#[cfg(feature = "fault-injection")]
#[test]
fn integration_recovery_an_interrupted_conversion_resumes() {
    let _guard = harness::mutating_test_guard();
    let env = environment_setup("home_format_interrupted");
    let fixture = harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    install(&env, &fixture);
    as_released(&env, "1.0.0");

    let output = midenup_command(env!("CARGO_BIN_EXE_midenup"), &env, UNREACHABLE_UPSTREAM)
        .args(["show", "list"])
        .env(midenup::fault::FAULT_POINT_ENV, "post-format-commit")
        .output()
        .expect("failed to run midenup");
    assert!(!output.status.success());
    assert_eq!(
        envelope(&env),
        Some(serde_json::json!({"format": 1, "conversion": "released-v1"})),
        "the commit point must record the conversion"
    );
    assert_eq!(state_json(&env)["state_version"], "1.0.0", "no step may have run yet");

    // Resumed from the record alone: upstream is unreachable.
    assert_success(&miden(&env, UNREACHABLE_UPSTREAM, &["help", "vm"]));
    assert_eq!(envelope(&env), Some(serde_json::json!({"format": 1})));
    assert_eq!(state_json(&env)["state_version"], "2.0.0");
}

/// Commands share the lease: two running components do not wait on each other, and both hold it
/// until they exit.
#[test]
fn integration_running_components_share_the_format_lease() {
    let _guard = harness::mutating_test_guard();
    let env = environment_setup("home_format_shared");
    let fixture = harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    make_vm_slow(&fixture, 2);
    install(&env, &fixture);

    let spawn = || {
        midenup_command(miden_exe(&env), &env, UNREACHABLE_UPSTREAM)
            .args(["help", "vm"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("failed to spawn miden")
    };
    let start = std::time::Instant::now();
    let children = [spawn(), spawn()];
    wait_for_lease(&env.midenup_home);

    for child in children {
        assert_success(&child.wait_with_output().unwrap());
    }
    assert!(start.elapsed().as_secs() < 4, "shared holders must not wait on each other");
    assert!(!lease_is_held(&env.midenup_home), "the lease must be released on exit");
}

/// A conversion waits for the shared lease held by a running component.
#[test]
fn integration_a_conversion_waits_for_running_components() {
    let _guard = harness::mutating_test_guard();
    let env = environment_setup("home_format_waits");
    let fixture = harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    make_vm_slow(&fixture, 2);
    install(&env, &fixture);

    let start = std::time::Instant::now();
    let mut running = midenup_command(miden_exe(&env), &env, UNREACHABLE_UPSTREAM)
        .args(["help", "vm"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("failed to spawn miden");
    wait_for_lease(&env.midenup_home);

    // A committed conversion that the next command has to finish under the exclusive lease.
    write_envelope(&env, r#"{"format": 1, "conversion": "released-v1"}"#);
    assert_success(&run_midenup(&env, UNREACHABLE_UPSTREAM, &["show", "list"]));

    assert!(
        start.elapsed().as_secs() >= 2,
        "the conversion must not proceed while the component runs"
    );
    assert!(running.wait().unwrap().success());
    assert_eq!(envelope(&env), Some(serde_json::json!({"format": 1})));
}
