//! Pinning mutable sources before an installation's identity is computed.
//!
//! Two of the three authority kinds name something that can change underneath us. A git *branch*
//! is a moving pointer; a filesystem *path* is a directory anyone can edit mid-build. Both must be
//! reduced to a fixed point before planning, for two reasons.
//!
//! First, the plan key is supposed to identify the inputs to an installation. A key computed over
//! "the tip of `main`" identifies nothing -- it would compare equal across two installs that
//! produced entirely different binaries.
//!
//! Second, a source that changes *during* the build produces an installation that matches neither
//! the before nor the after state. That is worse than a failure, because nothing reports it.

use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use crate::{
    utils,
    version::{Authority, GitTarget},
};

#[derive(Debug, thiserror::Error)]
pub enum PinError {
    #[error("failed to resolve branch '{branch}' of '{url}' to a commit: {source}")]
    UnresolvableBranch {
        url: String,
        branch: String,
        #[source]
        source: anyhow::Error,
    },
    #[error("failed to canonicalize source path '{path}': {source}")]
    UnreadablePath {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "the source at '{path}' changed while it was being installed; re-run to install the \
         current contents"
    )]
    PathChangedDuringInstall { path: PathBuf },
}

/// An authority reduced to something that cannot move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedAuthority {
    Registry {
        version: semver::Version,
    },
    Git {
        url: String,
        /// Always a concrete commit, never a branch name.
        revision: String,
        subpath: Option<String>,
    },
    Path {
        canonical: PathBuf,
        /// The most recent modification anywhere under `canonical`, at pin time.
        mtime: Option<SystemTime>,
    },
}

impl ResolvedAuthority {
    /// A stable identity string for the plan key.
    pub fn identity(&self) -> String {
        match self {
            Self::Registry { version } => format!("registry:{version}"),
            Self::Git { url, revision, subpath } => match subpath {
                Some(subpath) => format!("git:{url}#{revision}:{subpath}"),
                None => format!("git:{url}#{revision}"),
            },
            // The mtime deliberately does not appear: it decides *whether* to reinstall, but two
            // installs of an unchanged tree are the same installation.
            Self::Path { canonical, .. } => format!("path:{}", canonical.display()),
        }
    }
}

/// Reduces `authority` to something fixed.
///
/// `cwd` resolves relative path authorities, which are interpreted against the directory the
/// command was invoked from.
pub fn pin(authority: &Authority, cwd: &Path) -> Result<ResolvedAuthority, PinError> {
    match authority {
        Authority::Registry { version } => {
            Ok(ResolvedAuthority::Registry { version: version.clone() })
        },

        Authority::Git { repository_url, subpath, target } => {
            let revision = match target {
                // Already fixed.
                GitTarget::Revision { hash } => hash.clone(),
                // A tag is *nominally* fixed. It can be moved, but treating it as immutable is the
                // contract tags carry, and resolving it here would require a network round trip on
                // every plan.
                GitTarget::Tag { name } => name.clone(),
                // A branch is a moving pointer, so resolve it now and install that exact commit.
                GitTarget::Branch { name, .. } => {
                    utils::git::find_latest_hash(repository_url, name).map_err(|source| {
                        PinError::UnresolvableBranch {
                            url: repository_url.clone(),
                            branch: name.clone(),
                            source,
                        }
                    })?
                },
            };

            Ok(ResolvedAuthority::Git {
                url: repository_url.clone(),
                revision,
                subpath: subpath.clone(),
            })
        },

        Authority::Path { path, .. } => {
            let absolute = if path.is_absolute() {
                path.clone()
            } else {
                cwd.join(path)
            };
            let canonical = absolute
                .canonicalize()
                .map_err(|source| PinError::UnreadablePath { path: absolute.clone(), source })?;
            // A tree that cannot be walked is treated as having no known modification time, which
            // makes the next update reinstall it. Failing safe here means doing more work, not
            // less.
            let mtime = utils::fs::latest_modification(&canonical).ok().map(|(time, _)| time);

            Ok(ResolvedAuthority::Path { canonical, mtime })
        },
    }
}

