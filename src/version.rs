use std::{
    fmt,
    hash::{Hash, Hasher},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

/// Used to specify from which  particular revision of a repository.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum GitTarget {
    /// The components is pointing to a specific revision in the repository.
    Revision(String),
    /// The components is pointing to a specific tag in the repository.
    Tag(String),
    #[serde(untagged)]
    /// The components is pointing to a specific *branch* in the repository.
    /// NOTE: When an update is issued, these type of components will trigger an
    /// update if the branch they were pointing to had new commits since the
    /// time the component was installed. This means that these components are
    /// *not* deterministic and their behavior could change in-between updates.
    Branch {
        #[serde(rename = "branch")]
        /// This is the name of the branch being tracked.
        name: String,
        /// This field represents the revision hash that is currently presently
        /// installed. This is only meant to be used in the local manifest in
        /// order to check for updates.
        latest_revision: Option<String>,
    },
}
impl Default for GitTarget {
    fn default() -> Self {
        GitTarget::Branch {
            name: String::from("main"),
            latest_revision: None,
        }
    }
}
impl Eq for GitTarget {}
impl PartialEq for GitTarget {
    fn eq(&self, other: &Self) -> bool {
        match (&self, other) {
            (Self::Revision(hasha), Self::Revision(hashb)) => hasha == hashb,
            (Self::Tag(taga), Self::Tag(tagb)) => taga == tagb,
            // Two components are "equal" if they are pointing to the same
            // branch. Comparison between latest available commit is done ad-hoc
            (Self::Branch { name: name_a, .. }, Self::Branch { name: name_b, .. }) => {
                name_a == name_b
            },
            _ => false,
        }
    }
}

impl Hash for GitTarget {
    fn hash<H>(&self, state: &mut H)
    where
        H: Hasher,
    {
        match &self {
            Self::Revision(hash_a) => hash_a.hash(state),
            Self::Tag(tag_a) => tag_a.hash(state),
            Self::Branch { name, .. } => name.hash(state),
        }
    }
}

impl fmt::Display for GitTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            GitTarget::Branch { name, .. } => write!(f, "branch = {name}"),
            GitTarget::Revision(hash) => write!(f, "rev = {hash}"),
            GitTarget::Tag(tag) => write!(f, "tag = \"{tag}"),
        }
    }
}

impl GitTarget {
    pub fn to_cargo_flag(&self) -> [String; 2] {
        match &self {
            GitTarget::Branch { name, .. } => [String::from("--branch"), String::from(name)],
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
    #[serde(untagged)]
    Path {
        /// The path to the crate.
        path: PathBuf,

        /// This is the name of the crate that holds the executable we're going
        /// to install. This has to be specified because cargo needs the name of
        /// the crate to handle uninstallation.
        crate_name: String,
    },
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
        /// If None, then the package's name is the same as the component's
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
            Authority::Path { path, .. } => write!(f, "{}", path.display()),
        }
    }
}
