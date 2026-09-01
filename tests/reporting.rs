//! What `midenup` says while it works: which stream it lands on, and how the level is chosen.
//!
//! Every assertion here is on a real process's streams, because the split between them is the
//! contract: stdout is what the command was asked for, stderr is how it went.

use std::process::Output;

mod common;

use common::*;

/// Runs the real binary against `env`.
fn run(env: &TestEnvironment, manifest_uri: &str, args: &[&str]) -> Output {
    midenup_command(env!("CARGO_BIN_EXE_midenup"), env, manifest_uri)
        .args(args)
        .output()
        .expect("failed to run midenup")
}

fn streams(output: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// The default level announces each component, and does it on stderr.
#[test]
fn integration_reporting_default_announces_components_on_stderr() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_reporting_default");
    let fixture = common::harness::OfflineFixture::create(test_env.tmp_dir.path(), "0.15.0");

    let output = run(&test_env, &fixture.manifest_uri, &["install", "stable"]);
    let (stdout, stderr) = streams(&output);
    assert!(output.status.success(), "install must succeed: {stderr}");

    assert!(stderr.contains("component 'vm'"), "each component must be announced: {stderr}");
    assert!(
        stderr.contains("installed channel '0.15.0'"),
        "and the channel at the end: {stderr}"
    );
    assert!(
        !stdout.contains("component '"),
        "announcements are progress, and must not reach stdout: {stdout}"
    );
}

/// An install says what it is installing, and how fresh the data behind it is, before it starts.
#[test]
fn integration_reporting_an_install_names_its_channel_up_front() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_reporting_header");
    let fixture = common::harness::OfflineFixture::create(test_env.tmp_dir.path(), "0.15.0");

    let output = run(&test_env, &fixture.manifest_uri, &["install", "stable"]);
    let (stdout, stderr) = streams(&output);
    assert!(output.status.success(), "install must succeed: {stderr}");

    assert!(
        stderr.contains("syncing channel updates from upstream"),
        "the sync must be announced: {stderr}"
    );
    assert!(
        stderr.contains("upstream last updated on"),
        "and how fresh the manifest behind it is: {stderr}"
    );
    assert!(
        stderr.contains("installing mainnet (0.15.0)"),
        "the install line must name the network and what it resolved to: {stderr}"
    );
    assert!(!stdout.contains("syncing"), "the header is progress, not a result: {stdout}");

    // Ordering is the point of a header: sync, then what resolved, then the work.
    let sync = stderr.find("syncing channel updates").expect("the sync must be announced");
    let installing = stderr.find("installing mainnet").expect("the install line must be present");
    let first_component = stderr.find("component '").expect("components must be announced");
    assert!(sync < installing, "the sync precedes the resolution it enables: {stderr}");
    assert!(installing < first_component, "and the header precedes the work: {stderr}");
}

/// A component built from source says so, and inline at its own position -- unlike a package
/// extraction, which is batched to the end of the install.
#[test]
fn integration_reporting_a_source_build_is_announced_in_place() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_reporting_build");
    let fixture = common::harness::OfflineFixture::new(test_env.tmp_dir.path())
        .with_channel("0.15.0")
        .with_cargo_component("prover")
        .build();

    let output = run(&test_env, &fixture.manifest_uri, &["install", "stable"]);
    let (stdout, stderr) = streams(&output);
    assert!(output.status.success(), "install must succeed: {stderr}");

    assert!(
        stderr.contains("building component 'prover' from source"),
        "a build must be announced as a build: {stderr}"
    );
    assert!(
        !stdout.contains("building component"),
        "which is progress, and does not belong on stdout: {stdout}"
    );

    // The fixture's artifacts are `file://`, so its prebuilt components are copied rather than
    // downloaded. Either way they are the cheap steps the build sits among.
    let build = stderr.find("building component").expect("the build must be announced");
    let last_transfer = stderr
        .rfind("copying component")
        .expect("the prebuilt components must be announced");
    assert!(
        build < last_transfer,
        "an inline build is not the last thing to happen: {stderr}"
    );
}

