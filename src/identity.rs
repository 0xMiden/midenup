//! Local installation identity, independent of its upstream channel and publication.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::channel::UserChannel;

/// A lowercase, single-segment name in the explicit `custom:` namespace.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ToolchainName(String);

#[derive(Debug, thiserror::Error)]
#[error(
    "invalid toolchain name '{0}': use lowercase letters, digits, '-' or '_', with optional \
     interior dots"
)]
pub struct InvalidToolchainName(String);

impl FromStr for ToolchainName {
    type Err = InvalidToolchainName;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        if name.is_empty()
            || name.starts_with(['.', '-'])
            || name.ends_with('.')
            || !name.bytes().all(|c| {
                (c.is_ascii_lowercase() || c.is_ascii_digit()) || matches!(c, b'-' | b'_' | b'.')
            })
        {
            return Err(InvalidToolchainName(name.to_owned()));
        }
        Ok(Self(name.to_owned()))
    }
}

impl TryFrom<String> for ToolchainName {
    type Error = InvalidToolchainName;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<ToolchainName> for String {
    fn from(value: ToolchainName) -> Self {
        value.0
    }
}

impl AsRef<str> for ToolchainName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ToolchainName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InstallationId {
    Version(semver::Version),
    Custom(ToolchainName),
}

impl fmt::Display for InstallationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Version(version) => write!(f, "{version}"),
            Self::Custom(name) => write!(f, "custom:{name}"),
        }
    }
}

/// The stable name and upstream selector of a derived installation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CustomToolchain {
    pub name: ToolchainName,
    pub channel: UserChannel,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cannot_escape_their_namespace() {
        for name in ["", ".", "..", "../dev", "a/b", "a\\b", "a:b", "a\0b", "-dev", "a b", "Dev"] {
            assert!(name.parse::<ToolchainName>().is_err(), "accepted {name:?}");
            assert!(serde_json::from_value::<ToolchainName>(serde_json::json!(name)).is_err());
        }
        for name in ["dev", "project-dev", "project_1", "project.dev", "1.2.3"] {
            assert_eq!(name.parse::<ToolchainName>().unwrap().as_ref(), name);
        }
    }
}
