use std::{borrow::Cow, fmt};

use serde::{Deserialize, Serialize};

use crate::config::Config;
pub use crate::manifest::Channel;

#[derive(Serialize, Deserialize, Debug, Clone, Hash)]
#[serde(untagged)]
pub enum MigrationStrategy {
    NameChange { old_channel: semver::Version },
}

#[derive(Debug, Clone)]
pub enum UpstreamMatch {
    /// The remote Channel is this Channel's upstream equivalent.
    UpstreamCounterpart,
    /// The remote Channel is this Channel's equivalent and got migrated.
    Migrated(MigrationStrategy),
}

#[derive(Debug, Clone)]
pub struct UpstreamChannel {
    pub channel: Channel,
    pub upstream_match: UpstreamMatch,
}

impl UpstreamChannel {
    pub fn new(channel: Channel, upstream_match: UpstreamMatch, config: &Config) -> Self {
        let mut synced_channel = channel.clone();
        synced_channel.sync(config);
        UpstreamChannel { channel: synced_channel, upstream_match }
    }
}

/// A special alias/tag that a channel can posses. For more information see [`Channel::alias`].
/// These are only used for locally installed [`Channel`]s.
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

/// User-facing channel reference.
///
/// The main difference with this and [Channel] is the definition of "stable". The definition of
/// "stable" 'under the hood' is the lastest available non-nightly channel. If the user passes
/// [`UserChannel::Stable`] as the target channel, we then handle the mapping from it to the
/// underlying [Channel] representation.
#[derive(Serialize, Default, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum UserChannel {
    #[default]
    Stable,
    Nightly,
    #[serde(untagged)]
    Version(semver::Version),
    #[serde(untagged)]
    Other(Cow<'static, str>),
}

impl fmt::Display for UserChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(version) => write!(f, "{version}"),
            Self::Stable => f.write_str("stable"),
            Self::Nightly => f.write_str("nightly"),
            Self::Other(custom_name) => write!(f, "{custom_name}"),
        }
    }
}

impl<'de> serde::de::Deserialize<'de> for UserChannel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Unexpected;
        use serde_untagged::UntaggedEnumVisitor;

        UntaggedEnumVisitor::new()
            .string(|s| {
                s.parse::<UserChannel>().map_err(|err| {
                    serde::de::Error::invalid_value(Unexpected::Str(s), &err.to_string().as_str())
                })
            })
            .deserialize(deserializer)
    }
}

impl core::str::FromStr for UserChannel {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use anyhow::anyhow;

        match s {
            "stable" => Ok(Self::Stable),
            "nightly" => Ok(Self::Nightly),
            version => semver::Version::parse(version)
                .map(Self::Version)
                .map_err(|err| anyhow!("invalid channel version: {err}")),
        }
    }
}

/// Tags used to identify special qualities of a specific channel.
#[derive(Serialize, Deserialize, Debug, Clone, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Tags {
    /// The channel is partially installed, i.e. only a subset of components
    /// have been installed.
    Partial,
    /// The channel has been moved to a new channel or potentially even removed.
    Migration {
        #[serde(flatten)]
        migration: MigrationStrategy,
    },
}
