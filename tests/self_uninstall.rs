//! `midenup self uninstall`, always against a private copy of the binary and a private home.

use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Output, Stdio},
};

mod common;

use common::*;

const MANIFEST: &str = "https://127.0.0.1:1/manifest.json";

/// A populated home, the executable, and the `miden` links `init` would have made.
struct Installation {
    executable: PathBuf,
    cargo_link: PathBuf,
    sibling_link: PathBuf,
    /// Something in `CARGO_HOME` midenup does not own.
    cargo_file: PathBuf,
}

fn install(env: &TestEnvironment) -> Installation {
    let home = &env.midenup_home;
    std::fs::create_dir_all(home.join("toolchains").join("0.15.0")).unwrap();
    std::fs::create_dir_all(home.join("var").join("mainnet")).unwrap();
    std::fs::write(home.join("home-format.json"), r#"{"format": 1}"#).unwrap();
    std::fs::write(home.join("var").join("mainnet").join("store.sqlite3"), "data").unwrap();

    let executable = private_midenup(env, "bin/midenup");
    let cargo_bin = env.cargo_home.join("bin");
    std::fs::create_dir_all(&cargo_bin).unwrap();
    let cargo_link = cargo_bin.join("miden");
    std::os::unix::fs::symlink(&executable, &cargo_link).unwrap();
    let sibling_link = executable.with_file_name("miden");
    std::os::unix::fs::symlink("midenup", &sibling_link).unwrap();
    let cargo_file = cargo_bin.join("cargo-other");
    std::fs::write(&cargo_file, "not midenup's").unwrap();

    Installation {
        executable,
        cargo_link,
        sibling_link,
        cargo_file,
    }
}

/// Runs `midenup self uninstall` with `args`, answering the prompt with `input`.
fn uninstall(env: &TestEnvironment, executable: &Path, args: &[&str], input: &[u8]) -> Output {
    let mut child = midenup_command(executable, env, MANIFEST)
        .args(["self", "uninstall"])
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to run midenup");
    let mut stdin = child.stdin.take().unwrap();
    // The prompt may already have been refused or skipped, closing the pipe.
    let _ = stdin.write_all(input);
    drop(stdin);
    child.wait_with_output().unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

/// Nothing named after a reserved removal directory is left beside `home`.
fn assert_no_leftovers(home: &Path) {
    let leftovers: Vec<_> = std::fs::read_dir(home.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".uninstall-"))
        .collect();
    assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
}

#[test]
fn integration_self_uninstall_removes_everything_midenup_owns() {
    let env = environment_setup("self_uninstall_all");
    let installed = install(&env);

    let output = uninstall(&env, &installed.executable, &["--force"], b"");
    assert!(output.status.success(), "{}", stderr(&output));

    assert!(!exists(&env.midenup_home), "the home must be removed");
    assert!(!exists(&installed.executable), "the executable must be removed");
    assert!(!exists(&installed.cargo_link), "the CARGO_HOME link must be removed");
    assert!(
        !exists(&installed.sibling_link),
        "the link beside the executable must be removed"
    );
    assert!(exists(&installed.cargo_file), "files midenup does not own must be kept");
    assert_no_leftovers(&env.midenup_home);
}

#[test]
fn integration_self_uninstall_asks_once_and_only_yes_confirms() {
    for answer in [&b"y\n"[..], b"YES\n", b"  yes  \n"] {
        let env = environment_setup("self_uninstall_yes");
        let installed = install(&env);
        let output = uninstall(&env, &installed.executable, &[], answer);
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(stderr(&output).matches("Continue? [y/N]").count(), 1);
        assert!(!exists(&env.midenup_home), "{answer:?} must confirm");
        assert!(!exists(&installed.executable), "{answer:?} must confirm");
    }

    for answer in [&b"n\n"[..], b"\n", b"", b"maybe\n", b"yess\n"] {
        let env = environment_setup("self_uninstall_no");
        let installed = install(&env);
        let before = snapshot_tree(env.tmp_dir.path());
        let output = uninstall(&env, &installed.executable, &[], answer);
        assert!(output.status.success(), "{answer:?}: {}", stderr(&output));
        assert!(stderr(&output).contains("cancelled"), "{answer:?}: {}", stderr(&output));
        // Not even the format lease may be created.
        assert!(snapshot_tree(env.tmp_dir.path()) == before, "{answer:?} must change nothing");
    }
}

#[test]
fn integration_self_uninstall_keeps_miden_entries_it_does_not_own() {
    let env = environment_setup("self_uninstall_foreign");
    let installed = install(&env);
    // A link to another midenup, and a regular file.
    let other = env.tmp_dir.path().join("other-midenup");
    std::fs::write(&other, "another installation").unwrap();
    std::fs::remove_file(&installed.cargo_link).unwrap();
    std::os::unix::fs::symlink(&other, &installed.cargo_link).unwrap();
    std::fs::remove_file(&installed.sibling_link).unwrap();
    std::fs::write(&installed.sibling_link, "not a link").unwrap();

    let output = uninstall(&env, &installed.executable, &["--force"], b"");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!exists(&installed.executable));
    assert!(exists(&installed.cargo_link), "a link to another midenup must be kept");
    assert!(exists(&installed.sibling_link), "a regular file must be kept");
    assert!(exists(&other));
}