/// The work ahead is described before it starts, and each announcement says where in it we are.
#[test]
fn integration_reporting_states_the_work_ahead_and_counts_through_it() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_reporting_progress");
    let fixture = common::harness::OfflineFixture::new(test_env.tmp_dir.path())
        .with_channel("0.15.0")
        .with_cargo_component("prover")
        .build();

    let output = run(&test_env, &fixture.manifest_uri, &["install", "stable"]);
    let (_, stderr) = streams(&output);
    assert!(output.status.success(), "install must succeed: {stderr}");

    assert!(
        stderr.contains("3 steps: 2 copies, 1 source build"),
        "the summary must break the work down by kind: {stderr}"
    );

    // The counter must reach its total.
    for position in ["[1/3]", "[2/3]", "[3/3]"] {
        assert!(
            stderr.contains(position),
            "every position must appear, missing {position}: {stderr}"
        );
    }

    let summary = stderr.find("3 steps:").expect("the summary must be present");
    let first = stderr.find("[1/3]").expect("the first step must be numbered");
    assert!(summary < first, "the summary must come before the work: {stderr}");
}

/// `--quiet` suppresses the announcements without suppressing the command's result.
#[test]
fn integration_reporting_quiet_suppresses_announcements_but_not_results() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_reporting_quiet");
    let fixture = common::harness::OfflineFixture::create(test_env.tmp_dir.path(), "0.15.0");

    let installed = run(&test_env, &fixture.manifest_uri, &["install", "stable", "-q"]);
    let (_, stderr) = streams(&installed);
    assert!(installed.status.success(), "install must succeed: {stderr}");
    assert!(
        !stderr.contains("component '"),
        "--quiet must not announce components: {stderr}"
    );
    assert!(
        !stderr.contains("installed channel"),
        "nor the channel it finished installing: {stderr}"
    );
    assert!(!stderr.contains("syncing channel updates"), "nor the header: {stderr}");

    // The result of a command is not progress, so quiet has no bearing on it.
    let shown = run(&test_env, &fixture.manifest_uri, &["show", "active-toolchain", "-q"]);
    let (stdout, stderr) = streams(&shown);
    assert!(shown.status.success(), "show must succeed: {stderr}");
    // The active channel is the network name here, which is what `show` is being asked for.
    assert!(stdout.contains("mainnet"), "the result belongs on stdout regardless: {stdout}");
}

/// The trace tier traces the individual actions, which the levels below it never mention.
#[test]
fn integration_reporting_trace_traces_individual_actions() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_reporting_trace");
    let fixture = common::harness::OfflineFixture::create(test_env.tmp_dir.path(), "0.15.0");

    let output = run(&test_env, &fixture.manifest_uri, &["install", "stable", "--verbose=trace"]);
    let (stdout, stderr) = streams(&output);
    assert!(output.status.success(), "install must succeed: {stderr}");
    assert!(stderr.contains("trace:"), "the trace tier must trace: {stderr}");
    assert!(!stdout.contains("trace:"), "tracing is not a result: {stdout}");
}

/// A failing build's compiler errors reach the user.
#[test]
fn integration_reporting_a_failing_build_shows_its_compiler_errors() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_reporting_build_failure");
    let fixture = common::harness::OfflineFixture::new(test_env.tmp_dir.path())
        .with_channel("0.15.0")
        .with_cargo_component("prover")
        .build();

    std::fs::write(
        fixture.dir.join("prover-source").join("src").join("main.rs"),
        "compile_error!(\"fixture build failure\");\nfn main() {}\n",
    )
    .expect("failed to break the fixture crate");

    let output = run(&test_env, &fixture.manifest_uri, &["install", "stable"]);
    let (_, stderr) = streams(&output);
    assert!(!output.status.success(), "the install must fail with the build");
    assert!(
        stderr.contains("fixture build failure"),
        "the compiler's error must reach stderr: {stderr}"
    );
}
