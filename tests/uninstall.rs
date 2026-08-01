use clap::Parser;
use midenup::commands::Midenup;

mod common;

use common::*;

/// Uninstalling a toolchain must not delete the user's data with it.
///
/// `var/<channel>` holds mutable component-owned state -- the client's database, reached as
/// `%var(data)`. Removing a toolchain is not a request to delete it, so it is kept unless `--purge`
/// says otherwise.
#[test]
fn integration_uninstall_keeps_var_unless_purge_is_given() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_uninstall_var");

    let fixture = common::harness::OfflineFixture::build(test_env.tmp_dir.path(), "0.15.0");
    let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);
    let channel = semver::Version::new(0, 15, 0);

    Midenup::try_parse_from(["midenup", "install", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install");

    // Stand in for whatever the client would have written.
    let data = midenup::paths::var_dir(&test_env.midenup_home, &channel).join("data");
    std::fs::create_dir_all(data.parent().unwrap()).unwrap();
    std::fs::write(&data, b"user-database").unwrap();

    Midenup::try_parse_from(["midenup", "uninstall", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to uninstall");
    assert!(data.exists(), "var must be retained without --purge");

    // Reinstall so there is something to uninstall again.
    Midenup::try_parse_from(["midenup", "install", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to reinstall");
    assert_eq!(
        std::fs::read(&data).unwrap(),
        b"user-database",
        "reinstalling must find the data still there"
    );

    Midenup::try_parse_from(["midenup", "uninstall", "0.15.0", "--purge"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to purge");
    assert!(!data.exists(), "--purge must remove var");
}

/// Integration test to check that installing and uninstalling works.
///
/// Tries to install a toolchain under a [`channel::UserChannel`] (via the `stable` alias) and
/// also specific versions explicitly.
#[test]
fn integration_install_uninstall_test() {
    let _guard = common::harness::mutating_test_guard();
    let test_name = "integration_install_uninstall_test";
    let test_env = environment_setup(test_name);

    const FILE: &str =
        full_path_manifest!("tests/data/integration_install_uninstall_test/channel-manifest.json");

    let (mut local_manifest, config) = test_setup(&test_env, FILE);
    let toolchain_dir = test_env.midenup_home.join("toolchains");

    // We begin by initializing the midenup directory
    let command = Midenup::try_parse_from(["midenup", "init"]).unwrap();
    command
        .execute_with_state(&config, &mut local_manifest)
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
        .execute_with_state(&config, &mut local_manifest)
        .expect("failed to install stable");

    let latest_toolchain = toolchain_dir.join("0.16.0");
    assert!(latest_toolchain.exists());

    // Besides it should create the `mainnet` link
    let mainnet_dir = toolchain_dir.join("mainnet");
    assert!(mainnet_dir.exists());
    assert!(mainnet_dir.is_symlink());

    // mainnet should point to 0.16.0
    let mainnet_toolchain =
        std::fs::read_link(&mainnet_dir).expect("Failed to read the mainnet link");
    assert_eq!(mainnet_toolchain.file_name(), latest_toolchain.file_name());

    // Now we install a separate toolchain.
    let command = Midenup::try_parse_from(["midenup", "install", "0.15.0"]).unwrap();
    command
        .execute_with_state(&config, &mut local_manifest)
        .expect("failed to install 0.15.0");

    // This should install toolchain version 0.15.0.
    let older_toolchain = toolchain_dir.join("0.15.0");
    assert!(older_toolchain.exists());

    // Besides this new toolchain, all the other directories should still exists.
    assert!(mainnet_dir.exists());
    assert!(latest_toolchain.exists());

    let installed_toolchains = ["0.15.0", "0.16.0"].iter().map(|version| {
        semver::Version::parse(version)
            .unwrap_or_else(|_| panic!("Failed to turn {version} into semver::Version"))
    });

    // Besides creating the various directories, the local manifest should also reflect this
    // structure
    local_manifest
        .installations
        .iter()
        .map(|i| i.as_channel())
        .map(|channel| channel.name.clone())
        .eq(installed_toolchains);

    // Now, we'll uninstall 0.16.0.
    let command = Midenup::try_parse_from(["midenup", "uninstall", "0.16.0"]).unwrap();
    command
        .execute_with_state(&config, &mut local_manifest)
        .expect("failed to uninstall 0.16.0");

    // Afterwards, both the 0.16.0 directory and the `mainnet` link should be deleted.
    // But, 0.15.0 should still remain
    assert!(!latest_toolchain.exists());
    assert!(std::fs::symlink_metadata(&mainnet_dir).is_err());
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
        .installations
        .iter()
        .map(|i| i.as_channel())
        .map(|channel| channel.name.clone())
        .eq(installed_toolchains);
}
