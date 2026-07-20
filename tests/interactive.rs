use std::io::Cursor;

use clap::Parser;
use midenup::{
    commands::{self, Midenup, choose_interactive},
    options,
};

mod common;

use common::*;

/// EOF or truncated input must decline the remaining components instead of
/// silently accepting them.
#[test]
fn interactive_eof_declines_remaining_components() {
    let test_env = environment_setup("interactive_eof_test");

    const FILE: &str =
        full_path_manifest!("tests/data/integration_interactive_test/channel-manifest.json");
    let (_, config) = test_setup(&test_env, FILE);

    let channel = config.manifest.get_latest_stable().unwrap();

    // Only the first prompt (base) is answered; the remaining ones hit EOF.
    let mut input = Cursor::new("y\n");
    let partial = choose_interactive(channel, None, &mut input);

    assert_eq!(
        partial.components.len(),
        1,
        "components without an explicit confirmation should not be selected"
    );
    assert_eq!(partial.components[0].name, "base");
}

/// Dependencies of a selected component are included even when declined:
/// accepting only `midenc` must pull in `base` and `std`.
#[test]
fn interactive_selection_includes_dependencies() {
    let test_env = environment_setup("interactive_dependencies");

    const FILE: &str =
        full_path_manifest!("tests/data/integration_interactive_test/channel-manifest.json");
    let (_, config) = test_setup(&test_env, FILE);

    let channel = config.manifest.get_latest_stable().unwrap();

    // Prompt order is alphabetical: base, cargo-miden, client, faucet-client,
    // midenc, node, std, vm. Only midenc is accepted.
    let mut input = Cursor::new("n\nn\nn\nn\ny\nn\nn\nn\n");
    let partial = choose_interactive(channel, None, &mut input);

    let mut names: Vec<&str> =
        partial.components.iter().map(|component| component.name.as_ref()).collect();
    names.sort_unstable();
    assert_eq!(names, ["base", "midenc", "std"]);
}

/// Installing a channel with no selected components must fail instead of
/// registering an empty toolchain.
#[test]
fn interactive_empty_selection_is_rejected() {
    let test_env = environment_setup("interactive_empty_selection");

    const FILE: &str =
        full_path_manifest!("tests/data/integration_interactive_test/channel-manifest.json");
    let (mut local_manifest, config) = test_setup(&test_env, FILE);

    let channel = config.manifest.get_latest_stable().unwrap();

    let choices = "n\n".repeat(channel.components.len());
    let mut input = Cursor::new(choices.as_str());
    let partial = choose_interactive(channel, None, &mut input);
    assert!(partial.components.is_empty());

    let interactive_options =
        options::InstallationOptions { interactive: true, ..Default::default() };
    let result = commands::install(&config, &partial, &mut local_manifest, &interactive_options);
    assert!(result.is_err(), "installing a channel with no components must fail");
}

/// Tests the full interactive installation flow:
///
/// 1. Install a channel interactively, selecting only libraries and faucet-client
/// 2. Run `miden help toolchain` and verify nothing extra gets installed
/// 3. Update `miden-toolchain.toml` to add `client`, verify it gets installed
/// 4. Run interactive install again to also install `node`
/// 5. Run `midenup update` and verify only installed components are updated
#[test]
fn integration_interactive_test() {
    let test_name = "integration_interactive_test";
    let test_env = environment_setup(test_name);

    const FILE: &str =
        full_path_manifest!("tests/data/integration_interactive_test/channel-manifest.json");

    let (mut local_manifest, config) = test_setup(&test_env, FILE);
    let channel_version = semver::Version::new(0, 10, 0);

    // We install only std, base and faucet-client, since they are the quickest
    // to compile. Components are prompted in name order: base, cargo-miden,
    // client, faucet-client, midenc, node, std, vm.
    let stable_channel = config.manifest.get_latest_stable().unwrap();

    // Interactive installs go through `midenup install --interactive`, so the
    // installation options carry the interactive flag.
    let interactive_options =
        options::InstallationOptions { interactive: true, ..Default::default() };

    let choices = "ynnynnyn".chars().map(|c| format!("{c}\n")).collect::<String>();
    let mut input = Cursor::new(choices.as_str());
    let partial = choose_interactive(stable_channel, None, &mut input);
    commands::install(&config, &partial, &mut local_manifest, &interactive_options).unwrap();

    let installed = local_manifest.get_channel_by_name(&channel_version).unwrap();
    assert_eq!(installed.components.len(), 3, "Expected 3 components: std, base, faucet-client");
    assert!(installed.is_partially_installed());

    // The node component is optional and was not selected, so it must not be
    // installed at this point.
    let node_binary = test_env
        .midenup_home
        .join("toolchains")
        .join("0.10.0")
        .join("bin")
        .join("miden-node");
    assert!(!node_binary.exists(), "node was not selected and should not be installed");

    // After running miden help toolchain, no install should be triggered
    // since an explicit partial channel is considered valid.
    let command = Midenup::try_parse_from(["miden", "help", "toolchain"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to run miden help toolchain");

    let installed = local_manifest.get_channel_by_name(&channel_version).unwrap();
    assert_eq!(
        installed.components.len(),
        3,
        "miden help toolchain should not install extra components"
    );

    // Now we set the miden-toolchain.toml file, in order to add a couple of
    // components.
    let command = Midenup::try_parse_from(["midenup", "set", "0.10.0"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("Failed to set toolchain");

    let toolchain_file = test_env.present_working_dir.join("miden-toolchain.toml");
    assert!(toolchain_file.exists());

    // Overwrite with the pre-made file that adds client
    let toolchain_with_client =
        full_path!("tests/data/integration_interactive_test/miden-toolchain-with-client.toml");
    std::fs::copy(toolchain_with_client, &toolchain_file).unwrap();

    // miden help toolchain should trigger install of client only
    let command = Midenup::try_parse_from(["miden", "help", "toolchain"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("failed to run miden help toolchain");

    let installed = local_manifest.get_channel_by_name(&channel_version).unwrap();
    assert_eq!(
        installed.components.len(),
        4,
        "Expected 4 components: std, base, faucet-client, client"
    );

    // Now, interactively, we'll install the node. The components not yet
    // installed are prompted in name order: cargo-miden, midenc, node, vm.
    let channel = config.manifest.get_latest_stable().unwrap();
    let installed_channel = local_manifest.get_channel_by_name(&channel.name);

    let choices = "nnyn".chars().map(|c| format!("{c}\n")).collect::<String>();
    let mut input = Cursor::new(choices.as_str());
    let partial = choose_interactive(channel, installed_channel, &mut input);
    commands::install(&config, &partial, &mut local_manifest, &interactive_options).unwrap();

    let installed = local_manifest.get_channel_by_name(&channel_version).unwrap();
    assert_eq!(
        installed.components.len(),
        5,
        "Expected 5 components: std, base, faucet-client, client, node"
    );

    // node is marked optional in the manifest, but the user explicitly selected
    // it, so it must be installed even though the profile is minimal.
    assert!(
        node_binary.exists(),
        "node was explicitly selected interactively and should be installed despite being an \
         optional component under the minimal profile"
    );

    let command = Midenup::try_parse_from(["midenup", "update"]).unwrap();
    command
        .execute_with_manifest(&config, &mut local_manifest)
        .expect("Failed to update stable");

    let installed = local_manifest.get_channel_by_name(&channel_version).unwrap();
    assert_eq!(
        installed.components.len(),
        5,
        "midenup update should not install excluded components"
    );
}
