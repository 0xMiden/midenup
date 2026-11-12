//! `midenup self update`: replacing the running executable with the latest release.
//!
//! The release is chosen once, from GitHub's latest-release metadata, and every asset is fetched
//! from that release's own URLs, so a release published midway cannot mix into the update. The
//! archive must match the release's `SHA256SUMS` before anything is unpacked, and the executable
//! is replaced by a single rename of a fully written sibling: before that rename, any failure
//! leaves the installed midenup untouched. The new executable is never run here; it converts the
//! home, if it needs to, on its next ordinary invocation.

use std::cmp::Ordering;

use anyhow::{Context, anyhow, bail};
use semver::Version;
use sha2::{Digest, Sha256};

use crate::{artifact::SupportedFormat, config, install::download};

/// Where the latest release is described.
const LATEST_RELEASE_URI: &str = "https://api.github.com/repos/0xMiden/midenup/releases/latest";

/// Overrides [LATEST_RELEASE_URI], so tests can serve releases locally.
pub const MIDENUP_RELEASE_URI_ENV: &str = "MIDENUP_RELEASE_URI";

/// The release asset listing a SHA-256 digest for each of the others.
const CHECKSUMS: &str = "SHA256SUMS";

/// The name of the executable inside a release archive.
const ARCHIVE_MEMBER: &str = "midenup";

/// The subset of GitHub's release metadata this needs.
#[derive(Debug, serde::Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Debug, serde::Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

impl Release {
    fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|asset| asset.name == name)
    }
}

pub fn update(config: &config::Config) -> anyhow::Result<()> {
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .context("this midenup's own version is not valid semver")?;
    let release_uri =
        std::env::var(MIDENUP_RELEASE_URI_ENV).unwrap_or_else(|_| LATEST_RELEASE_URI.to_string());

    let release = latest_release(&release_uri)?;
    let version = release_version(&release)?;
    if version.cmp_precedence(&current) != Ordering::Greater {
        crate::info!("midenup {current} is up to date");
        return Ok(());
    }

    let archive_name = format!("midenup-{}.tar.gz", config.target);
    let Some(archive) = release.asset(&archive_name) else {
        bail!(
            "midenup {version} has no prebuilt binary for {target}. To build it from source, run \
             `cargo install midenup --locked --force`",
            target = config.target
        );
    };
    let checksums = release
        .asset(CHECKSUMS)
        .ok_or_else(|| anyhow!("midenup {version} does not publish {CHECKSUMS}"))?;

    let executable = super::running_executable()?;
    let mode = executable_mode(&executable)?;

    crate::info!("updating midenup {current} -> {version}");
    let checksums = download::fetch(&checksums.browser_download_url, CHECKSUMS)?;
    let expected = expected_digest(&checksums, &archive_name)?;
    let body = download::fetch(&archive.browser_download_url, &archive_name)?;
    let actual = hex(&Sha256::digest(&body));
    if actual != expected {
        bail!(
            "the SHA-256 digest of '{}' is {actual}, but {CHECKSUMS} lists {expected}",
            archive.browser_download_url
        );
    }

    download::publish_executable(
        &body,
        SupportedFormat::TarGz,
        &archive.browser_download_url,
        ARCHIVE_MEMBER,
        &executable,
        mode,
    )
    .with_context(|| format!("failed to replace '{}'", executable.display()))?;

    crate::info!("updated midenup to {version} at '{}'", executable.display());
    Ok(())
}

fn latest_release(uri: &str) -> anyhow::Result<Release> {
    let body = download::fetch(uri, "release metadata")?;
    let release: Release = serde_json::from_slice(&body)
        .with_context(|| format!("the release metadata at '{uri}' is malformed"))?;
    if release.draft || release.prerelease {
        bail!(
            "the latest release at '{uri}' is a draft or prerelease ('{}')",
            release.tag_name
        );
    }
    Ok(release)
}

