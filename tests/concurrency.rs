//! Two `midenup` processes against one `MIDENUP_HOME`.
//!
//! These spawn the real binary rather than calling into the library, because the hazard is between
//! *processes*: the advisory lock is a `flock`, and an in-process test would only exercise the
//! Rust-level plumbing around it.

use std::{path::Path, process::Command};

mod common;

use common::*;

/// Runs the built binary as `miden`, from `project`.
///
/// `miden` is a multicall binary -- it decides what it is from `argv[0]` -- so it is reached
/// through a symlink named `miden`, exactly as `midenup init` sets up for real.
fn spawn_miden(
    miden: &Path,
    project: &Path,
    env: &TestEnvironment,
    manifest_uri: &str,
) -> std::process::Child {
    Command::new(miden)
        .args(["help", "vm"])
        .current_dir(project)
        // The `miden` path deliberately ignores `--midenup-home` and locates the home the way a
        // user's shell would.
        .env("XDG_DATA_HOME", env.tmp_dir.path())
        .env("CARGO_HOME", &env.cargo_home)
        .env("MIDENUP_MANIFEST_URI", manifest_uri)
        .spawn()
        .expect("failed to spawn miden")
}

fn write_toolchain_file(project: &Path, components: &[&str]) {
    std::fs::create_dir_all(project).unwrap();
    let components = components
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    std::fs::write(
        project.join("miden-toolchain.toml"),
        format!("[toolchain]\nchannel = \"0.15.0\"\ncomponents = [{components}]\n"),
    )
    .unwrap();
}

/// Two projects activating the same channel at the same time must converge on the union of what
/// they asked for.
///
/// This is the hazard section 9.9 exists for, and it needs no user error to reach: `miden <cmd>`
/// installs the current toolchain if it is missing, so two shells in two project directories are
/// two concurrent writers. Without the lock the loser's publication is orphaned and its state write
/// is lost, and which one loses varies per run.
#[test]
fn integration_concurrent_activations_converge_on_a_superset() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("concurrent_activations");
    let fixture = common::harness::OfflineFixture::build(env.tmp_dir.path(), "0.15.0");

    // A `miden` symlink to the binary under test.
    let bin = env.tmp_dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let miden = bin.join("miden");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_midenup"), &miden).unwrap();

    // One project wants only the default profile; the other additionally wants `assets`.
    let plain = env.tmp_dir.path().join("project-plain");
    let with_assets = env.tmp_dir.path().join("project-assets");
    write_toolchain_file(&plain, &[]);
    write_toolchain_file(&with_assets, &["assets"]);

    let first = spawn_miden(&miden, &plain, &env, &fixture.manifest_uri);
    let second = spawn_miden(&miden, &with_assets, &env, &fixture.manifest_uri);

    let outputs = [first, second].map(|child| child.wait_with_output().expect("failed to wait"));
    for output in &outputs {
        assert!(
            output.status.success(),
            "both invocations must succeed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let channel = semver::Version::new(0, 15, 0);
    let state =
        midenup::state::LocalState::load(&midenup::paths::state_path(&env.midenup_home)).unwrap();
    let installation = state.get(&channel).expect("the channel must be recorded as installed");

    assert!(
        installation.intent.roots.contains("assets"),
        "the second project's request must survive the first's write: {:?}",
        installation.intent
    );

    let names: Vec<_> = installation.components.iter().map(|c| c.name.to_string()).collect();
    assert!(
        names.contains(&"vm".to_string()) && names.contains(&"assets".to_string()),
        "both activations' components must be installed; got {names:?}"
    );

    // ...and the active toolchain must actually contain them, not merely claim to.
    let toolchain = midenup::paths::toolchain_link(&env.midenup_home, &channel);
    assert!(toolchain.join("bin").join("miden-vm").exists());
    assert!(toolchain.join("etc").join("assets").join("config.yml").exists());

    // Exactly one publication survives: the one the state record names.
    let midenup::state::PublicationRef::Managed { id, .. } = &installation.publication else {
        panic!("expected a managed publication");
    };
    let publications: Vec<_> =
        std::fs::read_dir(midenup::paths::publications_dir(&env.midenup_home))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
    assert_eq!(publications, vec![format!("0.15.0-{id}")]);
}
