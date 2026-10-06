//! Named installations exercise the real CLI against local artifacts and dependency-free crates.

use std::{path::Path, process::Output};

mod common;
use common::{harness::OfflineFixture, *};

fn command(env: &TestEnvironment, manifest: &str, args: &[&str]) -> Output {
    midenup_command(env!("CARGO_BIN_EXE_midenup"), env, manifest)
        .env_remove("MIDENUP_TOOLCHAIN")
        .args(args)
        .arg("--plain")
        .output()
        .unwrap()
}

fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn project(env: &TestEnvironment, name: &str, channel: &str) {
    std::fs::write(
        env.present_working_dir.join("miden-toolchain.toml"),
        format!("[toolchain]\nname = {name:?}\nchannel = {channel:?}\n"),
    )
    .unwrap();
}

fn dispatch(env: &TestEnvironment, manifest: &str, args: &[&str]) -> Output {
    midenup_command(env.cargo_home.join("bin/miden"), env, manifest)
        .env_remove("MIDENUP_TOOLCHAIN")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn integration_custom_stable_override_survives_startup_migration() {
    let env = environment_setup("named_stable_override");
    let fixture = OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    project(&env, "stable", "0.15.0");
    success(command(&env, &fixture.manifest_uri, &["install"]));
    std::fs::remove_file(env.present_working_dir.join("miden-toolchain.toml")).unwrap();
    success(command(&env, &fixture.manifest_uri, &["override", "custom:stable"]));

    for _ in 0..2 {
        assert_eq!(
            success(command(&env, &fixture.manifest_uri, &["show", "active-toolchain"])).trim(),
            "custom:stable"
        );
        assert_eq!(
            success(dispatch(&env, &fixture.manifest_uri, &["help", "vm"])),
            "miden-vm 0.15.0\n"
        );
    }
}

#[test]
fn integration_canonical_and_two_named_installations_coexist() {
    let env = environment_setup("named_coexist");
    let fixture = OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    success(command(&env, &fixture.manifest_uri, &["install", "0.15.0"]));
    for name in ["project-dev", "other-dev"] {
        project(&env, name, "0.15.0");
        success(command(&env, &fixture.manifest_uri, &["install"]));
    }
    let toolchains = env.midenup_home.join("toolchains");
    let canonical = toolchains.join("0.15.0").canonicalize().unwrap();
    let first = toolchains.join("custom/project-dev").canonicalize().unwrap();
    let second = toolchains.join("custom/other-dev").canonicalize().unwrap();
    assert_ne!(canonical, first);
    assert_ne!(canonical, second);
    assert_ne!(first, second);
    for path in [&canonical, &first, &second] {
        assert!(path.join("bin/miden-vm").is_file());
        assert!(path.join("lib/core.masp").is_file());
    }
    let listed = success(command(&env, &fixture.manifest_uri, &["show", "list"]));
    assert!(listed.contains("custom:project-dev"), "{listed}");
    assert!(listed.contains("custom:other-dev"), "{listed}");
}

#[test]
fn integration_implicit_conflicting_source_requires_explicit_install() {
    let env = environment_setup("named_conflict");
    let fixture = OfflineFixture::new(env.tmp_dir.path())
        .with_channel("0.15.0")
        .with_channel("0.16.0")
        .build();
    project(&env, "project-dev", "0.15.0");
    success(command(&env, &fixture.manifest_uri, &["install"]));
    project(&env, "project-dev", "0.16.0");
    let output = dispatch(&env, &fixture.manifest_uri, &["help", "vm"]);
    assert!(!output.status.success(), "implicit selection must not replace a named recipe");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("midenup install"), "{error}");
    success(command(&env, &fixture.manifest_uri, &["install"]));
    assert_eq!(
        success(dispatch(&env, &fixture.manifest_uri, &["help", "vm"])),
        "miden-vm 0.16.0\n"
    );
}

