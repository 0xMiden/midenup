use std::{fmt, hash::Hash, path::PathBuf, time::SystemTime};

pub use semver;
use serde::{Deserialize, Serialize};

/// Used to specify from which  particular revision of a repository.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum GitTarget {
    /// The components is pointing to a specific revision in the repository.
    #[serde(untagged)]
    Revision {
        #[serde(rename = "revision")]
        hash: String,
    },
    /// The components is pointing to a specific tag in the repository.
    #[serde(untagged)]
    Tag {
        #[serde(rename = "tag")]
        name: String,
    },
    /// The components is pointing to a specific *branch* in the repository.
    ///
    /// NOTE: When an update is issued, these type of components will trigger an update if the
    /// branch they were pointing to had new commits since the time the component was installed.
    /// This means that these components are _not_ deterministic and their behavior could change
    /// in-between updates.
    #[serde(untagged)]
    Branch {
        /// This is the name of the branch being tracked.
        #[serde(rename = "branch")]
        name: String,
        /// This field represents the revision hash that is currently presently installed.
        ///
        /// This is only meant to be used in the local manifest in order to check for updates.
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

impl fmt::Display for GitTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            GitTarget::Branch { name, .. } => write!(f, "branch = \"{name}\""),
            GitTarget::Revision { hash } => write!(f, "rev = \"{hash}\""),
            GitTarget::Tag { name: tag } => write!(f, "tag = \"{tag}\""),
        }
    }
}

impl GitTarget {
    pub fn to_cargo_flag(&self) -> [String; 2] {
        match &self {
            GitTarget::Branch { name, .. } => [String::from("--branch"), String::from(name)],
            GitTarget::Revision { hash } => [String::from("--rev"), String::from(hash)],
            GitTarget::Tag { name: tag } => [String::from("--tag"), String::from(tag)],
        }
    }
}

/// Represents the canonical versioning authority for a tool or toolchain
#[derive(Serialize, Deserialize, Debug, Clone, Hash, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Authority {
    /// The authority for this tool/toolchain is a local filesystem path
    Path {
        /// The path to the artifact.
        path: PathBuf,
        /// Represents the latest modification done inside this directory.
        #[serde(skip_serializing_if = "Option::is_none")]
        last_modification: Option<SystemTime>,
    },
    /// The authority for this tool/toolchain is a git repository.
    Git {
        /// Points to the git repository containting the [crate::manifest::Component].
        repository_url: String,
        /// The subdirectory within the repository which contains the component
        #[serde(skip_serializing_if = "Option::is_none")]
        subpath: Option<String>,
        /// If the target is missing from the [crate::manifest::Manifest], then we assume that it
        /// is pointing to the tip of the `main` branch
        #[serde(default)]
        #[serde(flatten)]
        target: GitTarget,
    },
    /// The authority for this tool/toolchain is a semantic version resolved in the appropriate
    /// registry (e.g. crates.io).
    Registry {
        /// The semantic versioning string for the package/artifact to fetch
        version: semver::Version,
    },
}

/// Observed content of a mutable source, separate from its structural identity.
///
/// `None` inside a pin means the content is unknown, never that it is unchanged. Sources without
/// mutable content are compared through their structural definition instead.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SourcePin<'a> {
    Path(Option<SystemTime>),
    Git(Option<&'a str>),
    Immutable,
}

impl SourcePin<'_> {
    pub(crate) fn changed_to(self, next: SourcePin<'_>) -> bool {
        match (self, next) {
            (SourcePin::Path(old), SourcePin::Path(new)) => {
                old.is_none() || new.is_none() || old != new
            },
            (SourcePin::Git(old), SourcePin::Git(new)) => {
                old.is_none() || new.is_none() || old != new
            },
            // A change of source kind is handled by structural classification.
            _ => false,
        }
    }
}

impl Authority {
    pub(crate) fn source_pin(&self) -> SourcePin<'_> {
        match self {
            Self::Path { last_modification, .. } => SourcePin::Path(*last_modification),
            Self::Git {
                target: GitTarget::Branch { latest_revision, .. },
                ..
            } => SourcePin::Git(latest_revision.as_deref()),
            Self::Git { .. } | Self::Registry { .. } => SourcePin::Immutable,
        }
    }
}

impl core::str::FromStr for Authority {
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s)
    }
}

impl fmt::Display for Authority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self {
            Authority::Registry { version, .. } => write!(f, "{version}"),
            Authority::Git { repository_url, target, .. } => {
                write!(f, "{repository_url}:{target}")
            },
            Authority::Path { path, .. } => write!(f, "{}", path.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::ResolvedAuthority;

    fn path_pin(mtime: Option<SystemTime>) -> Authority {
        Authority::Path {
            path: "/source".into(),
            last_modification: mtime,
        }
    }

    fn branch_pin(revision: Option<&str>) -> Authority {
        Authority::Git {
            repository_url: "https://example.invalid/source".into(),
            subpath: None,
            target: GitTarget::Branch {
                name: "main".into(),
                latest_revision: revision.map(str::to_owned),
            },
        }
    }

    #[test]
    fn path_source_pin_comparison_agrees_before_and_after_planning() {
        let first = SystemTime::UNIX_EPOCH;
        let second = first + std::time::Duration::from_secs(1);
        for (old, new, changed) in [
            (None, None, true),
            (None, Some(first), true),
            (Some(first), None, true),
            (Some(first), Some(first), false),
            (Some(first), Some(second), true),
        ] {
            let installed = path_pin(old);
            let upstream = path_pin(new);
            let planned = ResolvedAuthority::Path { canonical: "/source".into(), mtime: new };
            assert_eq!(installed.source_pin().changed_to(upstream.source_pin()), changed);
            assert_eq!(installed.source_pin().changed_to(planned.source_pin()), changed);
        }
    }

    #[test]
    fn branch_source_pin_comparison_agrees_before_and_after_planning() {
        for (old, new, changed) in [
            (None, None, true),
            (None, Some("first"), true),
            (Some("first"), None, true),
            (Some("first"), Some("first"), false),
            (Some("first"), Some("second"), true),
        ] {
            let installed = branch_pin(old);
            let upstream = branch_pin(new);
            assert_eq!(installed.source_pin().changed_to(upstream.source_pin()), changed);
            if let Some(revision) = new {
                let planned = ResolvedAuthority::Git {
                    url: "https://example.invalid/source".into(),
                    revision: revision.into(),
                    subpath: None,
                };
                assert_eq!(installed.source_pin().changed_to(planned.source_pin()), changed);
            }
        }
    }

    #[test]
    fn immutable_sources_and_changes_of_source_kind_use_structural_comparison() {
        let registry = Authority::Registry { version: semver::Version::new(1, 0, 0) };
        let git = branch_pin(Some("first"));
        let path = path_pin(Some(SystemTime::UNIX_EPOCH));
        for (old, new) in [(&registry, &registry), (&git, &path), (&path, &git)] {
            assert!(!old.source_pin().changed_to(new.source_pin()));
        }
        for target in [
            GitTarget::Revision { hash: "first".into() },
            GitTarget::Tag { name: "v1".into() },
        ] {
            let fixed = Authority::Git {
                repository_url: "https://example.invalid/source".into(),
                subpath: None,
                target,
            };
            assert!(!fixed.source_pin().changed_to(git.source_pin()));
        }
    }
}
