use clap::Parser;
use midenup::commands::Midenup;

mod common;

use common::*;

/// Integration test to check that migration works correctly:
///
/// - Updating a toolchain that an upstream channel declares `migrates_from` installs into the NEW
///   name directory and removes the OLD one.
#[test]
fn integration_channel_migration_test() {
    let _guard = common::harness::mutating_test_guard();
    let test_name = "integration_channel_migration_test";
    let test_env = environment_setup(test_name);
    let toolchain_dir = test_env.midenup_home.join("toolchains");

    // Load manifest 1 (channel "0.20.3", no migration tag)
    let manifest: &str =
        full_path_manifest!("tests/data/integration_migration_test/channel-manifest-1.json");
    let (mut local_manifest, config) = test_setup(&test_env, manifest);

    // Initialize midenup
    let command = Midenup::try_parse_from(["midenup", "init"]).unwrap();
    command
        .execute_with_state(&config, &mut local_manifest)
        .expect("failed to initialize");

    // Install stable (0.20.3)
    let command = Midenup::try_parse_from(["midenup", "install", "stable"]).unwrap();
    command
        .execute_with_state(&config, &mut local_manifest)
        .expect("failed to install stable");

    // Check that binaries are installed in the bin directory
    assert!(toolchain_dir.join("0.20.3").join("bin").join("miden-vm").exists());
    // Check that libraries are installed in the lib directory
    assert!(toolchain_dir.join("0.20.3").join("lib").join("core.masp").exists());

    // Swap to manifest 2 (channel "0.13.0" with migration from "0.20.3")
    let manifest: &str =
        full_path_manifest!("tests/data/integration_migration_test/channel-manifest-2.json");
    let (_, config) = test_setup(&test_env, manifest);

    // Perform global update
    let command = Midenup::try_parse_from(["midenup", "update"]).unwrap();
    command
        .execute_with_state(&config, &mut local_manifest)
        .expect("failed to update");

    // Check 1: Components installed in 0.13.0 directory
    assert!(toolchain_dir.join("0.13.0").exists());

    // Check 2: The 0.20.3 directory has been entirely deleted
    assert!(!toolchain_dir.join("0.20.3").exists());

    // Check 3: The stable symlink points to the new channel directory
    let stable_symlink = toolchain_dir.join("stable");
    assert!(stable_symlink.exists(), "stable symlink should exist after migration");
    let symlink_target = std::fs::read_link(&stable_symlink).expect("stable should be a symlink");
    assert_eq!(
        symlink_target.file_name(),
        toolchain_dir.join("0.13.0").file_name(),
        "stable symlink should point to the migrated channel"
    );
}

/// A migrated channel carries its intent, and its `var/` goes with it.
///
/// `var/<channel>` is the user's data -- the client's database. Migration is the sole exception to
/// "nothing touches `var/`": leaving it under a channel name that no longer exists would strand it
/// as surely as deleting it would lose it.
#[test]
fn integration_channel_migration_carries_intent_and_renames_var() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_migration_var");

    let old = semver::Version::new(0, 20, 3);
    let new = semver::Version::new(0, 13, 0);

    let manifest: &str =
        full_path_manifest!("tests/data/integration_migration_test/channel-manifest-1.json");
    let (mut state, config) = test_setup(&test_env, manifest);

    Midenup::try_parse_from(["midenup", "install", "stable", "--profile", "complete"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install stable");

    let intent_before = state.get(&old).expect("0.20.3 must be installed").intent.clone();

    // Stand in for whatever the client would have written.
    let old_var = midenup::paths::var_dir(&test_env.midenup_home, &old);
    std::fs::create_dir_all(&old_var).unwrap();
    std::fs::write(old_var.join("data"), b"client-db").unwrap();

    let manifest: &str =
        full_path_manifest!("tests/data/integration_migration_test/channel-manifest-2.json");
    let (_, config) = test_setup(&test_env, manifest);

    Midenup::try_parse_from(["midenup", "update"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to update");

    let migrated = state.get(&new).expect("the new channel must be installed");
    assert_eq!(migrated.intent, intent_before, "intent must transfer verbatim");
    assert!(state.get(&old).is_none(), "the old channel's record must be gone");

    let new_var = midenup::paths::var_dir(&test_env.midenup_home, &new);
    assert_eq!(
        std::fs::read(new_var.join("data")).unwrap(),
        b"client-db",
        "client data must follow the toolchain"
    );
    assert!(!old_var.exists(), "and must not be left behind under the old name");
}

/// Uninstalling must not depend on being able to name every file a component installed.
///
/// The publication directory contains exactly what its receipt says it does, so it is removed
/// wholesale. The previous implementation walked components to delete their files one by one, which
/// meant every shape it got wrong -- a hidden executable with no shim, two components built from
/// one Cargo package -- left files behind or panicked.
#[test]
fn integration_uninstall_removes_the_publication_wholesale() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_uninstall_wholesale");

    let fixture = common::harness::OfflineFixture::build(test_env.tmp_dir.path(), "0.15.0");
    let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);
    let channel = semver::Version::new(0, 15, 0);

    Midenup::try_parse_from(["midenup", "install", "0.15.0", "--profile", "complete"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install");

    let midenup::state::PublicationRef::Managed { id, .. } =
        &state.get(&channel).unwrap().publication
    else {
        panic!("expected a managed publication");
    };
    let publication = midenup::paths::publication_dir(&test_env.midenup_home, &channel, id);
    let receipt = midenup::publish::read_receipt(&publication).unwrap();
    assert!(!receipt.outputs.is_empty(), "the receipt must describe what was installed");

    Midenup::try_parse_from(["midenup", "uninstall", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("uninstall must not fail");

    assert!(!publication.exists(), "nothing the publication owned may survive");
    assert!(state.get(&channel).is_none());
    assert!(
        std::fs::symlink_metadata(midenup::paths::toolchain_link(&test_env.midenup_home, &channel))
            .is_err(),
        "the toolchain link must be gone, tombstone included"
    );
}
