pub(crate) mod v1;
pub(crate) mod v3;
pub mod validate;
pub mod version;

use std::path::Path;

use thiserror::Error;

pub use self::v3::*;
use self::version::Compatibility;
use crate::channel::UserChannel;

pub type Alias = String;

#[derive(Error, Debug)]
pub enum ManifestError {
    #[error("manifest file is empty")]
    Empty,
    #[error("content downloaded from '{0}' is empty")]
    EmptyDownload(String),
    #[error("request for manifest from '{uri}' failed with status {code}")]
    DownloadError { uri: String, code: u32 },
    #[error("unable to execute request for '{uri}' with curl: {err}")]
    InternalCurlError { uri: String, err: String },
    #[error("manifest file is not present in `{0}`")]
    Missing(String),
    #[error("invalid channel manifest `{0}`")]
    Invalid(String),
    #[error("unsupported channel manifest URI: `{0}`")]
    Unsupported(String),
    #[error("unsupported/unknown channel manifest version `{0}`, expected {MANIFEST_VERSION}")]
    UnsupportedVersion(semver::Version),
    #[error("channel manifest v{0} requires a newer version of midenup")]
    OutdatedMidenup(semver::Version),
    #[error(
        "expected a {expected} document but found a {found} document; midenup keeps these \
         separate and will not read one as the other"
    )]
    WrongDocumentType {
        expected: &'static str,
        found: &'static str,
    },
    #[error(
        "missing or malformed `{0}` field: every manifest and state document must declare its \
         schema version as a semantic version string"
    )]
    MissingVersion(String),
    #[error(
        "conflicting alias '{alias}': defined by both '{component}' and '{prev_component}' \
         components"
    )]
    ConflictingAlias {
        prev_component: String,
        component: String,
        alias: String,
    },
}

/// Version-dispatched loading of channel manifests.
///
/// This is a namespace, not a data type: the schema version is read from a minimal header first
/// (see [version::read_version_header]) and only then is the document parsed with the matching
/// schema. Dispatching via a tagged enum would couple version detection to the shape of the whole
/// document, which defeats the point.
pub struct VersionedManifest;

impl VersionedManifest {
    pub const LOCAL_MANIFEST_URI: &str = "https://0xmiden.github.io/midenup/channel-manifest.json";
    pub const PUBLISHED_MANIFEST_URI: &str =
        "https://0xmiden.github.io/midenup/channel-manifest.json";

    /// Parses a channel manifest from `content`, and returns it in canonical form.
    pub fn parse_str(content: &str) -> Result<Manifest, ManifestError> {
        // Name the mistake rather than reporting a missing field. The two documents are kept
        // deliberately distinct, so confusing them is a recognizable error in its own right.
        if version::read_version_header(content, "state_version").is_ok()
            && version::read_version_header(content, "manifest_version").is_err()
        {
            return Err(ManifestError::WrongDocumentType {
                expected: "channel manifest",
                found: "local state",
            });
        }

        let header = version::read_version_header(content, "manifest_version")?;

        let mut manifest = match version::classify(&header.version, v3::MANIFEST_VERSION.major) {
            Compatibility::Supported => v3::Manifest::parse_str(content)?,
            Compatibility::RequiresNewer { found } => {
                return Err(ManifestError::OutdatedMidenup(found));
            },
            // v1.0.1 is the supported migration floor; anything older is rejected outright.
            Compatibility::TooOld { found } if found == v1::MANIFEST_VERSION => {
                let v1 = serde_json::from_str::<v1::Manifest>(content).map_err(|err| {
                    ManifestError::Invalid(format!("failed to parse v1 manifest: {err}"))
                })?;
                Manifest::try_from(v1)?
            },
            Compatibility::TooOld { found } => {
                return Err(ManifestError::UnsupportedVersion(found));
            },
        };

        // Sort channels by version, in ascending order
        if !manifest.channels.is_sorted_by_key(|channel| &channel.name) {
            manifest.channels.sort_by_key(|channel| channel.name.clone());
        };

        // Sort the components of each channel by name
        for channel in manifest.channels.iter_mut() {
            if !channel.components.is_sorted_by_key(|c| c.name.as_ref()) {
                channel.components.sort_by_key(|c| c.name.clone());
            }
        }

        Ok(manifest)
    }

