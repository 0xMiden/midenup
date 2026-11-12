//! `midenup self update`, against releases served locally and a private copy of the binary.

use std::{
    collections::HashMap,
    path::Path,
    process::Output,
    sync::{Arc, Mutex},
};

use sha2::{Digest, Sha256};

mod common;

use common::*;

const MANIFEST: &str = "https://127.0.0.1:1/manifest.json";

/// What the published midenup is replaced with: a script whose output proves it is the one running.
const PAYLOAD: &[u8] = b"#!/bin/sh\necho updated-payload\n";

const CURRENT: &str = env!("CARGO_PKG_VERSION");

fn archive_name() -> String {
    format!("midenup-{}.tar.gz", env!("TARGET"))
}

type Routes = HashMap<String, (u16, Vec<u8>)>;

/// A local stand-in for GitHub: release metadata at `/latest`, assets under `/download/`.
struct ReleaseServer {
    base: String,
    routes: Arc<Mutex<Routes>>,
    user_agents: Arc<Mutex<Vec<String>>>,
}

impl ReleaseServer {
    fn start() -> Self {
        let server = tiny_http::Server::http("127.0.0.1:0").expect("failed to bind");
        let base = format!("http://{}", server.server_addr());
        let routes = Arc::new(Mutex::new(Routes::new()));
        let user_agents = Arc::new(Mutex::new(Vec::new()));
        let (served, agents) = (routes.clone(), user_agents.clone());
        std::thread::spawn(move || {
            for request in server.incoming_requests() {
                if let Some(agent) = request.headers().iter().find(|h| h.field.equiv("User-Agent"))
                {
                    agents.lock().unwrap().push(agent.value.to_string());
                }
                let (status, body) = served
                    .lock()
                    .unwrap()
                    .get(request.url())
                    .cloned()
                    .unwrap_or((404, b"not found".to_vec()));
                let response = tiny_http::Response::from_data(body)
                    .with_status_code(tiny_http::StatusCode(status));
                let _ = request.respond(response);
            }
        });
        Self { base, routes, user_agents }
    }

    fn latest(&self) -> String {
        format!("{}/latest", self.base)
    }

    /// Replaces everything served with `metadata` at `/latest` and `assets` under `/download/`.
    fn serve(&self, metadata: Vec<u8>, assets: Vec<(String, u16, Vec<u8>)>) {
        let mut routes = self.routes.lock().unwrap();
        routes.clear();
        routes.insert("/latest".to_string(), (200, metadata));
        for (name, status, body) in assets {
            routes.insert(format!("/download/{name}"), (status, body));
        }
    }

    /// Serves a release `tag` listing `assets`, each as a downloadable asset.
    fn release(&self, tag: &str, assets: Vec<(String, u16, Vec<u8>)>) {
        let metadata = self.metadata(tag, false, assets.iter().map(|(name, ..)| name.as_str()));
        self.serve(metadata, assets);
    }

    fn metadata<'a>(
        &self,
        tag: &str,
        prerelease: bool,
        names: impl Iterator<Item = &'a str>,
    ) -> Vec<u8> {
        let assets: Vec<_> = names
            .map(|name| {
                serde_json::json!({
                    "name": name,
                    "browser_download_url": format!("{}/download/{name}", self.base),
                })
            })
            .collect();
        serde_json::to_vec(&serde_json::json!({
            "tag_name": tag,
            "draft": false,
            "prerelease": prerelease,
            "assets": assets,
        }))
        .unwrap()
    }
}

