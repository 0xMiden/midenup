//! End-to-end tests for the manifest authoring tool.
//!
//! These drive the real binary rather than internal functions: the tool's contract is "run this
//! command, get a correct manifest on disk", and the mutation logic lives in `main.rs`.

use std::{path::Path, process::Command};

/// Runs `update-manifest` against `path`, returning stderr on failure.
fn run(path: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new(env!("CARGO_BIN_EXE_update-manifest"))
        .arg("--manifest-path")
        .arg(path)
        .args(args)
        .output()
        .expect("failed to spawn update-manifest");

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.status.success() {
        Ok(stdout)
    } else {
        // Both streams: a failure has to be judged on everything the tool said, including any
        // outcome it announced on stdout before erroring out.
        Err(format!("{stdout}{stderr}"))
    }
}

fn write_manifest(dir: &Path, value: serde_json::Value) -> std::path::PathBuf {
    let path = dir.join("channel-manifest.json");
    std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    path
}

fn read_manifest(path: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn manifest_with(components: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "manifest_version": "3.0.0",
        "date": 1735689600,
        "networks": {"mainnet": "0.15.0"},
        "channels": [{"name": "0.15.0", "components": components}]
    })
}

/// A manifest with two toolchains, `mainnet` naming the older one.
///
/// This is the shape `promote` exists to change: a network that has somewhere to move to.
fn fixture() -> (tempdir::TempDir, std::path::PathBuf) {
    let dir = tempdir::TempDir::new("update-manifest-promote").unwrap();
    let path = write_manifest(
        dir.path(),
        serde_json::json!({
            "manifest_version": "3.0.0",
            "date": 1735689600,
            "networks": {"mainnet": "0.15.0"},
            "channels": [
                {"name": "0.15.0", "components": [cargo_executable("vm", "miden-vm")]},
                {"name": "0.16.0", "components": [cargo_executable("vm", "miden-vm")]}
            ]
        }),
    );
    (dir, path)
}

/// The same, except that toolchain 0.16.0 cannot be resolved: `client` requires a component that
/// is not in the channel.
fn fixture_with_dangling_requirement() -> (tempdir::TempDir, std::path::PathBuf) {
    let dir = tempdir::TempDir::new("update-manifest-promote-dangling").unwrap();
    let path = write_manifest(
        dir.path(),
        serde_json::json!({
            "manifest_version": "3.0.0",
            "date": 1735689600,
            "networks": {"mainnet": "0.15.0"},
            "channels": [
                {"name": "0.15.0", "components": [cargo_executable("vm", "miden-vm")]},
                {"name": "0.16.0", "components": [
                    {"name": "client", "version": {"kind": "registry", "version": "0.16.0"},
                     "kind": "executable", "requires": ["ghost"], "profiles": ["minimal"],
                     "installation_method": {"kind": "cargo", "crate_name": "c"},
                     "installed-executable": "miden-client"}
                ]}
            ]
        }),
    );
    (dir, path)
}

fn cargo_executable(name: &str, installed: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "version": {"kind": "registry", "version": "0.15.0"},
        "kind": "executable",
        "installation_method": {"kind": "cargo", "crate_name": "some-crate"},
        "installed-executable": installed,
        "profiles": ["minimal"]
    })
}

fn component<'a>(manifest: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    manifest["channels"][0]["components"]
        .as_array()
        .expect("components array")
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("component '{name}' not found"))
}

/// A partial `--kind` update must change the named fields and preserve the rest.
///
/// The merge applies the user's new value over the existing one, so the new value wins.
#[test]
fn partial_kind_update_changes_the_requested_field() {
    let dir = tempdir::TempDir::new("update-manifest-patch").unwrap();
    let path = write_manifest(
        dir.path(),
        manifest_with(serde_json::json!([cargo_executable("vm", "miden-vm")])),
    );

    run(
        &path,
        &[
            "update-component",
            "--channel",
            "0.15.0",
            "vm",
            "--authority",
            r#"{"kind":"registry","version":"0.15.0"}"#,
            "--kind",
            r#"{"installation_method":{"kind":"prebuilt"}}"#,
        ],
    )
    .expect("update-component should succeed");

    let vm = read_manifest(&path);
    let vm = component(&vm, "vm");
    assert_eq!(
        vm["installation_method"]["kind"], "prebuilt",
        "the requested field must actually change"
    );
    assert_eq!(
        vm["installed-executable"], "miden-vm",
        "fields the patch did not mention must be preserved"
    );
}

