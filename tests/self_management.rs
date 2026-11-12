//! `midenup self` runs on a startup path of its own, so a midenup that cannot understand its home
//! can still update or uninstall itself.

use clap::Parser;
use midenup::{commands::Midenup, paths, state::LocalState};

mod common;

use common::*;

/// An upstream that cannot be reached: nothing listens here.
const UNREACHABLE_UPSTREAM: &str = "https://127.0.0.1:1/nope.json";

/// Neither changes anything: update fails on the unreachable release, and uninstall, given no
/// answer, cancels.
const SELF_COMMANDS: [&[&str]; 2] = [&["self", "update"], &["self", "uninstall"]];

/// A home that every other command refuses or fails on: a format from a later release, state that
/// does not parse, and an interrupted publication whose journal entry does not parse either.
fn unreadable_home(env: &TestEnvironment) {
    let home = &env.midenup_home;
    std::fs::create_dir_all(paths::journal_dir(home)).unwrap();
    std::fs::write(paths::home_format_path(home), r#"{"format": 999}"#).unwrap();
    std::fs::write(paths::state_path(home), "{ not json").unwrap();
    std::fs::write(paths::journal_dir(home).join("entry.json"), "{ not json").unwrap();
}

/// Runs `midenup self` from a private copy, since `self` acts on the executable it runs as. Its
/// input is closed.
fn run_self(env: &TestEnvironment, args: &[&str]) -> std::process::Output {
    midenup_command(private_midenup(env, "bin/midenup"), env, UNREACHABLE_UPSTREAM)
        .args(args)
        .output()
        .expect("failed to run midenup")
}

/// Reaching the handler is how a test knows startup was bypassed.
fn assert_reached_handler(output: &std::process::Output, args: &[&str]) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reached = match args {
        ["self", "update"] => UNREACHABLE_RELEASE,
        _ => "Continue? [y/N]",
    };
    assert!(stderr.contains(reached), "{args:?} must reach its handler: {stderr}");
}

#[test]
fn integration_self_commands_bypass_an_unreadable_home_and_leave_it_unchanged() {
    let env = environment_setup("self_unreadable_home");
    unreadable_home(&env);
    let before = snapshot_tree(&env.midenup_home);

    // The same home stops an ordinary command, so the bypass below is what lets `self` through.
    let output = run_midenup(&env, UNREACHABLE_UPSTREAM, &["show", "list"]);
    assert!(!output.status.success(), "an ordinary command must refuse this home");

    for args in SELF_COMMANDS {
        assert_reached_handler(&run_self(&env, args), args);
    }

    assert!(snapshot_tree(&env.midenup_home) == before, "a self command changed the home");
}

#[test]
fn integration_self_uninstall_removes_an_unreadable_home() {
    let env = environment_setup("self_unreadable_home_removed");
    unreadable_home(&env);

    let output = run_self(&env, &["self", "uninstall", "--force"]);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(!env.midenup_home.exists(), "the unreadable home must be removed");
}

#[test]
fn integration_self_commands_do_not_create_a_missing_home() {
    let env = environment_setup("self_missing_home");

    for args in SELF_COMMANDS {
        assert_reached_handler(&run_self(&env, args), args);
    }

    assert!(!env.midenup_home.exists(), "a self command created the home");
}

/// Library callers that build their own `Config` must get the same bypass as the CLI.
///
/// In-process, `self` would act on the test runner itself, so this uses a configuration uninstall
/// refuses before it removes anything: a `CARGO_HOME` inside the midenup home.
#[test]
fn integration_in_process_self_commands_bypass_startup() {
    let env = environment_setup("self_in_process");
    unreadable_home(&env);
    let before = snapshot_tree(&env.midenup_home);

    let config = midenup::config::Config::init(
        env.present_working_dir.clone(),
        env.midenup_home.clone(),
        env.midenup_home.join("cargo"),
        UNREACHABLE_UPSTREAM,
        false,
    )
    .unwrap();

    let midenup = Midenup::try_parse_from(["midenup", "self", "uninstall", "--force"]).unwrap();
    let err = midenup
        .execute_with_state(&config, &mut LocalState::default())
        .expect_err("uninstall must refuse this home");
    assert!(err.to_string().contains("contains CARGO_HOME"), "{err:#}");

    assert!(snapshot_tree(&env.midenup_home) == before, "a self command changed the home");
}