/// A refused configuration: what the error must say, and how the command is set up for it.
type Refusal<'a> = (&'static str, Box<dyn Fn(&mut std::process::Command) + 'a>);

#[test]
fn integration_self_uninstall_refuses_a_home_holding_more_than_midenup_data() {
    let env = environment_setup("self_uninstall_refused");
    let installed = install(&env);
    let before = snapshot_tree(env.tmp_dir.path());

    let refusals: [Refusal; 2] = [
        (
            "contains your home directory",
            Box::new(|command| {
                command.env("HOME", &env.midenup_home);
            }),
        ),
        (
            "contains CARGO_HOME",
            Box::new(|command| {
                command.env("CARGO_HOME", env.midenup_home.join("cargo"));
            }),
        ),
    ];
    for (expected, configure) in refusals {
        let mut command = midenup_command(&installed.executable, &env, MANIFEST);
        command.args(["self", "uninstall", "--force"]);
        configure(&mut command);
        let output = command.output().unwrap();
        assert!(!output.status.success(), "{expected}: must refuse");
        assert!(stderr(&output).contains(expected), "{expected}: {}", stderr(&output));
        assert!(
            snapshot_tree(env.tmp_dir.path()) == before,
            "{expected}: nothing may be removed"
        );
    }
}

#[test]
fn integration_self_uninstall_handles_an_executable_inside_the_home() {
    let env = environment_setup("self_uninstall_inside");
    install(&env);
    let executable = private_midenup(&env, "midenup/bin/midenup");

    let output = uninstall(&env, &executable, &["--force"], b"");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!exists(&env.midenup_home));
    assert_no_leftovers(&env.midenup_home);
}

#[test]
fn integration_self_uninstall_removes_a_symlinked_home_and_its_target() {
    let env = environment_setup("self_uninstall_symlinked");
    let installed = install(&env);
    let real = env.tmp_dir.path().join("real-home");
    std::fs::rename(&env.midenup_home, &real).unwrap();
    std::os::unix::fs::symlink(&real, &env.midenup_home).unwrap();

    let output = uninstall(&env, &installed.executable, &["--force"], b"");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!exists(&env.midenup_home), "the configured link must be removed");
    assert!(!exists(&real), "the real home must be removed");
    assert_no_leftovers(&real);
}

#[test]
fn integration_self_uninstall_without_a_home_removes_the_rest_and_creates_nothing() {
    let env = environment_setup("self_uninstall_no_home");
    let installed = install(&env);
    std::fs::remove_dir_all(&env.midenup_home).unwrap();

    let output = uninstall(&env, &installed.executable, &["--force"], b"");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!exists(&env.midenup_home), "the home must not be created");
    assert!(!exists(&installed.executable));
    assert!(!exists(&installed.cargo_link));
}

/// Anything running against the home holds the format lease shared; uninstall must not remove the
/// home out from under it.
#[test]
fn integration_self_uninstall_refuses_while_the_home_is_in_use() {
    let env = environment_setup("self_uninstall_in_use");
    let installed = install(&env);
    let _running = midenup::lock::shared_lease(&env.midenup_home).unwrap();
    let before = snapshot_tree(env.tmp_dir.path());

    let output = uninstall(&env, &installed.executable, &["--force"], b"");
    assert!(!output.status.success(), "must refuse while the home is in use");
    assert!(stderr(&output).contains("in use"), "{}", stderr(&output));
    assert!(snapshot_tree(env.tmp_dir.path()) == before, "nothing may be removed");
}

/// The home is moved aside through a directory reserved beside it. When that cannot be created,
/// nothing else may have been removed either.
#[test]
fn integration_self_uninstall_removes_nothing_when_the_home_cannot_be_moved_aside() {
    use std::os::unix::fs::PermissionsExt;

    let env = environment_setup("self_uninstall_unmovable");
    let installed = install(&env);
    let locked = env.tmp_dir.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    let home = locked.join("midenup");
    std::fs::rename(&env.midenup_home, &home).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    let before = snapshot_tree(env.tmp_dir.path());

    let output = midenup_command(&installed.executable, &env, MANIFEST)
        .env("MIDENUP_HOME", &home)
        .args(["self", "uninstall", "--force"])
        .output()
        .unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(!output.status.success(), "must fail when the home cannot be moved aside");
    assert!(stderr(&output).contains("nothing was removed"), "{}", stderr(&output));
    assert!(snapshot_tree(env.tmp_dir.path()) == before, "nothing may be removed");
}
