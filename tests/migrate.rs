use clap::Parser;
use midenup::commands::Midenup;

mod common;

use common::*;

/// Integration test to check that migration works correctly:
///
/// - Updating a toolchain with a migration tag installs into the NEW name directory and removes the
///   OLD directory.
#[test]
fn integration_migration_test() {
    let test_name = "integration_migration_test";
    let test_env = environment_setup(test_name);
    let tmp_home = test_env.midenup_dir;
    let midenup_home = tmp_home.join("midenup");
    let toolchain_dir = midenup_home.join("toolchains");

    // Load manifest 1 (channel "0.20.3", no migration tag)
    let manifest: &str =
        full_path_manifest!("tests/data/integration_migration_test/channel-manifest-1.json");
    let (mut local_manifest, config) = test_setup(&midenup_home, manifest);

    // Initialize midenup
    let command = Midenup::try_parse_from(["midenup", "init"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to initialize");

    // Install stable (0.20.3)
    let command = Midenup::try_parse_from(["midenup", "install", "stable"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to install stable");

    // Check that binaries are installed in the bin directory
    assert!(toolchain_dir.join("0.20.3").join("bin").join("miden-client").exists());
    // Check that libraries are installed in the lib directory
    assert!(toolchain_dir.join("0.20.3").join("lib").join("core.masp").exists());

    // Swap to manifest 2 (channel "0.13.0" with migration from "0.20.3")
    let manifest: &str =
        full_path_manifest!("tests/data/integration_migration_test/channel-manifest-2.json");
    let (_, config) = test_setup(&midenup_home, manifest);

    // Perform global update
    let command = Midenup::try_parse_from(["midenup", "update"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
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
