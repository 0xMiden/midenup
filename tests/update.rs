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
    assert!(
        stderr.contains("Would you like to update this component?"),
        "missing prompt: {stderr}"
    );
    assert!(stderr.contains("Skipping prover"), "missing acknowledgement: {stderr}");

    let cancelled = interact(&["update", "--path-update=interactive", "-q"], b"c\n");
    assert!(cancelled.status.success(), "{}", String::from_utf8_lossy(&cancelled.stderr));
    assert!(cancelled.stdout.is_empty(), "prompt leaked to stdout: {:?}", cancelled.stdout);
    let stderr = String::from_utf8_lossy(&cancelled.stderr);
    assert!(
        stderr.contains("Would you like to update this component?"),
        "quiet suppressed the prompt: {stderr}"
    );
    assert!(stderr.contains("Cancelling update"), "missing acknowledgement: {stderr}");
}