/// The release's version, from a tag such as `v1.3.0`. A malformed one is an error rather than
/// permission to install.
fn release_version(release: &Release) -> anyhow::Result<Version> {
    let tag = &release.tag_name;
    let version = Version::parse(tag.strip_prefix('v').unwrap_or(tag))
        .with_context(|| format!("the latest release's tag '{tag}' is not a version"))?;
    if !version.pre.is_empty() {
        bail!("the latest release's tag '{tag}' is a prerelease");
    }
    Ok(version)
}

/// The digest `SHA256SUMS` lists for `name`, in `sha256sum` format. Exactly one well-formed entry
/// must name it.
fn expected_digest(checksums: &[u8], name: &str) -> anyhow::Result<String> {
    let checksums =
        std::str::from_utf8(checksums).map_err(|_| anyhow!("{CHECKSUMS} is not valid UTF-8"))?;
    let mut matches = checksums.lines().filter_map(|line| {
        let (digest, file) = line.trim().split_once(char::is_whitespace)?;
        // `sha256sum` marks binary-mode entries with a leading `*`.
        let file = file.trim_start();
        let file = file.strip_prefix('*').unwrap_or(file);
        (file == name).then_some(digest)
    });
    let (Some(digest), None) = (matches.next(), matches.next()) else {
        bail!("{CHECKSUMS} must list exactly one digest for '{name}'");
    };
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("{CHECKSUMS} lists a malformed digest for '{name}': '{digest}'");
    }
    Ok(digest.to_ascii_lowercase())
}

/// The permissions the replacement keeps.
fn executable_mode(executable: &std::path::Path) -> anyhow::Result<u32> {
    let metadata = std::fs::metadata(executable)
        .with_context(|| format!("failed to read '{}'", executable.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        Ok(metadata.permissions().mode() & 0o7777)
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Ok(0o755)
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "4a8a9154adfd650c8fdfafae96cc06151f94a8356c15ffc94e934698adec4979";

    #[test]
    fn the_listed_digest_is_found_in_either_sha256sum_mode() {
        for line in [format!("{DIGEST}  midenup-x.tar.gz"), format!("{DIGEST} *midenup-x.tar.gz")] {
            let sums = format!("{}  other.sh\n{line}\n", "0".repeat(64));
            assert_eq!(expected_digest(sums.as_bytes(), "midenup-x.tar.gz").unwrap(), DIGEST);
        }
        let upper = format!("{}  midenup-x.tar.gz", DIGEST.to_uppercase());
        assert_eq!(expected_digest(upper.as_bytes(), "midenup-x.tar.gz").unwrap(), DIGEST);
    }

    #[test]
    fn a_missing_duplicate_or_malformed_digest_is_refused() {
        let missing = format!("{DIGEST}  other.tar.gz\n");
        let duplicate = format!("{DIGEST}  a.tar.gz\n{DIGEST}  a.tar.gz\n");
        let malformed = "abc123  a.tar.gz\n".to_string();
        for sums in [missing, duplicate, malformed] {
            expected_digest(sums.as_bytes(), "a.tar.gz").expect_err(&sums);
        }
        // A name that merely contains the asset's is a different asset.
        let prefixed = format!("{DIGEST}  a.tar.gz.sig\n");
        expected_digest(prefixed.as_bytes(), "a.tar.gz").expect_err("a different file");
    }

    fn release(tag: &str) -> Release {
        Release {
            tag_name: tag.to_string(),
            draft: false,
            prerelease: false,
            assets: vec![],
        }
    }

    #[test]
    fn release_tags_are_versions_with_or_without_a_v() {
        assert_eq!(release_version(&release("v1.4.0")).unwrap(), Version::new(1, 4, 0));
        assert_eq!(release_version(&release("1.4.0")).unwrap(), Version::new(1, 4, 0));
        for tag in ["latest", "v1.4", "v1.4.0-rc.1"] {
            release_version(&release(tag)).expect_err(tag);
        }
    }

    #[test]
    fn build_metadata_does_not_make_a_release_newer() {
        let built = Version::parse("1.3.0+abc").unwrap();
        assert_eq!(built.cmp_precedence(&Version::new(1, 3, 0)), Ordering::Equal);
    }
}
