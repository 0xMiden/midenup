//! The whole lifecycle, once, with the physical layout checked at every step.
//!
//! Every other suite pins one behaviour in isolation. This one asks a different question: after a
//! realistic sequence of operations -- install, activate from two projects, update in place, follow
//! a version bump, uninstall -- is what is on disk still exactly what `state.json` says it is?
//!
//! The invariant it checks after each step is the one the whole publication protocol exists to
//! maintain: the recorded publication exists, contains every file its receipt claims with the mode
//! it claims, is what `toolchains/<channel>` resolves to, and nothing is left in flight.

use std::path::{Path, PathBuf};

use clap::Parser;
use midenup::{
    commands::Midenup,
    paths,
    state::{LocalState, PublicationRef},
};

mod common;

use common::*;

/// A two-channel fixture: 0.15.0, and a 0.16.0 that supersedes it.
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

    fn executable(&self, name: &str, profiles: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "version": {"kind": "registry", "version": "0.1.0"},
            "kind": "executable",
            "installation_method": {"kind": "prebuilt"},
            "installed-executable": format!("miden-{name}"),
            "profiles": profiles,
            "artifacts": {
                format!("miden-{name}"): {
                    "uri": format!("file://{}", self.dir.join("miden-vm").display())
                }
            }
        })
    }

    fn package(&self, name: &str, profiles: &[&str]) -> serde_json::Value {
        let artifact = self.dir.join(format!("{name}.masp"));
        std::fs::write(&artifact, format!("{name}\n")).unwrap();

        serde_json::json!({
            "name": name,
            "version": {"kind": "registry", "version": "0.1.0"},
            "kind": "package",
            "profiles": profiles,
            "artifacts": {
                format!("{name}.masp"): {"uri": format!("file://{}", artifact.display())}
            }
        })
    }

    fn components(&self) -> Vec<serde_json::Value> {
        vec![
            self.executable("vm", &["minimal"]),
            self.package("core", &["minimal"]),
            self.executable("debug", &[]),
            self.executable("client", &[]),
        ]
    }

    /// Only 0.15.0 exists.
    fn initial(&self) -> String {
        self.write(
            "manifest-1.json",
            serde_json::json!([{
                "name": "0.15.0",
                "components": self.components()
            }]),
        )
    }

    /// 0.15.0 is withdrawn and 0.16.0 supersedes it.
    ///
    /// The old channel has to be *gone* for this to be a migration: a channel that still exists
    /// upstream is not migrated away from, whatever another channel claims to supersede (11.4).
    fn with_successor(&self) -> String {
        self.write(
            "manifest-2.json",
            serde_json::json!([{
                "name": "0.16.0",
                "migrates_from": "0.15.0",
                "components": self.components()
            }]),
        )
    }

    fn write(&self, file: &str, channels: serde_json::Value) -> String {
        let manifest = serde_json::json!({
            "manifest_version": "2.0.0",
            "date": 1735689600,
            "channels": channels
        });
        let path = self.dir.join(file);
        std::fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
        format!("file://{}", path.display())
    }
}

fn project(env: &TestEnvironment, name: &str, channel: &str, components: &[&str]) -> PathBuf {
    let dir = env.tmp_dir.path().join(name);
    std::fs::create_dir_all(&dir).unwrap();

    let components = components
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        dir.join("miden-toolchain.toml"),
        format!("[toolchain]\nchannel = \"{channel}\"\ncomponents = [{components}]\n"),
    )
    .unwrap();

    dir
}

fn state_of(env: &TestEnvironment) -> LocalState {
    LocalState::load(&paths::state_path(&env.midenup_home)).expect("state must be readable")
}

fn installed(state: &LocalState, channel: &semver::Version) -> Vec<String> {
    let mut names: Vec<String> = state
        .get(channel)
        .map(|installation| installation.components.iter().map(|c| c.name.to_string()).collect())
        .unwrap_or_default();
    names.sort();
    names
}

