//! Getting the file out of an archived artifact.
//!
//! An artifact is exactly one installed file (spec section 6.1), so the archive must hold exactly
//! one file. That file is streamed to a writer the caller owns rather than returned as bytes: a
//! release binary runs to hundreds of megabytes, and the compressed body it comes out of is still
//! in hand while it is being unpacked. Nothing but the artifact is written anywhere.
//!
//! # Adding a new supported format
//!
//! A reader beside [from_tar], and an arm for it in [extract]. A reader takes a stream, so any
//! decompression composes in front of it, and owes that stream a read through to EOF; one needing
//! random access -- a zip's central directory sits at the end -- can take the whole body instead.
//! The size limit lives in [extract], where it bounds every byte any reader pulls.

use std::io::{Read, Write};

use crate::artifact::SupportedFormat;

/// 2 GB limit on decompressed archive size.
const MAX_UNPACKED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("failed to read the archive fetched from '{uri}': {reason}")]
    Malformed { uri: String, reason: String },
    #[error("the archive fetched from '{uri}' contains more than one file")]
    NonZeroFiles { uri: String },
    #[error("the archive fetched from '{uri}' contains no files at all")]
    Empty { uri: String },
    #[error("the archive fetched from '{uri}' unpacks to more than {limit} bytes")]
    TooLarge { uri: String, limit: u64 },
    #[error("failed to write the file unpacked from '{uri}': {reason}")]
    Unwritable { uri: String, reason: String },
}

/// Writes the one file `body` holds, as packaged by `format`, to `out`.
///
/// Every format has a reader by construction: [SupportedFormat] is exactly the set this build can
/// read, so there is no unreadable case to report from here.
///
/// `uri` is only used to say what failed: the bytes are already in hand.
///
/// A failure can leave `out` written to: the file is streamed out before the trailer proving the
/// archive intact has been read, and a second file only turns up after the first has been handed
/// over. What arrived is the artifact when this returns `Ok`, and is to be discarded otherwise.
pub fn extract(
    body: &[u8],
    format: SupportedFormat,
    uri: &str,
    out: &mut impl Write,
) -> Result<(), ArchiveError> {
    extract_bounded(body, format, uri, out, MAX_UNPACKED_BYTES)
}

/// [extract], against a stated `limit`.
fn extract_bounded(
    body: &[u8],
    format: SupportedFormat,
    uri: &str,
    out: &mut impl Write,
    limit: u64,
) -> Result<(), ArchiveError> {
    // One past the limit, so that running out of allowance is distinguishable from a stream that
    // ended on it.
    let mut stream = match format {
        SupportedFormat::TarGz => flate2::read::GzDecoder::new(body).take(limit + 1),
    };

    let result = from_tar(&mut stream, uri, out);

    // Ahead of `result`: a stream cut off at the limit reaches the format reader as one that ended,
    // which it may well accept, and a corrupt archive is the wrong thing to report about an archive
    // that is merely too big.
    if stream.limit() == 0 {
        return Err(ArchiveError::TooLarge { uri: uri.to_string(), limit });
    }

    result
}

