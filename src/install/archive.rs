//! Selecting installed files from an archive in one decompression pass.
//!
//! Only requested regular members are retained, plus the first regular file until a second
//! rules out sole-file compatibility. The caller supplies temporary writers, so decompressed
//! files need not live in memory. Member paths are lookup keys, never filesystem destinations.

use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    io::{Read, Write},
};

use crate::artifact::SupportedFormat;

/// 2 GiB limit on decompressed archive size, including unselected members and metadata.
const MAX_UNPACKED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("failed to read the archive fetched from '{uri}': {reason}")]
    Malformed { uri: String, reason: String },
    #[error("the archive fetched from '{uri}' contains no files at all")]
    Empty { uri: String },
    #[error("the archive fetched from '{uri}' has no regular file named '{member}'")]
    MissingMember { uri: String, member: String },
    #[error("the archive fetched from '{uri}' contains '{member}' more than once")]
    DuplicateMember { uri: String, member: String },
    #[error("the member '{member}' in the archive fetched from '{uri}' is not a regular file")]
    NotRegular { uri: String, member: String },
    #[error("the archive fetched from '{uri}' unpacks to more than {limit} bytes")]
    TooLarge { uri: String, limit: u64 },
    #[error("failed to write the file unpacked from '{uri}': {reason}")]
    Unwritable { uri: String, reason: String },
}

#[derive(Debug)]
enum MemberError {
    Duplicate,
    NotRegular,
}

/// Selected temporary files from a fully validated archive.
#[derive(Debug)]
pub struct Extracted<W> {
    files: BTreeMap<Vec<u8>, Result<W, MemberError>>,
    sole: Option<Vec<u8>>,
    uri: String,
}

impl<W> Extracted<W> {
    /// Exact relative path match, or the sole regular file for legacy single-file archives.
    pub fn member(&self, name: &str) -> Result<&W, ArchiveError> {
        let selected = self
            .files
            .get(name.as_bytes())
            .or_else(|| self.sole.as_ref().and_then(|sole| self.files.get(sole)));
        match selected {
            Some(Ok(file)) => Ok(file),
            Some(Err(MemberError::Duplicate)) => Err(ArchiveError::DuplicateMember {
                uri: self.uri.clone(),
                member: name.to_string(),
            }),
            Some(Err(MemberError::NotRegular)) => Err(ArchiveError::NotRegular {
                uri: self.uri.clone(),
                member: name.to_string(),
            }),
            None => Err(ArchiveError::MissingMember {
                uri: self.uri.clone(),
                member: name.to_string(),
            }),
        }
    }
}

/// Scans the complete archive before exposing any selected file. Duplicate and non-regular
/// selected members are per-member errors; malformed streams invalidate the entire result.
/// Every temporary writer is dropped on failure, including a candidate discarded when a second
/// regular file rules out sole-file compatibility.
pub fn extract<W: Write>(
    body: &[u8],
    format: SupportedFormat,
    uri: &str,
    requested: &[&str],
    create: impl FnMut() -> std::io::Result<W>,
) -> Result<Extracted<W>, ArchiveError> {
    extract_bounded(body, format, uri, requested, create, MAX_UNPACKED_BYTES)
}

fn extract_bounded<W: Write>(
    body: &[u8],
    format: SupportedFormat,
    uri: &str,
    requested: &[&str],
    create: impl FnMut() -> std::io::Result<W>,
    limit: u64,
) -> Result<Extracted<W>, ArchiveError> {
    let mut stream = match format {
        SupportedFormat::TarGz => flate2::read::MultiGzDecoder::new(body).take(limit + 1),
    };
    let result = from_tar(&mut stream, uri, requested, create);
    if stream.limit() == 0 {
        return Err(ArchiveError::TooLarge { uri: uri.to_string(), limit });
    }
    result
}

