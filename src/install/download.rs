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
//! Pending archived artifacts sharing a source and format are fetched and scanned once (see
//! [Acquisitions]). Selected files are streamed into temporary storage and published individually
//! only after complete archive validation. Decompressed files stay on disk rather than in memory,
//! and the archive container is never written anywhere.
//!
//! When the manifest declares a digest, the transferred bytes are checked against it before
//! anything is unpacked or written: a mismatch leaves nothing on disk.

use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::{Seek, Write},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use crate::{
    artifact::{Digest, SupportedFormat},
    install::{ArchiveError, archive},
    plan::{ArchiveMember, PlanStep},
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
    /// One source failure is shared by all outputs depending on that archive.
    #[error(transparent)]
    Shared(Arc<ExecError>),
    #[error(
        "artifact '{artifact}' of component '{component}' from '{uri}' does not match the digest \
         in the manifest: expected {expected}, got {actual}. midenup will not install it or build \
         it from source instead; retry, and report it to the toolchain maintainers if it persists"
    )]
    DigestMismatch {
        component: String,
        artifact: String,
        uri: String,
        expected: String,
        actual: String,
    },
}

/// Performs one acquisition step.
///
/// Only [PlanStep::Download] and [PlanStep::CopyLocal] are handled; build steps belong to the
/// Cargo executor. Every path is taken verbatim from the plan.
pub fn acquire(step: &PlanStep) -> Result<(), ExecError> {
    Acquisitions::new(std::slice::from_ref(step)).acquire(step)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source<'a> {
    Http(&'a str),
    Local(&'a Path),
}

fn archived(step: &PlanStep) -> Option<(Source<'_>, &ArchiveMember, Option<&Digest>)> {
    match step {
        PlanStep::Download { uri, archive: Some(archive), digest, .. } => {
            Some((Source::Http(uri), archive, digest.as_ref()))
        },
        PlanStep::CopyLocal { src, archive: Some(archive), digest, .. } => {
            Some((Source::Local(src), archive, digest.as_ref()))
        },
        _ => None,
    }
}

type SharedArchive = Result<Rc<archive::Extracted<TemporaryFile>>, Arc<ExecError>>;

/// Run-scoped acquisition. Each pending archive source is fetched and scanned together; its
/// temporary files live only until the last dependent step has consumed them. Seeded outputs
/// never enter the group. Logical steps and their fallbacks still run in plan order.
pub(crate) struct Acquisitions<'a> {
    steps: &'a [PlanStep],
    pending: BTreeMap<PathBuf, SharedArchive>,
}

impl<'a> Acquisitions<'a> {
    pub fn new(steps: &'a [PlanStep]) -> Self {
        Self { steps, pending: BTreeMap::new() }
    }

    pub fn acquire(&mut self, step: &PlanStep) -> Result<(), ExecError> {
        if let Some((source, archive, digest)) = archived(step) {
            let extracted = match self.pending.remove(step.dest()) {
                Some(result) => result,
                None => {
                    let group: Vec<_> = self
                        .steps
                        .iter()
                        .filter(|other| {
                            !other.dest().exists()
                                && archived(other).is_some_and(
                                    |(other_source, other_archive, other_digest)| {
                                        source == other_source
                                            && archive.format == other_archive.format
                                            && digest == other_digest
                                    },
                                )
                        })
                        .collect();
                    // A fallback is itself a step and need not be among the primary steps.
                    let mut members: Vec<_> = group
                        .iter()
                        .map(|other| archived(other).unwrap().1.member.as_str())
                        .collect();
                    members.push(&archive.member);
                    let result = load_archive(source, archive.format, digest, &members, step)
                        .map(Rc::new)
                        .map_err(Arc::new);
                    for other in group {
                        if other.dest() != step.dest() {
                            self.pending.insert(other.dest().to_path_buf(), result.clone());
                        }
                    }
                    result
                },
            }
            .map_err(ExecError::Shared)?;
            return publish_member(extracted.member(&archive.member)?, step.dest(), step.mode());
        }

        match step {
            PlanStep::Download { uri, dest, mode, digest, owner, .. } => {
                download(uri, dest, *mode, digest.as_ref(), None, owner)
            },
            PlanStep::CopyLocal { src, dest, mode, digest, owner, .. } => {
                copy_local(src, dest, *mode, digest.as_ref(), None, owner)
            },
            PlanStep::CargoBuild { .. } | PlanStep::ExtractPackage { .. } => Ok(()),
        }
    }
}

fn load_archive(
    source: Source<'_>,
    format: SupportedFormat,
    digest: Option<&Digest>,
    members: &[&str],
    step: &PlanStep,
) -> Result<archive::Extracted<TemporaryFile>, ExecError> {
    let (body, uri) = match source {
        Source::Http(uri) => (fetch(uri, step.owner())?, uri.to_string()),
        Source::Local(src) => (
            std::fs::read(src).map_err(|source| ExecError::Copy {
                src: src.to_path_buf(),
                dest: step.dest().to_path_buf(),
                source,
            })?,
            format!("file://{}", src.display()),
        ),
    };
    verify(&body, digest, &uri, step.dest(), step.owner())?;
    Ok(archive::extract(&body, format, &uri, members, || {
        TemporaryFile::new(step.dest())
    })?)
}

/// Fetches `uri` to `dest`, atomically, after checking it against `digest`. For an archive, the
/// destination filename is the member selector. Planned acquisitions use the explicit artifact key,
/// which may be a relative path.
///
/// `label` names what is being transferred for the progress display; it is the owning component,
/// never anything derived from the URL.
pub fn download(
    uri: &str,
    dest: &Path,
    mode: u32,
    digest: Option<&Digest>,
    archive: Option<SupportedFormat>,
    label: &str,
) -> Result<(), ExecError> {
    let body = fetch(uri, label)?;
    verify(&body, digest, uri, dest, label)?;
    publish(&body, archive, uri, dest, mode)
}

/// Copies `src` to `dest`, atomically, after checking it against `digest` and unpacking it when it
/// is an archive. `owner` is the component it belongs to.
pub fn copy_local(
    src: &Path,
    dest: &Path,
    mode: u32,
    digest: Option<&Digest>,
    archive: Option<SupportedFormat>,
    owner: &str,
) -> Result<(), ExecError> {
    let body = std::fs::read(src).map_err(|source| ExecError::Copy {
        src: src.to_path_buf(),
        dest: dest.to_path_buf(),
        source,
    })?;
    let source = format!("file://{}", src.display());
    verify(&body, digest, &source, dest, owner)?;
    publish(&body, archive, &source, dest, mode)
}

/// Rejects `body` unless it matches `expected`. An artifact without a declared digest passes.
fn verify(
    body: &[u8],
    expected: Option<&Digest>,
    uri: &str,
    dest: &Path,
    owner: &str,
) -> Result<(), ExecError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let actual = Digest::sha256(body);
    if &actual == expected {
        return Ok(());
    }
    Err(ExecError::DigestMismatch {
        component: owner.to_string(),
        // The destination is named after the artifact id.
        artifact: dest.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        uri: uri.to_string(),
        expected: expected.to_string(),
        actual: actual.to_string(),
    })
}