/// Streams the sole regular file out of a tar stream to `out`.
///
/// The stream is read through to EOF even once the tar data is accounted for, because a
/// decompressor in front of it only verifies its checksum and length trailer when a read pushes
/// past the compressed data, and the tar end-of-archive blocks come first. Stopping at the end of
/// the tar takes a truncated or tampered archive for an intact one.
fn from_tar<R: Read>(stream: R, uri: &str, out: &mut impl Write) -> Result<(), ArchiveError> {
    let malformed = |err: std::io::Error| ArchiveError::Malformed {
        uri: uri.to_string(),
        reason: err.to_string(),
    };

    let mut tar = tar::Archive::new(stream);
    let entries = tar.entries().map_err(malformed)?;

    // A flag rather than the bytes: the file is gone once written out, and a decompressing stream
    // cannot be rewound to come back for it.
    let mut found = false;
    // 64 KiB sized buffer.
    let mut buffer = [0u8; 64 * 1024];

    for entry in entries {
        let mut entry = entry.map_err(malformed)?;

        // A directory is not a candidate, and neither is a symlink or any other special entry.
        if !entry.header().entry_type().is_file() {
            continue;
        }

        // A second file settles it, so there is nothing to learn from the rest of the stream.
        if found {
            return Err(ArchiveError::NonZeroFiles { uri: uri.to_string() });
        }
        found = true;

        // Copied by hand rather than with `io::copy`, which gives both sides one error type: a
        // destination that cannot be written to says nothing about whether the archive reads.
        loop {
            let read = entry.read(&mut buffer).map_err(malformed)?;
            if read == 0 {
                break;
            }
            out.write_all(&buffer[..read]).map_err(|err| ArchiveError::Unwritable {
                uri: uri.to_string(),
                reason: err.to_string(),
            })?;
        }
    }

    // Read for the errors it raises rather than for the bytes: whatever follows the tar is the
    // stream's own bookkeeping, and there is nothing left to hand back a decoded byte to.
    let mut rest = tar.into_inner();
    std::io::copy(&mut rest, &mut std::io::sink()).map_err(malformed)?;

    if !found {
        return Err(ArchiveError::Empty { uri: uri.to_string() });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use flate2::{Compression, write::GzEncoder};

    use super::*;

    const URI: &str = "https://example.invalid/artifact.tar.gz";

    /// A gzip stream ends with a CRC32 of the decompressed bytes and their length, four bytes each.
    const TRAILER: usize = 8;

    /// What [extract] writes out for `body`, or the error it refuses with.
    fn extracted(body: &[u8]) -> Result<Vec<u8>, ArchiveError> {
        let mut out = Vec::new();
        extract(body, SupportedFormat::TarGz, URI, &mut out).map(|()| out)
    }

    /// What [extract] writes out for `body` under a limit of `limit`, or the error it refuses with.
    fn extracted_under(limit: u64, body: &[u8]) -> Result<Vec<u8>, ArchiveError> {
        let mut out = Vec::new();
        extract_bounded(body, SupportedFormat::TarGz, URI, &mut out, limit).map(|()| out)
    }

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
        let extracted = extracted(&nested()).expect("should extract");
        assert_eq!(extracted, b"binary", "the directory entry must not count as a file");
    }

    /// Bigger than the copy buffer, so the chunk loop is exercised across several reads.
    #[test]
    fn a_file_larger_than_the_copy_buffer_arrives_whole() {
        let contents: Vec<u8> = (0..300_000u32).map(|byte| byte as u8).collect();

        let extracted = extracted(&tarball(&[("big", &contents)])).expect("should extract");
        assert_eq!(extracted, contents, "every read must reach the writer, in order");
    }

    /// Which of two files is the artifact is not something to guess.
    #[test]
    fn several_files_is_an_error() {
        let err = extracted(&tarball(&[("a", b"one"), ("b", b"two")])).expect_err("must fail");
        assert!(matches!(err, ArchiveError::NonZeroFiles { .. }), "{err}");
        assert!(err.to_string().contains(URI), "the message must name the source: {err}");
    }

    #[test]
    fn an_archive_with_no_files_is_an_error() {
        let mut tar = tar::Builder::new(Vec::new());
        tar.append(&directory("empty/"), std::io::empty()).unwrap();

        let err = extracted(&gzipped(tar.into_inner().unwrap())).expect_err("must fail");
        assert!(matches!(err, ArchiveError::Empty { .. }), "{err}");
    }

    /// The tar inside is intact, so only reading the stream to its end catches this. `gzip -t`
    /// rejects the same bytes.
    #[test]
    fn a_missing_gzip_trailer_is_an_error() {
        let mut body = nested();
        body.truncate(body.len() - TRAILER);

        let err = extracted(&body).expect_err("must fail");
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

        let err = extracted(&body).expect_err("must fail");
        assert!(matches!(err, ArchiveError::Malformed { .. }), "{err}");
    }

    /// Not an archive at all -- an HTML error page served with a 200, say.
    #[test]
    fn garbage_is_reported_as_a_malformed_archive() {
        let err = extracted(b"<html>not a tarball</html>").expect_err("must fail");
        assert!(matches!(err, ArchiveError::Malformed { .. }), "{err}");
        assert!(err.to_string().contains(URI), "the message must name the source: {err}");
    }

    /// A destination that refuses the bytes is not a defect in the archive.
    #[test]
    fn a_writer_that_fails_is_reported_as_such() {
        struct Full;
        impl Write for Full {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("no space left on device"))
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let err =
            extract(&nested(), SupportedFormat::TarGz, URI, &mut Full).expect_err("must fail");
        assert!(matches!(err, ArchiveError::Unwritable { .. }), "{err}");
        assert!(err.to_string().contains(URI), "the message must name the source: {err}");
    }

    /// Compression hides how much there is to unpack, so the amount is capped as it comes out.
    #[test]
    fn an_archive_that_unpacks_past_the_limit_is_refused() {
        // What a gzip bomb is made of: all one byte, so a little compressed stands for a lot.
        let body = tarball(&[("big", &vec![0u8; 512 * 1024])]);
        assert!(body.len() < 4096, "the compressed size must say nothing about the unpacked one");

        let limit = 64 * 1024;
        let err = extracted_under(limit, &body).expect_err("must fail");

        assert!(matches!(err, ArchiveError::TooLarge { .. }), "{err}");
        assert!(err.to_string().contains(URI), "the message must name the source: {err}");
        assert!(
            err.to_string().contains(&limit.to_string()),
            "the message must name the limit: {err}"
        );
    }

    /// The limit is the amount refused, not an amount to stop short of.
    #[test]
    fn an_archive_that_ends_on_the_limit_is_accepted() {
        let body = tarball(&[("snug", b"contents")]);
        let mut plain = Vec::new();
        flate2::read::GzDecoder::new(&body[..]).read_to_end(&mut plain).unwrap();

        let extracted = extracted_under(plain.len() as u64, &body)
            .expect("a stream exactly the limit's length must be read");
        assert_eq!(extracted, b"contents");
    }
}
