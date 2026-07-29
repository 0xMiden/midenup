//! Migrating a v1.0.1 local manifest, as it actually happens: at startup, in the real binary.
//!
//! These spawn `midenup` rather than calling into the library, because the property under test is
//! about *ordering within startup* -- migration must happen before the upstream manifest is
//! fetched, so an unreachable upstream cannot strand a user on an unreadable local document. An
//! in-process test that builds its own `Config` has already done the fetch.

use std::{path::Path, process::Command};

use midenup::{
    migrate_v1::v1_manifest_path,
    paths,
    state::{LocalState, PublicationRef},
};

mod common;

use common::*;

/// An upstream that cannot be reached: nothing listens here.
const UNREACHABLE_UPSTREAM: &str = "https://127.0.0.1:1/nope.json";

fn v1_manifest(version: &str, channel: &str, components: &[&str]) -> String {
    let components: Vec<_> = components
        .iter()
        .map(|name| {
            serde_json::json!({
                "name": name,
                "version": "0.1.0",
                "installed_executable": format!("miden-{name}"),
            })
        })
        .collect();

    serde_json::json!({
        "manifest_version": version,
        "date": 1735689600,
        "channels": [{"name": channel, "components": components}]
    })
    .to_string()
}

fn write_v1_manifest(env: &TestEnvironment, contents: &str) {
    std::fs::create_dir_all(&env.midenup_home).unwrap();
    std::fs::write(v1_manifest_path(&env.midenup_home), contents).unwrap();
}

fn run_midenup(env: &TestEnvironment, manifest_uri: &str, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_midenup"))
        .args(args)
        .current_dir(&env.present_working_dir)
        .env("MIDENUP_HOME", &env.midenup_home)
        .env("CARGO_HOME", &env.cargo_home)
        .env("MIDENUP_MANIFEST_URI", manifest_uri)
        .output()
        .expect("failed to run midenup")
}

fn state_of(home: &Path) -> LocalState {
    LocalState::load(&paths::state_path(home)).expect("state.json must be readable")
}

/// Migration is the first local operation, ahead of any upstream fetch.
///
/// A user whose network is down must still end up with a readable `state.json`: making migration
/// depend on a successful fetch would strand exactly the people whose installation most needs to
/// keep working.
#[test]
fn integration_a_v1_manifest_is_migrated_even_when_upstream_is_unreachable() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("v1_migrate_offline");
    write_v1_manifest(&env, &v1_manifest("1.0.1", "0.15.0", &["vm", "client"]));

    let output = run_midenup(&env, UNREACHABLE_UPSTREAM, &["list"]);
    assert!(
        !output.status.success(),
        "the command itself must fail on the unreachable upstream"
    );

    let state = state_of(&env.midenup_home);
    assert_eq!(state.installations.len(), 1, "the installed channel must be carried forward");

    let installation = state.get(&semver::Version::new(0, 15, 0)).expect("0.15.0 must be recorded");
    assert!(matches!(installation.publication, PublicationRef::NeedsReinstall));
    assert!(installation.intent.roots.contains("vm"));
    assert!(installation.intent.roots.contains("client"));
    assert!(
        installation.intent.profiles.is_empty(),
        "migrated intent is roots-only, so unrelated new profile members are not pulled in"
    );

    assert!(
        !v1_manifest_path(&env.midenup_home).exists(),
        "the v1 document is removed once the state document is committed"
    );
}

/// Below the floor, the file is left byte-for-byte intact: the user's only remaining option is the
/// binary that wrote it, and destroying its record would take that away too.
#[test]
fn integration_a_document_below_the_migration_floor_is_rejected_and_preserved() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("v1_too_old");
    write_v1_manifest(&env, &v1_manifest("1.0.0", "0.15.0", &["vm"]));

    let path = v1_manifest_path(&env.midenup_home);
    let before = std::fs::read(&path).unwrap();

    let output = run_midenup(&env, UNREACHABLE_UPSTREAM, &["list"]);
    assert!(!output.status.success(), "must refuse");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("1.0.0"), "the diagnostic must name the version found: {stderr}");

    assert_eq!(std::fs::read(&path).unwrap(), before, "the file must be untouched");
    assert!(
        !paths::state_path(&env.midenup_home).exists(),
        "and must leave no partial state document"
    );
}

/// Startup must be idempotent: a second run migrates nothing and rewrites nothing.
#[test]
fn integration_re_running_startup_after_migration_changes_nothing() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("v1_idempotent");
    write_v1_manifest(&env, &v1_manifest("1.0.1", "0.15.0", &["vm"]));

    run_midenup(&env, UNREACHABLE_UPSTREAM, &["list"]);
    let after_first = std::fs::read(paths::state_path(&env.midenup_home)).unwrap();

    run_midenup(&env, UNREACHABLE_UPSTREAM, &["list"]);
    assert_eq!(std::fs::read(paths::state_path(&env.midenup_home)).unwrap(), after_first);
}

/// A failure anywhere before the rename leaves the v1 document intact and writes no state.
///
/// Needs the `fault-injection` feature, which compiles the abort point; run with
/// `make recovery-test`.
#[cfg(feature = "fault-injection")]
#[test]
fn integration_recovery_a_failure_before_the_migration_commit_preserves_the_v1_document() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("v1_precommit_failure");
    write_v1_manifest(&env, &v1_manifest("1.0.1", "0.15.0", &["vm"]));

    let path = v1_manifest_path(&env.midenup_home);
    let before = std::fs::read(&path).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_midenup"))
        .arg("list")
        .current_dir(&env.present_working_dir)
        .env("MIDENUP_HOME", &env.midenup_home)
        .env("CARGO_HOME", &env.cargo_home)
        .env("MIDENUP_MANIFEST_URI", UNREACHABLE_UPSTREAM)
        .env(midenup::fault::FAULT_POINT_ENV, "pre-migration-commit")
        .output()
        .expect("failed to run midenup");

    assert!(!output.status.success());
    assert_eq!(std::fs::read(&path).unwrap(), before, "the v1 document must be untouched");
    assert!(
        !paths::state_path(&env.midenup_home).exists(),
        "and no partial state document may be left behind"
    );
}
