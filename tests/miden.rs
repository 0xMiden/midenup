use clap::Parser;
use midenup::commands::Midenup;

mod common;

use common::*;

/// Checks that the `miden` utility is able to recognize when the currently active toolchain is
/// not installed, and then installing it before executing the passed in command.
#[test]
fn integration_miden_test() {
    let test_name = "integration_miden_test";
    let test_env = environment_setup(test_name);

    // SIDENOTE: This tests uses a toolchain with version number 0.14.0. This
    // is simply used for testing purposes and is not a "real" toolchain.
    const FILE: &str =
        full_path_manifest!("tests/data/integration_miden_test/channel-manifest.json");

    let (mut local_manifest, config) = test_setup(&test_env, FILE);
    let toolchain_dir = test_env.midenup_home.join("toolchains");

    // By default, the active toolchain is the latest stable version. In the
    // case of the manifest present in FILE, that is version 0.16.0.
    let command = Midenup::try_parse_from(["miden", "client", "--version"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to get client version");

    // After this, `midenup` should:
    // 1. Recognize that the user wants to run a component
    // 2. Recognize that the active toolchain is not installed, and thus trigger an installation
    // 3. Before issuing the install, it should recognize that midenup hasn't been initialized and
    //    thus needs to be initialized.

    // midenup initialized check
    assert!(test_env.midenup_home.exists());
    assert!(toolchain_dir.exists());
    // The miden symlink should be in $CARGO_HOME/bin
    assert!(test_env.cargo_home.join("bin").join("miden").exists());

    // Stable toolchain installed check
    let latest_toolchain = toolchain_dir.join("0.16.0");
    assert!(latest_toolchain.exists());

    // Symlink check
    let stable_dir = toolchain_dir.join("stable");
    assert!(stable_dir.exists());
    assert!(stable_dir.is_symlink());

    // Global default

    // Now, we set a global default toolchain. This should change the current active toolchain
    // to 0.15.0.
    let command = Midenup::try_parse_from(["midenup", "override", "0.15.0"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to override toolchain");

    // This should also trigger an install, since toolchain 0.15.0 is missing and is now the
    // active toolchain.
    let command = Midenup::try_parse_from(["miden", "client", "--version"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to get client version");

    let older_toolchain = toolchain_dir.join("0.15.0");
    assert!(older_toolchain.exists());

    // Directory only toolchain
    //
    // Now, we'll create a `miden-toolchain.toml` file. This will change the current active
    // toolchain. By default, the active toolchain is the latest stable version. In the case of
    // the manifest present in FILE, that is version 0.16.0.
    let command = Midenup::try_parse_from(["midenup", "set", "0.14.0"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to set local toolchain");

    // This should also trigger an install, since toolchain 0.14.0 is now missing
    let command = Midenup::try_parse_from(["miden", "client", "--version"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to get client version");

    let oldest_toolchain = toolchain_dir.join("0.14.0");
    assert!(oldest_toolchain.exists());

    // Afterwards, all of the newly installed toolchains should be present in the local
    // manifest.
    let installed_toolchains = ["0.14.0", "0.15.0", "0.16.0"].iter().map(|version| {
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

/// Checks that the `miden` utility recognizes the existence of a `miden-toolchain.toml` file.
///
/// This file contains the required toolchain for the current project, along with a list of
/// required components. `miden` should be able to:
///
/// - create said file
/// - recognize the list of required components and install them
/// - recognize if the list gets expanded and install the missing components
#[test]
fn integration_miden_toolchain_toml() {
    let test_name = "integration_miden_toolchain_toml";
    let test_env = environment_setup(test_name);

    let pwd = &test_env.present_working_dir;

    const FILE: &str =
        full_path_manifest!("tests/data/integration_miden_toolchain_toml/channel-manifest.json");

    let (mut local_manifest, config) = test_setup(&test_env, FILE);

    // This should create a miden-toolchain.toml file that sets toolchain 0.16.0 as the active
    // one on the current project. Since the toolchain is not installed, the component list is
    // left empty.
    let command = Midenup::try_parse_from(["midenup", "set", "0.16.0"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to set local toolchain");

    // There should now be a `miden-toolchain.toml` file in the PWD.
    let miden_toolchain_file = pwd.join("miden-toolchain.toml");
    assert!(miden_toolchain_file.exists());

    // Now, we update the file to include the vm
    let toolchain_with_components =
        full_path!("tests/data/integration_miden_toolchain_toml/miden-toolchain-1.toml");
    std::fs::copy(toolchain_with_components, &miden_toolchain_file).unwrap();

    // `miden` should now install the components listed in the toolchain file.
    let command = Midenup::try_parse_from(["miden", "help", "toolchain"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to get subcommand help");

    let toolchain_dir = test_env.midenup_home.join("toolchains");
    assert!(toolchain_dir.exists());

    let installed_channel =
        local_manifest.get_channel_by_name(&semver::Version::new(0, 16, 0)).unwrap();
    assert!(installed_channel.components.len() == 1);

    // Now, we'll add the miden compiler to the list.
    let toolchain_with_components =
        full_path!("tests/data/integration_miden_toolchain_toml/miden-toolchain-2.toml");
    std::fs::copy(toolchain_with_components, miden_toolchain_file).unwrap();

    // `miden` should now install:
    // - The compiler which was just added
    // - Both the standard library and transaction kernel libraris since they are a dependency for
    //   the compiler.
    let command = Midenup::try_parse_from(["miden", "help", "toolchain"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to get subcommand help");

    let installed_channel = local_manifest
        .get_channel_by_name(&semver::Version::new(0, 16, 0))
        .unwrap()
        .clone();

    // VM, Compiler and both libraries
    assert_eq!(installed_channel.components.len(), 4);

    // Now, we try updating the installed toolchain. This should only update the installed
    // components and ignore the rest.
    let command = Midenup::try_parse_from(["midenup", "update", "stable"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to update stable toolchain");

    // No components should have been added
    assert_eq!(installed_channel.components.len(), 4);

    // Finally, we attempt to install the entire stable toolchain, which should install the
    // remaining components.
    let command = Midenup::try_parse_from(["midenup", "install", "stable"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to install stable toolchain");

    // Now, the entire toolchain should be installed
    let installed_channel =
        local_manifest.get_channel_by_name(&semver::Version::new(0, 16, 0)).unwrap();
    assert_eq!(installed_channel.components.len(), 6);
}

/// This 'midenc' component present in this manifest is lacking its required 'rustup_channel"
/// and thus installation should fail.
#[test]
#[should_panic]
fn integration_midenup_catches_installation_failure() {
    let test_name = "midenup_catches_installation_failure";
    let test_env = environment_setup(test_name);

    const FILE_PRE_UPDATE: &str = full_path_manifest!(
        "tests/data/unit_test_manifest_additional/manifest-uncompilable-midenc.json"
    );

    let (mut local_manifest, config) = test_setup(&test_env, FILE_PRE_UPDATE);

    let command = Midenup::try_parse_from(["midenup", "install", "stable"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to install stable");
    // After install is executed, the local manifest should be present
    let manifest = test_env.midenup_home.join("manifest").with_extension("json");
    assert!(manifest.exists());
}
