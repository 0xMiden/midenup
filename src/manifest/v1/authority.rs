use std::{path::PathBuf, time::SystemTime};

use serde::{Deserialize, Serialize};

use crate::version::GitTarget;

/// Represents the canonical versioning authority for a tool or toolchain
#[derive(Serialize, Deserialize, Debug, Clone, Hash)]
#[serde(untagged, rename_all = "snake_case")]
pub enum Authority {
    /// The authority for this tool/toolchain is a local filesystem path
    Path {
        /// The path to the crate.
        path: PathBuf,
        crate_name: String,
        /// Represents the latest modification done inside this directory.
        #[serde(skip_serializing_if = "Option::is_none")]
        last_modification: Option<SystemTime>,
    },
    /// The authority for this tool/toolchain is a git repository.
    Git {
        crate_name: String,
        /// Points to the git repository containting the [crate::channel::Component].
        repository_url: String,
        /// The subdirectory within the repository which contains the component
        ///
        /// This is only required for components which are not crates
        #[serde(skip_serializing_if = "Option::is_none")]
        subpath: Option<String>,
        /// If the target is missing from the [crate::manifest::Manifest], then we assume that it
        /// is pointing to the tip of the `main` branch
        #[serde(default)]
        #[serde(flatten)]
        target: GitTarget,
    },
    /// The authority for this tool/toolchain is crates.io
    Cargo {
        #[serde(skip_serializing_if = "Option::is_none")]
        package: Option<String>,
        /// The semantic versioning string for the package to fetch
        version: semver::Version,
    },
}

impl From<Authority> for crate::version::Authority {
    fn from(value: Authority) -> Self {
        match value {
            Authority::Path { path, last_modification, crate_name: _ } => {
                Self::Path { path, last_modification }
            },
            Authority::Git {
                repository_url,
                subpath,
                target,
                crate_name: _,
            } => Self::Git { repository_url, subpath, target },
            Authority::Cargo { version, package: _ } => Self::Registry { version },
        }
    }
}