#[test]
fn integration_invalid_names_and_unnamed_patches_fail_before_installation() {
    for name in ["", "..", "../escape", "a/b", "a\\b", "custom:other"] {
        let env = environment_setup("named_invalid");
        let fixture = OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
        project(&env, name, "0.15.0");
        assert!(!command(&env, &fixture.manifest_uri, &["install"]).status.success(), "{name:?}");
        assert!(!env.midenup_home.join("toolchains/0.15.0").exists());
    }
    let env = environment_setup("unnamed_patches");
    let fixture = OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    std::fs::write(
        env.present_working_dir.join("miden-toolchain.toml"),
        "[toolchain]\nchannel = \"0.15.0\"\n[patches.vm]\nversion = {kind = \"registry\", version \
         = \"0.1.0\"}\n",
    )
    .unwrap();
    let output = command(&env, &fixture.manifest_uri, &["install"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("name"));
    assert!(!env.midenup_home.join("toolchains/0.15.0").exists());
}

fn patched_project(env: &TestEnvironment, name: &str, source: &Path) {
    std::fs::write(
        env.present_working_dir.join("miden-toolchain.toml"),
        format!(
            "[toolchain]\nname = {name:?}\nchannel = \"0.15.0\"\ncomponents = \
             [\"prover\"]\n\n[patches.prover]\nversion = {{kind = \"path\", path = {source:?}}}\n"
        ),
    )
    .unwrap();
}

fn unnamed_patch_project(env: &TestEnvironment) {
    std::fs::write(
        env.present_working_dir.join("miden-toolchain.toml"),
        "[toolchain]\nchannel = \"0.15.0\"\n[patches.vm]\ncrate_name = \"miden-vm\"\nversion = { \
         kind = \"registry\", version = \"0.1.0\" }\n",
    )
    .unwrap();
}

#[test]
fn integration_unnamed_patches_do_not_break_unrelated_commands() {
    let env = environment_setup("unnamed_patch_maintenance");
    let fixture = OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    success(command(&env, &fixture.manifest_uri, &["install", "0.15.0"]));
    unnamed_patch_project(&env);
    success(
        midenup_command(env!("CARGO_BIN_EXE_midenup"), &env, &fixture.manifest_uri)
            .env_remove("MIDENUP_TOOLCHAIN")
            .args(["show", "home"])
            .output()
            .unwrap(),
    );
    for args in [vec!["gc"], vec!["update", "0.15.0"]] {
        success(command(&env, &fixture.manifest_uri, &args));
    }
    for output in [
        command(&env, &fixture.manifest_uri, &["install"]),
        dispatch(&env, &fixture.manifest_uri, &["help", "vm"]),
    ] {
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("patches require"));
    }
    success(command(&env, &fixture.manifest_uri, &["uninstall", "0.15.0"]));
}

