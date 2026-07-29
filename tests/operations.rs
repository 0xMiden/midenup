//! Install, activation and update as *one* operation with three intent policies.
//!
//! Everything here is a property of resolution against a manifest, so the components are `file://`
//! stand-ins: one executable, so activation has something to run, and assets for the rest.

use std::path::{Path, PathBuf};

use clap::Parser;
use midenup::{commands::Midenup, config::Config, state::LocalState};

mod common;

use common::*;

/// One component in a fixture manifest: name, profiles, requirements.
type Spec<'a> = (&'a str, &'a [&'a str], &'a [&'a str]);

/// Local artifacts plus a manifest writer, so a test can describe a channel in one line.
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(root: &Path) -> Self {
        let dir = root.join("fixture");
        std::fs::create_dir_all(&dir).unwrap();

        let vm = dir.join("miden-vm");
        std::fs::write(&vm, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&vm, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        Self { dir }
    }

    fn artifact(&self, name: &str) -> String {
        let path = self.dir.join(format!("{name}.txt"));
        if !path.exists() {
            std::fs::write(&path, format!("{name}\n")).unwrap();
        }
        format!("file://{}", path.display())
    }

    fn component(&self, (name, profiles, requires): Spec<'_>) -> serde_json::Value {
        // `vm` is the one executable, so `miden vm` has something to run.
        if name == "vm" {
            return serde_json::json!({
                "name": "vm",
                "version": {"kind": "registry", "version": "0.1.0"},
                "kind": "executable",
                "installation_method": {"kind": "prebuilt"},
                "installed-executable": "miden-vm",
                "profiles": profiles,
                "requires": requires,
                "artifacts": {
                    "miden-vm": {"uri": format!("file://{}", self.dir.join("miden-vm").display())}
                }
            });
        }

        serde_json::json!({
            "name": name,
            "version": {"kind": "registry", "version": "0.1.0"},
            "kind": "asset",
            "profiles": profiles,
            "requires": requires,
            "artifacts": {format!("{name}.txt"): {"uri": self.artifact(name)}}
        })
    }

    /// Writes a one-channel manifest and returns its URI.
    fn manifest(&self, file: &str, components: &[Spec<'_>]) -> String {
        let manifest = serde_json::json!({
            "manifest_version": "2.0.0",
            "date": 1735689600,
            "channels": [{
                "name": "0.15.0",
                "components": components.iter().map(|spec| self.component(*spec)).collect::<Vec<_>>()
            }]
        });

        let path = self.dir.join(file);
        std::fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
        format!("file://{}", path.display())
    }
}

/// A `Config` rooted in `project`, so `miden-toolchain.toml` discovery finds that project's file.
fn config_in(env: &TestEnvironment, project: &Path, manifest_uri: &str) -> Config {
    Config::init(
        project.to_path_buf(),
        env.midenup_home.clone(),
        env.cargo_home.clone(),
        manifest_uri,
        true,
    )
    .expect("failed to build config")
}

fn project(env: &TestEnvironment, name: &str, components: &[&str]) -> PathBuf {
    let dir = env.tmp_dir.path().join(name);
    std::fs::create_dir_all(&dir).unwrap();

    let components = components
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        dir.join("miden-toolchain.toml"),
        format!("[toolchain]\nchannel = \"0.15.0\"\ncomponents = [{components}]\n"),
    )
    .unwrap();

    dir
}

/// Runs `miden help vm` from `project`, which activates that project's toolchain.
fn activate(env: &TestEnvironment, project: &Path, manifest_uri: &str, state: &mut LocalState) {
    let config = config_in(env, project, manifest_uri);
    Midenup::try_parse_from(["miden", "help", "vm"])
        .unwrap()
        .execute_with_state(&config, state)
        .expect("activation failed");
}

fn installed(state: &LocalState) -> Vec<String> {
    let mut names: Vec<String> = state
        .get(&semver::Version::new(0, 15, 0))
        .map(|installation| installation.components.iter().map(|c| c.name.to_string()).collect())
        .unwrap_or_default();
    names.sort();
    names
}

/// Activating one project must never take away what another project asked for.
///
/// Regression: activation installed a *narrowed* channel, so switching between two projects
/// alternately uninstalled each other's components.
#[test]
fn integration_switching_between_two_projects_is_additive_in_both_directions() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("two_projects");
    let fixture = Fixture::new(env.tmp_dir.path());
    let manifest = fixture.manifest(
        "manifest.json",
        &[("vm", &["minimal"], &[]), ("debug", &[], &[]), ("client", &[], &[])],
    );

    let first = project(&env, "project-a", &["debug"]);
    let second = project(&env, "project-b", &["client"]);
    let mut state = LocalState::default();

    activate(&env, &first, &manifest, &mut state);
    assert_eq!(installed(&state), vec!["debug", "vm"]);

    activate(&env, &second, &manifest, &mut state);
    assert_eq!(installed(&state), vec!["client", "debug", "vm"], "activation must add");

    activate(&env, &first, &manifest, &mut state);
    assert_eq!(
        installed(&state),
        vec!["client", "debug", "vm"],
        "switching back must not remove the other project's component"
    );
}

