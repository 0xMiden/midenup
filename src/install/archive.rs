//! Getting the file out of an archived artifact.
//!
//! An artifact is exactly one installed file (spec section 6.1), so the archive must hold exactly
//! one file. Everything happens in memory: nothing but the artifact is written anywhere.
//!
//! # Adding a new supported format
//!
//! A reader beside [from_tar], and an arm for it in [file]. A reader takes a stream, so any
//! decompression composes in front of it, and owes that stream a read through to EOF; one needing
//! random access -- a zip's central directory sits at the end -- can take the whole body instead.

use std::io::Read;

use crate::artifact::SupportedFormat;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("failed to read the archive fetched from '{uri}': {reason}")]
    Malformed { uri: String, reason: String },
    #[error("the archive fetched from '{uri}' contains more than one file")]
    NonZeroFiles { uri: String },
    #[error("the archive fetched from '{uri}' contains no files at all")]
    Empty { uri: String },
}

/// Reads the one file `body` holds, as packaged by `format`.
///
/// Every format has a reader by construction: [SupportedFormat] is exactly the set this build can
/// read, so there is no unreadable case to report from here.
///
/// `uri` is only used to say what failed: the bytes are already in hand.
pub fn file(body: &[u8], format: SupportedFormat, uri: &str) -> Result<Vec<u8>, ArchiveError> {
    match format {
        SupportedFormat::TarGz => from_tar(flate2::read::GzDecoder::new(body), uri),
    }
}

/// Reads the sole regular file out of a tar stream.
fn from_tar<R: Read>(stream: R, uri: &str) -> Result<Vec<u8>, ArchiveError> {
    let mut tar = tar::Archive::new(stream);
    let entries = tar.entries().map_err(|err| ArchiveError::Malformed {
        uri: uri.to_string(),
        reason: err.to_string(),
    })?;

    // Held as bytes rather than as a position, because a decompressing stream cannot be rewound to
    // come back for it.
    let mut only: Option<Vec<u8>> = None;

    for entry in entries {
        let mut entry = entry.map_err(|err| ArchiveError::Malformed {
            uri: uri.to_string(),
            reason: err.to_string(),
        })?;

        // A directory is not a candidate, and neither is a symlink or any other special entry.
        if !entry.header().entry_type().is_file() {
            continue;
        }

        // A second file settles it, so there is nothing to learn from the rest of the stream.
        if only.is_some() {
            return Err(ArchiveError::NonZeroFiles { uri: uri.to_string() });
        }

        let mut body = Vec::new();
        entry.read_to_end(&mut body).map_err(|err| ArchiveError::Malformed {
            uri: uri.to_string(),
            reason: err.to_string(),
        })?;
        only = Some(body);
    }

    // Read for the errors it raises rather than for the bytes: whatever follows the tar is the
    // stream's own bookkeeping, and there is nothing left to hand back a decoded byte to.
    let mut rest = tar.into_inner();
    std::io::copy(&mut rest, &mut std::io::sink()).map_err(|err| ArchiveError::Malformed {
        uri: uri.to_string(),
        reason: err.to_string(),
    })?;

    only.ok_or_else(|| ArchiveError::Empty { uri: uri.to_string() })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::{Compression, write::GzEncoder};

    use super::*;

    const URI: &str = "https://example.invalid/artifact.tar.gz";

    /// A gzip stream ends with a CRC32 of the decompressed bytes and their length, four bytes each.
    const TRAILER: usize = 8;

    /// A gzipped tar archive of `(path, contents)` entries.
    fn tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (path, contents) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, path, *contents).unwrap();
        }
        gzipped(tar.into_inner().unwrap())
    }

    fn gzipped(plain: Vec<u8>) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&plain).unwrap();
        encoder.finish().unwrap()
    }

    fn directory(path: &str) -> tar::Header {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Directory);
        header.set_size(0);
        header.set_mode(0o755);
        header.set_path(path).unwrap();
        header.set_cksum();
        header
    }

    /// A directory entry with the binary nested inside it, as most release tarballs are laid out.
    fn nested() -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        tar.append(&directory("miden-vm-aarch64-apple-darwin/"), std::io::empty())
            .unwrap();

        let mut header = tar::Header::new_gnu();
        header.set_size(6);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "miden-vm-aarch64-apple-darwin/miden-vm", &b"binary"[..])
            .unwrap();

        gzipped(tar.into_inner().unwrap())
    }

    #[test]
    fn the_sole_file_is_extracted() {
        let extracted = file(&nested(), SupportedFormat::TarGz, URI).expect("should extract");
        assert_eq!(extracted, b"binary", "the directory entry must not count as a file");
    }

    /// Which of two files is the artifact is not something to guess.
    #[test]
    fn several_files_is_an_error() {
        let err = file(&tarball(&[("a", b"one"), ("b", b"two")]), SupportedFormat::TarGz, URI)
            .expect_err("must fail");
        assert!(matches!(err, ArchiveError::NonZeroFiles { .. }), "{err}");
        assert!(err.to_string().contains(URI), "the message must name the source: {err}");
    }

    #[test]
    fn an_archive_with_no_files_is_an_error() {
        let mut tar = tar::Builder::new(Vec::new());
        tar.append(&directory("empty/"), std::io::empty()).unwrap();

        let err = file(&gzipped(tar.into_inner().unwrap()), SupportedFormat::TarGz, URI)
            .expect_err("must fail");
        assert!(matches!(err, ArchiveError::Empty { .. }), "{err}");
    }

    /// The tar inside is intact, so only reading the stream to its end catches this. `gzip -t`
    /// rejects the same bytes.
    #[test]
    fn a_missing_gzip_trailer_is_an_error() {
        let mut body = nested();
        body.truncate(body.len() - TRAILER);

        let err = file(&body, SupportedFormat::TarGz, URI).expect_err("must fail");
        assert!(matches!(err, ArchiveError::Malformed { .. }), "{err}");
        assert!(err.to_string().contains(URI), "the message must name the source: {err}");
    }

    /// An archive edited after the fact: the trailer is there and disagrees with the bytes that
    /// came out of it.
    #[test]
    fn a_gzip_checksum_that_does_not_match_is_an_error() {
        let mut body = nested();
        let crc = body.len() - TRAILER;
        body[crc] ^= 0xff;

        let err = file(&body, SupportedFormat::TarGz, URI).expect_err("must fail");
        assert!(matches!(err, ArchiveError::Malformed { .. }), "{err}");
    }

    /// Not an archive at all -- an HTML error page served with a 200, say.
    #[test]
    fn garbage_is_reported_as_a_malformed_archive() {
        let err = file(b"<html>not a tarball</html>", SupportedFormat::TarGz, URI)
            .expect_err("must fail");
        assert!(matches!(err, ArchiveError::Malformed { .. }), "{err}");
        assert!(err.to_string().contains(URI), "the message must name the source: {err}");
    }
}