#[test]
fn integration_unnamed_patches_fail_update_before_mutating_installations() {
    let env = environment_setup("unnamed_patch_update_preflight");
    let fixture = OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    success(command(&env, &fixture.manifest_uri, &["install", "0.15.0"]));
    let state_path = midenup::paths::state_path(&env.midenup_home);
    let before = std::fs::read_to_string(&state_path).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.manifest_path).unwrap()).unwrap();
    let vm = manifest["channels"][0]["components"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|component| component["name"] == "vm")
        .unwrap();
    vm["aliases"] = serde_json::json!({"new-alias": ["%installed-executable"]});
    std::fs::write(&fixture.manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    unnamed_patch_project(&env);

    let output = command(&env, &fixture.manifest_uri, &["update"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("patches require"));
    assert_eq!(
        std::fs::read_to_string(&state_path).unwrap(),
        before,
        "invalid active definitions must fail before updating anything"
    );
}

#[test]
fn integration_named_conflicts_fail_update_before_mutating_installations() {
    let env = environment_setup("named_update_preflight");
    let fixture = OfflineFixture::new(env.tmp_dir.path())
        .with_channel("0.15.0")
        .with_channel("0.16.0")
        .build();
    project(&env, "project-dev", "0.15.0");
    success(command(&env, &fixture.manifest_uri, &["install"]));
    let state_path = midenup::paths::state_path(&env.midenup_home);
    let before = std::fs::read_to_string(&state_path).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.manifest_path).unwrap()).unwrap();
    let vm = manifest["channels"][0]["components"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|component| component["name"] == "vm")
        .unwrap();
    vm["aliases"] = serde_json::json!({"new-alias": ["%installed-executable"]});
    std::fs::write(&fixture.manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    project(&env, "project-dev", "0.16.0");

    let output = command(&env, &fixture.manifest_uri, &["update"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("midenup install"));
    assert_eq!(
        std::fs::read_to_string(&state_path).unwrap(),
        before,
        "known definition conflicts must fail before updating anything"
    );
}

fn fork_crate(original: &Path, destination: &Path, marker: &str) {
    std::fs::create_dir_all(destination.join("src")).unwrap();
    for file in ["Cargo.toml", "Cargo.lock"] {
        std::fs::copy(original.join(file), destination.join(file)).unwrap();
    }
    write_marker(destination, marker);
}

fn write_marker(source: &Path, marker: &str) {
    std::fs::write(source.join("src/main.rs"), format!("fn main() {{ println!({marker:?}); }}\n"))
        .unwrap();
}

#[test]
fn integration_two_path_variants_preserve_their_recipes_when_selected_offline() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("named_patch_isolation");
    let fixture = OfflineFixture::new(env.tmp_dir.path())
        .with_channel("0.15.0")
        .with_cargo_component("prover")
        .build();
    write_marker(&fixture.dir.join("prover-source"), "canonical");
    success(command(&env, &fixture.manifest_uri, &["install", "0.15.0"]));
    for name in ["alpha", "beta"] {
        let source = env.tmp_dir.path().join(name);
        fork_crate(&fixture.dir.join("prover-source"), &source, name);
        patched_project(&env, name, &source);
        success(command(&env, &fixture.manifest_uri, &["install"]));
    }
    std::fs::remove_file(env.present_working_dir.join("miden-toolchain.toml")).unwrap();
    std::fs::remove_file(&fixture.manifest_path).unwrap();
    std::fs::remove_file(midenup::paths::manifest_cache(&env.midenup_home)).unwrap();

    for (selector, marker) in [
        ("+custom:alpha", "alpha\n"),
        ("+custom:beta", "beta\n"),
        ("+0.15.0", "canonical\n"),
    ] {
        assert_eq!(
            success(dispatch(&env, &fixture.manifest_uri, &[selector, "help", "prover"])),
            marker
        );
    }
    success(command(&env, &fixture.manifest_uri, &["override", "custom:alpha"]));
    assert_eq!(success(dispatch(&env, &fixture.manifest_uri, &["help", "prover"])), "alpha\n");
    assert_eq!(
        success(command(&env, &fixture.manifest_uri, &["show", "active-toolchain"])).trim(),
        "custom:alpha"
    );
    success(command(&env, &fixture.manifest_uri, &["set", "custom:beta"]));
    assert_eq!(success(dispatch(&env, &fixture.manifest_uri, &["help", "prover"])), "beta\n");
    let output = midenup_command(env.cargo_home.join("bin/miden"), &env, &fixture.manifest_uri)
        .env("MIDENUP_TOOLCHAIN", "custom:alpha")
        .args(["help", "prover"])
        .output()
        .unwrap();
    assert_eq!(success(output), "alpha\n");
    let state =
        midenup::state::LocalState::load(&midenup::paths::state_path(&env.midenup_home)).unwrap();
    assert_eq!(state.installations.len(), 3);
    assert!(state.get(&semver::Version::new(0, 15, 0)).unwrap().patches.is_empty());
}

