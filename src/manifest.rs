use std::{borrow::Cow, path::Path};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};

use crate::channel::{CanonicalChannel, Channel, ChannelType};

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
        if let Some(manifest_path) = uri.strip_prefix("file://") {
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
                    .with_context(|| format!("failed to load channel manifest from '{uri}'"))?;
            }
            serde_json::from_slice::<Manifest>(&data).context("invalid channel manifest")
        } else {
            bail!("unsupported channel manifest uri: '{}'", uri)
        }
    }

    /// Attempts to fetch the [Channel] corresponding to the given [ChannelType]
    pub fn get_channel(&self, channel: &CanonicalChannel) -> Option<&Channel> {
        match channel {
            CanonicalChannel::Nightly => {
                todo!("Nightly channel not yet implemented")
            },
            CanonicalChannel::Version { version, .. } => {
                self.channels.iter().find(|c| &c.name == channel)
            },
        }
    }

    /// Attempts to fetch the version corresponding to the `stable` [Channel], by definition this is
    /// the latest version
    pub fn get_stable_version(&self) -> Option<&semver::Version> {
        self.channels
            .iter()
            .filter_map(|channel| match &channel.name {
                CanonicalChannel::Nightly => None,
                CanonicalChannel::Version { version, .. } => Some(version),
            })
            .max_by(|x, y| x.cmp_precedence(y))
    }
}

#[cfg(test)]
mod tests {
    use super::Manifest;
    use crate::channel::ChannelType;

    #[test]
    fn validate_current_channel_manifest() {
        let manifest = Manifest::load_from("file://manifest/channel-manifest.json").unwrap();

        let stable = manifest.get_channel(&ChannelType::Stable).unwrap();

        assert!(stable.get_component("std").is_some());
    }

    #[test]
    fn validate_published_channel_manifest() {
        let manifest = Manifest::load_from(Manifest::PUBLISHED_MANIFEST_URI).unwrap();

        let stable = manifest.get_channel(&ChannelType::Stable).unwrap();

        assert!(stable.get_component("std").is_some());
    }
}
