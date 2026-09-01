//! What `midenup` says while it works: which stream it lands on, and how the level is chosen.
//!
//! Every assertion here is on a real process's streams, because the split between them is the
//! contract: stdout is what the command was asked for, stderr is how it went.

use std::process::Output;
#[cfg(unix)]
use std::{
    fs::File,
    io::Read,
    os::fd::{FromRawFd, OwnedFd},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

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

/// Runs only when one of the PTY regression tests launches this test binary recursively.
#[cfg(unix)]
#[test]
fn live_child_reporting_helper() {
    let Ok(mode) = std::env::var("MIDENUP_TEST_LIVE_CHILD") else {
        return;
    };

    midenup::report::set(
        midenup::report::Verbosity::Info,
        midenup::report::ProgressStyle::Pretty,
        midenup::report::ColorChoice::False,
    );
    assert!(midenup::report::activity_is_live(), "the helper's stderr must be a terminal");

    let script = match mode.as_str() {
        // The invalid byte proves the forwarding path is byte-oriented. Waiting for the outer
        // test's release marker makes it observable whether PROMPT reached the terminal before a
        // newline or process exit without relying on scheduler timing.
        "prompt" => {
            "printf 'PROMPT>' >&2; printf '\\377' >&2; while [ ! -e \"$MIDENUP_TEST_RELEASE\" ]; \
             do sleep 0.01; done; printf 'DONE\\n' >&2"
        },
        // This is larger than the bounded reader channel, so the post-exit path must continue
        // draining rather than retaining only the first channelful of a direct child's tail.
        "tail" => {
            "i=0; while [ \"$i\" -lt 16384 ]; do printf '0123456789abcdef' >&2; i=$((i + 1)); \
             done; printf 'END\\n' >&2"
        },
        // The shell exits immediately, but its background child retains the stderr pipe.
        "descendant" => "sleep 5 & printf 'TAIL' >&2",
        other => panic!("unknown live-child helper mode '{other}'"),
    };
    let mut command = Command::new("/bin/sh");
    command.args(["-c", script]);
    let status = midenup::install::run_reporting_progress(&mut command, "child")
        .expect("failed to run reporting helper child");
    assert!(status.success(), "reporting helper child failed with {status}");
}

#[cfg(unix)]
#[test]
fn integration_reporting_live_child_output_is_raw_and_prompt() {
    let signals = tempdir::TempDir::new("midenup-reporting-prompt").unwrap();
    let release = signals.path().join("release");
    let (mut child, master) = spawn_live_child_helper("prompt", Some(&release));
    let (reader, output) = read_pty(master);

    let prompt = b"PROMPT>\xff";
    let mut transcript = Vec::new();
    let prompt_deadline = Instant::now() + Duration::from_secs(5);
    while !transcript.windows(prompt.len()).any(|window| window == prompt) {
        let remaining = prompt_deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "unterminated prompt was not forwarded: {transcript:?}");
        transcript.extend(
            output
                .recv_timeout(remaining)
                .expect("PTY closed before the unterminated prompt was forwarded"),
        );
    }

    // Once a partial child line owns the terminal, the live activity must not erase it.
    let prompt_at = transcript.windows(prompt.len()).position(|window| window == prompt).unwrap();
    let quiet_deadline = Instant::now() + Duration::from_millis(300);
    while let Ok(bytes) =
        output.recv_timeout(quiet_deadline.saturating_duration_since(Instant::now()))
    {
        transcript.extend(bytes);
        if Instant::now() >= quiet_deadline {
            break;
        }
    }
    assert!(
        !transcript[prompt_at + prompt.len()..]
            .windows(4)
            .any(|window| window == b"\x1b[2K"),
        "the activity display redrew over a partial child line: {transcript:?}"
    );

    std::fs::write(&release, b"go").expect("failed to release the prompt helper");
    wait_for_test_child(&mut child, Duration::from_secs(5));
    reader.join().expect("PTY reader panicked");
    transcript.extend(output.try_iter().flatten());
    assert!(
        transcript.windows(4).any(|window| window == b"DONE"),
        "missing tail: {transcript:?}"
    );
}

#[cfg(unix)]
#[test]
fn integration_reporting_drains_a_large_direct_child_tail() {
    let (mut child, master) = spawn_live_child_helper("tail", None);
    let (reader, output) = read_pty(master);

    wait_for_test_child(&mut child, Duration::from_secs(10));
    reader.join().expect("PTY reader panicked");
    let transcript = output.try_iter().flatten().collect::<Vec<_>>();
    assert!(
        transcript.windows(3).any(|window| window == b"END"),
        "large child tail was truncated ({} bytes captured)",
        transcript.len()
    );
}

#[cfg(unix)]
fn read_pty(master: OwnedFd) -> (std::thread::JoinHandle<()>, mpsc::Receiver<Vec<u8>>) {
    let (sender, output) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut master = File::from(master);
        let mut buffer = [0u8; 4096];
        loop {
            match master.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if sender.send(buffer[..read].to_vec()).is_err() {
                        break;
                    }
                },
            }
        }
    });
    (reader, output)
}

#[cfg(unix)]
#[test]
fn integration_reporting_does_not_wait_for_a_descendants_stderr_pipe() {
    let started = Instant::now();
    let (mut child, _master) = spawn_live_child_helper("descendant", None);
    wait_for_test_child(&mut child, Duration::from_secs(2));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "reporting waited for a background descendant to close stderr"
    );
}

#[cfg(unix)]
fn spawn_live_child_helper(
    mode: &str,
    release: Option<&std::path::Path>,
) -> (std::process::Child, OwnedFd) {
    let (master, slave) = open_pty().expect("failed to open a pseudo-terminal");
    let mut command = Command::new(std::env::current_exe().expect("test executable has no path"));
    command
        .args(["--exact", "live_child_reporting_helper", "--nocapture"])
        .env("MIDENUP_TEST_LIVE_CHILD", mode)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(File::from(slave)));
    if let Some(release) = release {
        command.env("MIDENUP_TEST_RELEASE", release);
    }
    let child = command.spawn().expect("failed to launch live reporting helper");
    (child, master)
}

#[cfg(unix)]
fn open_pty() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut master = -1;
    let mut slave = -1;
    // SAFETY: openpty initializes both descriptors on success. The optional terminal metadata and
    // window-size pointers may be null, and each initialized descriptor is immediately owned.
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == -1
    {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the successful openpty call returned fresh, valid descriptors owned by this process.
    Ok(unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) })
}

#[cfg(unix)]
fn wait_for_test_child(child: &mut std::process::Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().expect("failed to inspect helper status") {
            assert!(status.success(), "live reporting helper failed with {status}");
            return;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("live reporting helper exceeded {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
