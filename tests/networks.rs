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

    // Spelled `stable` because `UserChannel::Named` does not exist yet. The fixture's mainnet
    // is its only channel, so this reaches the same place either way.
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

    assert!(
        test_env.midenup_home.join("toolchains").join("mainnet").is_symlink(),
        "installing 'stable' must produce the mainnet link, not one named stable"
    );
}
