use std::{borrow::Cow, path::Path};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::channel::{Channel, ChannelAlias, UserChannel};

const MANIFEST_VERSION: &str = "1.0.0";

/// The global manifest of all known channels and their toolchains
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Manifest {
    /// This version is used to handle breaking changes in the manifest format itself
    manifest_version: Cow<'static, str>,
    /// The UTC timestamp at which this manifest was generated
    date: i64,
    /// The channels described in this manifest
    /// NOTE: Changing this to pub momentarily
    channels: Vec<Channel>,
}

impl Default for Manifest {
    fn default() -> Self {
        let date = chrono::Utc::now().timestamp();
        Self {
            manifest_version: Cow::Borrowed(MANIFEST_VERSION),
            date,
            channels: vec![],
        }
    }
}

#[derive(Error, Debug)]
pub enum ManifestError {
    #[error("Manifest file is empty")]
    Empty,
    #[error("Manifest file is not present in `{0}`")]
    Missing(String),
    #[error("Invalid channel manifest in URI: `{0}`")]
    Invalid(String),
    #[error("unsupported channel manifest URI: `{0}`")]
    Unsupported(String),
}

impl Manifest {
    pub const PUBLISHED_MANIFEST_URI: &str =
        "https://0xmiden.github.io/midenup/channel-manifest.json";
    pub const LOCAL_MANIFEST_URI: &str = "https://0xmiden.github.io/midenup/channel-manifest.json";

    /// Loads a [Manifest] from the given URI
    pub fn load_from(uri: impl AsRef<str>) -> Result<Manifest, ManifestError> {
        let uri = uri.as_ref();
        let manifest = if let Some(manifest_path) = uri.strip_prefix("file://") {
            let path = Path::new(manifest_path);
            let contents = std::fs::read_to_string(path)
                .map_err(|_| ManifestError::Missing(path.display().to_string()))?;
            // This could potentially be valid if we are parsing the local manifest
            if contents.is_empty() {
                return Err(ManifestError::Empty);
            }
            serde_json::from_str::<Manifest>(&contents)
                .map_err(|_| ManifestError::Invalid(String::from("invalid channel manifest")))
        } else if uri.starts_with("https://") {
            let mut data = Vec::new();
            let mut handle = curl::easy::Easy::new();
            handle
                .url(uri)
                .map_err(|_| ManifestError::Invalid(String::from("invalid channel manifest")))?;
            {
                let mut transfer = handle.transfer();
                transfer
                    .write_function(|new_data| {
                        data.extend_from_slice(new_data);
                        Ok(new_data.len())
                    })
                    .unwrap();
                transfer
                    .perform()
                    .map_err(|_| ManifestError::Invalid(String::from("invalid channel manifest")))?
            }
            serde_json::from_slice::<Manifest>(&data)
                .map_err(|_| ManifestError::Invalid(uri.to_string()))
        } else {
            return Err(ManifestError::Unsupported(uri.to_string()));
        }?;

        Ok(manifest)
    }

    pub fn add_channel(&mut self, channel: Channel) {
        // Before adding the new stable channel, remove the stable alias from
        // all the channels that have it.
        // NOTE: This should be only a single channel, we check for multiple
        // just in case.
        if self.is_latest_stable(&channel) {
            for channel in self
                .channels
                .iter_mut()
                .filter(|c| c.alias.as_ref().is_some_and(|a| matches!(a, ChannelAlias::Stable)))
            {
                channel.alias = None
            }
        }

        // NOTE: If the channel already exists in the manifest, remove the old
        // version. This happens when updating
        self.channels.retain(|c| c.name != channel.name);

        self.channels.push(channel);
    }
    pub fn is_latest_stable(&self, channel: &Channel) -> bool {
        self.channels.iter().filter(|c| c.is_stable()).all(|c| {
            let comparison = channel.name.cmp_precedence(&c.name);
            matches!(comparison, std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        })
    }

    /// Attempts to fetch the version corresponding to the `stable` [Channel], by definition this is
    /// the latest version
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

    pub fn get_latest_nightly(&self) -> Option<&Channel> {
        self.channels.iter().find(|c| c.is_latest_nightly()).or_else(|| {
            self.channels
                .iter()
                .filter(|c| c.is_nightly())
                .max_by(|x, y| x.name.cmp_precedence(&y.name))
        })
    }

    pub fn get_named_nightly(&self, name: impl AsRef<str>) -> Option<&Channel> {
        self.channels.iter().find(|c| {
            c.alias.as_ref().is_some_and(
                |alias| matches!(alias, ChannelAlias::Nightly(Some(tag)) if tag == name.as_ref()),
            )
        })
    }
    /// Attempts to fetch the [Channel] corresponding to the given [ChannelType]
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

    pub fn get_channels(&self) -> impl Iterator<Item = &Channel> {
        self.channels.iter()
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::Manifest;
    use crate::{channel::UserChannel, manifest::ChannelAlias};

    #[test]
    fn validate_current_channel_manifest() {
        let manifest = Manifest::load_from("file://manifest/channel-manifest.json")
            .expect("couldn't load manifest");

        let stable = manifest
            .get_channel(&UserChannel::Stable)
            .expect("Could not convert UserChannel to internal channel representation");

        assert!(stable.get_component("std").is_some());
    }

    #[test]
    fn validate_published_channel_manifest() {
        let manifest =
            Manifest::load_from(Manifest::PUBLISHED_MANIFEST_URI).expect("couldn't load manifest");

        let stable = manifest
            .get_channel(&UserChannel::Stable)
            .expect("Could not convert UserChannel to internal channel representation");

        assert!(stable.get_component("std").is_some());
    }

    #[test]
    fn validate_stable_is_latest() {
        const FILE: &str = "file://tests/data/manifest-check-stable.json";
        let manifest = Manifest::load_from(FILE).unwrap();

        let stable = manifest
            .get_latest_stable()
            .expect("Could not convert UserChannel to internal channel representation from {FILE}");

        assert_eq!(stable.name, semver::Version::new(0, 15, 0));

        let specific_version = manifest
            .get_channel(&UserChannel::Version(semver::Version::new(0, 14, 0)))
            .expect("Could not convert UserChannel to internal channel representation from {FILE}");

        assert_eq!(specific_version.name, semver::Version::new(0, 14, 0));
    }

    #[test]
    /// Do note that this encapsulates all non-stable channels, i.e. nightly,
    /// nightly-suffix and tagged channels
    fn validate_non_stable() {
        const FILE: &str = "file://tests/data/manifest-non-stable.json";
        let manifest = Manifest::load_from(FILE).unwrap();

        let stable = manifest
            .get_channel(&UserChannel::Other(Cow::Borrowed("custom-dev-build")))
            .expect(
                "Could not convert UserChannel to internal channel representation from
    {FILE}",
            );

        assert_eq!(
            stable.name,
            semver::Version {
                major: 0,
                minor: 16,
                patch: 0,
                pre: semver::Prerelease::new("custom-build").expect("invalid pre-release"),
                build: semver::BuildMetadata::EMPTY,
            }
        );

        assert_eq!(stable.alias, Some(ChannelAlias::Tag(Cow::Borrowed("custom-dev-build"))));
    }
}
