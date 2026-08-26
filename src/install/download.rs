//! Acquiring an artifact: HTTP(S) transfers and local copies.
//!
//! # What this module guarantees
//!
//! A body reaches disk only when the response says it should. `response_code()` is read **after**
//! `transfer.perform()`, because before the transfer curl has no response and reports 0: read
//! early, the status check passes for everything, and the body of a 404 or 500 is written to disk
//! *as the artifact*. A component then installs "successfully" and fails to execute, with an HTML
//! error page where its binary should be.
//!
//! Destinations come from the plan, so the URL is never consulted for a name at all.
//!
//! The temporary file an artifact stages through is unique per attempt, not derived from the
//! destination's stem: `core.masp` and `core.wasm` must not stage through the same path.
//!
//! An archived artifact takes the same path with one step inserted: the file inside the transferred
//! bytes is read out (see [crate::install::archive]) and *that* is what gets published. It is
//! streamed into the temporary as it is unpacked, so the artifact is never held in memory alongside
//! the bytes it came out of, and the container is never written anywhere.

use std::{
    cell::RefCell,
    io::Write,
    path::{Path, PathBuf},
};

use crate::{
    artifact::SupportedFormat,
    install::{ArchiveError, archive},
    plan::PlanStep,
    report,
};

/// How many redirects to follow before giving up.
///
/// Release artifacts are routinely served via one or two redirects to object storage. A bounded
/// number is required: unbounded following turns a redirect loop into a hang.
const MAX_REDIRECTS: u32 = 10;

