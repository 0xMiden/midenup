use std::{borrow::Cow, path::Path};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::channel::{CanonicalChannel, Channel};

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

        // Mark the largest version as stable
        let channels = &mut manifest.channels;
        let stable_channel = channels
            .iter_mut()
            .filter(|channel| matches!(channel.name, CanonicalChannel::Version { .. }))
            .max_by(|x, y| match (&x.name, &y.name) {
                (CanonicalChannel::Nightly, _) => unreachable!(),
                (_, CanonicalChannel::Nightly) => unreachable!(),
                (
                    CanonicalChannel::Version { version: x, .. },
                    CanonicalChannel::Version { version: y, .. },
                ) => x.cmp_precedence(y),
            });

        if let Some(stable) = stable_channel {
            stable.name = match &stable.name {
                CanonicalChannel::Nightly => CanonicalChannel::Nightly,
                CanonicalChannel::Version { version: x, .. } => {
                    CanonicalChannel::Version { version: x.clone(), is_stable: true }
                },
            }
        };
        Ok(manifest)
    }

    /// Attempts to fetch the [Channel] corresponding to the given [ChannelType]
    pub fn get_channel(&self, channel: &CanonicalChannel) -> Option<&Channel> {
        match channel {
            CanonicalChannel::Nightly => {
                todo!("Nightly channel not yet implemented")
            },
            CanonicalChannel::Version { .. } => self.channels.iter().find(|c| &c.name == channel),
        }
    }

    /// Attempts to fetch the version corresponding to the `stable` [Channel], by definition this is
    /// the latest version
    pub fn get_stable_version(&self) -> Option<&Channel> {
        self.channels.iter().find(|channel| {
            matches!(channel.name, CanonicalChannel::Version { is_stable: true, .. })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Manifest;
    use crate::channel::{CanonicalChannel, ChannelType};

    #[test]
    fn validate_current_channel_manifest() {
        let manifest = Manifest::load_from("file://manifest/channel-manifest.json").unwrap();

        let channel = CanonicalChannel::from_input(ChannelType::Stable, &manifest)
            .expect("Couldn't parse Canonical Stable channel from the input stable channel");

        let stable = manifest.get_channel(&channel).unwrap();

        assert!(stable.get_component("std").is_some());
    }

    #[test]
    fn validate_published_channel_manifest() {
        let manifest = Manifest::load_from(Manifest::PUBLISHED_MANIFEST_URI).unwrap();

        let channel = CanonicalChannel::from_input(ChannelType::Stable, &manifest)
            .expect("Couldn't parse Canonical Stable channel from the input stable channel");

        let stable = manifest.get_channel(&channel).unwrap();

        assert!(stable.get_component("std").is_some());
    }

    #[test]
    fn validate_stable_is_latest() {
        let manifest = Manifest::load_from("file://tests/manifest-check-stable.json").unwrap();

        let channel = CanonicalChannel::from_input(ChannelType::Stable, &manifest)
            .expect("Couldn't parse Canonical Stable channel from the test file");

        let stable = manifest.get_channel(&channel).unwrap();
        assert_eq!(
            stable.name,
            CanonicalChannel::Version {
                version: semver::Version::new(0, 15, 0),
                is_stable: true
            }
        );

        let version = CanonicalChannel::from_input(
            ChannelType::Version(semver::Version::new(0, 14, 0)),
            &manifest,
        )
        .expect("Couldn't parse Canonical Stable channel from the test file");

        assert_eq!(
            version,
            CanonicalChannel::Version {
                version: semver::Version::new(0, 14, 0),
                is_stable: false,
            }
        );
    }
}
