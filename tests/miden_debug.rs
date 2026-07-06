use std::fs;

use clap::Parser;
use midenup::commands::Midenup;

mod common;

use common::*;

/// Writes a fake `miden-debug` artifact: a script that records its arguments, one per line, into
/// `record_path`.
#[cfg(unix)]
fn write_fake_debugger(dir: &std::path::Path, record_path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;

    let script = format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", record_path.display());
    let path = dir.join("miden-debug");
    fs::write(&path, &script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Checks that `miden debug`, executed from within a Miden project directory, resolves the
/// `debug` component from the channel manifest and augments the invocation with the project's
/// compiled package artifact and its `inputs.toml`.
#[test]
#[cfg(unix)]
fn integration_miden_debug_test() {
    let test_name = "integration_miden_debug_test";
    let test_env = environment_setup(test_name);

    // Generate the channel manifest at runtime: the fake debugger artifacts live in the test's
    // temporary directory, so their `file://` URIs are only known here.
    let artifacts_dir = test_env.tmp_dir.path().join("artifacts");
    fs::create_dir_all(&artifacts_dir).unwrap();
    let record_path = test_env.tmp_dir.path().join("recorded-args.txt");
    write_fake_debugger(&artifacts_dir, &record_path);

    let manifest_path = test_env.tmp_dir.path().join("channel-manifest.json");
    let manifest = format!(
        r#"{{
  "manifest_version": "2.0.0",
  "date": 1745931671,
  "channels": [
    {{
      "name": "0.16.0",
      "components": [
        {{
          "name": "debug",
          "version": {{"kind": "registry", "version": "0.4.6"}},
          "kind": "executable",
          "installation_method": {{"kind": "prebuilt"}},
          "installed-executable": "miden-debug",
          "profiles": ["minimal"],
          "artifacts": {{
            "miden-debug": {{"uri": "file://{dir}/miden-debug"}}
          }}
        }}
      ]
    }}
  ]
}}"#,
        dir = artifacts_dir.display()
    );
    fs::write(&manifest_path, manifest).unwrap();
    let manifest_uri = format!("file://{}", manifest_path.display());

    let (mut local_manifest, config) = test_setup(&test_env, &manifest_uri);

    // Lay out a Miden project in the configured working directory: a manifest, an inputs file,
    // and a compiled package artifact in one of the toolchain's output layouts.
    let project = &test_env.present_working_dir;
    fs::write(project.join("Cargo.toml"), "[package]\nname = \"proj\"\n").unwrap();
    fs::write(project.join("inputs.toml"), "inputs = [25]\n").unwrap();
    let out_dir = project.join("target").join("midenc").join("miden").join("dev");
    fs::create_dir_all(&out_dir).unwrap();
    let package = out_dir.join("proj:proj.masp");
    fs::write(&package, b"fake package").unwrap();

    // `miden debug --repl` from inside the project: the package artifact and the inputs file
    // must be injected, and the user's own flags forwarded.
    let command = Midenup::try_parse_from(["miden", "debug", "--repl"]).unwrap();
    command
        .execute_with_state(&config, &mut local_manifest)
        .expect("failed to run `miden debug`");

    let recorded = fs::read_to_string(&record_path).expect("fake debugger was not invoked");
    let args = recorded.lines().collect::<Vec<_>>();
    assert_eq!(
        args,
        vec![
            package.to_str().unwrap(),
            "--inputs",
            project.join("inputs.toml").to_str().unwrap(),
            "--repl",
        ],
        "expected the project package and inputs to be injected"
    );

    // An explicitly named input artifact must pass through untouched: nothing is injected.
    let explicit = project.join("explicit.masp");
    fs::write(&explicit, b"explicit package").unwrap();
    let command =
        Midenup::try_parse_from(["miden", "debug", explicit.to_str().unwrap(), "--repl"]).unwrap();
    command
        .execute_with_state(&config, &mut local_manifest)
        .expect("failed to run `miden debug` with an explicit input");

    let recorded = fs::read_to_string(&record_path).unwrap();
    let args = recorded.lines().collect::<Vec<_>>();
    assert_eq!(
        args,
        vec![explicit.to_str().unwrap(), "--repl"],
        "explicit inputs must not be augmented"
    );
}