#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("failed to create '{path}': {source}")]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to copy '{src}' to '{dest}': {source}")]
    Copy {
        src: PathBuf,
        dest: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("request for '{uri}' failed with status {status}")]
    HttpStatus { uri: String, status: u32 },
    #[error("request for '{uri}' returned an empty body")]
    EmptyBody { uri: String },
    #[error("transfer of '{uri}' failed: {reason}")]
    Transfer { uri: String, reason: String },
    #[error("failed to publish '{dest}': {source}")]
    Publish {
        dest: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Archive(#[from] ArchiveError),
}

/// Performs one acquisition step.
///
/// Only [PlanStep::Download] and [PlanStep::CopyLocal] are handled; build steps belong to the
/// Cargo executor. Every path is taken verbatim from the plan.
pub fn acquire(step: &PlanStep) -> Result<(), ExecError> {
    match step {
        PlanStep::Download { uri, dest, mode, archive, owner, .. } => {
            download(uri, dest, *mode, *archive, owner)
        },
        PlanStep::CopyLocal { src, dest, mode, archive, .. } => {
            copy_local(src, dest, *mode, *archive)
        },
        PlanStep::CargoBuild { .. } | PlanStep::ExtractPackage { .. } => Ok(()),
    }
}

/// Fetches `uri` to `dest`, atomically, unpacking it first when it is an archive.
///
/// `label` names what is being transferred for the progress display; it is the owning component,
/// never anything derived from the URL.
pub fn download(
    uri: &str,
    dest: &Path,
    mode: u32,
    archive: Option<SupportedFormat>,
    label: &str,
) -> Result<(), ExecError> {
    let body = fetch(uri, label)?;
    publish(&body, archive, uri, dest, mode)
}

/// Copies `src` to `dest`, atomically, unpacking it first when it is an archive.
pub fn copy_local(
    src: &Path,
    dest: &Path,
    mode: u32,
    archive: Option<SupportedFormat>,
) -> Result<(), ExecError> {
    let body = std::fs::read(src).map_err(|source| ExecError::Copy {
        src: src.to_path_buf(),
        dest: dest.to_path_buf(),
        source,
    })?;
    let source = format!("file://{}", src.display());
    publish(&body, archive, &source, dest, mode)
}

/// Retrieves `uri`, rejecting anything that is not a usable body.
fn fetch(uri: &str, label: &str) -> Result<Vec<u8>, ExecError> {
    let mut body = Vec::new();
    let mut handle = curl::easy::Easy::new();

    let setup = |handle: &mut curl::easy::Easy| -> Result<(), curl::Error> {
        handle.url(uri)?;
        handle.follow_location(true)?;
        handle.max_redirections(MAX_REDIRECTS)?;
        // Without this curl never calls the progress function at all.
        handle.progress(true)?;
        Ok(())
    };
    setup(&mut handle).map_err(|err| ExecError::Transfer {
        uri: uri.to_string(),
        reason: err.description().to_string(),
    })?;

    // Shared with the progress callback below, which runs while `write_function` also holds a
    // borrow of `body`. Dropped at the end of this scope, which erases the display whether the
    // transfer succeeded or failed.
    let progress = RefCell::new(report::Transfer::begin(label));

    {
        let mut transfer = handle.transfer();
        transfer
            .write_function(|chunk| {
                body.extend_from_slice(chunk);
                Ok(chunk.len())
            })
            .map_err(|err| ExecError::Transfer {
                uri: uri.to_string(),
                reason: err.description().to_string(),
            })?;
        transfer
            .progress_function(|total, done, _, _| {
                // A server that sends no Content-Length reports a total of zero, which the display
                // reads as "count what has arrived and claim no denominator".
                progress.borrow_mut().update(done as u64, total as u64);
                true
            })
            .map_err(|err| ExecError::Transfer {
                uri: uri.to_string(),
                reason: err.description().to_string(),
            })?;
        crate::trace!("GET {uri}");
        transfer.perform().map_err(|err| ExecError::Transfer {
            uri: uri.to_string(),
            reason: err.description().to_string(),
        })?;
    }

    // *After* the transfer. Before it, curl has no response and reports 0, so this check would
    // pass for every status and error pages would land on disk as artifacts.
    let status = handle.response_code().map_err(|err| ExecError::Transfer {
        uri: uri.to_string(),
        reason: err.description().to_string(),
    })?;
    if !(200..300).contains(&status) {
        return Err(ExecError::HttpStatus { uri: uri.to_string(), status });
    }
    if body.is_empty() {
        return Err(ExecError::EmptyBody { uri: uri.to_string() });
    }

    Ok(body)
}

/// Writes the artifact `body` carries to a unique temporary sibling of `dest`, then renames it into
/// place. `body` can either be the artifact itself or the archive holding it.
///
/// The temporary is a sibling so the rename stays within one filesystem, and unique so that two
/// artifacts sharing a stem cannot stage through the same path.
fn publish(
    body: &[u8],
    archive: Option<SupportedFormat>,
    source: &str,
    dest: &Path,
    mode: u32,
) -> Result<(), ExecError> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| ExecError::Create { path: parent.to_path_buf(), source })?;
    }

    let temporary = temporary_sibling(dest);
    let unusable = |source: std::io::Error| ExecError::Create { path: temporary.clone(), source };

    let result = (|| -> Result<(), ExecError> {
        let mut file = std::fs::File::create(&temporary).map_err(unusable)?;
        match archive {
            Some(format) => archive::extract(body, format, source, &mut file)?,
            None => file.write_all(body).map_err(unusable)?,
        }
        file.flush().map_err(unusable)?;
        file.sync_all().map_err(unusable)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(mode)).map_err(unusable)?;
        }
        #[cfg(not(unix))]
        let _ = mode;

        Ok(())
    })();

    if let Err(err) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(err);
    }

    crate::trace!("publishing {} -> {}", temporary.display(), dest.display());
    if let Err(source) = std::fs::rename(&temporary, dest) {
        let _ = std::fs::remove_file(&temporary);
        return Err(ExecError::Publish { dest: dest.to_path_buf(), source });
    }

    Ok(())
}

