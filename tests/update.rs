use std::{io::Write, process::Stdio};

use clap::Parser;
use midenup::{commands::Midenup, version};

mod common;

use common::*;

/// Update semantics: stable version bumps, and every kind of per-component change.
///
/// Everything asserted here is a property of the manifest rather than of the components it names,
/// so the fixture uses `file://` stand-ins. Real components would prove nothing extra and cost
/// minutes.
#[test]
fn integration_update_test() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_update_test");
    let fixture = common::harness::UpdateFixture::build(test_env.tmp_dir.path());

    let toolchain_dir = test_env.midenup_home.join("toolchains");
    let toolchain_v14 = toolchain_dir.join("0.14.0");
    let toolchain_v15 = toolchain_dir.join("0.15.0");
    let toolchain_v16 = toolchain_dir.join("0.16.0");
    let toolchain_mainnet = toolchain_dir.join("mainnet");

    let mainnet_points_at = || {
        std::fs::read_link(&toolchain_mainnet)
            .expect("the mainnet link must exist")
            .file_name()
            .expect("the mainnet link must name a channel")
            .to_string_lossy()
            .into_owned()
    };

    // Only 0.14.0 exists upstream, so that is what `stable` means.
    let (mut state, config) = test_setup(&test_env, &fixture.initial());

    Midenup::try_parse_from(["midenup", "init"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to initialize");

    Midenup::try_parse_from(["midenup", "install", "stable"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install stable");

    assert!(toolchain_v14.exists());
    assert_eq!(mainnet_points_at(), "0.14.0");

    // 0.15.0 is released. Updating stable must install it *and* leave 0.14.0 alone: a version bump
    // is an additional installation, not a replacement.
    let (_, config) = test_setup(&test_env, &fixture.with_new_stable());

    Midenup::try_parse_from(["midenup", "update", "stable"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to update stable");

    assert!(toolchain_v14.exists(), "the previous toolchain must be retained");
    assert!(toolchain_v15.exists(), "the new stable toolchain must be installed");
    assert!(toolchain_mainnet.is_symlink());
    assert_eq!(mainnet_points_at(), "0.15.0", "mainnet must follow the upstream bump");

    // A global update touches every *installed* toolchain. The manifest now changes something of
    // each kind at once -- see `UpdateFixture::with_every_change`.
    let (_, config) = test_setup(&test_env, &fixture.with_every_change());

    Midenup::try_parse_from(["midenup", "update"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to update");

    // A global update must not move mainnet, even though 0.16.0 now exists upstream: it updates
    // what is installed, and 0.16.0 is not.
    assert!(toolchain_mainnet.is_symlink());
    assert_eq!(mainnet_points_at(), "0.15.0", "a global update must not move mainnet");
    assert!(!toolchain_v16.exists(), "a global update must not install a new channel");

    // A component removed upstream must be removed on disk.
    assert!(
        !toolchain_v15.join("lib").join("core.masp").exists(),
        "core was removed from 0.15.0 upstream, so its artifact must be gone"
    );

    // A component added upstream must appear on disk.
    assert!(
        toolchain_v14.join("bin").join("miden-client").exists(),
        "client was added to 0.14.0 upstream, so it must be installed"
    );

    // A component whose *authority kind* changed must be recorded with the new authority.
    let core_authority = &state
        .get(&semver::Version::new(0, 14, 0))
        .expect("0.14.0 must still be installed")
        .components
        .iter()
        .find(|c| c.name == "core")
        .expect("core must still be part of 0.14.0")
        .version;
    assert!(
        matches!(core_authority, version::Authority::Git { .. }),
        "core's authority changed from registry to git upstream, got {core_authority:#?}"
    );

    // A version moving *backwards* is still a change. `vm`'s artifact is versioned, so a downgrade
    // is observable as a different source file having been installed.
    let vm_authority = &state
        .get(&semver::Version::new(0, 14, 0))
        .unwrap()
        .components
        .iter()
        .find(|c| c.name == "vm")
        .expect("vm must still be part of 0.14.0")
        .version;
    assert!(
        matches!(
            vm_authority,
            version::Authority::Registry { version } if *version == semver::Version::new(0, 23, 1)
        ),
        "0.14.0's vm was downgraded upstream, got {vm_authority:#?}"
    );

    // Updating stable again picks up the newly released 0.16.0 and moves the symlink.
    Midenup::try_parse_from(["midenup", "update", "stable"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to update stable");

    assert!(toolchain_v16.exists());
    assert_eq!(mainnet_points_at(), "0.16.0");
}

/// Local diagnostics must not depend on the network: with an unreachable manifest and no cache,
/// an update with nothing installed still says so, and a missing pinned version is still named.
#[test]
fn integration_update_checks_local_state_before_syncing() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_update_offline");
    let unreachable = format!("file://{}/no-such-manifest.json", test_env.tmp_dir.path().display());
    let (mut state, config) = test_setup(&test_env, &unreachable);

    Midenup::try_parse_from(["midenup", "update"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("an update with nothing installed must not need the manifest");

    let err = Midenup::try_parse_from(["midenup", "update", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect_err("a version that is not installed must be an error");
    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("No installed channel found with version 0.15.0"),
        "the local diagnostic must name the version: {rendered}"
    );
    assert!(
        !rendered.contains("unable to fetch"),
        "the network failure must not mask the local diagnostic: {rendered}"
    );
}

#[test]
fn integration_update_clears_an_equivalent_patch_without_republishing() {
    assert_equivalent_patch_cleared("update_equivalent_patch", false);
}

#[test]
fn integration_metadata_update_clears_an_equivalent_patch_without_republishing() {
    assert_equivalent_patch_cleared("metadata_update_equivalent_patch", true);
}

/// Removing a patch with the same build inputs as upstream only changes recorded metadata.
fn assert_equivalent_patch_cleared(test_name: &str, add_alias: bool) {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup(test_name);
    let fixture = common::harness::OfflineFixture::new(env.tmp_dir.path())
        .with_channel("0.15.0")
        .with_cargo_component("prover")
        .build();
    let project = env.tmp_dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join("miden-toolchain.toml"),
        format!(
            r#"[toolchain]
channel = "0.15.0"
components = ["prover"]

[patches.prover]
version = {{ kind = "path", path = "{}" }}
"#,
            fixture.dir.join("prover-source").display()
        ),
    )
    .unwrap();
    let project_config = midenup::config::Config::init(
        project,
        env.midenup_home.clone(),
        env.cargo_home.clone(),
        &fixture.manifest_uri,
        true,
    )
    .unwrap();
    let (mut state, _) = test_setup(&env, &fixture.manifest_uri);
    Midenup::try_parse_from(["miden", "help", "prover"])
        .unwrap()
        .execute_with_state(&project_config, &mut state)
        .expect("failed to activate the patched toolchain");

    let channel = semver::Version::new(0, 15, 0);
    let before = state.get(&channel).unwrap().clone();
    assert!(before.patches.contains_key("prover"), "the project must record its patch");

    if add_alias {
        let mut manifest: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&fixture.manifest_path).unwrap())
                .unwrap();
        let prover = manifest["channels"][0]["components"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|component| component["name"] == "prover")
            .unwrap();
        prover["aliases"] = serde_json::json!({"prove": ["%installed-executable"]});
        std::fs::write(&fixture.manifest_path, serde_json::to_vec_pretty(&manifest).unwrap())
            .unwrap();
    }

    let (_, config) = test_setup(&env, &fixture.manifest_uri);
    let listed = run_midenup(&env, &fixture.manifest_uri, &["list"]);
    assert!(listed.status.success());
    Midenup::try_parse_from(["midenup", "update", "--path-update=off", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to update the patched toolchain");

    let reloaded =
        midenup::state::LocalState::load(&midenup::paths::state_path(&env.midenup_home)).unwrap();
    let after = reloaded.get(&channel).unwrap();
    assert!(after.patches.is_empty(), "update must remove the equivalent patch from state");
    assert_eq!(after.publication, before.publication, "equivalent inputs must not be rebuilt");
    assert_eq!(
        after.as_channel().get_component("prover").unwrap().version,
        before.as_channel().get_component("prover").unwrap().version,
        "the recorded source pin must survive a metadata update"
    );
    if add_alias {
        assert!(after.as_channel().get_alias_names().contains("prove"));
    }

    Midenup::try_parse_from(["miden", "+0.15.0", "help", "prover"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("the unpatched toolchain must be usable outside the project");
    assert_eq!(state.get(&channel).unwrap().publication, before.publication);
    assert!(
        String::from_utf8_lossy(&listed.stdout).contains("(update available)"),
        "a recorded patch must be reported as an available update"
    );
}

/// Repointing a network must reconcile the whole installed target before clearing its patches.
#[test]
fn integration_network_update_clears_a_patch_and_reacquires_other_changed_components() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("network_update_patched_target");
    let fixture = common::harness::OfflineFixture::new(env.tmp_dir.path())
        .with_channel("0.16.0")
        .with_channel("0.15.0")
        .with_cargo_component("prover")
        .build();
    let (mut state, config) = test_setup(&env, &fixture.manifest_uri);
    Midenup::try_parse_from(["midenup", "install", "mainnet"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install the network's original channel");

    let project = env.tmp_dir.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        project.join("miden-toolchain.toml"),
        format!(
            r#"[toolchain]
channel = "0.15.0"
components = ["prover"]

[patches.prover]
version = {{ kind = "path", path = "{}" }}
"#,
            fixture.dir.join("prover-source").display()
        ),
    )
    .unwrap();
    let project_config = midenup::config::Config::init(
        project,
        env.midenup_home.clone(),
        env.cargo_home.clone(),
        &fixture.manifest_uri,
        true,
    )
    .unwrap();
    Midenup::try_parse_from(["miden", "help", "prover"])
        .unwrap()
        .execute_with_state(&project_config, &mut state)
        .expect("failed to activate the patched target");

    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fixture.manifest_path).unwrap()).unwrap();
    manifest["networks"]["mainnet"] = serde_json::json!("0.15.0");
    let updated_vm = fixture.dir.join("0.15.0").join("miden-vm-updated");
    let updated_bytes = b"#!/bin/sh\necho updated-vm\n";
    std::fs::write(&updated_vm, updated_bytes).unwrap();
    let vm = manifest["channels"][1]["components"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|component| component["name"] == "vm")
        .unwrap();
    vm["artifacts"]["miden-vm"]["uri"] =
        serde_json::json!(format!("file://{}", updated_vm.display()));
    std::fs::write(&fixture.manifest_path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();

    let (_, config) = test_setup(&env, &fixture.manifest_uri);
    Midenup::try_parse_from(["midenup", "update", "mainnet"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to update the network to its patched target");

    let target = midenup::paths::toolchain_link(&env.midenup_home, &semver::Version::new(0, 15, 0));
    assert_eq!(
        std::fs::read(target.join("bin").join("miden-vm")).unwrap(),
        updated_bytes,
        "the unpatched component's changed bytes must be acquired too"
    );
    let reloaded =
        midenup::state::LocalState::load(&midenup::paths::state_path(&env.midenup_home)).unwrap();
    assert!(reloaded.get(&semver::Version::new(0, 15, 0)).unwrap().patches.is_empty());
    assert_eq!(
        std::fs::read_link(env.midenup_home.join("toolchains").join("mainnet")).unwrap(),
        std::path::PathBuf::from("0.15.0")
    );
}

/// Interactive update UI is diagnostic interaction, not a command result, and survives quiet.
#[test]
fn integration_interactive_path_update_uses_stderr() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_interactive_path_update_streams");
    let fixture = common::harness::OfflineFixture::new(test_env.tmp_dir.path())
        .with_channel("0.15.0")
        .with_cargo_component("prover")
        .build();

    let installed = run_midenup(&test_env, &fixture.manifest_uri, &["install", "stable"]);
    assert!(installed.status.success(), "{}", String::from_utf8_lossy(&installed.stderr));
    let mut manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&fixture.manifest_path).expect("failed to read fixture manifest"),
    )
    .expect("fixture manifest is invalid");
    let prover = manifest["channels"][0]["components"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|component| component["name"] == "prover")
        .expect("fixture has no prover component");
    // A changed Cargo feature is installation-impacting, so policy for the path-sourced component
    // must ask before rebuilding it. The declined choices below keep the nonexistent feature from
    // ever reaching Cargo.
    prover["installation_method"]["features"] = serde_json::json!(["changed-upstream"]);
    std::fs::write(&fixture.manifest_path, serde_json::to_vec_pretty(&manifest).unwrap())
        .expect("failed to update fixture manifest");

    let interact = |args: &[&str], response: &[u8]| {
        let mut child =
            midenup_command(env!("CARGO_BIN_EXE_midenup"), &test_env, &fixture.manifest_uri)
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("failed to launch interactive update");
        child
            .stdin
            .take()
            .expect("interactive update has no stdin")
            .write_all(response)
            .expect("failed to answer interactive update");
        child.wait_with_output().expect("failed to wait for interactive update")
    };

    let skipped = interact(&["update", "--path-update=interactive"], b"n\n");
    assert!(skipped.status.success(), "{}", String::from_utf8_lossy(&skipped.stderr));
    assert!(skipped.stdout.is_empty(), "prompt leaked to stdout: {:?}", skipped.stdout);
    let stderr = String::from_utf8_lossy(&skipped.stderr);
    assert!(stderr.contains("Would you like to update prover?"), "missing prompt: {stderr}");
    assert!(stderr.contains("Skipping prover"), "missing acknowledgement: {stderr}");

    let cancelled = interact(&["update", "--path-update=interactive", "-q"], b"c\n");
    assert!(cancelled.status.success(), "{}", String::from_utf8_lossy(&cancelled.stderr));
    assert!(cancelled.stdout.is_empty(), "prompt leaked to stdout: {:?}", cancelled.stdout);
    let stderr = String::from_utf8_lossy(&cancelled.stderr);
    assert!(
        stderr.contains("Would you like to update prover?"),
        "quiet suppressed the prompt: {stderr}"
    );
    assert!(stderr.contains("Cancelling update"), "missing acknowledgement: {stderr}");
}