fn tar_gz(member: &str, contents: &[u8]) -> Vec<u8> {
    let mut builder =
        tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    let mut header = tar::Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder.append_data(&mut header, member, contents).unwrap();
    builder.into_inner().unwrap().finish().unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The assets of a well-formed release: the archive for this target and its checksums.
fn good_assets(archive: &[u8]) -> Vec<(String, u16, Vec<u8>)> {
    let sums =
        format!("{}  {}\n{}  installer.sh\n", sha256(archive), archive_name(), "0".repeat(64));
    vec![
        (archive_name(), 200, archive.to_vec()),
        ("SHA256SUMS".to_string(), 200, sums.into_bytes()),
    ]
}

fn self_update(env: &TestEnvironment, program: &Path, server: &ReleaseServer) -> Output {
    midenup_command(program, env, MANIFEST)
        .env(midenup::commands::MIDENUP_RELEASE_URI_ENV, server.latest())
        .args(["self", "update"])
        .output()
        .expect("failed to run midenup")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Every entry beside `executable`, so a test can tell nothing was left staged.
fn siblings(executable: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(executable.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn integration_self_update_replaces_the_executable_a_symlink_resolves_to() {
    use std::os::unix::fs::PermissionsExt;

    let env = environment_setup("self_update_replaces");
    let server = ReleaseServer::start();
    server.release("v99.0.0", good_assets(&tar_gz("midenup", PAYLOAD)));

    // A renamed executable in a directory with a space, run through a symlink.
    let executable = private_midenup(&env, "my bin/midenup-custom");
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o750)).unwrap();
    let link = env.tmp_dir.path().join("links").join("midenup");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&executable, &link).unwrap();
    let before = siblings(&executable);

    let output = self_update(&env, &link, &server);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).contains("99.0.0"), "{}", stderr(&output));

    assert_eq!(std::fs::read(&executable).unwrap(), PAYLOAD);
    let mode = std::fs::metadata(&executable).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode, 0o750, "the executable's permissions must be kept");
    assert!(
        link.symlink_metadata().unwrap().file_type().is_symlink(),
        "the link must remain"
    );
    assert_eq!(siblings(&executable), before, "nothing may be left staged beside it");

    let ran = std::process::Command::new(&link).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), "updated-payload");

    let agents = server.user_agents.lock().unwrap();
    assert!(
        !agents.is_empty() && agents.iter().all(|agent| agent.starts_with("midenup/")),
        "{agents:?}"
    );
    assert!(!env.midenup_home.exists(), "an update must not create the home");
}

#[test]
fn integration_self_update_is_a_no_op_unless_the_release_is_newer() {
    let env = environment_setup("self_update_no_op");
    let server = ReleaseServer::start();
    let executable = private_midenup(&env, "bin/midenup");
    let original = std::fs::read(&executable).unwrap();

    // Build metadata does not make a release newer.
    for tag in [format!("v{CURRENT}"), format!("v{CURRENT}+other-build"), "v0.1.0".to_string()] {
        server.release(&tag, good_assets(&tar_gz("midenup", PAYLOAD)));
        let output = self_update(&env, &executable, &server);
        assert!(output.status.success(), "{tag}: {}", stderr(&output));
        assert!(stderr(&output).contains("up to date"), "{tag}: {}", stderr(&output));
        assert!(std::fs::read(&executable).unwrap() == original, "{tag}: the executable changed");
    }
}

/// A failure: what it is, how the server is set up for it, and what the error must say.
type FailureCase<'a> = (&'static str, Box<dyn Fn() + 'a>, &'static str);

