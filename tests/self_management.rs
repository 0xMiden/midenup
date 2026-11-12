//! `midenup self` runs on a startup path of its own, so a midenup that cannot understand its home
//! can still update or uninstall itself.

use clap::Parser;
use midenup::{commands::Midenup, paths, state::LocalState};

mod common;

use common::*;

/// An upstream that cannot be reached: nothing listens here.
const UNREACHABLE_UPSTREAM: &str = "https://127.0.0.1:1/nope.json";

const SELF_COMMANDS: [&[&str]; 3] =
    [&["self", "update"], &["self", "uninstall"], &["self", "uninstall", "--force"]];

/// A home that every other command refuses or fails on: a format from a later release, state that
/// does not parse, and an interrupted publication whose journal entry does not parse either.
fn unreadable_home(env: &TestEnvironment) {
    let home = &env.midenup_home;
    std::fs::create_dir_all(paths::journal_dir(home)).unwrap();
    std::fs::write(paths::home_format_path(home), r#"{"format": 999}"#).unwrap();
    std::fs::write(paths::state_path(home), "{ not json").unwrap();
    std::fs::write(paths::journal_dir(home).join("entry.json"), "{ not json").unwrap();
}

/// Runs `midenup self` from a private copy, since `self` acts on the executable it runs as.
fn run_self(env: &TestEnvironment, args: &[&str]) -> std::process::Output {
    midenup_command(private_midenup(env, "bin/midenup"), env, UNREACHABLE_UPSTREAM)
        .args(args)
        .output()
        .expect("failed to run midenup")
}

/// Reaching the handler is how a test knows startup was bypassed: update fails on the unreachable
/// release, and uninstall is not implemented yet.
fn assert_reached_handler(output: &std::process::Output, args: &[&str]) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reached = match args {
        ["self", "update"] => UNREACHABLE_RELEASE,
        _ => "is not implemented yet",
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
fn integration_self_commands_do_not_create_a_missing_home() {
    let env = environment_setup("self_missing_home");

    for args in SELF_COMMANDS {
        assert_reached_handler(&run_self(&env, args), args);
    }

    assert!(!env.midenup_home.exists(), "a self command created the home");
}

/// Library callers that build their own `Config` must get the same bypass as the CLI.
///
/// Only uninstall runs here: in-process, `self update` would act on the test runner itself.
#[test]
fn integration_in_process_self_commands_bypass_startup() {
    let env = environment_setup("self_in_process");
    unreadable_home(&env);
    let before = snapshot_tree(&env.midenup_home);

    let config = midenup::config::Config::init(
        env.present_working_dir.clone(),
        env.midenup_home.clone(),
        env.cargo_home.clone(),
        UNREACHABLE_UPSTREAM,
        false,
    )
    .unwrap();

    for args in [&["self", "uninstall"][..], &["self", "uninstall", "--force"]] {
        let argv = std::iter::once("midenup").chain(args.iter().copied());
        let midenup = Midenup::try_parse_from(argv).unwrap();
        let err = midenup
            .execute_with_state(&config, &mut LocalState::default())
            .expect_err("the handlers are not implemented yet");
        assert!(err.to_string().contains("is not implemented yet"), "{args:?}: {err:#}");
    }

    assert!(snapshot_tree(&env.midenup_home) == before, "a self command changed the home");
}