    /// Loads a [Manifest] from the given file path.
    pub fn load_from_file(path: impl AsRef<Path>) -> Result<Manifest, ManifestError> {
        let path = path.as_ref();
        let manifest_contents = std::fs::read_to_string(path)
            .map_err(|_| ManifestError::Missing(path.display().to_string()))?;
        // This could potentially be valid if we are parsing the local manifest
        if manifest_contents.is_empty() {
            return Err(ManifestError::Empty);
        }

        Self::parse_str(&manifest_contents)
    }

    /// Loads a [Manifest] from the given URI.
    pub fn load_from(uri: impl AsRef<str>) -> Result<Manifest, ManifestError> {
        Self::parse_str(&Self::read_from(uri)?)
    }

    /// Reads the document at `uri` without parsing it.
    ///
    /// Separate from [Self::load_from] so that a caller can keep the bytes it fetched -- the cached
    /// copy of the upstream manifest is written verbatim, not re-serialized from the parsed form,
    /// so that a manifest carrying fields this build does not understand is cached exactly as
    /// published.
    pub fn read_from(uri: impl AsRef<str>) -> Result<String, ManifestError> {
        let uri = uri.as_ref();

        if let Some(manifest_path) = uri.strip_prefix("file://") {
            crate::trace!("reading {manifest_path}");
            let contents = std::fs::read_to_string(manifest_path)
                .map_err(|_| ManifestError::Missing(manifest_path.to_string()))?;
            if contents.is_empty() {
                return Err(ManifestError::Empty);
            }
            return Ok(contents);
        }

        if !uri.starts_with("https://") {
            return Err(ManifestError::Unsupported(uri.to_string()));
        }

        let mut data = Vec::new();
        let mut handle = curl::easy::Easy::new();
        let curl_error = |error: curl::Error| {
            let mut err = format!("Error code {}: ", error.code());
            err.push_str(error.description());
            ManifestError::InternalCurlError { uri: uri.to_string(), err }
        };
        handle.url(uri).map_err(curl_error)?;
        handle.follow_location(true).map_err(curl_error)?;
        {
            let mut transfer = handle.transfer();
            transfer
                .write_function(|new_data| {
                    data.extend_from_slice(new_data);
                    Ok(new_data.len())
                })
                .map_err(curl_error)?;
            crate::trace!("GET {uri}");
            transfer.perform().map_err(curl_error)?;
        }

        // *After* the transfer: read beforehand, curl has no response yet and reports 0, which
        // passes the check below and lets an error page be parsed as though it were the manifest.
        let response_code = handle.response_code().map_err(curl_error)?;
        if !(200..300).contains(&response_code) {
            return Err(ManifestError::DownloadError {
                uri: uri.to_string(),
                code: response_code,
            });
        }
        if data.is_empty() {
            return Err(ManifestError::EmptyDownload(uri.to_string()));
        }
        let manifest_data = core::str::from_utf8(&data).map_err(|err| {
            ManifestError::Invalid(format!("manifest contains invalid utf8 data: {err}"))
        })?;

        Ok(manifest_data.to_string())
    }
}

/// What a call to [`Manifest::promote`] did.
///
/// Returned rather than printed so the caller decides how to report it: `update-manifest` prints a
/// sentence, tests assert on the variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Promotion {
    Created {
        at: semver::Version,
    },
    Moved {
        from: semver::Version,
        to: semver::Version,
    },
    Unchanged,
}

impl Manifest {
    pub fn last_updated(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(self.date, 0).expect("manifest has invalid timestamp")
    }

