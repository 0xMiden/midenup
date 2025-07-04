use std::{fmt, path::PathBuf};

use serde::{Deserialize, Serialize};

/// Represents the canonical versioning authority for a tool or toolchain
#[derive(Serialize, Deserialize, Debug, Clone, Hash, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Authority {
    /// The authority for this tool/toolchain is a local filesystem path
    Path(PathBuf),
    /// The authority for this tool/toolchain is a git repository.
    #[serde(untagged)]
    Git { repository_url: String },
    /// The authority for this tool/toolchain is crates.io
    #[serde(untagged)]
    Cargo {
        /// The name of the crates.io package under which this tool is provided
        /// In None, then the package's name is the same as the component's
        #[serde(skip_serializing_if = "Option::is_none")]
        package: Option<String>,
        /// The semantic versioning string for the package to fetch
        version: semver::Version,
    },
}

impl fmt::Display for Authority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            Authority::Cargo { version, .. } => write!(f, "{version}"),
            Authority::Git { repository_url } => write!(f, "{repository_url}"),
            Authority::Path(path) => write!(f, "{}", path.display()),
        }
    }
}
