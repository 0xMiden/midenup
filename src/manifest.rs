use std::{borrow::Cow, path::Path};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

use crate::channel::{Channel, ChannelAlias, UserChannel};

const MANIFEST_VERSION: &str = "1.0.0";

/// The global manifest of all known channels and their toolchains
#[derive(Serialize, Deserialize, Debug)]
pub struct Manifest {
    /// This version is used to handle breaking changes in the manifest format itself
    manifest_version: Cow<'static, str>,
    /// The UTC timestamp at which this manifest was generated
    date: i64,
    /// The channels described in this manifest
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

impl Manifest {
    pub const PUBLISHED_MANIFEST_URI: &str =
        "https://0xmiden.github.io/midenup/channel-manifest.json";

    /// Loads a [Manifest] from the given URI
    pub fn load_from(uri: impl AsRef<str>) -> anyhow::Result<Self> {
        let uri = uri.as_ref();
        let mut manifest = if let Some(manifest_path) = uri.strip_prefix("file://") {
            let path = Path::new(manifest_path);
            let contents = std::fs::read_to_string(path).with_context(|| {
                format!("failed to read channel manifest from '{}'", path.display())
            })?;
            serde_json::from_str::<Manifest>(&contents).context("invalid channel manifest")
        } else if uri.starts_with("https://") {
            let mut data = Vec::new();
            let mut handle = curl::easy::Easy::new();
            handle
                .url(uri)
                .with_context(|| format!("invalid channel manifest uri: '{uri}'",))?;
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
                    .with_context(|| format!("failed to load channel manifest from '{uri}'"))?
            }
            serde_json::from_slice::<Manifest>(&data).context("invalid channel manifest")
        } else {
            bail!("unsupported channel manifest uri: '{}'", uri)
        }?;

        // // Mark the largest version as stable
        // let channels = &mut manifest.channels;
        // let stable_channel =
        //     channels.iter_mut().filter(|channel| channel.is_stable()).max_by(|x, y| {
        //         match (&x.name, &y.name) {
        //             (CanonicalChannel::Nightly, _) => unreachable!(),
        //             (_, CanonicalChannel::Nightly) => unreachable!(),
        //             (
        //                 CanonicalChannel::Version { version: x, .. },
        //                 CanonicalChannel::Version { version: y, .. },
        //             ) => x.cmp_precedence(y),
        //         }
        //     });

        // if let Some(stable) = stable_channel {
        //     stable.name = match &stable.name {
        //         CanonicalChannel::Nightly => CanonicalChannel::Nightly,
        //         CanonicalChannel::Version { version: x, .. } => {
        //             CanonicalChannel::Version { version: x.clone(), is_stable: true }
        //         },
        //     }
        // };
        Ok(manifest)
    }

    /// Attempts to fetch the version corresponding to the `stable` [Channel], by definition this is
    /// the latest version
    pub fn get_latest_stable(&self) -> Option<&Channel> {
        self.channels.iter().find(|c| c.is_latest_stable()).or_else(|| {
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
        const FILE: &str = "file://tests/manifest-check-stable.json";
        let manifest = Manifest::load_from(FILE).unwrap();

        let stable = manifest
            .get_channel(&UserChannel::Stable)
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
        const FILE: &str = "file://tests/manifest-non-stable.json";
        let manifest = Manifest::load_from(FILE).unwrap();
        std::dbg!(&manifest);

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
