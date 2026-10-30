//! Released `midenup` binaries against what this checkout would publish.
//!
//! Each test runs the released binary named by its variable and is skipped when it is unset:
//! `MIDENUP_RELEASED_CLIENT` for any release, `MIDENUP_GUARDED_RELEASE` for releases that check
//! the home format.

use std::{
    path::{Path, PathBuf},
    process::Output,
};

use midenup::paths;

mod common;

use common::*;

/// The manifest this checkout publishes.
const CANDIDATE_MANIFEST: &str = full_path!("manifest/v3/manifest.json");

fn released(variable: &str) -> Option<PathBuf> {
    let binary = std::env::var_os(variable).map(PathBuf::from);
    if binary.is_none() {
        eprintln!("{variable} is not set; skipping");
    }
    binary
}

fn run(program: &Path, env: &TestEnvironment, manifest_uri: &str, args: &[&str]) -> Output {
    midenup_command(program, env, manifest_uri).args(args).output().unwrap()
}

fn assert_success(output: &Output, what: &str) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{what}\nstdout: {stdout}\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

#[test]
#[cfg(unix)]
fn released_client_reads_the_candidate_manifest() {
    let Some(released) = released("MIDENUP_RELEASED_CLIENT") else {
        return;
    };
    let _guard = harness::mutating_test_guard();
    let env = environment_setup("released_client");

    // Every channel of the candidate is listed.
    let candidate: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(CANDIDATE_MANIFEST).unwrap()).unwrap();
    let listed = assert_success(
        &run(&released, &env, &format!("file://{CANDIDATE_MANIFEST}"), &["list"]),
        "list the candidate manifest",
    );
    for channel in candidate["channels"].as_array().unwrap() {
        let name = channel["name"].as_str().unwrap();
        assert!(listed.lines().any(|line| line.trim() == name), "{name} is not listed: {listed}");
    }

    // A toolchain installs and runs from local artifacts under the candidate's header.
    let fixture = harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fixture.manifest_path).unwrap()).unwrap();
    for field in ["manifest_version", "min_client_version"] {
        manifest[field] = candidate[field].clone();
    }
    std::fs::write(&fixture.manifest_path, manifest.to_string()).unwrap();

    assert_success(&run(&released, &env, &fixture.manifest_uri, &["install", "0.15.0"]), "install");
    let miden = env.cargo_home.join("bin").join("miden");
    let vm = assert_success(&run(&miden, &env, &fixture.manifest_uri, &["vm"]), "run vm");
    assert_eq!(vm.trim(), "miden-vm 0.15.0");
}

#[test]
#[cfg(unix)]
fn released_midenup_refuses_a_newer_home_format_unchanged() {
    let Some(released) = released("MIDENUP_GUARDED_RELEASE") else {
        return;
    };
    let _guard = harness::mutating_test_guard();
    let env = environment_setup("released_guard");
    let fixture = harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    assert_success(&run(&released, &env, &fixture.manifest_uri, &["install", "0.15.0"]), "install");

    // A home one format past the one the release wrote.
    let envelope = paths::home_format_path(&env.midenup_home);
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&envelope).expect("no home format")).unwrap();
    let newer = written["format"].as_u64().unwrap() + 1;
    std::fs::write(&envelope, serde_json::json!({ "format": newer }).to_string()).unwrap();
    let before = snapshot_tree(&env.midenup_home);

    let miden = env.cargo_home.join("bin").join("miden");
    for (program, args) in [
        (&released, &["show", "list"][..]),
        (&released, &["install", "0.15.0"]),
        (&released, &["update"]),
        (&released, &["uninstall", "0.15.0"]),
        (&released, &["gc"]),
        (&miden, &["vm"]),
    ] {
        let output = run(program, &env, &fixture.manifest_uri, args);
        assert!(!output.status.success(), "{args:?} must refuse format {newer}");
    }
    assert!(snapshot_tree(&env.midenup_home) == before, "the home changed");
}
