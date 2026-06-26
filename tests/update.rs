use clap::Parser;
use midenup::{channel, commands::Midenup, version};

mod common;

use common::*;

/// This tests checks that midenup's update behavior works correctly
#[test]
fn integration_update_test() {
    let test_name = "integration_update_test";
    let test_env = environment_setup(test_name);
    eprintln!("KEEPING temp dir at: {}", test_env.tmp_dir.path().display());

    // This manifest contains toolchain version 0.14.0 as its only toolchain
    //
    // WARNING: This test uses toolchain files which were created for testing purposes only.
    // For instance, they are lacking many components in order to save time.
    let manifest: &str =
        full_path_manifest!("tests/data/integration_update_test/channel-manifest-1.json");
    let (mut local_manifest, config) = test_setup(&test_env, manifest);
    let toolchain_dir = test_env.midenup_home.join("toolchains");

    // We begin by initializing the midenup directory
    let command = Midenup::try_parse_from(["midenup", "init"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to initialize");

    // Now, we install stable. That is going to be version 0.14.0
    let command = Midenup::try_parse_from(["midenup", "install", "stable"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to install stable");

    // Now, we re-generate the config with a newer manifest that contains version 0.15.0. This
    // is trying to emulate the release of a new stable version
    let manifest: &str =
        full_path_manifest!("tests/data/integration_update_test/channel-manifest-2.json");
    let (_, config) = test_setup(&test_env, manifest);

    // Now, we update stable. The stable symlink should point to version 0.15.0
    let command = Midenup::try_parse_from(["midenup", "update", "stable"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to update stable");

    // The original toolchain should still exist
    let toolchain_v14 = toolchain_dir.join("0.14.0");
    assert!(toolchain_v14.exists());

    // The newer toolchain should also now be installed
    let toolchain_v15 = toolchain_dir.join("0.15.0");
    assert!(toolchain_v15.exists());

    // We check that the stable symlink still exits.
    let toolchain_stable = toolchain_dir.join("stable");
    assert!(toolchain_stable.exists());
    assert!(toolchain_stable.is_symlink());
    // The stable symlink should now point to the v0.15.0 toolchain
    let resolved_stable_toolchain = std::fs::read_link(toolchain_stable.as_path())
        .expect("Couldn't obtain directory where the stable directory is pointing to");
    assert_eq!(resolved_stable_toolchain.file_name(), toolchain_v15.file_name());

    // Now, we perform a "global" update. This performs an update on every *installed*
    // toolchain.
    //
    // The manifest file tests/data/integration_update_test/channel-manifest-3.json, besides
    // adding toolchain 0.16.0, also changed some fields on components from version 0.15.0.
    //
    // This update should perform the following changes:
    //
    // - Update 0.15.0's miden-vm to version 0.16.2.
    // - Remove base.masp from 0.15.0's toolchain dir.
    // - Downgrade 0.14.0's miden-vm.
    // - Add the miden-client to 0.14.0's toolchain dir
    // - Change 0.14.0's std's authority from Cargo to Git.
    //
    // Currently this does _not_ update the `stable` symlink
    let manifest: &str =
        full_path_manifest!("tests/data/integration_update_test/channel-manifest-3.json");
    let (_, config) = test_setup(&test_env, manifest);

    let command = Midenup::try_parse_from(["midenup", "update"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to update");

    // We check that the stable symlink still exits and is pointing to 0.15.0.
    assert!(toolchain_stable.exists());
    assert!(toolchain_stable.is_symlink());

    // The stable symlink should still point to the v0.15.0 toolchain
    let resolved_stable_toolchain = std::fs::read_link(toolchain_stable.as_path())
        .expect("Couldn't obtain directory where the stable directory is pointing to");
    assert_eq!(resolved_stable_toolchain.file_name(), toolchain_v15.file_name());

    let vm_exe_stable = toolchain_stable.join("bin").join("miden-vm");
    let command = std::process::Command::new(&vm_exe_stable)
        .arg("--version")
        .output()
        .unwrap_or_else(|err| {
            panic!("error occurred executing {}: {err}", vm_exe_stable.display())
        });
    assert_eq!(String::from_utf8(command.stdout).unwrap(), "miden-vm 0.16.2\n");
    assert!(!toolchain_v15.join("lib").join("base.masp").exists());

    let std_version = &local_manifest
        .get_channel(&channel::UserChannel::Version(semver::Version::new(0, 14, 0)))
        .expect("Couldn't find toolchain 0.14.0 in local manifest")
        .get_component("std")
        .expect("Couldn't find std library despite being listed in manifest.")
        .version;

    assert!(
        matches!(std_version, version::Authority::Git { .. }),
        "expected git authority for {std_version:#?}"
    );

    let vm_exe_v14 = toolchain_v14.join("bin").join("miden");
    let command = std::process::Command::new(vm_exe_v14).arg("--version").output().unwrap();
    assert_eq!(String::from_utf8(command.stdout).unwrap(), "Miden 0.13.0\n");
    let client_v14 = toolchain_v14.join("bin").join("miden-client");
    assert!(client_v14.exists());

    // Now, we use the same manifest that we used previously to update the current stable
    // toolchain.
    let command = Midenup::try_parse_from(["midenup", "update", "stable"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to update stable");

    let toolchain_v16 = toolchain_dir.join("0.16.0");
    assert!(toolchain_v16.exists());

    // The stable symlink should now point to the newest toolchain
    let stable_toolchain = std::fs::read_link(toolchain_stable.as_path())
        .expect("Couldn't obtain directory where the stable directory is pointing to");
    assert_eq!(stable_toolchain.file_name(), toolchain_v16.file_name());
}