    /// Sets the timestamp of this manifest to now in UTC seconds
    pub fn update_last_modified(&mut self) {
        self.date = chrono::Utc::now().timestamp();
    }

    pub fn remove_channel(&mut self, channel_name: semver::Version) {
        self.channels.retain(|c| c.name != channel_name);
    }

    pub fn add_channel(&mut self, channel: Channel) {
        // NOTE: If the channel already exists in the manifest, remove the old version. This happens
        // when updating
        self.channels.retain(|c| c.name != channel.name);
        self.channels.push(channel);
    }

    pub fn get_channel_by_name(&self, ver: &semver::Version) -> Option<&Channel> {
        self.channels.iter().find(|c| &c.name == ver)
    }

    pub fn get_channel_by_name_mut(&mut self, ver: &semver::Version) -> Option<&mut Channel> {
        self.channels.iter_mut().find(|c| &c.name == ver)
    }

    /// The channel version a network currently names, whether or not that channel exists here.
    pub fn network_version(&self, name: &str) -> Option<&semver::Version> {
        self.networks.get(name)
    }

    /// The channel a network currently names.
    ///
    /// `None` covers two different situations -- an undeclared network, and one naming a channel
    /// that is not in this document -- because a caller can do nothing different about them.
    /// `validate_manifest` is what distinguishes and reports the second.
    pub fn resolve_network(&self, name: &str) -> Option<&Channel> {
        self.get_channel_by_name(self.networks.get(name)?)
    }

    /// Every network naming `version`. Usually one; more once a toolchain is shared.
    pub fn networks_for(&self, version: &semver::Version) -> impl Iterator<Item = &str> {
        self.networks
            .iter()
            .filter(move |(_, named)| *named == version)
            .map(|(name, _)| name.as_str())
    }

    /// Every declared network name, in order. For diagnostics that have to list what is available.
    pub fn network_names(&self) -> impl Iterator<Item = &str> {
        self.networks.keys().map(String::as_str)
    }

    /// Points `name` at `version`, creating the network if it does not exist.
    ///
    /// Says nothing about whether `version` is a channel in this manifest, or whether moving there
    /// is a downgrade. Both are policy, and both belong to the caller that has a user to refuse.
    pub fn promote(&mut self, name: &str, version: semver::Version) -> Promotion {
        match self.networks.insert(name.to_string(), version.clone()) {
            None => Promotion::Created { at: version },
            Some(previous) if previous == version => Promotion::Unchanged,
            Some(previous) => Promotion::Moved { from: previous, to: version },
        }
    }

    /// Attempts to fetch the [Channel] corresponding to the given [UserChannel]
    pub fn get_channel(&self, channel: &UserChannel) -> Option<&Channel> {
        match channel {
            UserChannel::Version(version) => self.get_channel_by_name(version),
            UserChannel::Named(name) => self.resolve_network(name),
        }
    }

    pub fn get_channel_mut(&mut self, channel: &UserChannel) -> Option<&mut Channel> {
        match channel {
            UserChannel::Version(version) => self.get_channel_by_name_mut(version),
            UserChannel::Named(name) => {
                let version = self.networks.get(name.as_ref())?.clone();
                self.get_channel_by_name_mut(&version)
            },
        }
    }

