use std::{ffi::OsString, fs::OpenOptions};

use clap::Parser;
use midenup::{commands::Midenup, manifest::ComponentKind, miden_wrapper, version};

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

/// `path` and `git` authorities must be recorded at install time and re-checked on update.
///
/// The behaviour under test is update *detection*: midenup records a path's modification time and
/// a git revision, then reinstalls when either changes. The sources are trivial local crates
/// rather than real components -- cloning a component repository and building it proves nothing
/// extra here and costs minutes.
#[test]
fn integration_install_from_non_cargo() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_install_from_non_cargo");

    let fixture = common::harness::SourceFixture::build(test_env.tmp_dir.path());
    let manifest_dir = test_env.tmp_dir.path();

    // Reads the recorded path mtime and git revision out of local state.
    let recorded = |state: &LocalManifest| {
        let channel = state
            .latest_stable()
            .expect("no stable channel found; despite having installed stable")
            .as_channel();

        let last_modification = match channel.get_component("vm").unwrap().version {
            version::Authority::Path { last_modification, .. } => last_modification
                .expect("a path authority must record the tree's modification time"),
            ref authority => panic!("expected 'vm' to have a path authority, got {authority}"),
        };

        let revision = match &channel.get_component("client").unwrap().version {
            version::Authority::Git {
                target: version::GitTarget::Revision { hash },
                ..
            } => hash.clone(),
            authority => panic!("expected 'client' to have a git authority, got {authority}"),
        };

        (last_modification, revision)
    };

    let first = common::harness::write_source_manifest(
        manifest_dir,
        "manifest-1.json",
        &fixture,
        &fixture.revisions[0],
    );
    let (mut state, config) = test_setup(&test_env, &first);

    Midenup::try_parse_from(["midenup", "install", "stable"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install stable");

    let (time_when_installed, hash_when_installed) = recorded(&state);
    assert_eq!(
        hash_when_installed, fixture.revisions[0],
        "the installed revision must be the one the manifest named"
    );

    // Nothing has changed, so an update must be a no-op for both authorities.
    Midenup::try_parse_from(["midenup", "update"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to update");

    let (unchanged_time, unchanged_revision) = recorded(&state);
    assert_eq!(unchanged_time, time_when_installed, "an unchanged path must not be reinstalled");
    assert_eq!(
        unchanged_revision, hash_when_installed,
        "an unchanged revision must not be reinstalled"
    );

    // Now change both: point the manifest at the second commit, and touch the path source.
    let second = common::harness::write_source_manifest(
        manifest_dir,
        "manifest-2.json",
        &fixture,
        &fixture.revisions[1],
    );
    let (_, config) = test_setup(&test_env, &second);
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(fixture.path_crate.join("trigger-update"))
        .unwrap();

    Midenup::try_parse_from(["midenup", "update", "--path-update=all"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to update");

    let (new_time, new_revision) = recorded(&state);
    assert!(new_time > time_when_installed, "a touched path source must be reinstalled");
    assert_eq!(
        new_revision, fixture.revisions[1],
        "a changed revision must be reinstalled at the new commit"
    );
}

/// Pre-release check: every component in the real stable toolchain is actually executable.
///
/// This relies on every component respecting the `--help` flag, an assumption `miden_wrapper`
/// already makes because clap generates help automatically.
///
/// Deliberately kept on the real manifest and a real install -- it is the one test that proves the
/// whole pipeline produces binaries that run, which means it downloads and builds real components
/// and takes minutes. Everything asserting only on layout or recorded state uses the offline
/// fixture instead.
///
/// The `prerelease` marker in the name excludes it from `make integration-test`; run it with
/// `make prerelease-test`. See the Makefile.
///
/// [See here for details](https://docs.rs/clap/latest/clap/struct.Command.html#method.disable_help_flag)
#[test]
fn integration_prerelease_components_are_runnable() {
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
