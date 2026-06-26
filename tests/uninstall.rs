use clap::Parser;
use midenup::commands::Midenup;

mod common;

use common::*;

/// Integration test to check that installing and uninstalling works.
///
/// Tries to install a toolchain under a [`channel::UserChannel`] (via the `stable` alias) and
/// also specific versions explicitly.
#[test]
fn integration_install_uninstall_test() {
    let test_name = "integration_install_uninstall_test";
    let test_env = environment_setup(test_name);

    const FILE: &str =
        full_path_manifest!("tests/data/integration_install_uninstall_test/channel-manifest.json");

    let (mut local_manifest, config) = test_setup(&test_env, FILE);
    let toolchain_dir = test_env.midenup_home.join("toolchains");

    // We begin by initializing the midenup directory
    let command = Midenup::try_parse_from(["midenup", "init"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to initialize");

    // We check that the basic midenup directory structure is present
    assert!(test_env.midenup_home.exists());
    assert!(toolchain_dir.exists());
    // The miden symlink should be in $CARGO_HOME/bin
    assert!(test_env.cargo_home.join("bin").join("miden").exists());

    // Now, we install stable
    let command = Midenup::try_parse_from(["midenup", "install", "stable"]).unwrap();
    // This should install version 0.16.0, since it's the latest available stable toolchain
    // present in FILE
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to install stable");

    let latest_toolchain = toolchain_dir.join("0.16.0");
    assert!(latest_toolchain.exists());

    // Besides it should create the `stable` symlink
    let stable_dir = toolchain_dir.join("stable");
    assert!(stable_dir.exists());
    assert!(stable_dir.is_symlink());

    // Stable should point to 0.16.0
    let stable_toolchain = std::fs::read_link(&stable_dir).expect("Failed to read stable symlink");
    assert_eq!(stable_toolchain.file_name(), latest_toolchain.file_name());

    // Now we install a separate toolchain.
    let command = Midenup::try_parse_from(["midenup", "install", "0.15.0"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to install 0.15.0");

    // This should install toolchain version 0.15.0.

    let older_toolchain = toolchain_dir.join("0.15.0");
    assert!(older_toolchain.exists());

    // Besides this new toolchain, all the other directories should still exists.
    assert!(stable_dir.exists());
    assert!(latest_toolchain.exists());

    let installed_toolchains = ["0.15.0", "0.16.0"].iter().map(|version| {
        semver::Version::parse(version)
            .unwrap_or_else(|_| panic!("Failed to turn {version} into semver::Version"))
    });

    // Besides creating the various directories, the local manifest should also reflect this
    // structure
    local_manifest
        .get_channels()
        .map(|channel| channel.name.clone())
        .eq(installed_toolchains);

    // Now, we'll uninstall 0.16.0.
    let command = Midenup::try_parse_from(["midenup", "uninstall", "0.16.0"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to uninstall 0.16.0");

    // Afterwards, both the 0.16.0 directory and the `stable` symlink should be deleted.
    // But, 0.15.0 should still remain
    assert!(!latest_toolchain.exists());
    assert!(!stable_dir.exists());
    assert!(older_toolchain.exists());

    // Similarly, the local manifest should now also reflect the that the older toolchain got
    // uninstalled
    let installed_toolchains = ["0.15.0"].iter().map(|version| {
        semver::Version::parse(version)
            .unwrap_or_else(|_| panic!("Failed to turn {version} into semver::Version"))
    });

    // Besides creating the various directories, the local manifest should also reflect this
    // structure
    local_manifest
        .get_channels()
        .map(|channel| channel.name.clone())
        .eq(installed_toolchains);
}
