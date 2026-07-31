use std::{borrow::Cow, fmt};

use serde::Serialize;

use crate::config::Config;
pub use crate::manifest::Channel;

#[derive(Debug, Clone)]
pub enum UpstreamMatch {
    /// The remote Channel is this Channel's upstream equivalent.
    UpstreamCounterpart,
    /// The remote channel supersedes this one, and declares so with `migrates_from`.
    Migrated { old_channel: semver::Version },
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
    ///
    /// **Declared, never derived.** A channel is stable only by saying so, which is what keeps
    /// publishing a channel distinct from promoting one: adding a channel to the manifest has no
    /// effect on what `stable` names until someone says it does. Only the author knows which of the
    /// two acts they are performing, so only the author can express it.
    Stable,
    /// An ad-hoc named alias for a channel. This can be used to tag custom channels with names such
    /// as `0.15.0-stable`.
    ///
    /// This is the only kind of alias besides `stable`. A name with no built-in meaning needs no
    /// dedicated variant: it is carried, matched and resolved as a string, which is all any of them
    /// ever required.
    #[serde(untagged)]
    Tag(Cow<'static, str>),
}

/// The network a channel's toolchain targets.
///
/// This is the primary way a pre-release toolchain is selected: a channel under development is
/// published with the network it is deployed to, and `midenup install devnet` installs it. A
/// network names something checkable -- deployment is an external fact, and one toolchain is
/// deployed to a given network at a time -- which a "how finished is it" marker does not.
///
/// Independent of the `stable` alias. `stable` says which channel is the default choice; this says
/// what a channel can be used against. Neither follows from the other, because a release ships
/// before it is deployed.
///
/// Unrecognized values deserialize into [`Network::Other`] rather than failing, for the same reason
/// unknown component kinds do (spec section 4.4): a network this build has not heard of must not
/// make the channel -- or the manifest around it -- unreadable.
#[derive(Serialize, Debug, PartialEq, Eq, Clone, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Network {
    Devnet,
    Testnet,
    Mainnet,
    #[serde(untagged)]
    Other(Cow<'static, str>),
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Devnet => f.write_str("devnet"),
            Self::Testnet => f.write_str("testnet"),
            Self::Mainnet => f.write_str("mainnet"),
            Self::Other(name) => f.write_str(name),
        }
    }
}

impl core::str::FromStr for Network {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "devnet" => Ok(Self::Devnet),
            "testnet" => Ok(Self::Testnet),
            "mainnet" => Ok(Self::Mainnet),
            other => Ok(Self::Other(Cow::Owned(other.to_string()))),
        }
    }
}

impl<'de> serde::de::Deserialize<'de> for Network {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde_untagged::UntaggedEnumVisitor;

        UntaggedEnumVisitor::new()
            .string(|s| Ok(s.parse::<Network>().expect("Network::from_str is infallible")))
            .deserialize(deserializer)
    }
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
            tag => Ok(Self::Tag(Cow::Owned(tag.to_string()))),
        }
    }
}

/// User-facing channel reference.
///
/// The main difference with this and [Channel] is that `stable` is a name rather than a version:
/// when the user passes [`UserChannel::Stable`], the mapping to the underlying [Channel] is
/// resolved against the manifest that declares it.
#[derive(Serialize, Default, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum UserChannel {
    #[default]
    Stable,
    #[serde(untagged)]
    Version(semver::Version),
    /// Any other name: a network (`devnet`, `testnet`, `mainnet`), or an ad-hoc
    /// [`ChannelAlias::Tag`].
    ///
    /// Networks are deliberately *not* a variant of their own. A network is a name the manifest
    /// binds to a channel, exactly like a tag, and resolving both through one arm means a network
    /// this build has never heard of is still selectable -- there is no list of known networks to
    /// fall off. See [`crate::manifest::Manifest::get_channel`] for the resolution order.
    #[serde(untagged)]
    Other(Cow<'static, str>),
}