#[test]
fn integration_named_path_update_honors_policy_and_keeps_the_patch() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("named_path_update");
    let fixture = OfflineFixture::new(env.tmp_dir.path())
        .with_channel("0.15.0")
        .with_cargo_component("prover")
        .build();
    let source = env.tmp_dir.path().join("patched-prover");
    fork_crate(&fixture.dir.join("prover-source"), &source, "before");
    patched_project(&env, "project-dev", &source);
    success(command(&env, &fixture.manifest_uri, &["install"]));
    std::fs::remove_file(env.present_working_dir.join("miden-toolchain.toml")).unwrap();
    write_marker(&source, "after");
    success(command(
        &env,
        &fixture.manifest_uri,
        &["update", "custom:project-dev", "--path-update=off"],
    ));
    assert_eq!(
        success(dispatch(
            &env,
            &fixture.manifest_uri,
            &["+custom:project-dev", "help", "prover"]
        )),
        "before\n"
    );
    success(command(
        &env,
        &fixture.manifest_uri,
        &["update", "custom:project-dev", "--path-update=all"],
    ));
    assert_eq!(
        success(dispatch(
            &env,
            &fixture.manifest_uri,
            &["+custom:project-dev", "help", "prover"]
        )),
        "after\n"
    );
    write_marker(&source, "explicit");
    success(command(&env, &fixture.manifest_uri, &["install", "custom:project-dev"]));
    assert_eq!(
        success(dispatch(
            &env,
            &fixture.manifest_uri,
            &["+custom:project-dev", "help", "prover"]
        )),
        "explicit\n"
    );
    assert!(!env.midenup_home.join("toolchains/0.15.0").exists());
}

#[test]
fn integration_implicit_conflicting_patch_requires_explicit_install() {
    conflicting_patch_requires_explicit_install("target");
}

#[test]
fn integration_patch_replacement_isolates_shared_cargo_intermediates() {
    conflicting_patch_requires_explicit_install("build");
}

#[test]
fn integration_patch_replacement_isolates_cargo_config_build_directories() {
    conflicting_patch_requires_explicit_install("config");
}

fn conflicting_patch_requires_explicit_install(shared_directory: &str) {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("named_patch_conflict");
    let fixture = OfflineFixture::new(env.tmp_dir.path())
        .with_channel("0.15.0")
        .with_cargo_component("prover")
        .build();
    for name in ["before", "after"] {
        let source = env.tmp_dir.path().join(name);
        fork_crate(&fixture.dir.join("prover-source"), &source, name);
        if shared_directory == "config" {
            std::fs::create_dir(source.join(".cargo")).unwrap();
            std::fs::write(
                source.join(".cargo/config.toml"),
                format!(
                    "[build]\ntarget-dir = {:?}\nbuild-dir = {:?}\n",
                    env.tmp_dir.path().join("shared-target"),
                    env.tmp_dir.path().join("shared-build")
                ),
            )
            .unwrap();
        }
    }
    let install = || {
        let mut install =
            midenup_command(env!("CARGO_BIN_EXE_midenup"), &env, &fixture.manifest_uri);
        install
            .env_remove("MIDENUP_TOOLCHAIN")
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("CARGO_BUILD_BUILD_DIR")
            .args(["install", "--plain"]);
        match shared_directory {
            "target" => {
                install.env("CARGO_TARGET_DIR", env.tmp_dir.path().join("shared-target"));
            },
            "build" => {
                install.env("CARGO_BUILD_BUILD_DIR", env.tmp_dir.path().join("shared-build"));
            },
            "config" => {},
            _ => unreachable!(),
        }
        success(install.output().unwrap())
    };
    patched_project(&env, "project-dev", &env.tmp_dir.path().join("before"));
    install();
    patched_project(&env, "project-dev", &env.tmp_dir.path().join("after"));
    let output = dispatch(&env, &fixture.manifest_uri, &["help", "prover"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("midenup install"));
    assert_eq!(
        success(dispatch(
            &env,
            &fixture.manifest_uri,
            &["+custom:project-dev", "help", "prover"]
        )),
        "before\n"
    );
    install();
    assert_eq!(success(dispatch(&env, &fixture.manifest_uri, &["help", "prover"])), "after\n");
}

fn move_mainnet(fixture: &OfflineFixture, version: &str) {
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.manifest_path).unwrap()).unwrap();
    manifest["networks"]["mainnet"] = version.into();
    std::fs::write(&fixture.manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
}

