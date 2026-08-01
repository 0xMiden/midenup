use clap::Parser;
use midenup::commands::Midenup;

mod common;

use common::*;

/// Installing a channel that several networks name writes a symlink for each of them.
///
/// This is the state right after a testnet toolchain is promoted to mainnet, and it is what the
/// per-channel alias could not express: with one alias per channel, only one of these names could
/// have existed.
#[test]
fn integration_networks_one_install_writes_every_naming_link() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_networks_shared");
    let fixture = common::harness::OfflineFixture::build(test_env.tmp_dir.path(), "0.15.0");
    let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);

    Midenup::try_parse_from(["midenup", "init"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to initialize");

    // Spelled `stable` deliberately: the synonym reaching the network it names is part of what is
    // under test. The fixture's mainnet is its only channel, so this reaches the same place either
    // way.
    Midenup::try_parse_from(["midenup", "install", "stable"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install");

    let toolchains = test_env.midenup_home.join("toolchains");
    for network in ["mainnet", "testnet", "devnet"] {
        assert_eq!(
            std::fs::read_link(toolchains.join(network))
                .unwrap_or_else(|err| panic!("the {network} link must exist: {err}")),
            std::path::PathBuf::from("0.15.0"),
            "{network} must name the installed channel"
        );
    }
}

/// A synonym reaches the same channel as the network it names, and produces the network's link.
#[test]
fn integration_networks_stable_still_installs_mainnet() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_networks_synonym");
    let fixture = common::harness::OfflineFixture::build(test_env.tmp_dir.path(), "0.15.0");
    let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);

    Midenup::try_parse_from(["midenup", "init"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to initialize");

    Midenup::try_parse_from(["midenup", "install", "stable"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install stable");

    let toolchains = test_env.midenup_home.join("toolchains");
    assert!(
        toolchains.join("mainnet").is_symlink(),
        "installing 'stable' must produce the mainnet link, not one named stable"
    );
    assert!(
        toolchains.join("stable").symlink_metadata().is_err(),
        "no link named stable may be written"
    );
}

/// A rollback to a channel the user still has installed.
///
/// The pointer moving is the whole operation here: the target is already installed with the same
/// intent, so there is nothing to install, and an update that only installs would leave the link
/// naming the newer channel while `var/` had already been carried back.
#[test]
fn integration_networks_update_follows_a_rollback_to_an_installed_channel() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_networks_rollback_installed");
    let fixture = common::harness::UpdateFixture::build(test_env.tmp_dir.path());

    let (mut state, config) = test_setup(&test_env, &fixture.initial());
    for args in [vec!["midenup", "init"], vec!["midenup", "install", "mainnet"]] {
        Midenup::try_parse_from(args.clone())
            .unwrap()
            .execute_with_state(&config, &mut state)
            .unwrap_or_else(|err| panic!("{args:?} failed: {err:#}"));
    }

    let var = test_env.midenup_home.join("var").join("0.14.0");
    std::fs::create_dir_all(&var).unwrap();
    std::fs::write(var.join("store.sqlite3"), b"client data").unwrap();

    // Promoted to 0.15.0 -- installs it, and keeps 0.14.0.
    let (_, config) = test_setup(&test_env, &fixture.with_new_stable());
    Midenup::try_parse_from(["midenup", "update", "mainnet"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to follow the promotion");

    // Rolled back. 0.14.0 is still installed with the same intent, so there is nothing to install.
    let (_, config) = test_setup(&test_env, &fixture.initial());
    Midenup::try_parse_from(["midenup", "update", "mainnet"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to follow the rollback");

    assert_eq!(
        std::fs::read_link(test_env.midenup_home.join("toolchains").join("mainnet")).unwrap(),
        std::path::PathBuf::from("0.14.0"),
        "the pointer must move even when there is nothing to install"
    );
    assert_eq!(
        std::fs::read(test_env.midenup_home.join("var").join("0.14.0").join("store.sqlite3"))
            .unwrap(),
        b"client data",
        "client data must be where the active channel will look for it"
    );
}
