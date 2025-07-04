use std::{fmt, path::PathBuf};

use serde::{Deserialize, Serialize};

/// Used to specify a particular revision of a repository.
#[derive(Serialize, Deserialize, Debug, Clone, Hash, PartialEq, Eq)]
pub enum GitTarget {
    Branch(String),
    Revision(String),
    Tag(String),
}
impl Default for GitTarget {
    fn default() -> Self {
        GitTarget::Branch(String::from("main"))
    }
}

impl fmt::Display for GitTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            GitTarget::Branch(branch_name) => write!(f, "branch = {branch_name}"),
            GitTarget::Revision(hash) => write!(f, "rev = {hash}"),
            GitTarget::Tag(tag) => write!(f, "tag = {tag}"),
        }
    }
}

impl GitTarget {
    pub fn to_cargo_flag(&self) -> [String; 2] {
        match &self {
            GitTarget::Branch(branch_name) => [String::from("--branch"), String::from(branch_name)],
            GitTarget::Revision(hash) => [String::from("--rev"), String::from(hash)],
            GitTarget::Tag(tag) => [String::from("--tag"), String::from(tag)],
        }
    }
}

/// Represents the canonical versioning authority for a tool or toolchain
#[derive(Serialize, Deserialize, Debug, Clone, Hash, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Authority {
    /// The authority for this tool/toolchain is a local filesystem path
    Path(PathBuf),
    /// The authority for this tool/toolchain is a git repository.
    #[serde(untagged)]
    Git {
        /// Points to the git repository containting the [Component].
        repository_url: String,
        /// This is the name of the crate that holds the executable we're going
        /// to install. This has to be specified because some repositories hold
        /// multiple crates inside them.
        crate_name: String,
        /// NOTE: If the target is missing from the [Manifest], then we assume
        /// that it is pointing to the tip of the `main` branch
        #[serde(default)]
        target: GitTarget,
    },
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
            Authority::Git { repository_url, target, .. } => {
                write!(f, "{repository_url}:{target}")
            },
            Authority::Path(path) => write!(f, "{}", path.display()),
        }
    }
}
