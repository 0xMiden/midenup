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
                // v1 had no concept of a target network, and one is not inferable: which network a
                // toolchain was pointed at is an external fact, not a property of the document.
                network: None,
                components,
                extra: Default::default(),
            });
        }

        // v2 requires the stable channel to declare itself (spec section 5.1), and v1 had no way to
        // express that: `stable` there *was* the highest non-prerelease channel. Converting has to
        // supply the equivalent declaration, or every v1 document would come out with no stable
        // channel and `midenup install stable` would stop resolving.
        //
        // Nominating is safe here in a way it would not be as a general rule. A v1 document is
        // frozen -- nothing is being published into it any more -- so interpreting one cannot
        // promote a channel that is still under development. v2 stays strictly declared.
        if !channels.iter().any(|channel| channel.is_stable())
            && let Some(newest) = channels
                .iter_mut()
                .filter(|channel| channel.name.pre.is_empty())
                .max_by(|a, b| a.name.cmp_precedence(&b.name))
        {
            newest.alias = Some(crate::channel::ChannelAlias::Stable);
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
