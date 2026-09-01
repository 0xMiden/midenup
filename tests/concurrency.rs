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
    // The `miden` path deliberately ignores `--midenup-home` and locates the home the way a
    // user's shell would, which is what the `_via_xdg` variant arranges.
    midenup_command_via_xdg(miden, env, manifest_uri)
        .args(["help", "vm"])
        .current_dir(project)
        // Piped so that a failure reports what the child said, including anything the component it
        // spawned wrote -- `execute_command` gives the component these same descriptors.
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
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
    let fixture = common::harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");

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
            "both invocations must succeed.\nstdout:\n{}\nstderr:\n{}\ntree:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(
                &Command::new("find")
                    .arg(env.tmp_dir.path())
                    .output()
                    .map(|o| o.stdout)
                    .unwrap_or_default()
            )
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

    // The publication the state record names is the one on disk. The one it replaced is left
    // behind for `midenup gc` -- deleting it here would pull the directory out from under whichever
    // process is executing a component from it, which is fatal.
    let midenup::state::PublicationRef::Managed { id, .. } = &installation.publication else {
        panic!("expected a managed publication");
    };
    assert!(
        midenup::paths::publication_dir(&env.midenup_home, &channel, id).is_dir(),
        "the recorded publication must exist"
    );
}

/// Dispatch against an installed toolchain must not touch the network.
///
/// The manifest URI points at a closed port, so any fetch fails loudly. `miden vm ...` answers from
/// `state.json` and the active publication, which is what makes it usable offline and what keeps a
/// network round trip out of every component invocation (spec section 13.1).
#[test]
fn integration_dispatch_against_an_installed_toolchain_makes_no_network_request() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("offline_dispatch");
    let fixture = common::harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");

    // Install with a reachable manifest...
    let install = midenup_command(env!("CARGO_BIN_EXE_midenup"), &env, &fixture.manifest_uri)
        .args(["install", "0.15.0"])
        .output()
        .expect("failed to run midenup");
    assert!(install.status.success(), "{}", String::from_utf8_lossy(&install.stderr));

    assert!(
        env.midenup_home.join("toolchains").join("0.15.0").exists(),
        "the toolchain must be installed before dispatch is tested"
    );

    // Remove the cached manifest the install left behind. Without it, *any* attempt to consult
    // upstream is fatal rather than quietly satisfied from disk -- which is the difference between
    // "does not need the network" and "does not need it to be up".
    std::fs::remove_file(midenup::paths::manifest_cache(&env.midenup_home)).unwrap();

    let bin = env.tmp_dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let miden = bin.join("miden");
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_midenup"), &miden).unwrap();

    let project = env.tmp_dir.path().join("project");
    write_toolchain_file(&project, &[]);

    // Nothing listens on that port: reaching for upstream would fail, loudly.
    let output = midenup_command_via_xdg(&miden, &env, "https://127.0.0.1:1/nope.json")
        .args(["help", "vm"])
        .current_dir(&project)
        .output()
        .expect("failed to run miden");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "dispatch must not need upstream.\nstdout:\n{}\nstderr:\n{stderr}",
        String::from_utf8_lossy(&output.stdout),
    );
    assert!(
        !stderr.contains("could not reach"),
        "dispatch must not even *attempt* a fetch: {stderr}"
    );
}

/// When an operation genuinely needs upstream and the fetch fails, the cached copy is used -- and
/// the staleness is reported rather than passed off as current.
#[test]
fn integration_an_operation_needing_upstream_falls_back_to_the_cached_manifest() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("cached_upstream");
    let fixture = common::harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");

    let midenup = |manifest_uri: &str, args: &[&str]| {
        midenup_command(env!("CARGO_BIN_EXE_midenup"), &env, manifest_uri)
            .args(args)
            .output()
            .expect("failed to run midenup")
    };

    let installed = midenup(&fixture.manifest_uri, &["install", "0.15.0"]);
    assert!(installed.status.success(), "{}", String::from_utf8_lossy(&installed.stderr));
    assert!(
        midenup::paths::manifest_cache(&env.midenup_home).exists(),
        "a successful fetch must be cached"
    );

    // `list` needs upstream by definition -- it lists what is published.
    let output = midenup("https://127.0.0.1:1/nope.json", &["list"]);
    assert!(
        output.status.success(),
        "the cache must let it proceed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cached"), "staleness must be reported: {stderr}");
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("0.15.0"),
        "and the cached content must actually be used"
    );
}
