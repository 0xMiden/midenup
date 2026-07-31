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

        Ok(manifest_data.to_string())
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
        // Every named pointer -- the `stable` alias, a network -- names exactly one channel, so
        // binding it to this one un-binds whatever held it before. The pointer moves; the channels
        // stay. That is what makes promotion a one-line manifest edit rather than a re-authoring.
        //
        // Note that the condition is the incoming channel's own declaration, never a version
        // comparison: sorting highest is not a claim to a name.
        if channel.is_stable() {
            for existing in self.channels.iter_mut().filter(|c| c.is_stable()) {
                existing.alias = None;
            }
        }

        if let Some(network) = channel.network() {
            let network = network.clone();
            for existing in self.channels.iter_mut().filter(|c| c.network() == Some(&network)) {
                existing.network = None;
            }
        }

        // NOTE: If the channel already exists in the manifest, remove the old version. This happens
        // when updating
        self.channels.retain(|c| c.name != channel.name);

        self.channels.push(channel);
    }

    /// The channel carrying the `stable` alias, if any.
    ///
    /// There is no version-ordering fallback. A manifest that declares no stable channel has none,
    /// and callers say so rather than promoting whichever channel happens to sort highest.
    ///
    /// WARNING: intended for the _upstream_ manifest. Local state records channel versions, not
    /// aliases, so `stable` is resolved locally through the derived `toolchains/stable` symlink.
    pub fn get_latest_stable(&self) -> Option<&Channel> {
        self.channels.iter().find(|c| c.is_stable())
    }

    pub fn get_latest_stable_mut(&mut self) -> Option<&mut Channel> {
        let stable_version = self.get_latest_stable().map(|channel| channel.name.clone())?;
        self.get_channel_by_name_mut(&stable_version)
    }

    /// The channel pointed at the given network, if any.
    pub fn get_channel_by_network(&self, network: &crate::channel::Network) -> Option<&Channel> {
        self.channels.iter().find(|c| c.network() == Some(network))
    }

    /// The channel a network *name* points at.
    ///
    /// Matches on the rendered name rather than a parsed [`crate::channel::Network`] so that a
    /// network this build does not know about is still selectable: there is no list of known
    /// networks to fall off.
    pub fn get_channel_by_network_name(&self, name: &str) -> Option<&Channel> {
        self.channels
            .iter()
            .find(|c| c.network().is_some_and(|n| n.to_string() == name))
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
            // An ad-hoc name resolves as an explicit tag first, then as a network.
            //
            // Tags before networks so that a manifest can always override a name it also uses as a
            // network; in practice the two namespaces do not overlap.
            UserChannel::Other(name) => self
                .channels
                .iter()
                .find(|c| {
                    c.alias.as_ref().is_some_and(
                        |alias| matches!(alias, ChannelAlias::Tag(t) if t == name.as_ref()),
                    )
                })
                .or_else(|| self.get_channel_by_network_name(name.as_ref())),
        }
    }

    pub fn get_channel_mut(&mut self, channel: &UserChannel) -> Option<&mut Channel> {
        match channel {
            UserChannel::Version(v) => self.channels.iter_mut().find(|c| &c.name == v),
            UserChannel::Stable => self.get_latest_stable_mut(),
            // Same order as `get_channel`, resolved to a version first so the borrow checker is not
            // asked to hold two candidate mutable borrows at once.
            UserChannel::Other(name) => {
                let found = self
                    .channels
                    .iter()
                    .find(|c| {
                        c.alias.as_ref().is_some_and(
                            |alias| matches!(alias, ChannelAlias::Tag(t) if t == name.as_ref()),
                        )
                    })
                    .or_else(|| self.get_channel_by_network_name(name.as_ref()))
                    .map(|c| c.name.clone())?;
                self.get_channel_by_name_mut(&found)
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

    use super::{Channel, Manifest, VersionedManifest};
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

    /// The shipped manifest's networked pre-release channel must not be reachable as `stable`.
    ///
    /// The failure mode this guards is silent: if `stable` were derived from version ordering, a
    /// channel added for `devnet` would become what a bare `midenup install` gets purely by sorting
    /// highest. `stable` is declared, so a devnet channel simply is not it.
    #[test]
    fn the_shipped_devnet_channel_is_not_stable() {
        let manifest = VersionedManifest::load_from("file://manifest/channel-manifest.json")
            .expect("Couldn't load manifest");

        let devnet = manifest
            .get_channel(&UserChannel::Other("devnet".into()))
            .expect("the shipped manifest points devnet at a channel");
        let stable = manifest.get_channel(&UserChannel::Stable).expect("and declares a stable one");

        assert_ne!(devnet.name, stable.name, "devnet must not also be the stable channel");
        assert!(!devnet.is_stable(), "a devnet channel is not stable unless it says so");
        assert!(stable.is_stable(), "the stable channel declares the alias");
        assert!(
            stable.name < devnet.name,
            "expected devnet to be the newer channel, got stable={} devnet={}",
            stable.name,
            devnet.name
        );
    }

    /// `stable` is declared. A higher, unaliased channel does not take it.
    ///
    /// The property under test is that publishing a channel does not promote it.
    #[test]
    fn stable_is_declared_not_derived() {
        use crate::channel::ChannelAlias;

        let mut manifest = Manifest::default();
        manifest.add_channel(Channel::new(
            semver::Version::new(0, 15, 0),
            Some(ChannelAlias::Stable),
            vec![],
        ));
        // Higher, and released -- but nothing declares it stable, so it is not.
        manifest.add_channel(Channel::new(semver::Version::new(0, 17, 0), None, vec![]));

        assert_eq!(
            manifest.get_channel(&UserChannel::Stable).map(|c| c.name.clone()),
            Some(semver::Version::new(0, 15, 0)),
            "a newly published channel must not silently become stable"
        );
    }

    /// Converting a v1 manifest supplies the `stable` declaration v1 had no way to express.
    ///
    /// The live published manifest is v1 and marks no channel stable, so without this every
    /// existing user would find `midenup install stable` unresolvable. Confined to the
    /// conversion: a v1 document is frozen, so nominating from one cannot promote a channel
    /// still in development.
    #[test]
    fn converting_a_v1_manifest_nominates_a_stable_channel() {
        let v1 = r#"{
            "manifest_version": "1.0.1",
            "date": 1735689600,
            "channels": [
                { "name": "0.14.0", "components": [] },
                { "name": "0.15.0", "components": [] }
            ]
        }"#;

        let manifest = VersionedManifest::parse_str(v1).expect("a v1 manifest must convert");
        let stable = manifest
            .get_channel(&UserChannel::Stable)
            .expect("conversion must nominate a stable channel");
        assert_eq!(stable.name, semver::Version::new(0, 15, 0), "the highest release wins");
    }

    /// An explicit v1 declaration is respected rather than recomputed.
    #[test]
    fn converting_a_v1_manifest_keeps_an_explicit_stable_alias() {
        let v1 = r#"{
            "manifest_version": "1.0.1",
            "date": 1735689600,
            "channels": [
                { "name": "0.14.0", "alias": "stable", "components": [] },
                { "name": "0.15.0", "components": [] }
            ]
        }"#;

        let manifest = VersionedManifest::parse_str(v1).expect("a v1 manifest must convert");
        assert_eq!(
            manifest.get_channel(&UserChannel::Stable).map(|c| c.name.clone()),
            Some(semver::Version::new(0, 14, 0))
        );
    }

    /// A *v2* manifest that declares no stable channel has none, rather than nominating one.
    #[test]
    fn a_manifest_with_no_stable_alias_has_no_stable_channel() {
        let mut manifest = Manifest::default();
        manifest.add_channel(Channel::new(semver::Version::new(0, 15, 0), None, vec![]));
        manifest.add_channel(Channel::new(semver::Version::new(0, 16, 0), None, vec![]));

        assert!(manifest.get_channel(&UserChannel::Stable).is_none());
    }

    /// A network name selects the channel it is bound to.
    #[test]
    fn a_network_name_resolves_to_its_channel() {
        use crate::channel::Network;

        let mut manifest = Manifest::default();
        let mut testnet = Channel::new(semver::Version::new(0, 15, 0), None, vec![]);
        testnet.network = Some(Network::Testnet);
        manifest.add_channel(testnet);
        let mut devnet = Channel::new(semver::Version::new(0, 16, 0), None, vec![]);
        devnet.network = Some(Network::Devnet);
        manifest.add_channel(devnet);

        assert_eq!(
            manifest
                .get_channel(&UserChannel::Other("devnet".into()))
                .map(|c| c.name.clone()),
            Some(semver::Version::new(0, 16, 0))
        );
        assert_eq!(
            manifest
                .get_channel(&UserChannel::Other("testnet".into()))
                .map(|c| c.name.clone()),
            Some(semver::Version::new(0, 15, 0))
        );
        assert!(manifest.get_channel(&UserChannel::Other("mainnet".into())).is_none());
    }

    /// A network this build has no enum variant for is still selectable by name.
    #[test]
    fn an_unknown_network_name_still_resolves() {
        use crate::channel::Network;

        let mut manifest = Manifest::default();
        let mut channel = Channel::new(semver::Version::new(0, 16, 0), None, vec![]);
        channel.network = Some(Network::Other("perfnet".into()));
        manifest.add_channel(channel);

        assert_eq!(
            manifest
                .get_channel(&UserChannel::Other("perfnet".into()))
                .map(|c| c.name.clone()),
            Some(semver::Version::new(0, 16, 0))
        );
    }

    /// An explicit tag wins over a network of the same name.
    #[test]
    fn a_tag_takes_precedence_over_a_network_name() {
        use crate::channel::{ChannelAlias, Network};

        let mut manifest = Manifest::default();
        let mut networked = Channel::new(semver::Version::new(0, 16, 0), None, vec![]);
        networked.network = Some(Network::Devnet);
        manifest.add_channel(networked);
        manifest.add_channel(Channel::new(
            semver::Version::new(0, 17, 0),
            Some(ChannelAlias::Tag("devnet".into())),
            vec![],
        ));

        assert_eq!(
            manifest
                .get_channel(&UserChannel::Other("devnet".into()))
                .map(|c| c.name.clone()),
            Some(semver::Version::new(0, 17, 0))
        );
    }

    /// Declaring a new stable channel moves the alias off the previous holder.
    #[test]
    fn adding_a_stable_channel_clears_the_previous_one() {
        use crate::channel::ChannelAlias;

        let mut manifest = Manifest::default();
        manifest.add_channel(Channel::new(
            semver::Version::new(0, 15, 0),
            Some(ChannelAlias::Stable),
            vec![],
        ));
        manifest.add_channel(Channel::new(
            semver::Version::new(0, 16, 0),
            Some(ChannelAlias::Stable),
            vec![],
        ));

        let stable: Vec<_> = manifest
            .get_channels()
            .filter(|c| c.is_stable())
            .map(|c| c.name.clone())
            .collect();
        assert_eq!(stable, vec![semver::Version::new(0, 16, 0)]);
    }

    /// Pointing a network at a channel un-points whatever held it before.
    #[test]
    fn adding_a_channel_moves_its_network_pointer() {
        use crate::channel::Network;

        let mut manifest = Manifest::default();
        let mut old = Channel::new(semver::Version::new(0, 15, 0), None, vec![]);
        old.network = Some(Network::Testnet);
        manifest.add_channel(old);

        let mut new = Channel::new(semver::Version::new(0, 16, 0), None, vec![]);
        new.network = Some(Network::Testnet);
        manifest.add_channel(new);

        assert_eq!(
            manifest.get_channel_by_network(&Network::Testnet).map(|c| c.name.clone()),
            Some(semver::Version::new(0, 16, 0))
        );
        let pointed: Vec<_> = manifest
            .get_channels()
            .filter(|c| c.network() == Some(&Network::Testnet))
            .map(|c| c.name.clone())
            .collect();
        assert_eq!(pointed.len(), 1, "a network points at exactly one channel");
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
    /// - Non stable channels (ad-hoc tags)
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
            // `nightly` carries no built-in meaning: it is an ordinary tag, resolved by the same
            // path as any other name a manifest binds to a channel.
            let tagged = manifest
                .get_channel(&UserChannel::Other(Cow::Borrowed("nightly")))
                .unwrap_or_else(|| {
                    panic!(
                        "Could not convert UserChannel to internal channel representation from \
                         {FILE}",
                    )
                });
            assert_eq!(tagged.alias, Some(ChannelAlias::Tag(Cow::Borrowed("nightly"))));
            {
                let client = tagged
                    .get_component("client")
                    .unwrap_or_else(|| panic!("Could not find standard library in {FILE}",));

                assert!(matches!(client.version, Authority::Git { .. }));
            }
        }
    }
}
