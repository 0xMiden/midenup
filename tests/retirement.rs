//! Retiring a channel removes it from the upstream manifest and nothing else: an installed copy
//! keeps working, stays listed and selectable, and is removed only when the user uninstalls it.

use std::path::Path;

mod common;

use common::{harness::OfflineFixture, *};

/// Publishes `0.15.0` and `0.16.0`, and installs `0.15.0` from that manifest.
fn install_0_15(env: &TestEnvironment) {
    let published = OfflineFixture::new(&env.tmp_dir.path().join("published"))
        .with_channel("0.15.0")
        .with_channel("0.16.0")
        .build();
    let installed = run_midenup(env, &published.manifest_uri, &["install", "0.15.0"]);
    assert!(installed.status.success(), "{}", String::from_utf8_lossy(&installed.stderr));
}

/// A manifest in which `0.15.0` is retired: only `0.16.0` is published, superseding `0.15.0` when
/// `migrates` is set.
fn retired_manifest(env: &TestEnvironment, name: &str, migrates: bool) -> String {
    let fixture = OfflineFixture::new(&env.tmp_dir.path().join(name))
        .with_channel("0.16.0")
        .build();
    if migrates {
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&fixture.manifest_path).unwrap()).unwrap();
        manifest["channels"][0]["migrates_from"] = "0.15.0".into();
        std::fs::write(&fixture.manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    }
    fixture.manifest_uri
}

/// Runs the installed `miden` from `dir`, and returns what the selected `miden-vm` printed.
fn miden_vm(env: &TestEnvironment, manifest_uri: &str, dir: &Path, selector: &[&str]) -> String {
    let output = midenup_command(env.cargo_home.join("bin/miden"), env, manifest_uri)
        .current_dir(dir)
        .env_remove("MIDENUP_TOOLCHAIN")
        .args(selector)
        .args(["help", "vm"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn integration_a_retired_channel_stays_usable_until_uninstalled() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("retired_usable");
    install_0_15(&env);
    let retired = retired_manifest(&env, "retired", false);
    let toolchain = env.midenup_home.join("toolchains/0.15.0");

    // Every way of selecting it still runs the installed copy.
    let pwd = env.present_working_dir.clone();
    assert_eq!(miden_vm(&env, &retired, &pwd, &["+0.15.0"]), "miden-vm 0.15.0\n");

    let project = env.tmp_dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("miden-toolchain.toml"), "[toolchain]\nchannel = \"0.15.0\"\n")
        .unwrap();
    assert_eq!(miden_vm(&env, &retired, &project, &[]), "miden-vm 0.15.0\n");

    let overridden = run_midenup(&env, &retired, &["override", "0.15.0"]);
    assert!(overridden.status.success(), "{}", String::from_utf8_lossy(&overridden.stderr));
    assert_eq!(miden_vm(&env, &retired, &pwd, &[]), "miden-vm 0.15.0\n");

    let listed = run_midenup(&env, &retired, &["show", "list"]);
    assert!(
        String::from_utf8_lossy(&listed.stdout).contains("0.15.0"),
        "a retired installation must still be listed: {}",
        String::from_utf8_lossy(&listed.stdout)
    );

    // Updating has nothing to reconcile it against, so it leaves it as it is.
    let updated = run_midenup(&env, &retired, &["update"]);
    assert!(updated.status.success(), "{}", String::from_utf8_lossy(&updated.stderr));
    assert!(
        String::from_utf8_lossy(&updated.stderr).contains("not in the upstream manifest"),
        "{}",
        String::from_utf8_lossy(&updated.stderr)
    );
    assert!(
        toolchain.join("bin/miden-vm").exists(),
        "update must not remove a retired channel"
    );
    assert_eq!(miden_vm(&env, &retired, &pwd, &["+0.15.0"]), "miden-vm 0.15.0\n");

    let uninstalled = run_midenup(&env, &retired, &["uninstall", "0.15.0"]);
    assert!(uninstalled.status.success(), "{}", String::from_utf8_lossy(&uninstalled.stderr));
    assert!(!toolchain.exists(), "uninstall must remove it");
    let listed = run_midenup(&env, &retired, &["show", "list"]);
    assert!(
        !String::from_utf8_lossy(&listed.stdout).contains("0.15.0"),
        "an uninstalled channel is no longer listed: {}",
        String::from_utf8_lossy(&listed.stdout)
    );
}

/// A channel declaring `migrates_from` the retired one leaves the installed copy in use until the
/// user updates, which then moves the installation onto it.
#[test]
fn integration_a_successor_of_a_retired_channel_takes_over_only_on_update() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("retired_successor");
    install_0_15(&env);
    let successor = retired_manifest(&env, "successor", true);
    let pwd = env.present_working_dir.clone();

    assert_eq!(miden_vm(&env, &successor, &pwd, &["+0.15.0"]), "miden-vm 0.15.0\n");
    let listed = run_midenup(&env, &successor, &["show", "list"]);
    assert!(
        String::from_utf8_lossy(&listed.stdout).contains("update available"),
        "the successor must show as a pending update: {}",
        String::from_utf8_lossy(&listed.stdout)
    );

    let updated = run_midenup(&env, &successor, &["update"]);
    assert!(updated.status.success(), "{}", String::from_utf8_lossy(&updated.stderr));
    let toolchains = env.midenup_home.join("toolchains");
    assert!(toolchains.join("0.16.0/bin/miden-vm").exists(), "update must migrate to 0.16.0");
    assert!(!toolchains.join("0.15.0").exists(), "and retire the migrated installation");
    assert_eq!(miden_vm(&env, &successor, &pwd, &["+0.16.0"]), "miden-vm 0.16.0\n");
}