/// Retrieves `uri`, rejecting anything that is not a usable body. `label` names the transfer in
/// the progress display.
pub fn fetch(uri: &str, label: &str) -> Result<Vec<u8>, ExecError> {
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

/// Publishes a bare file or a selected member from a fully validated archive.
fn publish(
    body: &[u8],
    archive: Option<SupportedFormat>,
    source: &str,
    dest: &Path,
    mode: u32,
) -> Result<(), ExecError> {
    match archive {
        Some(format) => {
            let member = dest.file_name().unwrap_or_default().to_string_lossy();
            let extracted =
                archive::extract(body, format, source, &[&member], || TemporaryFile::new(dest))?;
            publish_member(extracted.member(&member)?, dest, mode)
        },
        None => publish_with(dest, mode, |file| file.write_all(body)),
    }
}

/// Temporary storage with ownership-based cleanup on every return path.
#[derive(Debug)]
struct TemporaryFile {
    path: PathBuf,
    file: std::fs::File,
}

impl TemporaryFile {
    fn new(dest: &Path) -> std::io::Result<Self> {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let path = temporary_sibling(dest);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self { path, file })
    }
}

impl Write for TemporaryFile {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.file.write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn publish_member(member: &TemporaryFile, dest: &Path, mode: u32) -> Result<(), ExecError> {
    let mut reader = &member.file;
    publish_with(dest, mode, |file| {
        reader.rewind()?;
        std::io::copy(&mut reader, file)?;
        Ok(())
    })
}

fn publish_with(
    dest: &Path,
    mode: u32,
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> Result<(), ExecError> {
    let mut temporary = TemporaryFile::new(dest)
        .map_err(|source| ExecError::Create { path: dest.to_path_buf(), source })?;
    let unusable =
        |source: std::io::Error| ExecError::Create { path: temporary.path.clone(), source };
    write(&mut temporary.file).map_err(unusable)?;
    temporary.file.flush().map_err(unusable)?;
    temporary.file.sync_all().map_err(unusable)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .file
            .set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(unusable)?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    crate::trace!("publishing {} -> {}", temporary.path.display(), dest.display());
    std::fs::rename(&temporary.path, dest)
        .map_err(|source| ExecError::Publish { dest: dest.to_path_buf(), source })
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

        let err = download(&server.url("/x"), &dest, 0o755, None, None, "fixture")
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

        let err = download(&server.url("/x"), &dest, 0o755, None, None, "fixture")
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

        let err = download(&server.url("/x"), &dest, 0o755, None, None, "fixture")
            .expect_err("an empty body must fail");
        assert!(matches!(err, ExecError::EmptyBody { .. }), "{err}");
        assert!(!dest.exists());
    }

    #[test]
    fn a_successful_download_lands_at_the_planned_path() {
        let server = TestServer::responding(200, b"payload");
        let dir = temp();
        let dest = dir.path().join("planned-name.bin");

        download(&server.url("/x"), &dest, 0o755, None, None, "fixture").expect("should succeed");
        assert_eq!(std::fs::read(&dest).unwrap(), b"payload");
        assert!(leftovers(dir.path(), "planned-name.bin").is_empty());
    }

    /// Release artifacts are routinely served through redirects to object storage.
    #[test]
    fn redirects_are_followed() {
        let server = TestServer::redirecting(b"payload", 2);
        let dir = temp();
        let dest = dir.path().join("a.bin");

        download(&server.url("/from"), &dest, 0o755, None, None, "fixture")
            .expect("redirects must be followed");
        assert_eq!(std::fs::read(&dest).unwrap(), b"payload");
    }

    /// The destination name comes from the plan, never from the URL.
    #[test]
    fn the_destination_name_is_never_derived_from_the_uri() {
        let server = TestServer::responding(200, b"payload");
        let dir = temp();
        let dest = dir.path().join("nothing-like-the-url");

        download(&server.url("/some/deep/path/other-name"), &dest, 0o755, None, None, "fixture")
            .unwrap();
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

            download(&server.url("/x"), &dest, 0o644, None, None, "fixture").unwrap();
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

    /// A gzipped tarball of several named members.
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

    #[test]
    fn a_named_file_is_selected_from_a_multi_file_archive() {
        let dir = temp();
        let src = dir.path().join("config.tar.gz");
        std::fs::write(&src, tarball_of(&[("README", b"ignored"), ("./config.yml", b"selected")]))
            .unwrap();
        let dest = dir.path().join("installed/config.yml");

        copy_local(&src, &dest, 0o644, None, Some(SupportedFormat::TarGz), "fixture").unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), b"selected");
        assert_eq!(std::fs::read_dir(dest.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn a_duplicate_selected_member_is_rejected_without_replacing_the_destination() {
        let dir = temp();
        let src = dir.path().join("config.tar.gz");
        std::fs::write(&src, tarball_of(&[("config.yml", b"one"), ("./config.yml", b"two")]))
            .unwrap();
        let dest = dir.path().join("config.yml");
        std::fs::write(&dest, b"existing").unwrap();

        let err = copy_local(&src, &dest, 0o644, None, Some(SupportedFormat::TarGz), "fixture")
            .unwrap_err();

        assert!(err.to_string().contains("config.yml"), "{err}");
        assert!(err.to_string().contains("more than once"), "{err}");
        assert_eq!(std::fs::read(&dest).unwrap(), b"existing");
        assert_eq!(leftovers(dir.path(), "config.yml"), vec!["config.tar.gz"]);
    }

    #[test]
    fn a_corrupt_archive_discards_selected_temporary_files() {
        let dir = temp();
        let src = dir.path().join("config.tar.gz");
        let mut body = tarball_of(&[("config.yml", b"selected"), ("README", b"ignored")]);
        let crc = body.len() - 8;
        body[crc] ^= 0xff;
        std::fs::write(&src, body).unwrap();
        let dest = dir.path().join("config.yml");
        std::fs::write(&dest, b"existing").unwrap();

        let err = copy_local(&src, &dest, 0o644, None, Some(SupportedFormat::TarGz), "fixture")
            .unwrap_err();

        assert!(matches!(err, ExecError::Archive(ArchiveError::Malformed { .. })), "{err}");
        assert_eq!(std::fs::read(&dest).unwrap(), b"existing");
        assert_eq!(leftovers(dir.path(), "config.yml"), vec!["config.tar.gz"]);
    }

    /// The archived file lands at the planned path with the planned name and mode.
    #[test]
    fn an_archived_download_installs_the_file_the_archive_holds() {
        let body = Box::leak(tarball("vm-0.16.0/miden-vm", b"binary").into_boxed_slice());
        let server = TestServer::responding(200, body);
        let dir = temp();
        let dest = dir.path().join("miden-vm");

        download(
            &server.url("/vm.tar.gz"),
            &dest,
            0o755,
            None,
            Some(SupportedFormat::TarGz),
            "fixture",
        )
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

        copy_local(&src, &dest, 0o644, None, Some(SupportedFormat::TarGz), "fixture")
            .expect("should succeed");
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
            None,
            Some(SupportedFormat::TarGz),
            "fixture",
        )
        .expect_err("must fail");
        assert!(matches!(err, ExecError::Archive(_)), "{err}");
        assert!(!dest.exists(), "the container must never be installed as the artifact");
        assert!(leftovers(dir.path(), "miden-vm").is_empty(), "no partial files");
    }

    /// A multi-file archive without the requested member must not publish an arbitrary file.
    #[test]
    fn an_archive_missing_the_requested_member_writes_nothing() {
        let dir = temp();
        let src = dir.path().join("two.tar.gz");
        std::fs::write(&src, tarball_of(&[("first", b"one"), ("second", b"two")])).unwrap();
        let dest = dir.path().join("miden-vm");

        let err = copy_local(&src, &dest, 0o755, None, Some(SupportedFormat::TarGz), "fixture")
            .expect_err("must fail");
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

        copy_local(&src, &dest, 0o644, None, None, "fixture").expect("should succeed");
        assert_eq!(std::fs::read(&dest).unwrap(), b"local", "parent dirs must be created");
    }

    #[test]
    fn a_missing_local_source_is_an_error() {
        let dir = temp();
        let err = copy_local(
            &dir.path().join("nope"),
            &dir.path().join("dest"),
            0o644,
            None,
            None,
            "fixture",
        )
        .expect_err("must fail");
        assert!(matches!(err, ExecError::Copy { .. }), "{err}");
    }

    #[test]
    fn bytes_matching_their_digest_are_installed() {
        let dir = temp();
        let src = dir.path().join("source");
        std::fs::write(&src, b"local").unwrap();
        let dest = dir.path().join("dest");

        copy_local(&src, &dest, 0o644, Some(&Digest::sha256(b"local")), None, "fixture")
            .expect("matching bytes must install");
        assert_eq!(std::fs::read(&dest).unwrap(), b"local");
    }

    /// A mismatch names everything needed to report it, and writes nothing.
    #[test]
    fn a_download_not_matching_its_digest_writes_nothing() {
        let server = TestServer::responding(200, b"tampered");
        let dir = temp();
        let dest = dir.path().join("miden-vm");
        let expected = Digest::sha256(b"payload");

        let err = download(&server.url("/x"), &dest, 0o755, Some(&expected), None, "vm")
            .expect_err("a mismatch must fail");
        let ExecError::DigestMismatch { component, artifact, actual, .. } = &err else {
            panic!("expected a digest mismatch, got {err}");
        };
        assert_eq!((component.as_str(), artifact.as_str()), ("vm", "miden-vm"));
        assert_eq!(actual, &Digest::sha256(b"tampered").to_string());
        for part in [expected.to_string(), actual.to_string()] {
            assert!(err.to_string().contains(&part), "the message must name {part}: {err}");
        }
        assert!(!dest.exists());
        assert!(leftovers(dir.path(), "miden-vm").is_empty(), "no partial files");
    }

    /// The digest covers the archive as transferred, so corrupt bytes are caught before the
    /// archive reader sees them.
    #[test]
    fn a_corrupt_archive_fails_its_digest_before_extraction() {
        let dir = temp();
        let good = tarball("packages/core.masp", b"package");
        let src = dir.path().join("core.tar.gz");
        std::fs::write(&src, &good[..good.len() / 2]).unwrap();
        let dest = dir.path().join("core.masp");

        let err = copy_local(
            &src,
            &dest,
            0o644,
            Some(&Digest::sha256(&good)),
            Some(SupportedFormat::TarGz),
            "core",
        )
        .expect_err("must fail");
        assert!(matches!(err, ExecError::DigestMismatch { .. }), "{err}");
        assert!(!dest.exists());
        assert!(leftovers(dir.path(), "core.tar.gz").is_empty(), "no partial files");
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