#[test]
fn integration_named_network_update_advances_independently_and_pinned_variant_stays_pinned() {
    let env = environment_setup("named_network_update");
    let fixture = OfflineFixture::new(env.tmp_dir.path())
        .with_channel("0.15.0")
        .with_channel("0.16.0")
        .build();
    success(command(&env, &fixture.manifest_uri, &["install", "mainnet"]));
    for (name, source) in [("tracking", "mainnet"), ("pinned", "0.15.0")] {
        project(&env, name, source);
        success(command(&env, &fixture.manifest_uri, &["install"]));
    }
    let data = env.midenup_home.join("var/custom/tracking/networks/mainnet/store.sqlite3");
    std::fs::create_dir_all(data.parent().unwrap()).unwrap();
    std::fs::write(&data, b"persistent state").unwrap();
    move_mainnet(&fixture, "0.16.0");
    success(command(&env, &fixture.manifest_uri, &["update", "custom:tracking"]));
    success(command(&env, &fixture.manifest_uri, &["update", "custom:pinned"]));
    assert_eq!(
        success(dispatch(&env, &fixture.manifest_uri, &["+custom:tracking", "help", "vm"])),
        "miden-vm 0.16.0\n"
    );
    for selector in ["+mainnet", "+0.15.0", "+custom:pinned"] {
        assert_eq!(
            success(dispatch(&env, &fixture.manifest_uri, &[selector, "help", "vm"])),
            "miden-vm 0.15.0\n"
        );
    }
    assert_eq!(std::fs::read(&data).unwrap(), b"persistent state");
    assert!(!env.midenup_home.join("toolchains/0.16.0").exists());

    success(command(&env, &fixture.manifest_uri, &["update", "mainnet"]));
    assert_eq!(
        success(dispatch(&env, &fixture.manifest_uri, &["+mainnet", "help", "vm"])),
        "miden-vm 0.16.0\n"
    );
    assert_eq!(
        success(dispatch(&env, &fixture.manifest_uri, &["+custom:pinned", "help", "vm"])),
        "miden-vm 0.15.0\n"
    );
}

#[test]
fn integration_an_existing_named_network_cannot_switch_network_identity() {
    let env = environment_setup("named_network_identity");
    let fixture = OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    project(&env, "project-dev", "mainnet");
    success(command(&env, &fixture.manifest_uri, &["install"]));
    for source in ["testnet", "0.15.0"] {
        project(&env, "project-dev", source);
        for output in [
            dispatch(&env, &fixture.manifest_uri, &["help", "vm"]),
            command(&env, &fixture.manifest_uri, &["install"]),
        ] {
            assert!(!output.status.success(), "a network change must not reuse runtime data");
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(error.contains("choose a new name"), "{error}");
            assert!(
                !error.contains("midenup install"),
                "must not suggest an install that will be rejected: {error}"
            );
        }
    }
    assert_eq!(
        success(dispatch(&env, &fixture.manifest_uri, &["+custom:project-dev", "help", "vm"])),
        "miden-vm 0.15.0\n"
    );
}

#[test]
fn integration_named_uninstall_preserves_other_installations_and_requires_purge_for_data() {
    let env = environment_setup("named_uninstall");
    let fixture = OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    success(command(&env, &fixture.manifest_uri, &["install", "0.15.0"]));
    for name in ["alpha", "beta"] {
        project(&env, name, "0.15.0");
        success(command(&env, &fixture.manifest_uri, &["install"]));
        let data = env.midenup_home.join("var/custom").join(name).join("pinned");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("store"), name).unwrap();
    }
    success(command(&env, &fixture.manifest_uri, &["uninstall", "custom:alpha"]));
    assert!(env.midenup_home.join("toolchains/custom/alpha").symlink_metadata().is_err());
    assert_eq!(
        std::fs::read_to_string(env.midenup_home.join("var/custom/alpha/pinned/store")).unwrap(),
        "alpha"
    );
    for selector in ["+0.15.0", "+custom:beta"] {
        assert_eq!(
            success(dispatch(&env, &fixture.manifest_uri, &[selector, "help", "vm"])),
            "miden-vm 0.15.0\n"
        );
    }
    success(command(&env, &fixture.manifest_uri, &["uninstall", "custom:beta", "--purge"]));
    assert!(!env.midenup_home.join("var/custom/beta").exists());
    assert!(env.midenup_home.join("var/custom/alpha/pinned/store").is_file());
    assert!(env.midenup_home.join("toolchains/0.15.0/bin/miden-vm").is_file());
}