/// The patch must be able to change a field to a value the previous one also had a key for,
/// which is where a reversed merge is least visible.
#[test]
fn partial_kind_update_can_change_a_nested_field() {
    let dir = tempdir::TempDir::new("update-manifest-nested").unwrap();
    let path = write_manifest(
        dir.path(),
        manifest_with(serde_json::json!([cargo_executable("vm", "miden-vm")])),
    );

    run(
        &path,
        &[
            "update-component",
            "--channel",
            "0.15.0",
            "vm",
            "--authority",
            r#"{"kind":"registry","version":"0.15.0"}"#,
            "--kind",
            r#"{"installation_method":{"kind":"cargo","crate_name":"renamed-crate"}}"#,
        ],
    )
    .expect("update-component should succeed");

    let manifest = read_manifest(&path);
    assert_eq!(component(&manifest, "vm")["installation_method"]["crate_name"], "renamed-crate");
}

/// A `--kind` argument that is not a JSON object must be rejected, loudly.
///
/// Under RFC 7386 a non-object patch *replaces* the target rather than merging into it, so a
/// `--kind` that arrives as a bare `Value::String` throws the component's kind away instead of
/// updating one of its fields -- and does so while reporting success.
#[test]
fn a_non_object_kind_patch_is_rejected() {
    let dir = tempdir::TempDir::new("update-manifest-nonobject").unwrap();
    let path = write_manifest(
        dir.path(),
        manifest_with(serde_json::json!([cargo_executable("vm", "miden-vm")])),
    );

    let err = run(
        &path,
        &[
            "update-component",
            "--channel",
            "0.15.0",
            "vm",
            "--authority",
            r#"{"kind":"registry","version":"0.15.0"}"#,
            "--kind",
            r#""just-a-string""#,
        ],
    )
    .expect_err("a non-object patch must be rejected");
    assert!(err.contains("object"), "the error should say an object was expected: {err}");

    // And the manifest must be untouched.
    let manifest = read_manifest(&path);
    assert_eq!(component(&manifest, "vm")["installation_method"]["kind"], "cargo");
}

/// A failed mutation must leave the original file byte-for-byte intact.
#[test]
fn a_failed_mutation_does_not_write() {
    let dir = tempdir::TempDir::new("update-manifest-nowrite").unwrap();
    let path = write_manifest(
        dir.path(),
        manifest_with(serde_json::json!([cargo_executable("vm", "miden-vm")])),
    );
    let before = std::fs::read(&path).unwrap();

    run(
        &path,
        &[
            "update-component",
            "--channel",
            "0.15.0",
            "nonexistent",
            "--authority",
            r#"{"kind":"registry","version":"0.15.0"}"#,
        ],
    )
    .expect_err("updating an unknown component must fail");

    assert_eq!(std::fs::read(&path).unwrap(), before, "the file must be unchanged");
}

/// `check` must detect a requirement cycle.
///
/// A cycle only shows up when the component graph is topologically sorted; building the graph and
/// discarding it accepts a cyclic manifest.
#[test]
fn check_rejects_a_cyclic_manifest() {
    let dir = tempdir::TempDir::new("update-manifest-cycle").unwrap();
    let path = write_manifest(
        dir.path(),
        manifest_with(serde_json::json!([
            {"name": "a", "version": {"kind": "registry", "version": "0.1.0"},
             "kind": "package", "requires": ["b"], "profiles": ["minimal"],
             "artifacts": {"a.masp": {"uri": "https://example.invalid/a.masp"}}},
            {"name": "b", "version": {"kind": "registry", "version": "0.1.0"},
             "kind": "package", "requires": ["a"], "profiles": ["minimal"],
             "artifacts": {"b.masp": {"uri": "https://example.invalid/b.masp"}}}
        ])),
    );

    let err = run(&path, &["check"]).expect_err("a cyclic manifest must fail check");
    assert!(err.contains("cycle"), "the error should name the problem: {err}");
}

#[test]
fn check_accepts_a_well_formed_manifest() {
    let dir = tempdir::TempDir::new("update-manifest-ok").unwrap();
    let path = write_manifest(
        dir.path(),
        manifest_with(serde_json::json!([cargo_executable("vm", "miden-vm")])),
    );
    run(&path, &["check"]).expect("a well-formed manifest must pass check");
}