fn temporary_sibling(dest: &Path) -> PathBuf {
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    dest.with_file_name(format!(
        ".{name}.{}.{nanos}.{}.part",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

#[cfg(test)]
mod tests {
    use flate2::{Compression, write::GzEncoder};

    use super::*;

    /// A single-request HTTP server, so status handling is exercised against a real socket.
    struct TestServer {
        url: String,
        _handle: std::thread::JoinHandle<()>,
    }

    impl TestServer {
        /// Serves one response and stops.
        fn responding(status: u16, body: &'static [u8]) -> Self {
            Self::spawn(move |request: tiny_http::Request| {
                let response = tiny_http::Response::from_data(body)
                    .with_status_code(tiny_http::StatusCode(status));
                let _ = request.respond(response);
            })
        }

        /// Redirects `/from` to `/to`, then serves `body` at `/to`.
        fn redirecting(body: &'static [u8], hops: usize) -> Self {
            Self::spawn_with(move |server: &tiny_http::Server, base: String| {
                for hop in 0..hops {
                    let Ok(request) = server.recv() else { return };
                    let next = format!("{base}/hop{}", hop + 1);
                    let response = tiny_http::Response::empty(tiny_http::StatusCode(302))
                        .with_header(
                            tiny_http::Header::from_bytes(&b"Location"[..], next.as_bytes())
                                .unwrap(),
                        );
                    let _ = request.respond(response);
                }
                if let Ok(request) = server.recv() {
                    let _ = request.respond(tiny_http::Response::from_data(body));
                }
            })
        }

        fn spawn(handler: impl FnOnce(tiny_http::Request) + Send + 'static) -> Self {
            Self::spawn_with(move |server, _| {
                if let Ok(request) = server.recv() {
                    handler(request);
                }
            })
        }

        fn spawn_with(handler: impl FnOnce(&tiny_http::Server, String) + Send + 'static) -> Self {
            let server = tiny_http::Server::http("127.0.0.1:0").expect("failed to bind");
            let url = format!("http://{}", server.server_addr());
            let base = url.clone();
            let handle = std::thread::spawn(move || handler(&server, base));
            Self { url, _handle: handle }
        }

        fn url(&self, path: &str) -> String {
            format!("{}{path}", self.url)
        }
    }

    fn temp() -> tempdir::TempDir {
        tempdir::TempDir::new("download").expect("failed to create temp dir")
    }

    fn leftovers(dir: &Path, keep: &str) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name != keep)
            .collect()
    }

    /// A 404 body must never reach disk.
    ///
    /// The status is read after `perform()`, so the check sees the real response code rather than
    /// the 0 curl reports for a transfer that has not run yet.
    #[test]
    fn a_404_is_rejected_and_writes_nothing() {
        let server = TestServer::responding(404, b"<html>Not Found</html>");
        let dir = temp();
        let dest = dir.path().join("artifact.bin");

        let err = download(&server.url("/x"), &dest, 0o755, None, "fixture")
            .expect_err("a 404 must fail");
        assert!(matches!(err, ExecError::HttpStatus { status: 404, .. }), "{err}");
        assert!(!dest.exists(), "a failed download must not leave a file");
        assert!(leftovers(dir.path(), "artifact.bin").is_empty(), "no partial files");
    }

    #[test]
    fn a_500_is_rejected() {
        let server = TestServer::responding(500, b"boom");
        let dir = temp();
        let dest = dir.path().join("a.bin");

        let err = download(&server.url("/x"), &dest, 0o755, None, "fixture")
            .expect_err("a 500 must fail");
        assert!(matches!(err, ExecError::HttpStatus { status: 500, .. }), "{err}");
        assert!(!dest.exists());
    }

    /// A 200 with nothing in it is not a usable artifact.
    #[test]
    fn an_empty_body_is_rejected() {
        let server = TestServer::responding(200, b"");
        let dir = temp();
        let dest = dir.path().join("a.bin");

        let err = download(&server.url("/x"), &dest, 0o755, None, "fixture")
            .expect_err("an empty body must fail");
        assert!(matches!(err, ExecError::EmptyBody { .. }), "{err}");
        assert!(!dest.exists());
    }

    #[test]
    fn a_successful_download_lands_at_the_planned_path() {
        let server = TestServer::responding(200, b"payload");
        let dir = temp();
        let dest = dir.path().join("planned-name.bin");

        download(&server.url("/x"), &dest, 0o755, None, "fixture").expect("should succeed");
        assert_eq!(std::fs::read(&dest).unwrap(), b"payload");
        assert!(leftovers(dir.path(), "planned-name.bin").is_empty());
    }

    /// Release artifacts are routinely served through redirects to object storage.
    #[test]
    fn redirects_are_followed() {
        let server = TestServer::redirecting(b"payload", 2);
        let dir = temp();
        let dest = dir.path().join("a.bin");

        download(&server.url("/from"), &dest, 0o755, None, "fixture")
            .expect("redirects must be followed");
        assert_eq!(std::fs::read(&dest).unwrap(), b"payload");
    }

    /// The destination name comes from the plan, never from the URL.
    #[test]
    fn the_destination_name_is_never_derived_from_the_uri() {
        let server = TestServer::responding(200, b"payload");
        let dir = temp();
        let dest = dir.path().join("nothing-like-the-url");

        download(&server.url("/some/deep/path/other-name"), &dest, 0o755, None, "fixture").unwrap();
        assert!(dest.exists(), "the planned name must win");
    }

    #[test]
    fn the_planned_mode_is_applied() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let server = TestServer::responding(200, b"x");
            let dir = temp();
            let dest = dir.path().join("pkg.masp");

            download(&server.url("/x"), &dest, 0o644, None, "fixture").unwrap();
            assert_eq!(
                std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
                0o644,
                "packages must not be marked executable"
            );
        }
    }

    /// A gzipped tarball with the binary nested under a release directory, as releases ship them.
    fn tarball(member: &str, contents: &[u8]) -> Vec<u8> {
        use std::io::Write;

        let mut tar = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        // Deliberately not 0755: the mode comes from the plan, never from the archive.
        header.set_mode(0o600);
        header.set_cksum();
        tar.append_data(&mut header, member, contents).unwrap();

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar.into_inner().unwrap()).unwrap();
        encoder.finish().unwrap()
    }

    /// A gzipped tarball of several members, for the archives that must be refused.
    fn tarball_of(members: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;

        let mut tar = tar::Builder::new(Vec::new());
        for (path, contents) in members {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o600);
            header.set_cksum();
            tar.append_data(&mut header, path, *contents).unwrap();
        }

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar.into_inner().unwrap()).unwrap();
        encoder.finish().unwrap()
    }

    /// The archived file lands at the planned path with the planned name and mode.
    #[test]
    fn an_archived_download_installs_the_file_the_archive_holds() {
        let body = Box::leak(tarball("vm-0.16.0/miden-vm", b"binary").into_boxed_slice());
        let server = TestServer::responding(200, body);
        let dir = temp();
        let dest = dir.path().join("miden-vm");

        download(&server.url("/vm.tar.gz"), &dest, 0o755, Some(SupportedFormat::TarGz), "fixture")
            .expect("should succeed");

        assert_eq!(std::fs::read(&dest).unwrap(), b"binary");
        assert!(leftovers(dir.path(), "miden-vm").is_empty(), "nothing else may be unpacked");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
                0o755,
                "the mode comes from the plan, not from the archive entry"
            );
        }
    }

    /// A local archive is unpacked the same way a fetched one is.
    #[test]
    fn an_archived_local_copy_installs_the_file_the_archive_holds() {
        let dir = temp();
        let src = dir.path().join("core.tar.gz");
        std::fs::write(&src, tarball("packages/core.masp", b"package")).unwrap();
        let dest = dir.path().join("lib").join("core.masp");

        copy_local(&src, &dest, 0o644, Some(SupportedFormat::TarGz)).expect("should succeed");
        assert_eq!(std::fs::read(&dest).unwrap(), b"package");
    }

    /// An unreadable archive fails like any other unusable body: nothing on disk, container
    /// included.
    #[test]
    fn an_unreadable_archive_writes_nothing() {
        let server = TestServer::responding(200, b"<html>Not a tarball</html>");
        let dir = temp();
        let dest = dir.path().join("miden-vm");

        let err = download(
            &server.url("/vm.tar.gz"),
            &dest,
            0o755,
            Some(SupportedFormat::TarGz),
            "fixture",
        )
        .expect_err("must fail");
        assert!(matches!(err, ExecError::Archive(_)), "{err}");
        assert!(!dest.exists(), "the container must never be installed as the artifact");
        assert!(leftovers(dir.path(), "miden-vm").is_empty(), "no partial files");
    }

    /// The temporary an archive unpacks into is written before the archive has been shown to hold
    /// exactly one file. Only the rename publishes anything, so a rejected archive leaves nothing.
    #[test]
    fn an_archive_holding_more_than_one_file_writes_nothing() {
        let dir = temp();
        let src = dir.path().join("two.tar.gz");
        std::fs::write(&src, tarball_of(&[("first", b"one"), ("second", b"two")])).unwrap();
        let dest = dir.path().join("miden-vm");

        let err =
            copy_local(&src, &dest, 0o755, Some(SupportedFormat::TarGz)).expect_err("must fail");
        assert!(matches!(err, ExecError::Archive(_)), "{err}");
        assert!(!dest.exists(), "a rejected archive must not be installed");
        assert_eq!(
            leftovers(dir.path(), "two.tar.gz"),
            Vec::<String>::new(),
            "the temporary the first file was written to must be gone"
        );
    }

    #[test]
    fn a_local_copy_lands_at_the_planned_path() {
        let dir = temp();
        let src = dir.path().join("source");
        std::fs::write(&src, b"local").unwrap();
        let dest = dir.path().join("nested").join("dest");

        copy_local(&src, &dest, 0o644, None).expect("should succeed");
        assert_eq!(std::fs::read(&dest).unwrap(), b"local", "parent dirs must be created");
    }

    #[test]
    fn a_missing_local_source_is_an_error() {
        let dir = temp();
        let err = copy_local(&dir.path().join("nope"), &dir.path().join("dest"), 0o644, None)
            .expect_err("must fail");
        assert!(matches!(err, ExecError::Copy { .. }), "{err}");
    }

    /// Two artifacts sharing a stem must not stage through the same temporary path, and neither
    /// must two attempts at the same destination.
    #[test]
    fn temporary_names_do_not_collide_for_a_shared_stem() {
        let masp = temporary_sibling(Path::new("/tmp/core.masp"));
        let wasm = temporary_sibling(Path::new("/tmp/core.wasm"));
        assert_ne!(masp, wasm);

        // Nor for repeated staging of the same destination.
        let first = temporary_sibling(Path::new("/tmp/core.masp"));
        let second = temporary_sibling(Path::new("/tmp/core.masp"));
        assert_ne!(first, second);

        for name in [masp, wasm, first, second] {
            assert_eq!(name.parent(), Some(Path::new("/tmp")), "must be a sibling");
        }
    }

    /// Build steps are not this module's business.
    #[test]
    fn acquire_ignores_build_steps() {
        let step = PlanStep::CargoBuild {
            crate_name: "c".to_string(),
            authority: crate::plan::ResolvedAuthority::Registry {
                version: semver::Version::new(1, 0, 0),
            },
            features: vec![],
            rustup_channel: None,
            expect_binary: "b".to_string(),
            dest: PathBuf::from("/tmp/b"),
            owner: "c".to_string(),
        };
        acquire(&step).expect("a build step is not an acquisition");
    }
}
