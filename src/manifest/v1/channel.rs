use std::hash::Hash;

use serde::Deserialize;

use super::Component;
use crate::channel::ChannelAlias;

/// Tags used to identify special qualities of a specific channel.
///
/// A **v1-only** concept, kept here because v1 documents in the wild carry it. v3 has neither:
/// `Migration` became the explicit `migrates_from` field on the upstream channel, and `Partial`
/// described local state, which now derives it from the installed component set (spec section 8.6).
#[derive(Deserialize, Debug, Clone, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Tags {
    /// The channel is partially installed, i.e. only a subset of components have been installed.
    Partial,
    /// The channel has been moved to a new channel or potentially even removed.
    Migration { old_channel: semver::Version },
}

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
