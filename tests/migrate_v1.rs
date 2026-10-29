//! Homes holding what an alpha midenup's local manifest left behind.
//!
//! An alpha home -- a `manifest.json` and no installations in `state.json` -- is refused without
//! changes. A released v1.x home may still hold the records v1.x carried forward from one, which
//! describe no publication, beside the alpha toolchain directory; converting the home removes the
//! directory, and the toolchain is reinstalled on first use.

use std::path::Path;

use midenup::{
    paths,
    resolve::Intent,
    state::{Installation, LocalState, PublicationRef},
};

mod common;

use common::*;

/// An upstream that cannot be reached: nothing listens here.
const UNREACHABLE_UPSTREAM: &str = "https://127.0.0.1:1/nope.json";

fn v1_manifest(version: &str, channel: &str, components: &[&str]) -> String {
    let components: Vec<_> = components
        .iter()
        .map(|name| {
            serde_json::json!({
                "name": name,
                "version": "0.1.0",
                "installed_executable": format!("miden-{name}"),
            })
        })
        .collect();

    serde_json::json!({
        "manifest_version": version,
        "date": 1735689600,
        "channels": [{"name": channel, "components": components}]
    })
    .to_string()
}

/// A home as an alpha midenup left it: its local manifest, the `stable` link and the toolchain
/// tree.
fn write_alpha_home(env: &TestEnvironment) {
    let toolchains = paths::toolchains_dir(&env.midenup_home);
    std::fs::create_dir_all(toolchains.join("0.15.0").join("bin")).unwrap();
    std::os::unix::fs::symlink("0.15.0", toolchains.join("stable")).unwrap();
    std::fs::write(
        env.midenup_home.join("manifest.json"),
        v1_manifest("1.0.1", "0.15.0", &["vm", "client"]),
    )
    .unwrap();
}

/// A released v1.x home holding the record v1.x carried forward from an alpha one.
fn write_carried_forward_home(env: &TestEnvironment, channel: &str, roots: &[&str]) {
    // v1.x left the alpha toolchain directory in place.
    let alpha = paths::toolchains_dir(&env.midenup_home).join(channel).join("bin");
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::write(alpha.join("miden-vm"), "alpha").unwrap();
    std::fs::create_dir_all(paths::publications_dir(&env.midenup_home)).unwrap();
    let mut state = LocalState::default();
    state.upsert(Installation {
        channel: channel.parse().unwrap(),
        custom: None,
        intent: Intent {
            profiles: Default::default(),
            roots: roots.iter().map(ToString::to_string).collect(),
        },
        components: Vec::new(),
        publication: PublicationRef::NeedsReinstall,
        installed_at: 1735689600,
        midenup_version: None,
        patches: Default::default(),
    });
    let mut state = serde_json::to_value(state).unwrap();
    state["state_version"] = "1.0.0".into();
    std::fs::write(paths::state_path(&env.midenup_home), state.to_string()).unwrap();
}

fn state_of(home: &Path) -> LocalState {
    LocalState::load(&paths::state_path(home)).expect("state.json must be readable")
}

/// Whatever the command, an alpha home is refused before anything in it changes, with the way to
/// start over in a separate home.
#[test]
fn integration_an_alpha_home_is_refused_unchanged() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("alpha_refused");
    write_alpha_home(&env);
    let before = snapshot_tree(&env.midenup_home);

    for args in [&["list"][..], &["show", "list"], &["install", "0.15.0"], &["init"]] {
        let output = run_midenup(&env, UNREACHABLE_UPSTREAM, args);
        assert!(!output.status.success(), "{args:?} must refuse");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("pre-1.0 midenup"), "{args:?}: {stderr}");
        assert!(stderr.contains("MIDENUP_HOME=<new dir> midenup init"), "{args:?}: {stderr}");
    }

    assert!(
        snapshot_tree(&env.midenup_home) == before,
        "the alpha home must be left as it was"
    );
}

/// Its local manifest alone is enough to refuse: nothing else records what it installed.
#[test]
fn integration_an_alpha_manifest_without_installed_state_is_refused() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("alpha_manifest_only");
    std::fs::create_dir_all(&env.midenup_home).unwrap();
    std::fs::write(env.midenup_home.join("manifest.json"), v1_manifest("1.0.1", "0.15.0", &["vm"]))
        .unwrap();
    std::fs::write(
        paths::state_path(&env.midenup_home),
        r#"{"state_version":"2.0.0","installations":[]}"#,
    )
    .unwrap();
    let before = snapshot_tree(&env.midenup_home);

    let output = run_midenup(&env, UNREACHABLE_UPSTREAM, &["list"]);
    assert!(!output.status.success());
    assert!(snapshot_tree(&env.midenup_home) == before);
}