#[test]
fn integration_self_update_failures_leave_the_executable_untouched() {
    let env = environment_setup("self_update_failures");
    let server = ReleaseServer::start();
    let executable = private_midenup(&env, "bin/midenup");
    let original = std::fs::read(&executable).unwrap();
    let before = siblings(&executable);

    let archive = tar_gz("midenup", PAYLOAD);
    let sums = |lines: &str| ("SHA256SUMS".to_string(), 200, lines.as_bytes().to_vec());
    let entry = format!("{}  {}\n", sha256(&archive), archive_name());
    let garbage = b"not a gzip stream".to_vec();
    let empty = tar_gz("midenup", b"");

    let cases: Vec<FailureCase> = vec![
        (
            "checksum mismatch",
            Box::new(|| {
                let wrong = format!("{}  {}\n", "a".repeat(64), archive_name());
                server
                    .release("v99.0.0", vec![(archive_name(), 200, archive.clone()), sums(&wrong)]);
            }),
            "SHA-256",
        ),
        (
            "no SHA256SUMS",
            Box::new(|| server.release("v99.0.0", vec![(archive_name(), 200, archive.clone())])),
            "does not publish SHA256SUMS",
        ),
        (
            "no entry for the archive",
            Box::new(|| {
                server.release(
                    "v99.0.0",
                    vec![(archive_name(), 200, archive.clone()), sums("abc  other.tar.gz\n")],
                )
            }),
            "exactly one digest",
        ),
        (
            "duplicate entries",
            Box::new(|| {
                let twice = format!("{entry}{entry}");
                server
                    .release("v99.0.0", vec![(archive_name(), 200, archive.clone()), sums(&twice)])
            }),
            "exactly one digest",
        ),
        (
            "archive request fails",
            Box::new(|| {
                server
                    .release("v99.0.0", vec![(archive_name(), 500, archive.clone()), sums(&entry)])
            }),
            "status 500",
        ),
        (
            "invalid archive with a matching digest",
            Box::new(|| {
                let line = format!("{}  {}\n", sha256(&garbage), archive_name());
                server.release("v99.0.0", vec![(archive_name(), 200, garbage.clone()), sums(&line)])
            }),
            "failed to read the archive",
        ),
        (
            "empty executable",
            Box::new(|| {
                let line = format!("{}  {}\n", sha256(&empty), archive_name());
                server.release("v99.0.0", vec![(archive_name(), 200, empty.clone()), sums(&line)])
            }),
            "is empty",
        ),
        (
            "no binary for this target",
            Box::new(|| {
                server.release(
                    "v99.0.0",
                    vec![
                        ("midenup-other-target.tar.gz".to_string(), 200, archive.clone()),
                        sums(&entry),
                    ],
                )
            }),
            "cargo install midenup --locked --force",
        ),
        (
            "prerelease",
            Box::new(|| {
                let assets = good_assets(&archive);
                let metadata =
                    server.metadata("v99.0.0", true, assets.iter().map(|(n, ..)| n.as_str()));
                server.serve(metadata, assets);
            }),
            "draft or prerelease",
        ),
        (
            "tag is not a version",
            Box::new(|| server.release("latest", good_assets(&archive))),
            "is not a version",
        ),
        (
            "malformed metadata",
            Box::new(|| server.serve(b"{ not json".to_vec(), vec![])),
            "malformed",
        ),
    ];

    for (case, serve, expected) in cases {
        serve();
        let output = self_update(&env, &executable, &server);
        assert!(!output.status.success(), "{case}: must fail");
        assert!(stderr(&output).contains(expected), "{case}: {}", stderr(&output));
        assert!(
            std::fs::read(&executable).unwrap() == original,
            "{case}: the executable changed"
        );
        assert_eq!(siblings(&executable), before, "{case}: something was left staged");
    }
}

/// Replacement goes through a new file, so a process already running the old executable keeps it.
#[test]
fn integration_self_update_does_not_disturb_a_running_copy() {
    let env = environment_setup("self_update_running");
    let server = ReleaseServer::start();
    server.release("v99.0.0", good_assets(&tar_gz("midenup", PAYLOAD)));
    let executable = private_midenup(&env, "bin/midenup");
    let original = std::fs::read(&executable).unwrap();

    // Held open the way a running image is: the old file must stay intact behind the rename.
    let mut held = std::fs::File::open(&executable).unwrap();
    let output = self_update(&env, &executable, &server);
    assert!(output.status.success(), "{}", stderr(&output));

    let mut contents = Vec::new();
    std::io::Read::read_to_end(&mut held, &mut contents).unwrap();
    assert!(contents == original, "the old executable must not be rewritten in place");
    assert_eq!(std::fs::read(&executable).unwrap(), PAYLOAD);
}
