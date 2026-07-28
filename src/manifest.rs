pub(crate) mod v1;
pub(crate) mod v2;
pub mod validate;
pub mod version;

use std::path::Path;

use thiserror::Error;

pub use self::v2::*;
use self::version::Compatibility;
use crate::channel::{ChannelAlias, UserChannel};

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

        let mut manifest = match version::classify(&header.version, v2::MANIFEST_VERSION.major) {
            Compatibility::Supported => v2::Manifest::parse_str(content)?,
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
        let uri = uri.as_ref();

        if let Some(manifest_path) = uri.strip_prefix("file://") {
            return Self::load_from_file(manifest_path);
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
            transfer.perform().map_err(curl_error)?;
        }

        // *After* the transfer. Read beforehand, curl has no response yet and reports 0, so the
        // error check never fired and an error page was parsed as though it were the manifest.
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

        Self::parse_str(manifest_data)
    }
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
        // Before adding the new stable channel, remove the stable alias from all the channels that
        // have it.
        //
        // NOTE: This should be only a single channel, we check for multiple just in case.
        if self.is_latest_stable(&channel) {
            for channel in self
                .channels
                .iter_mut()
                .filter(|c| c.alias.as_ref().is_some_and(|a| matches!(a, ChannelAlias::Stable)))
            {
                channel.alias = None
            }
        }

        // NOTE: If the channel already exists in the manifest, remove the old version. This happens
        // when updating
        self.channels.retain(|c| c.name != channel.name);

        self.channels.push(channel);
    }

    /// Determines whether the `channel` is the latest stable version.
    ///
    /// This can only be determined by the [Manifest], since this definition is dependant on all the
    /// other present [Channel]s
    pub fn is_latest_stable(&self, channel: &Channel) -> bool {
        self.channels.iter().filter(|c| c.is_stable()).all(|c| {
            let comparison = channel.name.cmp_precedence(&c.name);
            matches!(comparison, std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        })
    }

    /// Attempts to fetch the version corresponding to the `stable` [Channel].
    ///
    /// By definition this is the latest version.
    ///
    /// WARNING: This method is mainly intended to be used with the _upstream_ manifest, not the
    /// local manifest.  This is because, stable is simply defined to be "the latest non-nightly"
    /// channel in the [Manifest]. Therefore, in order to have a unified vision of what "stable"
    /// refers to, refer to the upstream [Manifest].
    pub fn get_latest_stable(&self) -> Option<&Channel> {
        self.channels
            .iter()
            .find(|c| matches!(c.alias, Some(ChannelAlias::Stable)))
            .or_else(|| {
                self.channels
                    .iter()
                    .filter(|c| c.is_stable())
                    .max_by(|x, y| x.name.cmp_precedence(&y.name))
            })
    }

    pub fn get_latest_stable_mut(&mut self) -> Option<&mut Channel> {
        let stable_version = self.get_latest_stable().map(|channel| channel.name.clone())?;
        self.get_channel_by_name_mut(&stable_version)
    }

    pub fn get_latest_nightly(&self) -> Option<&Channel> {
        self.channels.iter().find(|c| c.is_latest_nightly()).or_else(|| {
            self.channels
                .iter()
                .filter(|c| c.is_nightly())
                .max_by(|x, y| x.name.cmp_precedence(&y.name))
        })
    }

    pub fn get_latest_nightly_mut(&mut self) -> Option<&mut Channel> {
        let nightly_version = self.get_latest_nightly().map(|channel| channel.name.clone())?;
        self.get_channel_by_name_mut(&nightly_version)
    }

    pub fn get_named_nightly(&self, name: impl AsRef<str>) -> Option<&Channel> {
        self.channels.iter().find(|c| {
            c.alias.as_ref().is_some_and(
                |alias| matches!(alias, ChannelAlias::Nightly(Some(tag)) if tag == name.as_ref()),
            )
        })
    }

    pub fn get_named_nightly_mut(&mut self, name: impl AsRef<str>) -> Option<&mut Channel> {
        self.channels.iter_mut().find(|c| {
            c.alias.as_ref().is_some_and(
                |alias| matches!(alias, ChannelAlias::Nightly(Some(tag)) if tag == name.as_ref()),
            )
        })
    }

    pub fn get_channel_by_name(&self, ver: &semver::Version) -> Option<&Channel> {
        self.channels.iter().find(|c| &c.name == ver)
    }

    pub fn get_channel_by_name_mut(&mut self, ver: &semver::Version) -> Option<&mut Channel> {
        self.channels.iter_mut().find(|c| &c.name == ver)
    }

    /// Attempts to fetch the [Channel] corresponding to the given [UserChannel]
    pub fn get_channel(&self, channel: &UserChannel) -> Option<&Channel> {
        match channel {
            UserChannel::Version(v) => self.channels.iter().find(|c| &c.name == v),
            UserChannel::Stable => self.get_latest_stable(),
            UserChannel::Nightly => self.get_latest_nightly(),
            UserChannel::Other(tag) => match tag.strip_prefix("nightly-") {
                Some(suffix) => self.get_named_nightly(suffix),
                None => self.channels.iter().find(|c| {
                    c.alias.as_ref().is_some_and(|alias| {
                        matches!(alias, ChannelAlias::Tag(t) if t ==
            tag.as_ref())
                    })
                }),
            },
        }
    }

    pub fn get_channel_mut(&mut self, channel: &UserChannel) -> Option<&mut Channel> {
        match channel {
            UserChannel::Version(v) => self.channels.iter_mut().find(|c| &c.name == v),
            UserChannel::Stable => self.get_latest_stable_mut(),
            UserChannel::Nightly => self.get_latest_nightly_mut(),
            UserChannel::Other(tag) => match tag.strip_prefix("nightly-") {
                Some(suffix) => self.get_named_nightly_mut(suffix),
                None => self.channels.iter_mut().find(|c| {
                    c.alias.as_ref().is_some_and(|alias| {
                        matches!(alias, ChannelAlias::Tag(t) if t ==
                    tag.as_ref())
                    })
                }),
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
    use crate::{channel::UserChannel, manifest::ChannelAlias, version::Authority};

    /// A converted v1 manifest must report the *v2* version.
    ///
    /// Two separate defects made the in-memory version meaningless: `v2::Manifest` declared
    /// `manifest_version` as `skip_deserializing` with a default, and the v1 converter stamped the
    /// v1 constant. Together they meant every downstream version check was a tautology.
    #[test]
    fn converted_v1_manifest_reports_the_v2_version() {
        const FILE: &str = "file://tests/data/v1_manifest/channel-manifest.json";
        let manifest = VersionedManifest::load_from(FILE).expect("v1.0.1 must still be readable");
        assert_eq!(manifest.manifest_version(), &super::v2::MANIFEST_VERSION);
    }

    #[test]
    fn a_newer_major_version_asks_for_a_newer_midenup() {
        let err = VersionedManifest::parse_str(r#"{"manifest_version":"3.0.0","channels":[]}"#)
            .expect_err("a v3 manifest must be rejected");
        assert!(
            matches!(&err, super::ManifestError::OutdatedMidenup(v) if v.major == 3),
            "expected OutdatedMidenup, got: {err}"
        );
    }

    /// A newer *minor* is additive by construction, so it must remain readable.
    #[test]
    fn a_newer_minor_version_is_accepted() {
        VersionedManifest::parse_str(r#"{"manifest_version":"2.9.3","date":1,"channels":[]}"#)
            .expect("a newer minor must be readable");
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

    /// Validates that the current channel manifest is parseable.
    #[test]
    fn validate_current_channel_manifest() {
        let manifest = VersionedManifest::load_from("file://manifest/channel-manifest.json")
            .expect("Couldn't load manifest");

        let _stable = manifest
            .get_channel(&UserChannel::Stable)
            .expect("Could not convert UserChannel to internal channel representation");
    }

    /// Validates that the *published* channel manifest is parseable.
    /// NOTE: This test is mainly intended for backwards compatibilty reasons.
    #[test]
    fn validate_published_channel_manifest() {
        let manifest = VersionedManifest::load_from(VersionedManifest::PUBLISHED_MANIFEST_URI)
            .expect("Failed to parse upstream manifest.");

        let _ = manifest
            .get_channel(&UserChannel::Stable)
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
                .get_channel(&UserChannel::Other(Cow::Borrowed("custom-dev-build")))
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
            assert_eq!(
                custom_build.alias,
                Some(ChannelAlias::Tag(Cow::Borrowed("custom-dev-build")))
            );
            {
                let std_lib = custom_build
                    .get_component("std")
                    .unwrap_or_else(|| panic!("Could not find standard library in {FILE}",));

                assert!(matches!(std_lib.version, Authority::Path { .. }));
            }
        }
        {
            let nightly = manifest.get_channel(&UserChannel::Nightly).unwrap_or_else(|| {
                panic!(
                    "Could not convert UserChannel to internal channel representation from {FILE}",
                )
            });
            assert_eq!(nightly.alias, Some(ChannelAlias::Nightly(None)));
            {
                let client = nightly
                    .get_component("client")
                    .unwrap_or_else(|| panic!("Could not find standard library in {FILE}",));

                assert!(matches!(client.version, Authority::Git { .. }));
            }
        }
    }
}
