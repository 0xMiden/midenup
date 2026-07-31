mod authority;
pub mod channel;
pub mod component;

use serde::Deserialize;

pub use self::{channel::*, component::*};
use super::ManifestError;

pub const MANIFEST_VERSION: semver::Version = semver::Version::new(1, 0, 1);

/// The global manifest of all known channels and their toolchains
#[derive(Deserialize, Debug, Clone)]
pub struct Manifest {
    /// The UTC timestamp at which this manifest was generated
    pub(super) date: i64,
    /// The channels described in this manifest
    pub(super) channels: Vec<Channel>,
}

impl Default for Manifest {
    fn default() -> Self {
        let date = chrono::Utc::now().timestamp();
        Self { date, channels: vec![] }
    }
}

impl TryFrom<Manifest> for crate::manifest::v2::Manifest {
    type Error = ManifestError;

    fn try_from(value: Manifest) -> Result<Self, Self::Error> {
        use crate::manifest::v2;

        let mut channels = Vec::<v2::Channel>::with_capacity(value.channels.len());

        for channel in value.channels {
            let mut components = Vec::<v2::Component>::with_capacity(channel.components.len());
            for component in channel.components {
                components.push(component.try_into()?);
            }
            // `Partial` is dropped: it recorded local state in a document that describes upstream,
            // and v2 derives it. `Migration` becomes the explicit field.
            let migrates_from = channel.tags.iter().find_map(|tag| match tag {
                super::v1::channel::Tags::Migration { old_channel } => Some(old_channel.clone()),
                super::v1::channel::Tags::Partial => None,
            });

            channels.push(v2::Channel {
                name: channel.name,
                alias: channel.alias,
                migrates_from,
                components,
                extra: Default::default(),
            });
        }

        Ok(v2::Manifest {
            // The output of this conversion is a v2 manifest, so it declares the v2 version.
            // Stamping the v1 constant here left converted manifests claiming to be v1.0.1.
            manifest_version: v2::MANIFEST_VERSION,
            date: value.date,
            channels,
            extra: Default::default(),
        })
    }
}