#[test]
fn integration_dispatch_uses_each_named_installations_runtime_directory() {
    let env = environment_setup("named_runtime_data");
    let fixture = OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    std::fs::write(fixture.dir.join("0.15.0/miden-vm"), "#!/bin/sh\nprintf '%s\\n' \"$@\"\n")
        .unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.manifest_path).unwrap()).unwrap();
    manifest["channels"][0]["components"][0]["aliases"] =
        serde_json::json!({"store": ["%installed-executable", "%var(data)"]});
    std::fs::write(&fixture.manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    for name in ["alpha", "beta"] {
        project(&env, name, "mainnet");
        success(command(&env, &fixture.manifest_uri, &["install"]));
        let actual = success(dispatch(&env, &fixture.manifest_uri, &["store"]));
        let expected = env.midenup_home.join("var/custom").join(name).join("networks/mainnet/data");
        assert_eq!(actual.trim(), expected.to_str().unwrap());
    }
    let old_store = env.midenup_home.join("var/custom/alpha/networks/mainnet/data");
    std::fs::write(&old_store, b"mainnet accounts").unwrap();
    success(command(&env, &fixture.manifest_uri, &["uninstall", "custom:alpha"]));
    project(&env, "alpha", "testnet");
    success(command(&env, &fixture.manifest_uri, &["install"]));
    let selected = success(dispatch(&env, &fixture.manifest_uri, &["store"]));
    let expected = env.midenup_home.join("var/custom/alpha/networks/testnet/data");
    assert_eq!(selected.trim(), expected.to_str().unwrap());
    assert!(
        !expected.exists(),
        "reusing a name for another network must not reuse its accounts"
    );
    assert_eq!(std::fs::read(&old_store).unwrap(), b"mainnet accounts");
    assert!(!env.midenup_home.join("toolchains/mainnet").exists());
}

#[test]
fn integration_concurrent_named_dispatch_keeps_executable_libraries_and_path_in_one_publication() {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };

    let env = environment_setup("named_concurrent_dispatch");
    let fixture = OfflineFixture::new(env.tmp_dir.path())
        .with_channel("0.15.0")
        .with_channel("0.16.0")
        .build();
    let probe = r#"#!/bin/sh
if [ -n "$NAMED_READY" ]; then
    touch "$NAMED_READY"
    count=0
    while [ ! -f "$NAMED_RELEASE" ]; do
        count=$((count + 1))
        [ "$count" -lt 1000 ] || exit 90
        sleep 0.01
    done
