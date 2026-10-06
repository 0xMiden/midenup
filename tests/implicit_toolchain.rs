//! Commands that infer their toolchain must install under the command's existing lock and honor
//! the same selection flags as an explicit install. All artifacts are local stand-ins.

use std::{
    process::Stdio,
    time::{Duration, Instant},
};

use midenup::{paths, profile::Profile, state::LocalState};

mod common;
use common::*;

fn run(env: &TestEnvironment, manifest: &str, args: &[&str]) {
    let mut child = midenup_command(env!("CARGO_BIN_EXE_midenup"), env, manifest)
        .env_remove("MIDENUP_TOOLCHAIN")
        .args(args)
        .arg("--plain")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut timed_out = false;
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            child.kill().unwrap();
            timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        !timed_out && output.status.success(),
        "{args:?}: timed out: {timed_out}; status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn integration_implicit_install_installs_a_missing_toolchain() {
    let env = environment_setup("implicit_install_missing");
    let fixture = common::harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    run(&env, &fixture.manifest_uri, &["install"]);
    assert!(env.midenup_home.join("toolchains/default/bin/miden-vm").is_file());
}

#[test]
fn integration_implicit_update_installs_a_missing_toolchain() {
    let env = environment_setup("implicit_update_missing");
    let fixture = common::harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    run(&env, &fixture.manifest_uri, &["update"]);
    assert!(env.midenup_home.join("toolchains/mainnet/bin/miden-vm").is_file());
}

#[test]
fn integration_implicit_install_honors_selection_flags() {
    for flags in [vec!["--profile", "complete"], vec!["--component", "assets"]] {
        let env = environment_setup("implicit_install_flags");
        let fixture = common::harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
        run(&env, &fixture.manifest_uri, &["install", "mainnet"]);
        let mut args = vec!["install"];
        args.extend(flags.iter().copied());
        run(&env, &fixture.manifest_uri, &args);
        assert!(
            env.midenup_home.join("toolchains/mainnet/etc/assets/config.yml").is_file(),
            "{flags:?} must install assets"
        );
        let state = LocalState::load(&paths::state_path(&env.midenup_home)).unwrap();
        let installed = state.get(&semver::Version::new(0, 15, 0)).unwrap();
        if flags[0] == "--profile" {
            assert!(installed.intent.profiles.contains(&Profile::Complete));
        } else {
            assert!(installed.intent.roots.contains("assets"));
        }
    }
}

#[test]
fn integration_implicit_install_replaces_satisfied_intent() {
    let env = environment_setup("implicit_install_intent");
    let fixture = common::harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    run(&env, &fixture.manifest_uri, &["install", "mainnet", "--profile", "complete"]);
    run(
        &env,
        &fixture.manifest_uri,
        &["install", "--profile", "minimal", "--component", "core"],
    );
    let state = LocalState::load(&paths::state_path(&env.midenup_home)).unwrap();
    let installed = state.get(&semver::Version::new(0, 15, 0)).unwrap();
    assert_eq!(installed.intent.profiles, [Profile::Minimal].into_iter().collect());
    assert_eq!(installed.intent.roots, ["core".to_string()].into_iter().collect());
    assert!(!env.midenup_home.join("toolchains/mainnet/etc/assets/config.yml").exists());
}

#[test]
fn integration_implicit_install_uses_project_selection_unless_overridden() {
    for (flags, has_vm) in [(vec![], false), (vec!["--profile", "minimal"], true)] {
        let env = environment_setup("implicit_install_project");
        let fixture = common::harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
        std::fs::write(
            env.present_working_dir.join("miden-toolchain.toml"),
            "[toolchain]\nchannel = \"0.15.0\"\nprofile = \"empty\"\ncomponents = [\"assets\"]\n",
        )
        .unwrap();
        let mut args = vec!["install"];
        args.extend(flags);
        run(&env, &fixture.manifest_uri, &args);
        let channel = env.midenup_home.join("toolchains/0.15.0");
        assert!(channel.join("etc/assets/config.yml").is_file());
        assert_eq!(channel.join("bin/miden-vm").is_file(), has_vm);
    }
}

#[test]
fn integration_implicit_update_installs_missing_project_components() {
    let env = environment_setup("implicit_update_components");
    let fixture = common::harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");
    run(&env, &fixture.manifest_uri, &["install", "mainnet"]);
    std::fs::write(
        env.present_working_dir.join("miden-toolchain.toml"),
        "[toolchain]\nchannel = \"mainnet\"\ncomponents = [\"assets\"]\n",
    )
    .unwrap();
    run(&env, &fixture.manifest_uri, &["update"]);
    assert!(env.midenup_home.join("toolchains/mainnet/etc/assets/config.yml").is_file());
}