/// Confirms a pinned path source has not changed since it was pinned.
///
/// Called after the build. A path that moved mid-build produced an installation matching neither
/// the before nor the after state, and nothing else would report it.
pub fn recheck_path(resolved: &ResolvedAuthority) -> Result<(), PinError> {
    let ResolvedAuthority::Path { canonical, mtime } = resolved else {
        return Ok(());
    };

    let current = utils::fs::latest_modification(canonical).ok().map(|(time, _)| time);
    if &current != mtime {
        return Err(PinError::PathChangedDuringInstall { path: canonical.clone() });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|err| panic!("git {args:?}: {err}"));
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Builds a repo with one commit on `main`, returning its path and revision.
    fn repo_with_commit(root: &Path) -> (PathBuf, String) {
        let dir = root.join("repo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file"), b"one").unwrap();

        git(&dir, &["init", "--quiet", "--initial-branch=main"]);
        git(&dir, &["config", "user.email", "fixture@example.invalid"]);
        git(&dir, &["config", "user.name", "Fixture"]);
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "--quiet", "-m", "one"]);
        let revision = git(&dir, &["rev-parse", "HEAD"]);

        (dir, revision)
    }

    #[test]
    fn a_registry_authority_is_already_fixed() {
        let pinned =
            pin(&Authority::Registry { version: semver::Version::new(0, 15, 0) }, Path::new("."))
                .unwrap();
        assert_eq!(pinned, ResolvedAuthority::Registry { version: semver::Version::new(0, 15, 0) });
    }

    /// A branch is a moving pointer, so it must be reduced to the commit actually installed.
    #[test]
    fn a_git_branch_is_pinned_to_a_concrete_revision() {
        let temp = tempdir::TempDir::new("pin-branch").unwrap();
        let (repo, revision) = repo_with_commit(temp.path());

        let pinned = pin(
            &Authority::Git {
                repository_url: repo.display().to_string(),
                subpath: None,
                target: GitTarget::Branch {
                    name: "main".to_string(),
                    latest_revision: None,
                },
            },
            Path::new("."),
        )
        .expect("a local branch must resolve");

        match pinned {
            ResolvedAuthority::Git { revision: pinned, .. } => {
                assert_eq!(pinned, revision, "must pin to the commit the branch points at");
                assert_eq!(pinned.len(), 40, "must be a full commit hash, not a branch name");
            },
            other => panic!("expected a git authority, got {other:?}"),
        }
    }

    #[test]
    fn an_explicit_revision_is_left_alone() {
        let pinned = pin(
            &Authority::Git {
                repository_url: "https://example.invalid/repo.git".to_string(),
                subpath: None,
                target: GitTarget::Revision { hash: "abc123".to_string() },
            },
            Path::new("."),
        )
        .unwrap();
        assert!(matches!(pinned, ResolvedAuthority::Git { revision, .. } if revision == "abc123"));
    }

    #[test]
    fn an_unresolvable_branch_is_an_error() {
        let temp = tempdir::TempDir::new("pin-nobranch").unwrap();
        let result = pin(
            &Authority::Git {
                repository_url: temp.path().join("does-not-exist").display().to_string(),
                subpath: None,
                target: GitTarget::Branch {
                    name: "main".to_string(),
                    latest_revision: None,
                },
            },
            Path::new("."),
        );
        assert!(matches!(result, Err(PinError::UnresolvableBranch { .. })));
    }

    #[test]
    fn a_relative_path_is_resolved_against_the_working_directory() {
        let temp = tempdir::TempDir::new("pin-relative").unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("file"), b"x").unwrap();

        let pinned = pin(
            &Authority::Path {
                path: PathBuf::from("source"),
                last_modification: None,
            },
            temp.path(),
        )
        .expect("a relative path must resolve against the cwd");

        match pinned {
            ResolvedAuthority::Path { canonical, .. } => {
                assert_eq!(canonical, source.canonicalize().unwrap());
            },
            other => panic!("expected a path authority, got {other:?}"),
        }
    }

    #[test]
    fn a_missing_path_is_an_error() {
        let temp = tempdir::TempDir::new("pin-missing").unwrap();
        let result = pin(
            &Authority::Path {
                path: temp.path().join("nope"),
                last_modification: None,
            },
            Path::new("."),
        );
        assert!(matches!(result, Err(PinError::UnreadablePath { .. })));
    }

    /// A source edited mid-build yields an installation matching neither the before nor the after
    /// state. Nothing else would report that, so the recheck must.
    #[test]
    fn a_path_that_changes_during_installation_is_detected() {
        let temp = tempdir::TempDir::new("pin-changed").unwrap();
        let source = temp.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("file"), b"one").unwrap();

        let pinned = pin(
            &Authority::Path {
                path: source.clone(),
                last_modification: None,
            },
            Path::new("."),
        )
        .unwrap();
        recheck_path(&pinned).expect("an untouched tree must pass");

        // Filesystem timestamps are coarse; sleep past the resolution so the change is visible.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(source.join("file"), b"two").unwrap();

        assert!(
            matches!(recheck_path(&pinned), Err(PinError::PathChangedDuringInstall { .. })),
            "a mid-build change must abort rather than silently succeed"
        );
    }

    #[test]
    fn rechecking_a_non_path_authority_is_a_no_op() {
        let pinned = ResolvedAuthority::Registry { version: semver::Version::new(1, 0, 0) };
        recheck_path(&pinned).expect("nothing to recheck");
    }

    /// The identity must distinguish revisions, since that is what the plan key relies on.
    #[test]
    fn identity_distinguishes_revisions() {
        let at = |revision: &str| {
            ResolvedAuthority::Git {
                url: "https://example.invalid/r.git".to_string(),
                revision: revision.to_string(),
                subpath: None,
            }
            .identity()
        };
        assert_ne!(at("aaa"), at("bbb"));
    }

    /// ...but an unchanged path tree is the same installation, whatever its mtime.
    #[test]
    fn identity_ignores_path_mtime() {
        let at = |mtime: Option<SystemTime>| {
            ResolvedAuthority::Path { canonical: PathBuf::from("/src"), mtime }.identity()
        };
        assert_eq!(at(None), at(Some(SystemTime::UNIX_EPOCH)));
    }
}