/// `check` must report every problem in one pass, not one per run.
#[test]
fn check_reports_all_errors_at_once() {
    let dir = tempdir::TempDir::new("update-manifest-many").unwrap();
    let path = write_manifest(
        dir.path(),
        manifest_with(serde_json::json!([
            {"name": "a", "version": {"kind": "registry", "version": "0.1.0"},
             "kind": "package", "requires": ["ghost"], "profiles": ["minimal"],
             "artifacts": {"a.masp": {"uri": "https://example.invalid/a.masp"}}},
            {"name": "b", "version": {"kind": "registry", "version": "0.1.0"},
             "kind": "package", "requires": ["alsoghost"], "profiles": ["minimal"],
             "artifacts": {"b.masp": {"uri": "https://example.invalid/b.masp"}}}
        ])),
    );

    let err = run(&path, &["check"]).expect_err("must fail");
    assert!(err.contains("ghost"), "{err}");
    assert!(err.contains("alsoghost"), "both problems must be reported in one pass: {err}");
}

/// `check --against` judges the manifest as a replacement for the deployed one.
#[test]
fn check_against_the_deployed_manifest() {
    let (dir, deployed) = fixture();
    let against = format!("file://{}", deployed.display());

    // Unchanged is trivially fine, whatever the timestamp says.
    run(&deployed, &["check", "--against", &against]).expect("an unchanged manifest must pass");

    // A change that leaves the timestamp alone is the mistake the rule exists for.
    let next_dir = dir.path().join("next");
    std::fs::create_dir(&next_dir).unwrap();
    let mut stale = read_manifest(&deployed);
    stale["networks"]["mainnet"] = serde_json::json!("0.16.0");
    let stale = write_manifest(&next_dir, stale);
    let err = run(&stale, &["check", "--against", &against]).expect_err("must fail");
    assert!(err.contains("timestamp"), "{err}");

    // A promotion with a fresh timestamp is exactly what a release looks like.
    let mut next = read_manifest(&deployed);
    next["date"] = serde_json::json!(1735689601);
    next["networks"]["mainnet"] = serde_json::json!("0.16.0");
    let next = write_manifest(&next_dir, next);
    run(&next, &["check", "--against", &against]).expect("a promotion must pass");

    // Moving back afterwards is refused without the flag. Written elsewhere: `next` is about to
    // become the deployed side of the comparison.
    let back_dir = dir.path().join("back");
    std::fs::create_dir(&back_dir).unwrap();
    let mut back = read_manifest(&deployed);
    back["date"] = serde_json::json!(1735689602);
    back["networks"]["mainnet"] = serde_json::json!("0.15.0");
    let back = write_manifest(&back_dir, back);
    let next_uri = format!("file://{}", next.display());
    let err = run(&back, &["check", "--against", &next_uri]).expect_err("must fail");
    assert!(err.contains("moves back"), "{err}");
    run(&back, &["check", "--against", &next_uri, "--allow-downgrade"])
        .expect("the flag must allow it");
}