/// A direct install is the documented way to shrink a channel back to a known set -- and a project
/// that still wants more gets it back on its next activation.
#[test]
fn integration_direct_install_can_shrink_and_activation_re_adds() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("shrink_then_readd");
    let fixture = Fixture::new(env.tmp_dir.path());
    let manifest =
        fixture.manifest("manifest.json", &[("vm", &["minimal"], &[]), ("debug", &[], &[])]);

    let dir = project(&env, "project-a", &["debug"]);
    let mut state = LocalState::default();

    activate(&env, &dir, &manifest, &mut state);
    assert_eq!(installed(&state), vec!["debug", "vm"]);

    let config = config_in(&env, env.tmp_dir.path(), &manifest);
    Midenup::try_parse_from(["midenup", "install", "0.15.0", "--profile", "minimal"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install");
    assert_eq!(
        installed(&state),
        vec!["vm"],
        "a direct install replaces intent, and may shrink"
    );

    activate(&env, &dir, &manifest, &mut state);
    assert_eq!(installed(&state), vec!["debug", "vm"], "the project's request is re-added");
}

/// `profiles` are re-resolved on update, so a `minimal` installation gains a component newly
/// tagged `minimal` upstream.
///
/// Regression: `update stable` intersected the new channel with the installed component names, so
/// a component that did not exist locally could never appear.
#[test]
fn integration_a_minimal_installation_receives_newly_profiled_components() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("new_profile_members");
    let fixture = Fixture::new(env.tmp_dir.path());

    let before = fixture.manifest("before.json", &[("vm", &["minimal"], &[])]);
    let (mut state, config) = test_setup(&env, &before);
    Midenup::try_parse_from(["midenup", "install", "0.15.0", "--profile", "minimal"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install");
    assert_eq!(installed(&state), vec!["vm"]);

    let after = fixture
        .manifest("after.json", &[("vm", &["minimal"], &[]), ("newthing", &["minimal"], &[])]);
    let (_, config) = test_setup(&env, &after);
    Midenup::try_parse_from(["midenup", "update", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to update");

    assert_eq!(installed(&state), vec!["newthing", "vm"]);
}

/// A roots-only installation gains new *dependencies* of its roots, but not unrelated components
/// that merely joined a profile it never asked for.
#[test]
fn integration_a_roots_only_installation_gains_dependencies_but_not_unrelated_members() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("roots_only_update");
    let fixture = Fixture::new(env.tmp_dir.path());

    let before =
        fixture.manifest("before.json", &[("vm", &["minimal"], &[]), ("client", &[], &[])]);
    let (mut state, config) = test_setup(&env, &before);
    Midenup::try_parse_from([
        "midenup",
        "install",
        "0.15.0",
        "--profile",
        "empty",
        "--component",
        "client",
    ])
    .unwrap()
    .execute_with_state(&config, &mut state)
    .expect("failed to install");
    assert_eq!(installed(&state), vec!["client"]);

    // `client` gains a dependency; `unrelated` appears in the `minimal` profile, which this
    // installation never asked for.
    let after = fixture.manifest(
        "after.json",
        &[
            ("vm", &["minimal"], &[]),
            ("client", &[], &["newdep"]),
            ("newdep", &[], &[]),
            ("unrelated", &["minimal"], &[]),
        ],
    );
    let (_, config) = test_setup(&env, &after);
    Midenup::try_parse_from(["midenup", "update", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to update");

    assert_eq!(installed(&state), vec!["client", "newdep"]);
}

/// An explicit root that no longer exists upstream blocks the update, and the installation is left
/// exactly as it was. The schema has no rename declaration, so there is nothing to guess with.
#[test]
fn integration_a_removed_root_blocks_the_update_and_preserves_the_installation() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("removed_root");
    let fixture = Fixture::new(env.tmp_dir.path());

    let before =
        fixture.manifest("before.json", &[("vm", &["minimal"], &[]), ("goingaway", &[], &[])]);
    let (mut state, config) = test_setup(&env, &before);
    Midenup::try_parse_from([
        "midenup",
        "install",
        "0.15.0",
        "--profile",
        "empty",
        "--component",
        "goingaway",
    ])
    .unwrap()
    .execute_with_state(&config, &mut state)
    .expect("failed to install");
    assert_eq!(installed(&state), vec!["goingaway"]);

    let after = fixture.manifest("after.json", &[("vm", &["minimal"], &[])]);
    let (_, config) = test_setup(&env, &after);
    let err = Midenup::try_parse_from(["midenup", "update", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect_err("an update that cannot be resolved must not proceed");

    assert!(
        format!("{err:#}").contains("goingaway"),
        "the error must name the component that disappeared: {err:#}"
    );
    assert_eq!(
        installed(&LocalState::load(&midenup::paths::state_path(&env.midenup_home)).unwrap()),
        vec!["goingaway"],
        "a blocked update must leave the installation exactly as it was"
    );
}
