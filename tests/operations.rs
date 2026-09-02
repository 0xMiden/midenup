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

    /// As [`Fixture::manifest`], with an extra alias declared on `vm`.
    ///
    /// An alias is the canonical *runtime-metadata-only* change: recorded in local state, resolved
    /// at dispatch, reflected in no installed file.
    fn manifest_with_vm_alias(&self, file: &str, components: &[Spec<'_>], alias: &str) -> String {
        let mut components: Vec<serde_json::Value> =
            components.iter().map(|spec| self.component(*spec)).collect();
        for component in components.iter_mut() {
            if component["name"] == serde_json::json!("vm") {
                component["aliases"] = serde_json::json!({alias: ["%installed-executable", "run"]});
            }
        }
        self.write(file, components)
    }

    /// A `command` component with a `format` prefix and two subcommands, plus the asset its
    /// format refers to.
    fn manifest_with_command(&self, file: &str) -> String {
        let compose = self.dir.join("docker-compose.yml");
        std::fs::write(&compose, "services: {}\n").unwrap();

        let node = serde_json::json!({
            "name": "node",
            "version": {"kind": "registry", "version": "0.1.0"},
            "kind": "command",
            "profiles": ["minimal"],
            "format": ["docker", "compose", "-f", "%etc(node/docker-compose.yml)"],
            "subcommands": {
                "up": ["up", "-d"],
                "down": ["down"]
            },
            "artifacts": {
                "docker-compose.yml": {"uri": format!("file://{}", compose.display())}
            }
        });

        self.write(file, vec![self.component(("vm", &["minimal"], &[])), node])
    }

    /// Two executables that both declare the alias `run`, neither in any profile.
    ///
    /// Nothing installs them together by default, which is the situation section 8.5 is about: a
    /// superset that accreted both from different projects.
    fn manifest_with_conflicting_aliases(&self, file: &str) -> String {
        let executable = |name: &str| {
            let binary = self.dir.join(format!("miden-{name}"));
            std::fs::write(&binary, "#!/bin/sh\nexit 0\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
            }

            serde_json::json!({
                "name": name,
                "version": {"kind": "registry", "version": "0.1.0"},
                "kind": "executable",
                "installation_method": {"kind": "prebuilt"},
                "installed-executable": format!("miden-{name}"),
                "profiles": [],
                "aliases": {"run": ["%installed-executable", "run"]},
                "artifacts": {
                    format!("miden-{name}"): {"uri": format!("file://{}", binary.display())}
                }
            })
        };

        self.write(
            file,
            vec![
                self.component(("vm", &["minimal"], &[])),
                executable("first"),
                executable("second"),
            ],
        )
    }

    /// Writes a one-channel manifest and returns its URI.
    fn manifest(&self, file: &str, components: &[Spec<'_>]) -> String {
        self.write(file, components.iter().map(|spec| self.component(*spec)).collect())
    }

    /// A manifest whose only channel supersedes `0.15.0` via `migrates_from`, so an installation
    /// of 0.15.0 has no same-version counterpart upstream.
    fn manifest_migrated(&self, file: &str, components: &[Spec<'_>]) -> String {
        let manifest = serde_json::json!({
            "manifest_version": "3.0.0",
            "date": 1735689600,
            "networks": {"mainnet": "0.16.0"},
            "channels": [{
                "name": "0.16.0",
                "migrates_from": "0.15.0",
                "components": components
                    .iter()
                    .map(|spec| self.component(*spec))
                    .collect::<Vec<_>>()
            }]
        });

        let path = self.dir.join(file);
        std::fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
        format!("file://{}", path.display())
    }

    fn write(&self, file: &str, components: Vec<serde_json::Value>) -> String {
        let manifest = serde_json::json!({
            "manifest_version": "3.0.0",
            "date": 1735689600,
            "networks": {"mainnet": "0.15.0"},
            "channels": [{
                "name": "0.15.0",
                "components": components
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
/// Activation resolves against the union of recorded intent rather than against a channel narrowed
/// to the activating project, so switching back and forth adds and never removes.
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
/// `update stable` resolves the channel as upstream defines it; it is not narrowed to the set of
/// component names already installed locally, so a component with no local counterpart can appear.
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

/// Spec section 9.8: a change that no installed file reflects is committed as a single atomic
/// `state.json` write -- no journal, no staging, no new publication.
///
/// Adding an alias to a component does not force a reinstall: `is_up_to_date` compares the fields
/// that decide what gets installed, not the component's kind wholesale.
#[test]
fn integration_a_metadata_only_change_updates_state_without_republishing() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("logical_only_update");
    let fixture = Fixture::new(env.tmp_dir.path());
    let channel = semver::Version::new(0, 15, 0);

    let before = fixture.manifest("before.json", &[("vm", &["minimal"], &[])]);
    let (mut state, config) = test_setup(&env, &before);
    Midenup::try_parse_from(["midenup", "install", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install");

    let publication_of = |state: &LocalState| match &state.get(&channel).unwrap().publication {
        midenup::state::PublicationRef::Managed { id, .. } => id.clone(),
        other => panic!("expected a managed publication, got {other:?}"),
    };
    let before_publication = publication_of(&state);

    // Upstream adds an alias, and changes nothing else.
    let after = fixture.manifest_with_vm_alias("after.json", &[("vm", &["minimal"], &[])], "run");
    let (_, config) = test_setup(&env, &after);
    Midenup::try_parse_from(["midenup", "update", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to update");

    let reloaded = LocalState::load(&midenup::paths::state_path(&env.midenup_home)).unwrap();
    assert!(
        reloaded.get(&channel).unwrap().as_channel().get_alias_names().contains("run"),
        "the new alias must be recorded"
    );
    assert_eq!(
        publication_of(&reloaded),
        before_publication,
        "a metadata-only change must not produce a new publication"
    );

    let publications: Vec<_> =
        std::fs::read_dir(midenup::paths::publications_dir(&env.midenup_home))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
    assert_eq!(publications.len(), 1, "nothing was staged, so nothing was published");
}

/// Naming a subcommand a component does not declare must say which ones it does.
///
/// Spec section 13.3: `argv[1]` must name a declared subcommand, and the error lists the valid
/// ones -- a bare "invalid subcommand" leaves the user to go read the manifest.
#[test]
fn integration_an_invalid_subcommand_lists_the_valid_ones() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("invalid_subcommand");
    let fixture = Fixture::new(env.tmp_dir.path());
    let manifest = fixture.manifest_with_command("manifest.json");

    let (mut state, config) = test_setup(&env, &manifest);
    Midenup::try_parse_from(["midenup", "install", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install");

    let err = Midenup::try_parse_from(["miden", "node", "bogus"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect_err("an undeclared subcommand must not be passed through");

    let message = format!("{err:#}");
    assert!(message.contains("bogus"), "the error must name what was asked for: {message}");
    assert!(
        message.contains("up") && message.contains("down"),
        "and list what is available: {message}"
    );
}

/// A declared subcommand expands to its own words, with the component's `format` in front.
///
/// The map is consulted against the raw argument rather than through clap's `matches.subcommand()`,
/// which is always `None` for an external subcommand: going through clap leaves `miden node up`
/// passing `up` on as a literal argument, and a component whose `format` is empty -- the shipped
/// `node` among them -- then tries to execute it as a program.
#[test]
fn integration_a_declared_subcommand_expands_to_its_own_words() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("subcommand_expansion");
    let fixture = Fixture::new(env.tmp_dir.path());
    let manifest = fixture.manifest_with_command("manifest.json");

    let (mut state, config) = test_setup(&env, &manifest);
    Midenup::try_parse_from(["midenup", "install", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install");

    // `docker` is not installed here, and that is the point: the failure names what midenup tried
    // to run, which is what proves the expansion happened.
    let err = Midenup::try_parse_from(["miden", "node", "up"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect_err("docker is not available in the test environment");

    let message = format!("{err:#}");
    assert!(
        message.contains("miden node up"),
        "the failure must name the user's command: {message}"
    );

    // ...and asking for help on it lists the verbs rather than trying to run one.
    Midenup::try_parse_from(["miden", "help", "node"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("`miden help <command>` must list its subcommands, not fail");
}

/// A conflict that exists only in the superset must not break every command.
///
/// The installed set accretes components from every project that ever activated the channel, so two
/// components no project uses together could otherwise make `miden <anything>` fail. Section 8.5:
/// that is a warning, and the component in the active view wins.
#[test]
fn integration_a_superset_only_alias_conflict_does_not_break_every_command() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("alias_conflict");
    let fixture = Fixture::new(env.tmp_dir.path());
    let manifest = fixture.manifest_with_conflicting_aliases("manifest.json");

    // Install both, as two projects asking for one each would have.
    let (mut state, config) = test_setup(&env, &manifest);
    Midenup::try_parse_from(["midenup", "install", "0.15.0", "--profile", "complete"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install");
    assert_eq!(installed(&state), vec!["first", "second", "vm"]);

    // This project wants only `first`, so only one definition of `run` is in view.
    let dir = project(&env, "project-a", &["first"]);
    let config = config_in(&env, &dir, &manifest);

    Midenup::try_parse_from(["miden", "help", "vm"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("a superset-only conflict must not be fatal");

    Midenup::try_parse_from(["miden", "run"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("and the alias must resolve to the component in the active view");
}

/// A conflict *within* the active view is a real ambiguity: this project asked for both, and
/// `miden run` has no defensible answer.
#[test]
fn integration_an_alias_conflict_inside_the_active_view_is_an_error() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("alias_conflict_in_view");
    let fixture = Fixture::new(env.tmp_dir.path());
    let manifest = fixture.manifest_with_conflicting_aliases("manifest.json");

    let (mut state, config) = test_setup(&env, &manifest);
    Midenup::try_parse_from(["midenup", "install", "0.15.0", "--profile", "complete"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install");

    let dir = project(&env, "project-both", &["first", "second"]);
    let config = config_in(&env, &dir, &manifest);

    let err = Midenup::try_parse_from(["miden", "run"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect_err("an ambiguous alias in the active view must be reported");

    let message = format!("{err:#}");
    assert!(message.contains("run"), "the error must name the alias: {message}");
    assert!(
        message.contains("first") && message.contains("second"),
        "and both components that define it: {message}"
    );
}

/// Runs the midenup binary against `env` and `manifest_uri`.
fn midenup_run(env: &TestEnvironment, manifest_uri: &str, args: &[&str]) -> std::process::Output {
    midenup_command(env!("CARGO_BIN_EXE_midenup"), env, manifest_uri)
        .args(args)
        .output()
        .expect("failed to run midenup")
}

/// Spec section 8.6: local state carries no update-available flag, and with no manifest at all the
/// status is omitted.
#[test]
fn integration_update_status_is_not_stored() {
    let _guard = common::harness::mutating_test_guard();
    // Not named "update": the temp directory path ends up inside state.json, in artifact URIs.
    let env = environment_setup("derived_clean");
    let fixture = Fixture::new(env.tmp_dir.path());
    let manifest =
        fixture.manifest("manifest.json", &[("vm", &["minimal"], &[]), ("extra", &[], &[])]);

    let installed = midenup_run(&env, &manifest, &["install", "0.15.0", "--profile", "minimal"]);
    assert!(installed.status.success(), "{}", String::from_utf8_lossy(&installed.stderr));

    let raw = std::fs::read_to_string(midenup::paths::state_path(&env.midenup_home)).unwrap();
    assert!(!raw.contains("update"), "state must not persist an update flag: {raw}");

    // `extra` belongs to no profile, so a minimal install that never asked for it is up to date.
    let complete = midenup_run(&env, &manifest, &["show", "list"]);
    assert!(
        !String::from_utf8_lossy(&complete.stdout).contains("update available"),
        "an installation an update would not change has no update: {}",
        String::from_utf8_lossy(&complete.stdout)
    );

    // With upstream unavailable it is not shown. The cached manifest would answer, so it goes too.
    std::fs::remove_file(midenup::paths::manifest_cache(&env.midenup_home)).unwrap();
    let offline = midenup_run(&env, "https://127.0.0.1:1/nope.json", &["show", "list"]);
    assert!(offline.status.success(), "listing what is installed must work offline");

    let stdout = String::from_utf8_lossy(&offline.stdout);
    assert!(
        stdout.contains("0.15.0"),
        "the installed channel must still be listed: {stdout}"
    );
    assert!(
        !stdout.contains("update available"),
        "but its update status must not be: {stdout}"
    );
    assert!(!stdout.contains("mainnet"), "nor the networks naming it: {stdout}");
}

/// A definition change to a held component -- an alias, the canonical metadata-only change -- is
/// an update even though the component set is unchanged, in `show list` and `list` alike.
#[test]
fn integration_update_status_shows_definition_changes() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("derived_defchange");
    let fixture = Fixture::new(env.tmp_dir.path());
    let manifest =
        fixture.manifest("manifest.json", &[("vm", &["minimal"], &[]), ("extra", &[], &[])]);

    let installed = midenup_run(&env, &manifest, &["install", "0.15.0", "--profile", "minimal"]);
    assert!(installed.status.success(), "{}", String::from_utf8_lossy(&installed.stderr));

    let unchanged = midenup_run(&env, &manifest, &["list"]);
    assert!(
        String::from_utf8_lossy(&unchanged.stdout).contains("(installed)"),
        "an unchanged installation lists as installed: {}",
        String::from_utf8_lossy(&unchanged.stdout)
    );

    let aliased = fixture.manifest_with_vm_alias(
        "aliased.json",
        &[("vm", &["minimal"], &[]), ("extra", &[], &[])],
        "vm-alias",
    );
    for command in [["show", "list"].as_slice(), ["list"].as_slice()] {
        let shown = midenup_run(&env, &aliased, command);
        assert!(
            String::from_utf8_lossy(&shown.stdout).contains("update available"),
            "a changed component definition upstream must be shown by {command:?}: {}",
            String::from_utf8_lossy(&shown.stdout)
        );
    }
}

/// The minimal profile grows upstream, so the same intent resolves to more than is held.
#[test]
fn integration_update_status_shows_profile_growth() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("derived_growth");
    let fixture = Fixture::new(env.tmp_dir.path());
    let manifest =
        fixture.manifest("manifest.json", &[("vm", &["minimal"], &[]), ("extra", &[], &[])]);

    let installed = midenup_run(&env, &manifest, &["install", "0.15.0", "--profile", "minimal"]);
    assert!(installed.status.success(), "{}", String::from_utf8_lossy(&installed.stderr));

    let grown =
        fixture.manifest("grown.json", &[("vm", &["minimal"], &[]), ("extra", &["minimal"], &[])]);
    let shown = midenup_run(&env, &grown, &["show", "list"]);
    assert!(
        String::from_utf8_lossy(&shown.stdout).contains("update available"),
        "with upstream available it must be derived and shown: {}",
        String::from_utf8_lossy(&shown.stdout)
    );
    assert!(
        String::from_utf8_lossy(&shown.stdout).contains("(mainnet"),
        "and so must the networks naming the channel: {}",
        String::from_utf8_lossy(&shown.stdout)
    );
}

/// A superseded channel: the update *is* the migration, so both listings show it as updatable
/// rather than as unavailable or absent.
#[test]
fn integration_update_status_follows_migration_lineage() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("derived_lineage");
    let fixture = Fixture::new(env.tmp_dir.path());
    let manifest = fixture.manifest("manifest.json", &[("vm", &["minimal"], &[])]);

    let installed = midenup_run(&env, &manifest, &["install", "0.15.0", "--profile", "minimal"]);
    assert!(installed.status.success(), "{}", String::from_utf8_lossy(&installed.stderr));

    let migrated = fixture.manifest_migrated("migrated.json", &[("vm", &["minimal"], &[])]);
    let shown = midenup_run(&env, &migrated, &["show", "list"]);
    assert!(
        String::from_utf8_lossy(&shown.stdout).contains("update available"),
        "a superseded channel must show as updatable: {}",
        String::from_utf8_lossy(&shown.stdout)
    );

    // `list` shows the successor channel, marked because updating the predecessor lands on it.
    let listed = midenup_run(&env, &migrated, &["list"]);
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(
        stdout.contains("0.16.0") && stdout.contains("update available"),
        "the successor must be listed as the pending update: {stdout}"
    );
}

/// An explicit root removed upstream makes the intent unresolvable, which still shows as an
/// update: running it reports the missing root (spec section 11.3) rather than dropping it.
#[test]
fn integration_update_status_shows_an_unresolvable_intent() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("derived_dangling");
    let fixture = Fixture::new(env.tmp_dir.path());
    let manifest =
        fixture.manifest("manifest.json", &[("vm", &["minimal"], &[]), ("extra", &[], &[])]);

    let installed = midenup_run(
        &env,
        &manifest,
        &["install", "0.15.0", "--profile", "minimal", "--component", "extra"],
    );
    assert!(installed.status.success(), "{}", String::from_utf8_lossy(&installed.stderr));

    let shrunk = fixture.manifest("shrunk.json", &[("vm", &["minimal"], &[])]);
    let shown = midenup_run(&env, &shrunk, &["show", "list"]);
    let stdout = String::from_utf8_lossy(&shown.stdout);
    assert!(
        stdout.contains("update available"),
        "an unresolvable intent must be shown as an update: {stdout}"
    );
}