/// A fixture channel with `vm` (minimal) plus whatever else is named, all `file://` backed.
fn upstream(env: &TestEnvironment, file: &str, components: &[&str]) -> String {
    let dir = env.tmp_dir.path().join("fixture");
    std::fs::create_dir_all(&dir).unwrap();

    let vm = dir.join("miden-vm");
    std::fs::write(&vm, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&vm, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let mut declared = vec![serde_json::json!({
        "name": "vm",
        "version": {"kind": "registry", "version": "0.1.0"},
        "kind": "executable",
        "installation_method": {"kind": "prebuilt"},
        "installed-executable": "miden-vm",
        "profiles": ["minimal"],
        "artifacts": {"miden-vm": {"uri": format!("file://{}", vm.display())}}
    })];

    for name in components.iter().filter(|name| **name != "vm") {
        let artifact = dir.join(format!("{name}.txt"));
        std::fs::write(&artifact, format!("{name}\n")).unwrap();
        declared.push(serde_json::json!({
            "name": name,
            "version": {"kind": "registry", "version": "0.1.0"},
            "kind": "asset",
            "profiles": [],
            "artifacts": {format!("{name}.txt"): {"uri": format!("file://{}", artifact.display())}}
        }));
    }

    let manifest = serde_json::json!({
        "manifest_version": "3.0.0",
        "min_client_version": "1.0.0",
        "date": 1735689600,
        "channels": [{"name": "0.15.0", "components": declared}]
    });

    let path = dir.join(file);
    std::fs::write(&path, serde_json::to_string_pretty(&manifest).unwrap()).unwrap();
    format!("file://{}", path.display())
}

/// A carried-forward record describes a tree no receipt covers, so midenup will not execute against
/// it: the first use installs it properly, and only then dispatches.
#[test]
fn integration_a_migrated_installation_is_reinstalled_on_first_use() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("needs_reinstall");
    write_carried_forward_home(&env, "0.15.0", &["vm"]);

    let manifest = upstream(&env, "upstream.json", &["vm"]);
    run_midenup(&env, &manifest, &["list"]);

    assert!(
        matches!(
            state_of(&env.midenup_home).installations[0].publication,
            PublicationRef::NeedsReinstall
        ),
        "converting the home alone must not claim the toolchain is usable"
    );
    assert!(
        !paths::toolchains_dir(&env.midenup_home).join("0.15.0").exists(),
        "the conversion removes the alpha directory the reinstall's link replaces"
    );

    // Dispatch triggers the install, exactly as it would for a toolchain that was never installed.
    let output = midenup_command(env!("CARGO_BIN_EXE_midenup"), &env, &manifest)
        .args(["install", "0.15.0", "--profile", "minimal"])
        .output()
        .expect("failed to run midenup");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let installation = &state_of(&env.midenup_home).installations[0];
    assert!(
        matches!(installation.publication, PublicationRef::Managed { .. }),
        "the reinstall must produce a publication midenup owns"
    );
    assert!(!installation.components.is_empty(), "and a component snapshot to dispatch from");
    assert!(
        std::fs::symlink_metadata(paths::toolchains_dir(&env.midenup_home).join("0.15.0"))
            .unwrap()
            .is_symlink()
    );
}

/// Spec section 12.1: a migrated root that no longer exists upstream is dropped, once, with a
/// warning. Blocking would strand every v1 user whose channel happened to drop a component.
#[test]
fn integration_migrated_roots_missing_upstream_are_dropped_once_with_a_warning() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("migrated_root_dropped");
    write_carried_forward_home(&env, "0.15.0", &["vm", "goneaway"]);

    let manifest = upstream(&env, "upstream.json", &["vm"]);
    run_midenup(&env, &manifest, &["list"]);

    let output = run_midenup(&env, &manifest, &["install", "0.15.0", "--profile", "minimal"]);
    assert!(
        output.status.success(),
        "a missing migrated root must not block: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Diagnostics go to stderr; stdout carries results.
    let reported = String::from_utf8_lossy(&output.stderr);
    assert!(reported.contains("goneaway"), "the dropped root must be named: {reported}");

    let installation = &state_of(&env.midenup_home).installations[0];
    assert!(
        !installation.intent.roots.contains("goneaway"),
        "intent must be rewritten without it: {:?}",
        installation.intent
    );
    assert!(matches!(installation.publication, PublicationRef::Managed { .. }));
}

/// ...and only once. The relaxation exists because migrated roots were inferred rather than
/// chosen; once the user has installed on top of them, they are chosen.
#[test]
fn integration_after_the_first_operation_a_removed_root_blocks_as_normal() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("relaxation_is_one_time");
    write_carried_forward_home(&env, "0.15.0", &["vm", "client"]);

    let with_client = upstream(&env, "with-client.json", &["vm", "client"]);
    run_midenup(&env, &with_client, &["list"]);
    // Consumes the relaxation: the record stops being a migrated one here.
    let output = run_midenup(&env, &with_client, &["install", "0.15.0", "--profile", "minimal"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let installation = &state_of(&env.midenup_home).installations[0];
    assert!(
        installation.intent.roots.contains("client"),
        "the root survived the first install"
    );

    let without_client = upstream(&env, "without-client.json", &["vm"]);
    let output = run_midenup(&env, &without_client, &["update", "0.15.0"]);
    assert!(!output.status.success(), "a chosen root that disappeared must block the update");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("client"), "the diagnostic must name it: {stderr}");
    assert!(
        state_of(&env.midenup_home).installations[0].intent.roots.contains("client"),
        "and the installation must be preserved"
    );
}

/// A migrated channel that no longer exists upstream is reported, not deleted: the user may still
/// want `var/` and an explicit uninstall.
#[test]
fn integration_a_migrated_channel_absent_upstream_is_reported_not_deleted() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("migrated_channel_gone");
    write_carried_forward_home(&env, "0.1.0", &["vm"]);

    let manifest = upstream(&env, "upstream.json", &["vm"]);
    let output = run_midenup(&env, &manifest, &["show", "list"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));

    let reported = String::from_utf8_lossy(&output.stdout);
    assert!(reported.contains("0.1.0"), "the channel must still be listed: {reported}");
    assert!(reported.contains("unavailable"), "and marked unavailable: {reported}");

    assert_eq!(
        state_of(&env.midenup_home).installations.len(),
        1,
        "the record must be retained, not deleted"
    );
}
