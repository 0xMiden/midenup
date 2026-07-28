use std::{ffi::OsString, fs::OpenOptions};

use clap::Parser;
use midenup::{commands::Midenup, manifest::ComponentKind, miden_wrapper, utils, version};

mod common;

use common::*;

/// Tries to install the "stable" toolchain from the present manifest.
///
/// This differs from the test present in the .github directory which tries to install the
/// stable toolchain from published manifest.
#[test]
fn integration_install_stable() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_install_stable");

    // Offline fixture: this test asserts on recorded state and symlink layout, none of which
    // needs a real toolchain. See `OfflineFixture` for why that matters.
    let fixture = common::harness::OfflineFixture::build(test_env.tmp_dir.path(), "0.15.0");
    let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);

    Midenup::try_parse_from(["midenup", "install", "stable"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install stable");

    let state_file = test_env.midenup_home.join("state").with_extension("json");
    assert!(state_file.exists(), "install must write local state");

    let stable_dir = test_env.midenup_home.join("toolchains").join("stable");
    assert!(stable_dir.exists());
    assert!(stable_dir.is_symlink());

    // `stable` is not persisted in local state -- it is a property of the upstream manifest and a
    // derived symlink on disk. Assert on the symlink, and that state records the version it names.
    let stable_version = config
        .manifest
        .get_latest_stable()
        .expect("upstream must declare a stable channel")
        .name
        .clone();
    assert_eq!(
        std::fs::read_link(&stable_dir).unwrap().file_name().unwrap(),
        std::ffi::OsStr::new(&stable_version.to_string()),
        "the stable symlink must point at the upstream stable channel"
    );

    // Re-read from disk to confirm it was persisted, not merely held in memory.
    let reloaded = midenup::state::LocalState::load(&state_file).expect("state must reload");
    assert!(reloaded.get(&stable_version).is_some());
}

/// A fresh install must actually place every artifact kind where it belongs.
///
/// Regression: the existence check tested the pre-created `lib/` directory rather than the
/// artifact file, so every package download was skipped and reported "already installed" on a
/// completely fresh install.
#[test]
fn integration_install_places_every_artifact_kind() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_install_places_every_artifact_kind");

    let fixture = common::harness::OfflineFixture::build(test_env.tmp_dir.path(), "0.15.0");
    let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);

    Midenup::try_parse_from(["midenup", "install", "stable", "--profile", "complete"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install stable");

    let root = test_env.midenup_home.join("toolchains").join("stable");

    assert!(
        root.join("bin").join("miden-vm").exists(),
        "executable -> bin/<installed-executable>"
    );
    assert!(root.join("lib").join("core.masp").exists(), "package -> lib/<artifact-id>");
    assert!(
        root.join("etc").join("assets").join("config.yml").exists(),
        "asset -> etc/<component>/<artifact-id>"
    );

    // The package must be the real content, not a zero-length placeholder left by a partial write.
    assert_eq!(std::fs::read(root.join("lib").join("core.masp")).unwrap(), b"fixture-package");
}

/// Executable components must get their `opt/` shims.
///
/// `opt/` serves two purposes: the clap `argv[0]` trick, so help renders as `miden vm ...`; and
/// PATH discoverability, since `opt/` is the only toolchain directory on `PATH`.
///
/// Regression: shims were emitted only when `symlink-name` was set explicitly, so a stable install
/// produced one shim -- for the single hidden component that declared one -- and none for the
/// callable components.
#[test]
fn integration_install_creates_default_symlinks() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_install_creates_default_symlinks");

    let fixture = common::harness::OfflineFixture::build(test_env.tmp_dir.path(), "0.15.0");
    let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);

    Midenup::try_parse_from(["midenup", "install", "stable"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install stable");

    let opt = test_env.midenup_home.join("toolchains").join("stable").join("opt");
    assert!(
        opt.join("miden vm").symlink_metadata().is_ok(),
        "missing default shim for 'vm' in {}",
        opt.display()
    );
}

/// Validates that midenup manages to install components with [Authority]s different than
/// [`version::Authority::Cargo`]. Besides installing these components, we verify that midenup
/// manages to update them when needed.
#[test]
fn integration_install_from_non_cargo() {
    let _guard = common::harness::mutating_test_guard();
    let test_name = "integration_install_from_non_cargo";
    let test_env = environment_setup(test_name);

    let miden_vm_clone_path = test_env.present_working_dir.join("miden-vm");
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
        .execute_with_state(&config, &mut local_manifest)
        .expect("failed to install stable");

    let (time_when_installed, hash_when_installed) = {
        let stable_channel = local_manifest
            .latest_stable()
            .expect("No stable channel found; despite having installed stable")
            .as_channel();

        let vm_from_path = stable_channel.get_component("vm").unwrap();
        let last_modification = match vm_from_path.version {
            version::Authority::Path { last_modification, .. } => last_modification.unwrap(),
            _ => panic!(
                "Failed to recognize miden-vm's Authority as Path, despite being installed like \
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
        .execute_with_state(&config, &mut local_manifest)
        .expect("failed to update");

    let (new_time, new_revision) = {
        let stable_channel = local_manifest
            .latest_stable()
            .expect("No stable channel found; despite having installed stable")
            .as_channel();

        let vm_from_path = stable_channel.get_component("vm").unwrap();
        let last_modification = match vm_from_path.version {
            version::Authority::Path { last_modification, .. } => last_modification.unwrap(),
            _ => panic!(
                "Failed to recognize miden-vm's Authority as Path, despite being installed like \
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
            .open(miden_vm_clone_path.join("miden-vm").join("trigger-update"))
            .unwrap();
    }

    let command = Midenup::try_parse_from(["midenup", "update", "--path-update=all"]).unwrap();
    command
        .execute_with_state(&config, &mut local_manifest)
        .expect("failed to update");

    let (new_time, new_revision) = {
        let stable_channel = local_manifest
            .latest_stable()
            .expect("No stable channel found; despite having installed stable")
            .as_channel();

        let vm_from_path = stable_channel.get_component("vm").unwrap();
        let last_modification = match vm_from_path.version {
            version::Authority::Path { last_modification, .. } => last_modification.unwrap(),
            _ => panic!(
                "Failed to recognize miden-vm's Authority as Path, despite being installed like \
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

/// Validates that every component in the stable toolchain from the real manifest is executable.
///
/// This relies on every component respecting the `--help` flag, an assumption `miden_wrapper`
/// already makes because clap generates help automatically.
///
/// Deliberately kept on the real manifest and a real install: it is the one test that proves the
/// whole pipeline produces binaries that actually run. Everything asserting only on layout or
/// recorded state uses the offline fixture instead.
///
/// [See here for details](https://docs.rs/clap/latest/clap/struct.Command.html#method.disable_help_flag)
#[test]
fn integration_test_components_are_runnable() {
    let _guard = common::harness::mutating_test_guard();
    let test_name = "integration_test_components";
    let test_env = environment_setup(test_name);

    const FILE: &str = full_path_manifest!("manifest/channel-manifest.json");
    let (mut local_manifest, config) = test_setup(&test_env, FILE);

    // Install the latest stable toolchain
    let command =
        Midenup::try_parse_from(["midenup", "install", "stable", "--profile", "complete"]).unwrap();
    command
        .execute_with_state(&config, &mut local_manifest)
        .expect("failed to install stable");

    let stable_channel = local_manifest
        .latest_stable()
        .expect("No stable channel found after installing stable")
        .as_channel();

    println!("Installed: {}", stable_channel);

    // Verify each executable component is accessible and runnable
    for component in &stable_channel.components {
        match component.kind() {
            ComponentKind::Executable { installation_method, spec }
            | ComponentKind::CargoExtension { installation_method, spec }
                if !spec.is_hidden() =>
            {
                let argv: Vec<OsString> =
                    vec!["miden".into(), "help".into(), component.name.as_ref().into()];

                miden_wrapper::miden_wrapper(&argv, &config, &mut local_manifest).unwrap_or_else(
                    |err| {
                        panic!(
                            "Component '{}' is not runnable through the 'miden' interface: {}",
                            component.name, err
                        )
                    },
                );
            },
            // Skip executables that aren't meant to be executed directly
            ComponentKind::Executable { .. } | ComponentKind::CargoExtension { .. } => (),
            // Skip non-executable components, or command aliases
            ComponentKind::Asset
            | ComponentKind::Command { .. }
            | ComponentKind::Package
            | ComponentKind::LegacyPackage { .. } => (),
            // The checked-in manifest declares no unknown kinds; if one appears, the manifest and
            // this build have diverged and the test should say so rather than skipping quietly.
            ComponentKind::Unsupported { tag, .. } => {
                panic!("component '{}' has unsupported kind '{tag}'", component.name)
            },
        }
    }
}