    pub fn get_channels(&self) -> impl Iterator<Item = &Channel> {
        self.channels.iter()
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::VersionedManifest;
    use crate::{channel::UserChannel, version::Authority};

    /// A converted v1 manifest must report the *v3* version.
    #[test]
    fn converted_v1_manifest_reports_the_v3_version() {
        const FILE: &str = "file://tests/data/v1_manifest/channel-manifest.json";
        let manifest = VersionedManifest::load_from(FILE).expect("v1.0.1 must still be readable");
        assert_eq!(manifest.manifest_version(), &super::v3::MANIFEST_VERSION);
    }

    #[test]
    fn a_newer_major_version_asks_for_a_newer_midenup() {
        let err = VersionedManifest::parse_str(r#"{"manifest_version":"4.0.0","channels":[]}"#)
            .expect_err("a v4 manifest must be rejected");
        assert!(
            matches!(&err, super::ManifestError::OutdatedMidenup(v) if v.major == 4),
            "expected OutdatedMidenup, got: {err}"
        );
    }

    /// A newer *minor* is additive by construction, so it must remain readable.
    #[test]
    fn a_newer_minor_version_is_accepted() {
        VersionedManifest::parse_str(r#"{"manifest_version":"3.9.3","date":1,"channels":[]}"#)
            .expect("a newer minor must be readable");
    }

    /// v2 is not readable, and that is the point: it has no networks map, so a v2 document and a v3
    /// build would silently disagree about which channel mainnet names.
    #[test]
    fn a_v2_manifest_is_rejected() {
        let err = VersionedManifest::parse_str(r#"{"manifest_version":"2.0.0","channels":[]}"#)
            .expect_err("v2 is below the supported floor");
        assert!(
            matches!(&err, super::ManifestError::UnsupportedVersion(v) if *v == semver::Version::new(2, 0, 0)),
            "expected UnsupportedVersion(2.0.0), got: {err}"
        );
    }

    #[test]
    fn a_version_below_the_migration_floor_is_rejected() {
        let err = VersionedManifest::parse_str(r#"{"manifest_version":"1.0.0","channels":[]}"#)
            .expect_err("v1.0.0 is below the supported migration floor");
        assert!(
            matches!(&err, super::ManifestError::UnsupportedVersion(v) if *v == semver::Version::new(1, 0, 0)),
            "expected UnsupportedVersion(1.0.0), got: {err}"
        );
    }

    /// The state right after a testnet toolchain is promoted to mainnet: one channel, two names.
    /// This is the case the old per-channel `alias` field could not represent at all.
    #[test]
    fn two_networks_may_name_one_channel() {
        let src = serde_json::json!({
            "manifest_version": "3.0.0",
            "date": 1735689600,
            "networks": { "mainnet": "0.15.0", "testnet": "0.15.0", "devnet": "0.16.0" },
            "channels": [
                { "name": "0.15.0", "components": [] },
                { "name": "0.16.0", "components": [] },
            ]
        })
        .to_string();

        let manifest = VersionedManifest::parse_str(&src).expect("must parse");

        assert_eq!(manifest.network_version("mainnet"), Some(&semver::Version::new(0, 15, 0)));
        assert_eq!(
            manifest.resolve_network("testnet").map(|c| c.name.clone()),
            Some(semver::Version::new(0, 15, 0))
        );

        let mut sharing: Vec<&str> =
            manifest.networks_for(&semver::Version::new(0, 15, 0)).collect();
        sharing.sort_unstable();
        assert_eq!(sharing, vec!["mainnet", "testnet"]);

        assert!(manifest.resolve_network("nope").is_none());
    }

    /// A pointer naming a channel that is not in the document resolves to nothing rather than
    /// panicking. Validation is what reports it.
    #[test]
    fn a_dangling_network_resolves_to_nothing() {
        let src = serde_json::json!({
            "manifest_version": "3.0.0",
            "date": 1735689600,
            "networks": { "mainnet": "9.9.9" },
            "channels": [{ "name": "0.15.0", "components": [] }]
        })
        .to_string();

        let manifest = VersionedManifest::parse_str(&src).expect("parsing must stay permissive");
        assert_eq!(manifest.network_version("mainnet"), Some(&semver::Version::new(9, 9, 9)));
        assert!(manifest.resolve_network("mainnet").is_none());
    }

    #[test]
    fn promote_distinguishes_creation_movement_and_a_no_op() {
        let mut manifest = super::Manifest::default();
        manifest
            .channels
            .push(super::Channel::new(semver::Version::new(0, 15, 0), vec![]));
        manifest
            .channels
            .push(super::Channel::new(semver::Version::new(0, 16, 0), vec![]));

        assert!(matches!(
            manifest.promote("mainnet", semver::Version::new(0, 15, 0)),
            super::Promotion::Created { at } if at == semver::Version::new(0, 15, 0)
        ));
        assert!(matches!(
            manifest.promote("mainnet", semver::Version::new(0, 15, 0)),
            super::Promotion::Unchanged
        ));
        assert!(matches!(
            manifest.promote("mainnet", semver::Version::new(0, 16, 0)),
            super::Promotion::Moved { from, to }
                if from == semver::Version::new(0, 15, 0) && to == semver::Version::new(0, 16, 0)
        ));
    }

    /// The map is a `BTreeMap` so that `update-manifest format` is deterministic.
    #[test]
    fn networks_serialize_in_key_order() {
        let mut manifest = super::Manifest::default();
        manifest.promote("testnet", semver::Version::new(0, 15, 0));
        manifest.promote("mainnet", semver::Version::new(0, 15, 0));
        manifest.promote("devnet", semver::Version::new(0, 15, 0));

        let json = serde_json::to_string(&manifest).unwrap();
        let devnet = json.find("devnet").expect("devnet must be present");
        let mainnet = json.find("mainnet").expect("mainnet must be present");
        let testnet = json.find("testnet").expect("testnet must be present");
        assert!(devnet < mainnet && mainnet < testnet, "keys must be emitted in order: {json}");
    }

    /// Validates that the current channel manifest is parseable.
    #[test]
    fn validate_current_channel_manifest() {
        let manifest = VersionedManifest::load_from("file://manifest/channel-manifest.json")
            .expect("Couldn't load manifest");

        let _stable = manifest
            .get_channel(&UserChannel::Named(Cow::Borrowed("mainnet")))
            .expect("Could not convert UserChannel to internal channel representation");
    }

    /// Validates that non-standard manifest features are parsed correctly, these include:
    ///
    /// - Non stable channels (custom tags, nightly)
    /// - Components wwith git and a path as an [[Authority]].
    #[test]
    fn unit_test_manifest_additional() {
        const FILE: &str =
            "file://tests/data/unit_test_manifest_additional/manifest-non-stable.json";
        let manifest = VersionedManifest::load_from(FILE).unwrap();
        {
            let custom_build = manifest
                .get_channel(&UserChannel::Named(Cow::Borrowed("custom-dev-build")))
                .unwrap_or_else(|| {
                    panic!(
                        "Could not convert UserChannel to internal channel representation from \
                         {FILE}",
                    )
                });

            #[allow(unused_variables)]
            {
                let prerelease = semver::Prerelease::new("custom-build").unwrap();
                assert!(matches!(&custom_build.name, semver::Version { pre: _prerelease, .. }));
            }
            // The literal the fixture declares, not `custom_build.name` -- that channel was
            // looked up *through* this network, so comparing the two would assert nothing.
            assert_eq!(
                manifest.network_version("custom-dev-build"),
                Some(&semver::Version::parse("0.16.0-custom-build").unwrap())
            );
            {
                let std_lib = custom_build
                    .get_component("std")
                    .unwrap_or_else(|| panic!("Could not find standard library in {FILE}",));

                assert!(matches!(std_lib.version, Authority::Path { .. }));
            }
        }
        {
            let nightly = manifest
                .get_channel(&UserChannel::Named(Cow::Borrowed("devnet")))
                .unwrap_or_else(|| {
                    panic!(
                        "Could not convert UserChannel to internal channel representation from \
                         {FILE}",
                    )
                });
            // `nightly` is a synonym rewritten to `devnet`, so the v1 alias lands under that
            // name. Asserted against the fixture's literal version for the reason above.
            assert_eq!(
                manifest.network_version("devnet"),
                Some(&semver::Version::parse("0.15.0-nightly").unwrap())
            );
            {
                let client = nightly
                    .get_component("client")
                    .unwrap_or_else(|| panic!("Could not find standard library in {FILE}",));

                assert!(matches!(client.version, Authority::Git { .. }));
            }
        }
    }
}