/// The invariant every step must leave intact.
///
/// The recorded publication exists; every file its receipt claims is there, is a regular file, and
/// carries the recorded mode; `toolchains/<channel>` resolves to it; and no operation is still in
/// flight.
fn assert_publication_consistent(env: &TestEnvironment, channel: &semver::Version, step: &str) {
    let state = state_of(env);
    let installation = state
        .get(channel)
        .unwrap_or_else(|| panic!("{step}: {channel} must be installed"));

    let PublicationRef::Managed { id, .. } = &installation.publication else {
        panic!("{step}: {channel} must reference a publication this build owns");
    };
    let publication = paths::publication_dir(&env.midenup_home, channel, id);

    let receipt = midenup::publish::read_receipt(&publication)
        .unwrap_or_else(|err| panic!("{step}: the publication must describe itself: {err}"));
    assert_eq!(receipt.publication_id, *id, "{step}: the receipt must be this publication's");

    for output in &receipt.outputs {
        let path = publication.join(&output.path);
        let metadata = std::fs::symlink_metadata(&path)
            .unwrap_or_else(|_| panic!("{step}: {} is claimed but missing", output.path.display()));
        assert!(metadata.is_file(), "{step}: {} is not a regular file", output.path.display());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                metadata.permissions().mode() & 0o777,
                output.mode,
                "{step}: {} has the wrong mode",
                output.path.display()
            );
        }
    }

    let link = paths::toolchain_link(&env.midenup_home, channel);
    assert_eq!(
        std::fs::canonicalize(&link).unwrap_or_else(|err| panic!("{step}: {err}")),
        std::fs::canonicalize(&publication).unwrap(),
        "{step}: the toolchain link must resolve to the recorded publication"
    );

    assert!(
        midenup::publish::journal::read(&env.midenup_home).unwrap().is_none(),
        "{step}: no operation may still be in flight"
    );

    // Every installed component the receipt covers is also in the recorded snapshot, which is what
    // `miden` dispatches against offline.
    for output in &receipt.outputs {
        assert!(
            installation.components.iter().any(|component| component.name == output.owner),
            "{step}: '{}' owns installed files but is not in the recorded snapshot",
            output.owner
        );
    }
}

#[test]
fn integration_end_to_end_lifecycle() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("end_to_end");
    let fixture = Fixture::new(env.tmp_dir.path());

    let first = semver::Version::new(0, 15, 0);
    let second = semver::Version::new(0, 16, 0);

    // 1. A plain install of the minimal profile.
    let manifest = fixture.initial();
    let (mut state, config) = test_setup(&env, &manifest);
    Midenup::try_parse_from(["midenup", "install", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install");

    assert_eq!(installed(&state_of(&env), &first), vec!["core", "vm"]);
    assert_publication_consistent(&env, &first, "after install");

    // 2. Two projects activate the same channel, each wanting one component the other does not.
    for (name, component) in [("project-a", "debug"), ("project-b", "client")] {
        let dir = project(&env, name, "0.15.0", &[component]);
        let config = midenup::config::Config::init(
            dir.clone(),
            env.midenup_home.clone(),
            env.cargo_home.clone(),
            &manifest,
            true,
        )
        .unwrap();

        Midenup::try_parse_from(["miden", "help", "vm"])
            .unwrap()
            .execute_with_state(&config, &mut state)
            .unwrap_or_else(|err| panic!("activation of {name} failed: {err}"));
    }

    assert_eq!(
        installed(&state_of(&env), &first),
        vec!["client", "core", "debug", "vm"],
        "activation is additive in both directions"
    );
    assert_publication_consistent(&env, &first, "after two activations");

    // 3. An in-place update of the same channel.
    Midenup::try_parse_from(["midenup", "update", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to update");
    assert_publication_consistent(&env, &first, "after update");

    // 4. A successor channel is published, and the installation follows it.
    let manifest = fixture.with_successor();
    let (_, config) = test_setup(&env, &manifest);
    Midenup::try_parse_from(["midenup", "update"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to migrate");

    let state = state_of(&env);
    assert!(state.get(&first).is_none(), "the superseded channel must be gone");
    assert_eq!(
        installed(&state, &second),
        vec!["client", "core", "debug", "vm"],
        "intent transfers verbatim across a migration"
    );
    assert_publication_consistent(&env, &second, "after migration");

    // 5. Reclaim what the sequence left behind, then remove the channel entirely.
    Midenup::try_parse_from(["midenup", "gc"])
        .unwrap()
        .execute_with_state(&config, &mut state_of(&env))
        .expect("gc failed");
    assert_publication_consistent(&env, &second, "after gc");

    Midenup::try_parse_from(["midenup", "uninstall", "0.16.0"])
        .unwrap()
        .execute_with_state(&config, &mut state_of(&env))
        .expect("uninstall failed");

    assert!(
        state_of(&env).installations.is_empty(),
        "nothing may remain recorded after the last channel is removed"
    );

    let leftovers: Vec<_> = std::fs::read_dir(paths::publications_dir(&env.midenup_home))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(leftovers.is_empty(), "no publication may outlive its channel: {leftovers:?}");
}
