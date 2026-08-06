use std::{borrow::Cow, hash::Hash};

use serde::{Deserialize, Serialize};

use super::Component;

/// A special alias a channel could carry.
///
/// A **v1-only** concept, kept here because v1 documents in the wild carry it. v3 replaces it with
/// the manifest's top-level `networks` map: an alias could say a channel was `stable` *or*
/// `nightly` *or* a tag, and never that one toolchain was on two networks at once.
#[derive(Serialize, Debug, PartialEq, Eq, Clone, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ChannelAlias {
    /// Represents `stable`. Only one [Channel] can be marked as `stable` at a time.
    Stable,
    /// Represents either `nightly` or `nightly-$SUFFIX`
    Nightly(Option<Cow<'static, str>>),
    /// An ad-hoc named alias for a channel. This can be used to tag custom channels with names such
    /// as `0.15.0-stable`.
    #[serde(untagged)]
    Tag(Cow<'static, str>),
}

impl<'de> serde::de::Deserialize<'de> for ChannelAlias {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Unexpected;
        use serde_untagged::UntaggedEnumVisitor;

        UntaggedEnumVisitor::new()
            .string(|s| {
                s.parse::<ChannelAlias>().map_err(|err| {
                    serde::de::Error::invalid_value(Unexpected::Str(s), &err.to_string().as_str())
                })
            })
            .deserialize(deserializer)
    }
}

impl core::str::FromStr for ChannelAlias {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "stable" => Ok(Self::Stable),
            "nightly" => Ok(Self::Nightly(None)),
            tag => match tag.strip_prefix("nightly-") {
                Some(suffix) => Ok(Self::Nightly(Some(Cow::Owned(suffix.to_string())))),
                None => Ok(Self::Tag(Cow::Owned(tag.to_string()))),
            },
        }
    }
}

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