impl fmt::Display for UserChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(version) => write!(f, "{version}"),
            Self::Stable => f.write_str("stable"),
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

    /// Parses a user-supplied channel reference.
    ///
    /// A name that is neither a known alias nor a semantic version becomes [`UserChannel::Other`]
    /// -- which covers both networks (`midenup install devnet`) and ad-hoc
    /// [`ChannelAlias::Tag`] channels. Accepting them here is what makes every named channel
    /// reachable from the CLI and from `miden-toolchain.toml`.
    ///
    /// A typo is therefore not reported here but by the lookup, which is the only place that knows
    /// which channels exist -- and so can say that `midenup install stbale` names no known channel,
    /// rather than that it is not a version.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "stable" => Ok(Self::Stable),
            other => match semver::Version::parse(other) {
                Ok(version) => Ok(Self::Version(version)),
                Err(_) => Ok(Self::Other(Cow::Owned(other.to_string()))),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use core::str::FromStr;

    use super::*;

    #[test]
    fn known_channel_aliases_parse() {
        assert_eq!(ChannelAlias::from_str("stable").unwrap(), ChannelAlias::Stable);
    }

    /// `nightly` has no built-in meaning any more, so it parses as an ordinary tag.
    ///
    /// Removing the variant removed a concept, not a capability: a manifest that declares
    /// `alias: "nightly"` still parses and still resolves, by the same path as any other name.
    #[test]
    fn nightly_is_an_ordinary_tag() {
        assert_eq!(
            ChannelAlias::from_str("nightly").unwrap(),
            ChannelAlias::Tag(Cow::Borrowed("nightly"))
        );
        assert_eq!(
            ChannelAlias::from_str("nightly-2026-01-01").unwrap(),
            ChannelAlias::Tag(Cow::Borrowed("nightly-2026-01-01"))
        );
        assert!(matches!(
            UserChannel::from_str("nightly").unwrap(),
            UserChannel::Other(name) if name == "nightly"
        ));
    }

    /// An alias this build does not know must degrade to a tag, not fail.
    #[test]
    fn an_unknown_alias_becomes_a_tag() {
        assert_eq!(
            ChannelAlias::from_str("perfnet").unwrap(),
            ChannelAlias::Tag(Cow::Borrowed("perfnet"))
        );
    }

    #[test]
    fn a_stable_alias_round_trips_through_json() {
        let json = serde_json::to_string(&ChannelAlias::Stable).unwrap();
        assert_eq!(json, "\"stable\"");
        assert_eq!(serde_json::from_str::<ChannelAlias>(&json).unwrap(), ChannelAlias::Stable);
    }

    #[test]
    fn user_channels_parse() {
        assert!(matches!(UserChannel::from_str("stable").unwrap(), UserChannel::Stable));
        assert!(matches!(
            UserChannel::from_str("0.16.0").unwrap(),
            UserChannel::Version(v) if v == semver::Version::new(0, 16, 0)
        ));
    }

    /// Every named channel -- an ad-hoc tag, a network -- reaches `Manifest::get_channel` through
    /// this variant, so a name that is not an alias or a version has to land here.
    #[test]
    fn a_non_semver_name_becomes_an_ad_hoc_channel() {
        assert!(matches!(
            UserChannel::from_str("custom-dev-build").unwrap(),
            UserChannel::Other(name) if name == "custom-dev-build"
        ));
    }

    /// Network names are ordinary named channels, not a variant of their own, which is what makes
    /// `midenup install devnet` work without a list of known networks to fall off.
    #[test]
    fn network_names_parse_as_named_channels() {
        for name in ["devnet", "testnet", "mainnet", "perfnet"] {
            assert!(
                matches!(UserChannel::from_str(name).unwrap(), UserChannel::Other(n) if n == name),
                "{name} must parse as a named channel"
            );
        }
    }

    #[test]
    fn user_channels_display_as_they_parse() {
        for name in ["stable", "0.16.0", "devnet", "custom-dev-build"] {
            assert_eq!(UserChannel::from_str(name).unwrap().to_string(), name);
        }
    }

    #[test]
    fn networks_parse_and_display() {
        assert_eq!(Network::from_str("devnet").unwrap(), Network::Devnet);
        assert_eq!(Network::from_str("testnet").unwrap(), Network::Testnet);
        assert_eq!(Network::from_str("mainnet").unwrap(), Network::Mainnet);
        for name in ["devnet", "testnet", "mainnet"] {
            assert_eq!(Network::from_str(name).unwrap().to_string(), name);
        }
    }

    /// A network this build has not heard of must not make the channel unreadable (section 4.4).
    #[test]
    fn an_unknown_network_is_preserved_rather_than_rejected() {
        let parsed: Network = serde_json::from_str("\"perfnet\"").expect("must not fail");
        assert_eq!(parsed, Network::Other(Cow::Borrowed("perfnet")));
        assert_eq!(serde_json::to_string(&parsed).unwrap(), "\"perfnet\"");
    }
}
