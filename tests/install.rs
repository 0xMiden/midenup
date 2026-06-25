use std::{ffi::OsString, fs::OpenOptions};

use clap::Parser;
use midenup::{
    channel::{self, InstalledFile},
    commands::Midenup,
    miden_wrapper, utils, version,
};

mod common;

use common::*;

/// Tries to install the "stable" toolchain from the present manifest.
///
/// This differs from the test present in the .github directory which tries to install the
/// stable toolchain from published manifest.
#[test]
fn integration_install_stable() {
    let test_name = "integration_install_stable";
    let test_env = environment_setup(test_name);

    const FILE: &str = full_path_manifest!("manifest/channel-manifest.json");

    let (mut local_manifest, config) = test_setup(&test_env, FILE);

    let command = Midenup::try_parse_from(["midenup", "install", "stable"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to install stable");

    // After install is executed, the local manifest should be present
    let manifest = test_env.midenup_home.join("manifest").with_extension("json");
    assert!(manifest.exists());

    let stable_dir = test_env.midenup_home.join("toolchains").join("stable");
    assert!(stable_dir.exists());
    assert!(stable_dir.is_symlink());

    let stable_channel = local_manifest
        .get_latest_stable()
        .expect("No stable channel found; despite having installed stable");

    // We test if the in-memory representation of the local manifest contains the stable alias
    assert_eq!(stable_channel.alias, Some(channel::ChannelAlias::Stable));

    // We read the filesystem again, to check that the "stable" alias was correclty saved
    assert_eq!(
        local_manifest
            .get_channels()
            .next()
            .expect(
                "ERROR: The local_manifest in the filesystem has no alias, when it should have \
                 stable alias"
            )
            .alias
            .as_ref()
            .expect(
                "ERROR: The installed stable toolchain should be marked as stable in the local \
                 manifest"
            ),
        &channel::ChannelAlias::Stable
    );
}

/// Validates that midenup manages to install components with [Authority]s different than
/// [`version::Authority::Cargo`]. Besides installing these components, we verify that midenup
/// manages to update them when needed.
#[test]
fn integration_install_from_non_cargo() {
    let test_name = "integration_install_from_non_cargo";
    let test_env = environment_setup(test_name);

    let miden_vm_clone_path = test_env.present_working_dir.join("miden_vm");
    {
        let miden_vm_repo = "https://github.com/0xMiden/miden-vm.git";
        // Commit corresponding to release number 0.16.4 of the miden-vm
        // See https://github.com/0xMiden/miden-vm/releases/tag/v0.16.4
        let vm_release_16 = "fc368686bd1e6e171a51a1a5b365ef5400e4b8d5";
        utils::git::clone_specific_revision(miden_vm_repo, vm_release_16, &miden_vm_clone_path)
            .unwrap();
    };

    // Initial manifest with a client tracked by version::Authority::Git::Revision
    let manifest: &str = full_path_manifest!(
        "tests/data/integration_install_from_non_cargo/channel-manifest-1.json"
    );
    let (mut local_manifest, config) = test_setup(&test_env, manifest);

    // We install stable
    let command = Midenup::try_parse_from(["midenup", "install", "stable"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to install stable");

    let (time_when_installed, hash_when_installed) = {
        let stable_channel = local_manifest
            .get_latest_stable()
            .expect("No stable channel found; despite having installed stable")
            .clone();

        let vm_from_path = stable_channel.get_component("vm").unwrap();
        let last_modification = match vm_from_path.version {
            version::Authority::Path { last_modification, .. } => last_modification.unwrap(),
            _ => panic!(
                "Failed to recognize miden_vm's Authority as Path, despite being installed like \
                 so."
            ),
        };

        let client_from_git = stable_channel.get_component("client").unwrap();
        let revision = match &client_from_git.version {
            version::Authority::Git {
                target: version::GitTarget::Revision { hash },
                ..
            } => hash.clone(),
            authority => panic!(
                "Failed to recognize miden_client's Authority as Git, despite being installed \
                 like so. Found: {authority}"
            ),
        };

        (last_modification, revision)
    };

    // We call for an update. This should update the client since the revision in the manifest has
    // changed.
    let command = Midenup::try_parse_from(["midenup", "update"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to update");

    let (new_time, new_revision) = {
        let stable_channel = local_manifest
            .get_latest_stable()
            .expect("No stable channel found; despite having installed stable")
            .clone();

        let vm_from_path = stable_channel.get_component("vm").unwrap();
        let last_modification = match vm_from_path.version {
            version::Authority::Path { last_modification, .. } => last_modification.unwrap(),
            _ => panic!(
                "Failed to recognize miden_vm's Authority as Path, despite being installed like \
                 so."
            ),
        };

        let client_from_git = stable_channel.get_component("client").unwrap();
        let revision = match &client_from_git.version {
            version::Authority::Git {
                target: version::GitTarget::Revision { hash },
                ..
            } => hash.clone(),
            authority => panic!(
                "Failed to recognize miden_client's Authority as Git, despite being installed \
                 like so. Found: {authority}"
            ),
        };

        (last_modification, revision)
    };

    // These two should be equal since no updates should have been triggered.
    assert_eq!(new_time, time_when_installed);
    assert_eq!(new_revision, hash_when_installed);

    // Now, we need to check if udpates are handled properly. First, we update the manifest to
    // trigger an update for the client which is managed by git and also we create a new file on
    // the miden-vm path to trigger an update.
    let manifest: &str = full_path_manifest!(
        "tests/data/integration_install_from_non_cargo/channel-manifest-2.json"
    );
    let (_, config) = test_setup(&test_env, manifest);
    {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(miden_vm_clone_path.join("miden-vm/").join("trigger-update"))
            .unwrap();
    }

    let command = Midenup::try_parse_from(["midenup", "update", "--path-update=all"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to update");

    let (new_time, new_revision) = {
        let stable_channel = local_manifest
            .get_latest_stable()
            .expect("No stable channel found; despite having installed stable")
            .clone();

        let vm_from_path = stable_channel.get_component("vm").unwrap();
        let last_modification = match vm_from_path.version {
            version::Authority::Path { last_modification, .. } => last_modification.unwrap(),
            _ => panic!(
                "Failed to recognize miden_vm's Authority as Path, despite being installed like \
                 so."
            ),
        };

        let client_from_git = stable_channel.get_component("client").unwrap();
        let revision = match &client_from_git.version {
            version::Authority::Git {
                target: version::GitTarget::Revision { hash },
                ..
            } => hash.clone(),
            authority => panic!(
                "Failed to recognize miden_client's Authority as Git, despite being installed \
                 like so. Found: {authority}"
            ),
        };

        (last_modification, revision)
    };

    assert!(new_time > time_when_installed);
    assert_ne!(new_revision, hash_when_installed);
}

/// Validates that every component present in the stable toolchain from the published manifest
/// is able to be executed.
///
/// This relies on every component respecting the --help flag, which is an assumption we already
/// make in the miden_wrapper.rs file. This stems from the fact that the help command is
/// generated automatically.
///
/// [See here for details](https://docs.rs/clap/latest/clap/struct.Command.html#method.disable_help_flag)
#[test]
fn integration_test_components_are_runnable() {
    let test_name = "integration_test_components";
    let test_env = environment_setup(test_name);

    const FILE: &str = full_path_manifest!("manifest/channel-manifest.json");
    let (mut local_manifest, config) = test_setup(&test_env, FILE);

    // Install the latest stable toolchain
    let command = Midenup::try_parse_from(["midenup", "install", "stable"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to install stable");

    let stable_channel = local_manifest
        .get_latest_stable()
        .expect("No stable channel found after installing stable")
        .clone();

    println!("Installed: {}", stable_channel);

    // Verify each executable component is accessible and runnable
    for component in &stable_channel.components {
        let component_type = component.get_installed_file();
        // Skip libraries
        if matches!(component_type, InstalledFile::Library { .. }) {
            continue;
        }

        // Skip components not meant to be executed directly
        if matches!(component_type, InstalledFile::Executable { alias_only: true, .. }) {
            continue;
        }

        let argv: Vec<OsString> =
            vec!["miden".into(), "help".into(), component.name.as_ref().into()];

        miden_wrapper::miden_wrapper(&argv, &config, &mut local_manifest).unwrap_or_else(|err| {
            panic!(
                "Component '{}' is not runnable through the 'miden' interface: {}",
                component.name, err
            )
        });
    }
}
