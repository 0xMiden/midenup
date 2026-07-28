use std::hash::Hash;

use serde::Deserialize;

use super::Component;
use crate::channel::{ChannelAlias, Tags};

/// Represents a specific release channel for a toolchain.
///
/// Different channels have different stability guarantees. See the specific details for the
/// channel you are interested in to learn more.
#[derive(Deserialize, Debug, Clone, Hash)]
pub struct Channel {
    /// Channels are identified by their name. The name corresponds to the channel's version.
    /// The version can contain suffixes such as "-custom", "-beta".
    pub name: semver::Version,
    /// This is used to tag special channels. Most notably, the current "stable" channel is marked
    /// with the [`ChannelAlias::Stable`] alias.
    pub alias: Option<ChannelAlias>,
    /// Set of tags used to denote a special characteristic about the channel.
    ///
    /// Mainly used for locally installed channels.
    #[serde(default)]
    pub tags: Vec<Tags>,
    /// The set of toolchain components available in this channel
    pub components: Vec<Component>,
}