/// A clone must not inherit its source's predecessor: the update path picks a successor by that
/// field, and two channels claiming one predecessor is refused by `check`.
#[test]
fn clone_toolchain_does_not_carry_migrates_from() {
    let dir = tempdir::TempDir::new("update-manifest-clone").unwrap();
    let path = write_manifest(
        dir.path(),
        serde_json::json!({
            "manifest_version": "3.0.0",
            "date": 1735689600,
            "networks": {"mainnet": "0.15.0"},
            "channels": [
                {"name": "0.15.0", "migrates_from": "0.14.0",
                 "components": [cargo_executable("vm", "miden-vm")]}
            ]
        }),
    );

    run(&path, &["clone-toolchain", "--from", "0.15.0", "--to", "0.16.0"]).expect("clone");

    let manifest = read_manifest(&path);
    let cloned = manifest["channels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "0.16.0")
        .expect("the clone must exist");
    assert!(cloned.get("migrates_from").is_none(), "{cloned}");
    run(&path, &["check"]).expect("the result must pass check");
}

#[test]
fn clone_toolchain_can_declare_what_it_supersedes() {
    let (_dir, path) = fixture();

    run(
        &path,
        &[
            "clone-toolchain",
            "--from",
            "0.16.0",
            "--to",
            "0.17.0",
            "--migrates-from",
            "0.16.0",
        ],
    )
    .expect("clone");

    let manifest = read_manifest(&path);
    let cloned = manifest["channels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "0.17.0")
        .expect("the clone must exist");
    assert_eq!(cloned["migrates_from"], "0.16.0");

    // A predecessor newer than the clone fails validation, so the write is refused.
    let err = run(
        &path,
        &[
            "clone-toolchain",
            "--from",
            "0.16.0",
            "--to",
            "0.14.0",
            "--migrates-from",
            "0.16.0",
        ],
    )
    .expect_err("must fail");
    assert!(err.contains("newer"), "{err}");
    assert!(
        !read_manifest(&path)["channels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "0.14.0"),
        "a refused clone must not be written"
    );
}

#[test]
fn allow_downgrade_requires_against() {
    let (_dir, path) = fixture();
    let err = run(&path, &["check", "--allow-downgrade"]).expect_err("must be a usage error");
    assert!(err.contains("--against"), "{err}");
}

/// Removing a component that something else still requires would leave the channel unresolvable.
#[test]
fn removing_a_still_required_component_is_rejected() {
    let dir = tempdir::TempDir::new("update-manifest-remove").unwrap();
    let path = write_manifest(
        dir.path(),
        manifest_with(serde_json::json!([
            {"name": "core", "version": {"kind": "registry", "version": "0.1.0"},
             "kind": "package", "profiles": ["minimal"],
             "artifacts": {"core.masp": {"uri": "https://example.invalid/core.masp"}}},
            {"name": "client", "version": {"kind": "registry", "version": "0.1.0"},
             "kind": "executable", "requires": ["core"], "profiles": ["minimal"],
             "installation_method": {"kind": "cargo", "crate_name": "c"},
             "installed-executable": "miden-client"}
        ])),
    );

    let err = run(&path, &["remove-component", "--channel", "0.15.0", "core"])
        .expect_err("must refuse to orphan a dependent");
    assert!(err.contains("client"), "the error must name the dependent: {err}");

    // The component must still be there.
    let manifest = read_manifest(&path);
    assert_eq!(component(&manifest, "core")["name"], "core");
}

#[test]
fn removing_an_unrequired_component_succeeds() {
    let dir = tempdir::TempDir::new("update-manifest-remove-ok").unwrap();
    let path = write_manifest(
        dir.path(),
        manifest_with(serde_json::json!([cargo_executable("vm", "miden-vm")])),
    );
    run(&path, &["remove-component", "--channel", "0.15.0", "vm"]).expect("should succeed");
    assert!(read_manifest(&path)["channels"][0]["components"].as_array().unwrap().is_empty());
}

#[test]
fn add_component_accepts_profiles() {
    let dir = tempdir::TempDir::new("update-manifest-add").unwrap();
    let path = write_manifest(dir.path(), manifest_with(serde_json::json!([])));

    run(&path, &[
        "add-component",
        "--channel", "0.15.0",
        "vm",
        "--authority", r#"{"kind":"registry","version":"0.15.0"}"#,
        "--kind", r#"{"kind":"executable","installation_method":{"kind":"cargo","crate_name":"miden-vm"},"installed-executable":"miden-vm"}"#,
        "--profile", "minimal",
    ])
    .expect("add-component should succeed");

    let manifest = read_manifest(&path);
    assert_eq!(component(&manifest, "vm")["profiles"], serde_json::json!(["minimal"]));
}

/// `legacy-package` is closed to new authoring: packages ship prebuilt from here on.
#[test]
fn authoring_a_legacy_package_is_rejected() {
    let dir = tempdir::TempDir::new("update-manifest-legacy").unwrap();
    let path = write_manifest(dir.path(), manifest_with(serde_json::json!([])));

    let err = run(&path, &[
        "add-component",
        "--channel", "0.15.0",
        "protocol",
        "--authority", r#"{"kind":"registry","version":"0.15.0"}"#,
        "--kind", r#"{"kind":"legacy-package","installation_method":{"kind":"cargo","crate_name":"miden-protocol","extractor":"x()"}}"#,
    ])
    .expect_err("legacy-package must be closed to new channels");
    assert!(err.contains("deprecated"), "{err}");
}

/// A mutation whose result would be an invalid manifest must be refused before it can replace the
/// file it was editing.
///
/// A plain write commits bytes before anything confirms they are usable, so an edit that produces
/// something broken destroys the document. Here the write is staged, read back, validated, and
/// only then renamed into place.
#[test]
fn a_mutation_producing_an_invalid_manifest_is_refused() {
    let dir = tempdir::TempDir::new("update-manifest-invalid").unwrap();
    let path = write_manifest(
        dir.path(),
        manifest_with(serde_json::json!([cargo_executable("vm", "miden-vm")])),
    );
    let before = std::fs::read(&path).unwrap();

    let err = run(
        &path,
        &[
            "update-component",
            "--channel",
            "0.15.0",
            "vm",
            "--authority",
            r#"{"kind":"registry","version":"0.15.0"}"#,
            // A path traversal is not a valid installed filename.
            "--kind",
            r#"{"installed-executable":"../../etc/passwd"}"#,
        ],
    )
    .expect_err("an invalid result must be refused");

    assert!(
        err.contains("valid manifest"),
        "the error should explain what was rejected: {err}"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before, "the original must survive intact");

    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "channel-manifest.json")
        .collect();
    assert!(leftovers.is_empty(), "no temporary files may be left behind: {leftovers:?}");
}

#[test]
fn promote_moves_a_network() {
    let (_temp, path) = fixture();
    run(&path, &["promote", "mainnet", "0.16.0"]).expect("must promote");

    assert_eq!(read_manifest(&path)["networks"]["mainnet"], "0.16.0");
}

#[test]
fn promote_creates_a_network_that_does_not_exist_yet() {
    let (_temp, path) = fixture();
    let output = run(&path, &["promote", "devnet", "0.16.0"]).expect("must create");
    assert!(output.contains("created network 'devnet'"), "got: {output}");

    assert_eq!(read_manifest(&path)["networks"]["devnet"], "0.16.0");
}

#[test]
fn promote_reports_a_move_distinctly_from_a_creation() {
    let (_temp, path) = fixture();
    let output = run(&path, &["promote", "mainnet", "0.16.0"]).unwrap();
    assert!(output.contains("moved 'mainnet' from 0.15.0 to 0.16.0"), "got: {output}");
}

#[test]
fn promote_refuses_a_channel_that_is_not_in_the_manifest() {
    let (_temp, path) = fixture();
    let err = run(&path, &["promote", "mainnet", "9.9.9"]).expect_err("must refuse");
    assert!(err.contains("9.9.9"), "the diagnostic must name the channel: {err}");
}

/// A network must never name a toolchain that cannot be installed: every user tracking it would
/// discover that only at install time.
#[test]
fn promote_refuses_a_channel_that_does_not_resolve() {
    let (_temp, path) = fixture_with_dangling_requirement();
    let err = run(&path, &["promote", "mainnet", "0.16.0"]).expect_err("must refuse");
    assert!(err.contains("not installable"), "got: {err}");
}

#[test]
fn promote_refuses_to_move_a_network_backwards_without_the_flag() {
    let (_temp, path) = fixture();
    run(&path, &["promote", "mainnet", "0.16.0"]).unwrap();

    let err = run(&path, &["promote", "mainnet", "0.15.0"]).expect_err("must refuse");
    assert!(err.contains("--allow-downgrade"), "the diagnostic must say how: {err}");

    run(&path, &["promote", "mainnet", "0.15.0", "--allow-downgrade"]).expect("must allow it");
    assert_eq!(read_manifest(&path)["networks"]["mainnet"], "0.15.0");
}

#[test]
fn promote_refuses_a_network_named_like_a_channel() {
    let (_temp, path) = fixture();
    let err = run(&path, &["promote", "0.16.0", "0.16.0"]).expect_err("must refuse");
    assert!(err.contains("ambiguous"), "got: {err}");
}

#[test]
fn promote_refuses_a_reserved_synonym() {
    let (_temp, path) = fixture();
    let err = run(&path, &["promote", "stable", "0.16.0"]).expect_err("must refuse");
    assert!(err.contains("mainnet"), "must name what to use instead: {err}");
}

/// The no-op path must not rewrite the document: a `promote` that changes nothing should produce
/// no diff at all, timestamp included.
#[test]
fn promote_to_the_current_version_writes_nothing() {
    let (_temp, path) = fixture();
    let before = std::fs::read(&path).unwrap();

    let output = run(&path, &["promote", "mainnet", "0.15.0"]).expect("must succeed");
    assert!(output.contains("nothing to do"), "got: {output}");
    assert_eq!(std::fs::read(&path).unwrap(), before, "the file must be untouched");
}

/// The printed line is the only safeguard a reviewer has, so it must never describe a promotion
/// that the write then rejected.
#[test]
fn promote_does_not_announce_a_promotion_it_failed_to_write() {
    let (_temp, path) = fixture();
    let err = run(&path, &["promote", "", "0.16.0"]).expect_err("an empty network name is invalid");
    assert!(!err.contains("created network"), "must not claim to have created it: {err}");
}