fn from_tar<W: Write>(
    stream: impl Read,
    uri: &str,
    requested: &[&str],
    mut create: impl FnMut() -> std::io::Result<W>,
) -> Result<Extracted<W>, ArchiveError> {
    let malformed = |err: std::io::Error| ArchiveError::Malformed {
        uri: uri.to_string(),
        reason: err.to_string(),
    };
    let unwritable = |err: std::io::Error| ArchiveError::Unwritable {
        uri: uri.to_string(),
        reason: err.to_string(),
    };
    let requested: BTreeSet<&[u8]> = requested.iter().map(|name| name.as_bytes()).collect();
    let mut extracted = Extracted {
        files: BTreeMap::new(),
        sole: None,
        uri: uri.to_string(),
    };
    let mut count = 0;
    let mut tar = tar::Archive::new(stream);
    // Concatenated tar data cannot hide later members or arbitrary trailing payload.
    tar.set_ignore_zeros(true);
    let mut buffer = [0u8; 64 * 1024];

    for entry in tar.entries().map_err(malformed)? {
        let mut entry = entry.map_err(malformed)?;
        let path = entry.path_bytes();
        let mut name = path.as_ref();
        while let Some(rest) = name.strip_prefix(b"./") {
            name = rest;
        }
        let name = name.to_vec();
        let selected = requested.contains(name.as_slice());
        if !entry.header().entry_type().is_file() {
            if selected {
                extracted.files.insert(name, Err(MemberError::NotRegular));
            }
            continue;
        }

        count += 1;
        if count == 1 {
            extracted.sole = Some(name.clone());
        } else if count == 2 {
            extracted.sole = None;
            extracted.files.retain(|name, _| requested.contains(name.as_slice()));
        }
        if !selected && count != 1 {
            continue;
        }
        let slot = match extracted.files.entry(name) {
            Entry::Occupied(mut slot) => {
                *slot.get_mut() = Err(MemberError::Duplicate);
                continue;
            },
            Entry::Vacant(slot) => slot,
        };

        let mut file = create().map_err(unwritable)?;
        loop {
            let read = entry.read(&mut buffer).map_err(malformed)?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read]).map_err(unwritable)?;
        }
        slot.insert(Ok(file));
    }

    // Gzip trailers are validated only after the decompressed tar reaches EOF.
    std::io::copy(&mut tar.into_inner(), &mut std::io::sink()).map_err(malformed)?;
    if count == 0 {
        return Err(ArchiveError::Empty { uri: uri.to_string() });
    }
    Ok(extracted)
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
        extract(body, SupportedFormat::TarGz, URI, &["artifact"], || Ok(Vec::new()))?
            .member("artifact")
            .cloned()
    }

    /// What [extract] writes out for `body` under a limit of `limit`, or the error it refuses with.
    fn extracted_under(limit: u64, body: &[u8]) -> Result<Vec<u8>, ArchiveError> {
        extract_bounded(body, SupportedFormat::TarGz, URI, &["artifact"], || Ok(Vec::new()), limit)?
            .member("artifact")
            .cloned()
    }

    /// A tar archive of `(path, contents)` entries.
    fn tar(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut tar = tar::Builder::new(Vec::new());
        for (path, contents) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, path, *contents).unwrap();
        }
        tar.into_inner().unwrap()
    }

    /// A gzipped tar archive of `(path, contents)` entries.
    fn tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
        gzipped(tar(entries))
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

    /// Multi-file archives require the requested member, never an arbitrary first file.
    #[test]
    fn a_missing_member_in_a_multi_file_archive_is_an_error() {
        let err = extracted(&tarball(&[("a", b"one"), ("b", b"two")])).expect_err("must fail");
        assert!(matches!(err, ArchiveError::MissingMember { .. }), "{err}");
        assert!(err.to_string().contains(URI), "the message must name the source: {err}");
    }

    /// End markers separate concatenated tar archives but do not make later files disappear.
    #[test]
    fn files_after_a_tar_end_marker_are_still_counted() {
        let mut joined = tar(&[("a", b"one")]);
        joined.extend(tar(&[("b", b"two")]));

        let err = extracted(&gzipped(joined)).expect_err("must fail");
        assert!(matches!(err, ArchiveError::MissingMember { .. }), "{err}");
    }

    #[test]
    fn selected_members_match_exact_relative_paths_and_ignore_unrequested_files() {
        let body = tarball(&[
            ("other/config.yml", b"other"),
            ("./nested/config.yml", b"nested"),
            ("./config.yml", b"top-level"),
            ("README", b"ignored"),
        ]);
        let files = extract(
            &body,
            SupportedFormat::TarGz,
            URI,
            &["config.yml", "nested/config.yml"],
            || Ok(Vec::new()),
        )
        .unwrap();
        assert_eq!(files.member("config.yml").unwrap(), b"top-level");
        assert_eq!(files.member("nested/config.yml").unwrap(), b"nested");
        assert!(matches!(files.member("missing"), Err(ArchiveError::MissingMember { .. })));
    }

    #[test]
    fn a_later_matching_member_does_not_retain_bytes_from_the_first_candidate() {
        let body = tarball(&[("README", b"long unrequested contents"), ("config.yml", b"x")]);
        let files = extract(&body, SupportedFormat::TarGz, URI, &["config.yml"], || Ok(Vec::new()))
            .unwrap();
        assert_eq!(files.member("config.yml").unwrap(), b"x");
    }

    #[test]
    fn a_selected_link_cannot_fall_back_to_the_sole_regular_file() {
        let mut tar = tar::Builder::new(Vec::new());
        let mut link = tar::Header::new_gnu();
        link.set_entry_type(tar::EntryType::Symlink);
        link.set_size(0);
        link.set_mode(0o777);
        link.set_link_name("payload").unwrap();
        link.set_cksum();
        tar.append_data(&mut link, "config.yml", std::io::empty()).unwrap();
        let mut file = tar::Header::new_gnu();
        file.set_size(7);
        file.set_mode(0o644);
        file.set_cksum();
        tar.append_data(&mut file, "payload", &b"content"[..]).unwrap();
        let files = extract(
            &gzipped(tar.into_inner().unwrap()),
            SupportedFormat::TarGz,
            URI,
            &["config.yml"],
            || Ok(Vec::new()),
        )
        .unwrap();
        assert!(matches!(files.member("config.yml"), Err(ArchiveError::NotRegular { .. })));
    }

    #[test]
    fn a_duplicate_requested_member_does_not_invalidate_other_members() {
        let mut joined = tar(&[("config.yml", b"first"), ("valid.yml", b"valid")]);
        joined.extend(tar(&[("./config.yml", b"duplicate")]));
        let files = extract(
            &gzipped(joined),
            SupportedFormat::TarGz,
            URI,
            &["config.yml", "valid.yml"],
            || Ok(Vec::new()),
        )
        .unwrap();
        assert!(matches!(files.member("config.yml"), Err(ArchiveError::DuplicateMember { .. })));
        assert_eq!(files.member("valid.yml").unwrap(), b"valid");
    }

    #[test]
    fn selected_files_are_not_returned_when_a_later_gzip_trailer_is_corrupt() {
        let mut body = tarball(&[("config.yml", b"selected"), ("README", b"ignored")]);
        body.extend(gzipped(vec![0; 1024]));
        let crc = body.len() - TRAILER;
        body[crc] ^= 0xff;
        let result =
            extract(&body, SupportedFormat::TarGz, URI, &["config.yml"], || Ok(Vec::new()));
        assert!(matches!(result, Err(ArchiveError::Malformed { .. })));
    }

    #[test]
    fn unselected_members_still_count_toward_the_decompression_limit() {
        let body = tarball(&[("config.yml", b"selected"), ("unselected", &vec![0; 128 * 1024])]);
        let result = extract_bounded(
            &body,
            SupportedFormat::TarGz,
            URI,
            &["config.yml"],
            || Ok(Vec::new()),
            64 * 1024,
        );
        assert!(matches!(result, Err(ArchiveError::TooLarge { .. })));
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

    /// Every member belongs to the fetched gzip stream, so every trailer must be valid.
    #[test]
    fn a_later_gzip_member_checksum_must_match() {
        let mut body = nested();
        // Zero blocks are valid tar padding, ensuring the tar reader reaches the member's trailer.
        body.extend(gzipped(vec![0; 1024]));
        assert_eq!(extracted(&body).expect("every valid member should be accepted"), b"binary");

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
        #[derive(Debug)]
        struct Full;
        impl Write for Full {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("no space left on device"))
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let err = extract(&nested(), SupportedFormat::TarGz, URI, &["artifact"], || Ok(Full))
            .expect_err("must fail");
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
        flate2::read::MultiGzDecoder::new(&body[..]).read_to_end(&mut plain).unwrap();

        let extracted = extracted_under(plain.len() as u64, &body)
            .expect("a stream exactly the limit's length must be read");
        assert_eq!(extracted, b"contents");
    }
}