fi
printf '%s\n%s\n%s\n%s\n' "$0" "$MIDEN_SYSROOT" "$1" "${PATH%%:*}"
cat "$1"
cat "$MIDEN_SYSROOT/lib/core.masp"
"#;
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.manifest_path).unwrap()).unwrap();
    for channel in manifest["channels"].as_array_mut().unwrap() {
        let version = channel["name"].as_str().unwrap();
        std::fs::write(fixture.dir.join(version).join("miden-vm"), probe).unwrap();
        std::fs::write(fixture.dir.join(version).join("core.masp"), format!("{version}\n"))
            .unwrap();
        channel["components"][0]["aliases"] =
            serde_json::json!({"snapshot": ["%installed-executable", "%lib(core.masp)"]});
    }
    std::fs::write(&fixture.manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    for (name, version) in [("alpha", "0.15.0"), ("beta", "0.16.0")] {
        project(&env, name, version);
        success(command(&env, &fixture.manifest_uri, &["install"]));
    }
    let ready = env.tmp_dir.path().join("ready");
    let release = env.tmp_dir.path().join("release");
    let mut first = midenup_command(env.cargo_home.join("bin/miden"), &env, &fixture.manifest_uri)
        .env_remove("MIDENUP_TOOLCHAIN")
        .env("NAMED_READY", &ready)
        .env("NAMED_RELEASE", &release)
        .args(["+custom:alpha", "snapshot"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() && Instant::now() < deadline {
        if first.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !ready.exists() {
        let _ = first.kill();
        panic!(
            "first dispatch did not reach its barrier: {:?}",
            first.wait_with_output().unwrap()
        );
    }
    let second = dispatch(&env, &fixture.manifest_uri, &["+custom:beta", "snapshot"]);
    std::fs::write(&release, b"go").unwrap();
    let first = first.wait_with_output().unwrap();
    for (name, version, output) in [("alpha", "0.15.0", first), ("beta", "0.16.0", second)] {
        let publication =
            env.midenup_home.join("toolchains/custom").join(name).canonicalize().unwrap();
        let observed = success(output);
        let lines: Vec<_> = observed.lines().collect();
        assert_eq!(lines.len(), 6, "{observed}");
        for (line, expected) in [
            (lines[0], publication.join("bin/miden-vm")),
            (lines[1], publication.clone()),
            (lines[2], publication.join("lib/core.masp")),
            (lines[3], publication.join("opt")),
        ] {
            assert_eq!(Path::new(line).canonicalize().unwrap(), expected, "{observed}");
            assert!(!line.contains("/toolchains/"), "dispatch must pin its publication: {line}");
        }
        assert_eq!(&lines[4..], &[version, version]);
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn integration_named_recovery_never_replaces_the_canonical_or_another_named_installation() {
    use midenup::fault::{FAULT_POINT_ENV, FaultPoint};

    for point in FaultPoint::PUBLICATION {
        let env = environment_setup("named_recovery");
        let fixture = OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
        success(command(&env, &fixture.manifest_uri, &["install", "0.15.0"]));
        project(&env, "existing", "0.15.0");
        success(command(&env, &fixture.manifest_uri, &["install"]));
        let canonical = env.midenup_home.join("toolchains/0.15.0").canonicalize().unwrap();
        let existing = env.midenup_home.join("toolchains/custom/existing").canonicalize().unwrap();
        project(&env, "interrupted", "0.15.0");
        let output = midenup_command(env!("CARGO_BIN_EXE_midenup"), &env, &fixture.manifest_uri)
            .env_remove("MIDENUP_TOOLCHAIN")
            .env(FAULT_POINT_ENV, point.as_str())
            .args(["install", "--plain"])
            .output()
            .unwrap();
        assert!(!output.status.success(), "fault {point} must interrupt installation");
        success(command(&env, &fixture.manifest_uri, &["show", "list"]));
        assert!(midenup::publish::journal::read(&env.midenup_home).unwrap().is_none());
        assert_eq!(env.midenup_home.join("toolchains/0.15.0").canonicalize().unwrap(), canonical);
        assert_eq!(
            env.midenup_home.join("toolchains/custom/existing").canonicalize().unwrap(),
            existing
        );
        let committed = matches!(
            point,
            FaultPoint::PostCommit | FaultPoint::PostRecord | FaultPoint::PostDerive
        );
        assert_eq!(
            env.midenup_home.join("toolchains/custom/interrupted/bin/miden-vm").is_file(),
            committed,
            "{point}"
        );
        let state =
            midenup::state::LocalState::load(&midenup::paths::state_path(&env.midenup_home))
                .unwrap();
        assert_eq!(state.installations.len(), if committed { 3 } else { 2 }, "{point}");
        for selector in ["+0.15.0", "+custom:existing"] {
            assert_eq!(
                success(dispatch(&env, &fixture.manifest_uri, &[selector, "help", "vm"])),
                "miden-vm 0.15.0\n"
            );
        }
    }
}

#[cfg(feature = "fault-injection")]
#[test]
fn integration_first_named_install_protects_legacy_state_before_preparing_its_journal() {
    use midenup::fault::{FAULT_POINT_ENV, FaultPoint};

    let env = environment_setup("named_legacy_guard");
    let fixture = OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    success(command(&env, &fixture.manifest_uri, &["install", "0.15.0"]));
    let canonical = env.midenup_home.join("toolchains/0.15.0").canonicalize().unwrap();
    let state_path = midenup::paths::state_path(&env.midenup_home);
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    legacy["state_version"] = "1.0.0".into();
    std::fs::write(&state_path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    project(&env, "project-dev", "0.15.0");
    let output = midenup_command(env!("CARGO_BIN_EXE_midenup"), &env, &fixture.manifest_uri)
        .env_remove("MIDENUP_TOOLCHAIN")
        .env(FAULT_POINT_ENV, FaultPoint::PostPrepare.as_str())
        .args(["install", "--plain"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(midenup::publish::journal::read(&env.midenup_home).unwrap().is_some());
    let guarded: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    assert_eq!(
        guarded["state_version"], "2.0.0",
        "old binaries must refuse state before a custom journal exists"
    );
    success(command(&env, &fixture.manifest_uri, &["show", "list"]));
    assert_eq!(env.midenup_home.join("toolchains/0.15.0").canonicalize().unwrap(), canonical);
    assert!(
        env.midenup_home
            .join("toolchains/custom/project-dev")
            .symlink_metadata()
            .is_err()
    );
    assert_eq!(
        success(dispatch(&env, &fixture.manifest_uri, &["+0.15.0", "help", "vm"])),
        "miden-vm 0.15.0\n"
    );
}

#[test]
fn integration_explicit_custom_install_preserves_nonminimal_components_and_patches() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("named_install_preserves_recipe");
    let fixture = OfflineFixture::new(env.tmp_dir.path())
        .with_channel("0.15.0")
        .with_cargo_component("prover")
        .build();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&fixture.manifest_path).unwrap()).unwrap();
    let prover = manifest["channels"][0]["components"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|component| component["name"] == "prover")
        .unwrap();
    prover["profiles"] = serde_json::json!(["complete"]);
    std::fs::write(&fixture.manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let source = env.tmp_dir.path().join("patched-prover");
    fork_crate(&fixture.dir.join("prover-source"), &source, "patched");
    patched_project(&env, "project-dev", &source);
    success(command(
        &env,
        &fixture.manifest_uri,
        &["install", "--profile", "empty", "--component", "assets"],
    ));
    std::fs::remove_file(env.present_working_dir.join("miden-toolchain.toml")).unwrap();
    success(command(&env, &fixture.manifest_uri, &["install", "custom:project-dev"]));
    let root = env.midenup_home.join("toolchains/custom/project-dev");
    assert!(root.join("etc/assets/config.yml").is_file());
    assert!(
        !root.join("bin/miden-vm").exists(),
        "an explicit reference must preserve the empty profile"
    );
    assert_eq!(
        success(dispatch(
            &env,
            &fixture.manifest_uri,
            &["+custom:project-dev", "help", "prover"]
        )),
        "patched\n"
    );
}

#[test]
fn integration_custom_references_cannot_redeclare_names_or_patches() {
    let env = environment_setup("named_reference_validation");
    let fixture = OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    project(&env, "existing", "0.15.0");
    success(command(&env, &fixture.manifest_uri, &["install"]));
    let original = env.midenup_home.join("toolchains/custom/existing").canonicalize().unwrap();
    for extra in [
        "name = \"redirected\"\n",
        "[patches.vm]\nversion = {kind = \"registry\", version = \"0.1.0\"}\n",
    ] {
        std::fs::write(
            env.present_working_dir.join("miden-toolchain.toml"),
            format!("[toolchain]\nchannel = \"custom:existing\"\n{extra}"),
        )
        .unwrap();
        let output = command(&env, &fixture.manifest_uri, &["install"]);
        assert!(!output.status.success(), "custom reference must reject {extra}");
        assert_eq!(
            env.midenup_home.join("toolchains/custom/existing").canonicalize().unwrap(),
            original
        );
        assert!(!env.midenup_home.join("toolchains/custom/redirected").exists());
    }
}
