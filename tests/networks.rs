use clap::Parser;
use midenup::{channel::UserChannel, commands::Midenup, version};

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

    // The assertions at the bottom describe the state this test started in, so a regression making
    // the update a no-op would leave them true. Pin the premise before relying on it.
    assert_eq!(
        std::fs::read_link(test_env.midenup_home.join("toolchains").join("mainnet")).unwrap(),
        std::path::PathBuf::from("0.15.0"),
        "the promotion must have happened, or the rollback below proves nothing"
    );
    assert!(
        test_env.midenup_home.join("var").join("0.15.0").join("store.sqlite3").exists(),
        "and var/ must have been carried to it"
    );

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

/// A promotion moves mainnet to a channel the user does not have. Following it is an update of the
/// network, so the installation is carried across: intent verbatim, and var/ renamed so client data
/// follows the toolchain.
#[test]
fn integration_networks_update_follows_a_promotion() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_networks_promotion");
    let fixture = common::harness::UpdateFixture::build(test_env.tmp_dir.path());

    let (mut state, config) = test_setup(&test_env, &fixture.initial());
    Midenup::try_parse_from(["midenup", "init"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to initialize");
    // Deliberately not the default profile: intent transferring verbatim and intent being
    // discarded produce the same record for a default install, so only a non-default one can tell
    // them apart.
    Midenup::try_parse_from(["midenup", "install", "mainnet", "--profile", "complete"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install mainnet");

    let intent_before = state.get(&semver::Version::new(0, 14, 0)).unwrap().intent.clone();

    // Something the toolchain owns, which must survive the move.
    let var = test_env.midenup_home.join("var").join("0.14.0");
    std::fs::create_dir_all(&var).unwrap();
    std::fs::write(var.join("store.sqlite3"), b"client data").unwrap();

    // mainnet is promoted to 0.15.0.
    let (_, config) = test_setup(&test_env, &fixture.with_new_stable());
    Midenup::try_parse_from(["midenup", "update", "mainnet"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to update mainnet");

    assert_eq!(
        std::fs::read_link(test_env.midenup_home.join("toolchains").join("mainnet")).unwrap(),
        std::path::PathBuf::from("0.15.0"),
        "mainnet must name what the manifest now says"
    );
    assert_eq!(
        std::fs::read(test_env.midenup_home.join("var").join("0.15.0").join("store.sqlite3"))
            .unwrap(),
        b"client data",
        "client data must follow the toolchain"
    );
    assert!(
        test_env.midenup_home.join("toolchains").join("0.15.0").exists(),
        "the promoted channel must actually be installed"
    );
    assert_eq!(
        state.get(&semver::Version::new(0, 15, 0)).unwrap().intent,
        intent_before,
        "intent must transfer verbatim to the channel the network now names"
    );
    assert!(
        test_env.midenup_home.join("toolchains").join("0.14.0").exists(),
        "the previous toolchain is a usable pinned toolchain and must be retained"
    );
}

/// The pointer is authoritative in both directions. A rollback is rare and `promote` refuses to
/// author one without a flag, but once published, following it is what tracking a network means.
///
/// **This test reaches 0.15.0 directly, so 0.14.0 is never installed and the update always has work
/// to do.** That is deliberately the easy half. The hard half -- rolling back to a channel the user
/// still has, where there is nothing to install and the pointer move is the entire operation -- is
/// covered by `integration_networks_update_follows_a_rollback_to_an_installed_channel`.
#[test]
fn integration_networks_update_follows_a_rollback() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_networks_rollback");
    let fixture = common::harness::UpdateFixture::build(test_env.tmp_dir.path());

    let (mut state, config) = test_setup(&test_env, &fixture.with_new_stable());
    Midenup::try_parse_from(["midenup", "init"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to initialize");
    Midenup::try_parse_from(["midenup", "install", "mainnet"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install mainnet");

    // mainnet is rolled back to 0.14.0.
    let (_, config) = test_setup(&test_env, &fixture.initial());
    Midenup::try_parse_from(["midenup", "update", "mainnet"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("following a rollback must succeed");

    assert_eq!(
        std::fs::read_link(test_env.midenup_home.join("toolchains").join("mainnet")).unwrap(),
        std::path::PathBuf::from("0.14.0"),
        "mainnet must follow the pointer backwards"
    );
}

/// Updating one network must not disturb another that names a different channel, and must still
/// update the channel it does name.
///
/// The `update devnet` calls are the point of the test: without them this asserts only what DERIVE
/// does, which is already covered elsewhere. The second one runs against a manifest where no
/// pointer has moved but devnet's channel has changed underneath it -- the case where following the
/// pointer is a no-op and yet there is work to do. Two unmoved symlinks cannot tell that apart from
/// doing nothing at all; a component that moved can.
#[test]
fn integration_networks_update_leaves_other_networks_alone() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_networks_split");
    let fixture = common::harness::UpdateFixture::build(test_env.tmp_dir.path());

    let (mut state, config) = test_setup(&test_env, &fixture.with_split_networks());
    for args in [
        vec!["midenup", "init"],
        vec!["midenup", "install", "mainnet"],
        vec!["midenup", "install", "devnet"],
        vec!["midenup", "update", "devnet"],
    ] {
        Midenup::try_parse_from(args.clone())
            .unwrap()
            .execute_with_state(&config, &mut state)
            .unwrap_or_else(|err| panic!("{args:?} failed: {err:#}"));
    }

    // devnet still names 0.15.0, but 0.15.0's vm has been bumped upstream.
    let (_, config) = test_setup(&test_env, &fixture.with_split_networks_and_a_bumped_component());
    Midenup::try_parse_from(["midenup", "update", "devnet"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to update devnet");

    let toolchains = test_env.midenup_home.join("toolchains");
    assert_eq!(
        std::fs::read_link(toolchains.join("mainnet")).unwrap(),
        std::path::PathBuf::from("0.14.0"),
        "updating devnet must leave mainnet where it was"
    );
    assert_eq!(
        std::fs::read_link(toolchains.join("devnet")).unwrap(),
        std::path::PathBuf::from("0.15.0")
    );

    let vm_authority = &state
        .get(&semver::Version::new(0, 15, 0))
        .expect("devnet's channel must be installed")
        .components
        .iter()
        .find(|component| component.name == "vm")
        .expect("vm must be part of 0.15.0")
        .version;
    assert!(
        matches!(
            vm_authority,
            version::Authority::Registry { version } if *version == semver::Version::new(0, 23, 4)
        ),
        "a pointer that has not moved still has to pick up the channel's own changes, got \
         {vm_authority:#?}"
    );
}

#[test]
fn integration_networks_update_of_an_uninstalled_network_says_so() {
    let test_env = environment_setup("integration_networks_uninstalled");
    let fixture = common::harness::OfflineFixture::build(test_env.tmp_dir.path(), "0.15.0");
    let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);

    let err = Midenup::try_parse_from(["midenup", "update", "testnet"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect_err("updating something that is not installed must fail");

    let rendered = format!("{err:#}");
    assert!(
        rendered.contains("midenup install testnet"),
        "must say how to fix it: {rendered}"
    );
}

#[test]
fn integration_networks_update_of_an_unknown_network_lists_the_known_ones() {
    let test_env = environment_setup("integration_networks_unknown");
    let fixture = common::harness::OfflineFixture::build(test_env.tmp_dir.path(), "0.15.0");
    let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);

    let err = Midenup::try_parse_from(["midenup", "update", "mainet"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect_err("an unknown network must fail");

    let rendered = format!("{err:#}");
    assert!(rendered.contains("mainnet"), "must list what is declared: {rendered}");
}

#[test]
fn integration_networks_install_of_an_unknown_network_lists_the_known_ones() {
    let test_env = environment_setup("integration_networks_install_unknown_name");
    let fixture = common::harness::OfflineFixture::build(test_env.tmp_dir.path(), "0.15.0");
    let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);

    let err = Midenup::try_parse_from(["midenup", "install", "mainet"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect_err("an unknown network must fail");

    let rendered = format!("{err:#}");
    assert!(rendered.contains("mainnet"), "must list what is declared: {rendered}");
}

#[test]
fn integration_networks_install_of_an_unknown_version_names_that_version() {
    let test_env = environment_setup("integration_networks_install_unknown_version");
    let fixture = common::harness::OfflineFixture::build(test_env.tmp_dir.path(), "0.15.0");
    let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);

    let err = Midenup::try_parse_from(["midenup", "install", "9.9.9"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect_err("an unknown toolchain must fail");

    let rendered = format!("{err:#}");
    assert!(rendered.contains("9.9.9"), "must name the version asked for: {rendered}");
}

/// Uninstalling a channel three networks name must remove all three links, not just one.
#[test]
fn integration_networks_uninstall_removes_every_naming_link() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_networks_uninstall");
    let fixture = common::harness::OfflineFixture::build(test_env.tmp_dir.path(), "0.15.0");
    let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);

    for args in [
        vec!["midenup", "init"],
        vec!["midenup", "install", "mainnet"],
        vec!["midenup", "uninstall", "0.15.0"],
    ] {
        Midenup::try_parse_from(args.clone())
            .unwrap()
            .execute_with_state(&config, &mut state)
            .unwrap_or_else(|err| panic!("{args:?} failed: {err:#}"));
    }

    for network in ["mainnet", "testnet", "devnet"] {
        assert!(
            std::fs::symlink_metadata(test_env.midenup_home.join("toolchains").join(network))
                .is_err(),
            "{network} must not be left pointing at an uninstalled channel"
        );
    }
}

/// The links are found by scanning `toolchains/`, so the risk is removing too many. With two
/// networks naming two different channels, uninstalling one must leave the other's link both
/// present and resolving.
#[test]
fn integration_networks_uninstall_leaves_other_channels_alone() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_networks_uninstall_split");
    let fixture = common::harness::UpdateFixture::build(test_env.tmp_dir.path());
    let (mut state, config) = test_setup(&test_env, &fixture.with_split_networks());

    for args in [
        vec!["midenup", "init"],
        vec!["midenup", "install", "mainnet"],
        vec!["midenup", "install", "devnet"],
        vec!["midenup", "uninstall", "0.15.0"],
    ] {
        Midenup::try_parse_from(args.clone())
            .unwrap()
            .execute_with_state(&config, &mut state)
            .unwrap_or_else(|err| panic!("{args:?} failed: {err:#}"));
    }

    let toolchains = test_env.midenup_home.join("toolchains");
    assert!(
        std::fs::symlink_metadata(toolchains.join("devnet")).is_err(),
        "devnet named the uninstalled channel and must be gone"
    );
    assert!(
        std::fs::symlink_metadata(toolchains.join("mainnet")).is_ok(),
        "mainnet names a different channel and must survive the uninstall"
    );
    assert!(
        toolchains.join("mainnet").canonicalize().is_ok(),
        "and it must still resolve, not be left dangling"
    );
    assert!(
        toolchains.join("0.14.0").exists(),
        "the channel mainnet names must still be installed"
    );
}

/// Regression, both directions: `default` may point at a network link or straight at a toolchain
/// directory, and uninstalling the channel used to leave it dangling either way.
#[test]
fn integration_networks_uninstall_does_not_leave_default_dangling() {
    let _guard = common::harness::mutating_test_guard();

    for selector in ["mainnet", "0.15.0"] {
        let test_env = environment_setup("integration_networks_default");
        let fixture = common::harness::OfflineFixture::build(test_env.tmp_dir.path(), "0.15.0");
        let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);

        for args in [
            vec!["midenup", "init"],
            vec!["midenup", "install", "mainnet"],
            vec!["midenup", "override", selector],
            vec!["midenup", "uninstall", "0.15.0"],
        ] {
            Midenup::try_parse_from(args.clone())
                .unwrap()
                .execute_with_state(&config, &mut state)
                .unwrap_or_else(|err| panic!("{args:?} failed: {err:#}"));
        }

        let default = test_env.midenup_home.join("toolchains").join("default");
        assert!(
            std::fs::symlink_metadata(&default).is_err() || default.canonicalize().is_ok(),
            "with default set to '{selector}', it must be removed or valid, never dangling"
        );
    }
}

/// `default` must point at the network link, not at the toolchain the network happens to name
/// today, so that it follows the network as it moves.
#[test]
fn integration_networks_override_follows_the_network() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_networks_override");
    let fixture = common::harness::UpdateFixture::build(test_env.tmp_dir.path());

    let (mut state, config) = test_setup(&test_env, &fixture.initial());
    for args in [
        vec!["midenup", "init"],
        vec!["midenup", "install", "mainnet"],
        vec!["midenup", "override", "mainnet"],
    ] {
        Midenup::try_parse_from(args.clone())
            .unwrap()
            .execute_with_state(&config, &mut state)
            .unwrap_or_else(|err| panic!("{args:?} failed: {err:#}"));
    }

    let toolchains = test_env.midenup_home.join("toolchains");
    let default = toolchains.join("default");
    assert_eq!(
        std::fs::read_link(&default).unwrap().file_name().unwrap(),
        "mainnet",
        "default must name the network, not the channel"
    );

    let (_, config) = test_setup(&test_env, &fixture.with_new_stable());
    Midenup::try_parse_from(["midenup", "update", "mainnet"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to update mainnet");

    assert_eq!(
        default.canonicalize().unwrap(),
        toolchains.join("0.15.0").canonicalize().unwrap(),
        "default must have followed mainnet to its new channel"
    );
    // Canonicalizing to the right place is also true of a `default` rewritten to point straight at
    // the toolchain directory, which would stop following mainnet on the *next* promotion.
    assert_eq!(
        std::fs::read_link(&default).unwrap().file_name().unwrap(),
        "mainnet",
        "default must still name the network, not the channel it currently resolves to"
    );
}

/// A synonym is canonicalized on the way in, so what lands in the toolchain file is the network.
#[test]
fn integration_networks_set_writes_the_canonical_name() {
    let test_env = environment_setup("integration_networks_set");
    let fixture = common::harness::OfflineFixture::build(test_env.tmp_dir.path(), "0.15.0");
    let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);

    Midenup::try_parse_from(["midenup", "set", "stable"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to set the toolchain");

    let written =
        std::fs::read_to_string(test_env.present_working_dir.join("miden-toolchain.toml")).unwrap();
    assert!(written.contains(r#"channel = "mainnet""#), "got: {written}");
}

/// The whole point of resolving a network through its symlink: dispatch must name the active
/// channel with no upstream available at all.
///
/// Two channels are installed and the network is left naming the *older* of them, so that the
/// symlink is the only place the answer can come from. With a single installed channel the expected
/// version is simultaneously the symlink's target, the only installation, and the highest one, and
/// an implementation answering from `state.json` or from the highest `toolchains/<semver>` entry
/// would pass just as well. Here those answer 0.15.0 and the symlink answers 0.14.0.
#[test]
fn integration_networks_resolve_offline() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_networks_offline");
    let fixture = common::harness::UpdateFixture::build(test_env.tmp_dir.path());

    // Captured up front: these calls write the manifests, and the files are deleted below.
    let initial = fixture.initial();
    let with_new_stable = fixture.with_new_stable();

    // mainnet names 0.15.0, so that is what gets installed.
    let (mut state, config) = test_setup(&test_env, &with_new_stable);
    for args in [vec!["midenup", "init"], vec!["midenup", "install", "mainnet"]] {
        Midenup::try_parse_from(args.clone())
            .unwrap()
            .execute_with_state(&config, &mut state)
            .unwrap_or_else(|err| panic!("{args:?} failed: {err:#}"));
    }

    // Rolled back to 0.14.0, which installs it alongside 0.15.0 and moves the pointer back.
    let (_, config) = test_setup(&test_env, &initial);
    Midenup::try_parse_from(["midenup", "update", "mainnet"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to follow the rollback");

    // The premise the assertion below rests on: the version the network names is not the highest
    // one installed, so the two cannot be confused.
    let toolchains = test_env.midenup_home.join("toolchains");
    assert!(toolchains.join("0.14.0").exists(), "0.14.0 must be installed");
    assert!(
        toolchains.join("0.15.0").exists(),
        "and 0.15.0 must still be, as the higher one"
    );
    assert_eq!(
        std::fs::read_link(toolchains.join("mainnet")).unwrap(),
        std::path::PathBuf::from("0.14.0"),
        "mainnet must name the older channel, or this proves nothing"
    );

    // No manifest at all: neither upstream nor a cached copy. The fixture keeps its manifests under
    // `test_env.tmp_dir`, and the two above are every one it has written.
    for uri in [&initial, &with_new_stable] {
        let path = uri.strip_prefix("file://").expect("the fixture serves manifests from disk");
        std::fs::remove_file(path).unwrap();
    }
    let cache = midenup::paths::manifest_cache(&test_env.midenup_home);
    assert!(cache.exists(), "the install must have cached the manifest");
    std::fs::remove_file(&cache).unwrap();

    let (_, config) = test_setup(&test_env, &initial);
    let resolved = config
        .local_channel(&UserChannel::default())
        .expect("mainnet must resolve from the symlink with no manifest available");
    assert_eq!(resolved, semver::Version::new(0, 14, 0));
}
