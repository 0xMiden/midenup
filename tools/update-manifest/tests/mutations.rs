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
        Err(stderr)
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
/// Regression: the merge patch was applied in reverse -- the existing value was merged *onto* the
/// user's new one -- so the old value always won and the command silently did nothing.
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
/// Regression: clap resolved `serde_json::Value` through `From<String>` before `FromStr`, so the
/// argument arrived as `Value::String("{...}")`. Under RFC 7386 a non-object patch *replaces* the
/// target, which combined with the reversed merge to produce the worst possible outcome: the
/// command reported success and silently kept the old value.
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
/// Regression: it called `component_graph`, which built the graph and discarded it without ever
/// topologically sorting, so cyclic manifests were accepted.
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
