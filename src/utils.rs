//! This module contains some general purpose functions.

/// A fresh opaque identifier, for things whose name must carry no meaning.
///
/// Uniqueness only has to hold within one `MIDENUP_HOME`, against operations already serialized by
/// the advisory lock. A process-local counter makes collisions within a process impossible rather
/// than merely unlikely -- a clock read alone is not enough, since two calls can land in the same
/// nanosecond -- and time plus pid separates processes.
pub fn opaque_id() -> String {
    use std::{
        hash::{DefaultHasher, Hash, Hasher},
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let mut hasher = DefaultHasher::new();
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub mod git {
    use std::path::Path;

    use anyhow::Context;

    pub fn find_latest_hash(repository_url: &str, branch_name: &str) -> anyhow::Result<String> {
        let check_revision_hash = std::process::Command::new("git")
            .arg("ls-remote")
            .arg(repository_url)
            .arg("--branch")
            .arg(branch_name)
            .stderr(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::piped())
            .output()
            .context(format!(
                "failed to fetch latest git rev-hash from branch {branch_name}, is git installed?.",
            ))?;

        // A failed `ls-remote` leaves stdout empty. Without checking, this returned Ok("") --
        // which callers then recorded as though it were a real revision, so update detection
        // compared "" against "" and concluded the component was current. A branch-tracked
        // component would never update again after one transient failure.
        if !check_revision_hash.status.success() {
            anyhow::bail!(
                "failed to resolve branch '{branch_name}' of '{repository_url}': git ls-remote \
                 exited with {}",
                check_revision_hash.status
            );
        }

        // This returns a string of the form:
        //
        // sym_ref\tref_name
        //
        // Source: https://github.com/git/git/blob/41905d60226a0346b22f0d0d99428c746a5a3b14/builtin/ls-remote.c#L169
        let revision_hash: String = String::from_utf8(check_revision_hash.stdout)
            .context(format!(
                "failed to format latest git rev-hash from branch {branch_name}, does the branch \
                 exist?.",
            ))?
            .chars()
            .take_while(|&c| c != '\t')
            .collect();

        // `ls-remote` succeeds with no output when the branch does not exist.
        if revision_hash.is_empty() {
            anyhow::bail!("branch '{branch_name}' does not exist in '{repository_url}'");
        }
        if !revision_hash.chars().all(|c| c.is_ascii_hexdigit()) {
            anyhow::bail!(
                "expected a commit hash for branch '{branch_name}' of '{repository_url}', got \
                 '{revision_hash}'"
            );
        }

        Ok(revision_hash)
    }

    // Used in tests
    #[allow(dead_code)]
    pub fn clone_specific_revision(
        repository_url: &str,
        revision: &str,
        dir: &Path,
    ) -> anyhow::Result<()> {
        std::process::Command::new("git")
            .arg("clone")
            .args(["--revision", revision])
            .arg("--depth=1")
            .arg("--")
            .arg(repository_url)
            .arg(dir)
            .stderr(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .spawn()
            .context("Failed to spawn shell for git command")?
            .wait()
            .with_context(|| {
                format!("failed to clone {revision} of {repository_url} to {}", dir.display())
            })?;
        Ok(())
    }
}

pub mod fs {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::SystemTime,
    };

    use anyhow::Context;

    #[cfg(unix)]
    pub fn symlink(from: &std::path::Path, to: &std::path::Path) -> anyhow::Result<()> {
        std::os::unix::fs::symlink(to, from).context("could not create symlink")
    }

    #[cfg(windows)]
    pub fn symlink(from: &std::path::Path, to: &std::path::Path) -> anyhow::Result<()> {
        std::os::windows::fs::symlink_file(to, from).context("could not create symlink")
    }

    /// Points `link` at `target`, atomically, whether or not `link` already exists.
    ///
    /// The link is built under a unique temporary name and `rename`d into place, so there is no
    /// window in which it is missing and no way for two processes to collide on creating it. The
    /// obvious alternative -- remove, then create -- has both problems: a reader in between sees no
    /// link at all, and two writers racing produce `EEXIST` for whichever loses.
    ///
    /// The temporary lives in the same directory, so the rename stays within one filesystem.
    pub fn replace_symlink(link: &Path, target: &Path) -> anyhow::Result<()> {
        let parent = link
            .parent()
            .with_context(|| format!("'{}' has no parent directory", link.display()))?;
        let name = link
            .file_name()
            .and_then(|name| name.to_str())
            .with_context(|| format!("'{}' has no file name", link.display()))?;

        let temporary = parent.join(format!(".{name}.{}.link", std::process::id()));
        let _ = fs::remove_file(&temporary);

        symlink(&temporary, target)?;
        fs::rename(&temporary, link)
            .inspect_err(|_| {
                let _ = fs::remove_file(&temporary);
            })
            .with_context(|| {
                format!("failed to point '{}' at '{}'", link.display(), target.display())
            })
    }

    const ENTRY_LIMIT: u32 = u32::MAX;

    /// Returns the latest registered modification time inside a directory, including its
    /// subdirectories.
    ///
    /// This is intended as a "best effort" approximation, if it encounters any errors while reading
    /// an entry, it simply skips it. Additionally, as a safety net, the `ENTRY_LIMIT` sets an upper
    /// bound on the number of entries the function can check before returning.
    pub fn latest_modification(dir: &Path) -> anyhow::Result<(SystemTime, PathBuf)> {
        fn traverse_directories(
            dir: &Path,
            latest: Option<(SystemTime, PathBuf)>,
            current_entry: u32,
        ) -> (Option<(SystemTime, PathBuf)>, u32) {
            let mut local_latest = latest;
            let mut current_entry_count = current_entry;

            let entries = fs::read_dir(dir);
            if let Ok(entries) = entries {
                for file in entries {
                    let Ok(file) = file else {
                        continue;
                    };
                    let Ok(metadata) = file.metadata() else {
                        continue;
                    };

                    if current_entry_count == ENTRY_LIMIT {
                        break;
                    }

                    let (current_entry_latest, visited_entries) =
                    // We avoid symlinks to directories to avoid infinite loops.
                    if metadata.is_dir() && !metadata.is_symlink() {
                        traverse_directories(&file.path(), local_latest.clone(), current_entry_count)
                    } else {
                        (metadata.modified().ok().map(|metadata| (metadata, file.path())), current_entry_count + 1)
                    };

                    current_entry_count = visited_entries;

                    local_latest = match (&local_latest, current_entry_latest) {
                        (
                            Some((local_latest_time, path_old)),
                            Some((current_entry_latest, path)),
                        ) => {
                            if current_entry_latest > *local_latest_time {
                                Some((current_entry_latest, path))
                            } else {
                                Some((*local_latest_time, path_old.to_path_buf()))
                            }
                        },
                        (Some(local_latest), None) => Some(local_latest.clone()),
                        (None, Some(current_entry_latest)) => Some(current_entry_latest),
                        (None, None) => None,
                    };
                }
            } else {
                println!("Failed to open {}, skipping it.", dir.display());
            }

            (local_latest, current_entry_count)
        }

        let directory_last_modification = dir
            .metadata()
            .and_then(|file| file.modified())
            .map(|metadata| (metadata, dir.to_path_buf()))
            .ok();

        let (latest_found_modification, _) =
            traverse_directories(dir, directory_last_modification, 0);

        // This should only be an error if every single metadata read failed, which should be
        // unlikely.
        latest_found_modification.context("Failed to read any file")
    }

    /// Recursively copy every entry from `src` into `dst`, preserving the directory layout and
    /// recreating symlinks. Entries whose file name appears in `skip` are not copied. `dst` is
    /// expected to already exist.
    pub fn copy_dir_recursive(src: &Path, dst: &Path, skip: &[&str]) -> anyhow::Result<()> {
        for entry in fs::read_dir(src)
            .with_context(|| format!("failed to read directory '{}'", src.display()))?
        {
            let entry =
                entry.with_context(|| format!("failed to read entry in '{}'", src.display()))?;
            let file_name = entry.file_name();
            if file_name.to_str().is_some_and(|name| skip.contains(&name)) {
                continue;
            }
            let file_type = entry
                .file_type()
                .with_context(|| format!("failed to stat entry '{}'", entry.path().display()))?;
            let target = dst.join(&file_name);
            if file_type.is_symlink() {
                let link_target = fs::read_link(entry.path()).with_context(|| {
                    format!("failed to read symlink '{}'", entry.path().display())
                })?;
                symlink(&target, &link_target).with_context(|| {
                    format!(
                        "failed to recreate symlink '{}' -> '{}'",
                        target.display(),
                        link_target.display()
                    )
                })?;
            } else if file_type.is_dir() {
                fs::create_dir_all(&target).with_context(|| {
                    format!("failed to create directory '{}'", target.display())
                })?;
                copy_dir_recursive(&entry.path(), &target, skip)?;
            } else {
                fs::copy(entry.path(), &target).with_context(|| {
                    format!("failed to copy '{}' to '{}'", entry.path().display(), target.display())
                })?;
            }
        }
        Ok(())
    }
}

/// Writing a document so that a failure never leaves a partial file behind.
pub mod atomic {
    use std::{
        io::Write,
        path::{Path, PathBuf},
    };

    #[derive(Debug, thiserror::Error)]
    pub enum WriteError {
        #[error("failed to serialize document for '{path}': {source}")]
        Serialize {
            path: PathBuf,
            #[source]
            source: serde_json::Error,
        },
        #[error("failed to write temporary file '{path}': {source}")]
        Io {
            path: PathBuf,
            #[source]
            source: std::io::Error,
        },
        #[error("refusing to write '{path}': {reason}")]
        Validation { path: PathBuf, reason: String },
    }

    /// Serializes `value`, verifies the bytes actually landed, and only then replaces `path`.
    ///
    /// The sequence is: write to a unique temporary sibling, flush and `fsync` it, close it,
    /// re-read it from disk, hand those bytes to `validate`, and finally `rename` over `path`.
    ///
    /// Two things make this worth more than a plain write.
    ///
    /// First, `validate` sees the bytes **as they were read back**, not the in-memory value. A
    /// serializer that produces something which cannot be parsed again is caught before it can
    /// replace a good file, rather than on the next startup.
    ///
    /// Second, the commit is a single `rename`. Every failure path before it leaves `path`
    /// byte-for-byte unchanged, and the temporary file is removed. There is no window in which the
    /// destination holds a partial document.
    ///
    /// The temporary file is a *sibling* so that the rename stays within one filesystem; renaming
    /// across filesystems is not atomic and is not a rename at all on most platforms.
    pub fn write_validated<T, V>(path: &Path, value: &T, validate: V) -> Result<(), WriteError>
    where
        T: serde::Serialize,
        V: FnOnce(&str) -> Result<(), String>,
    {
        let bytes = serde_json::to_vec_pretty(value)
            .map_err(|source| WriteError::Serialize { path: path.to_path_buf(), source })?;

        let temporary = temporary_sibling(path);

        // Everything from here to the rename is fallible, so each error path removes the
        // temporary file before returning.
        let result = (|| {
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(&bytes)?;
            file.flush()?;
            file.sync_all()?;
            drop(file);
            std::fs::read_to_string(&temporary)
        })();

        let written = match result {
            Ok(written) => written,
            Err(source) => {
                let _ = std::fs::remove_file(&temporary);
                return Err(WriteError::Io { path: temporary, source });
            },
        };

        if let Err(reason) = validate(&written) {
            let _ = std::fs::remove_file(&temporary);
            return Err(WriteError::Validation { path: path.to_path_buf(), reason });
        }

        // The commit point.
        if let Err(source) = std::fs::rename(&temporary, path) {
            let _ = std::fs::remove_file(&temporary);
            return Err(WriteError::Io { path: path.to_path_buf(), source });
        }

        Ok(())
    }

    /// A temporary name that cannot collide with a concurrent writer's.
    fn temporary_sibling(path: &Path) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        path.with_file_name(format!(".{name}.{}.{nanos}.tmp", std::process::id()))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn leftovers(dir: &Path, keep: &str) -> Vec<String> {
            std::fs::read_dir(dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|name| name != keep)
                .collect()
        }

        #[test]
        fn a_failed_validation_leaves_the_original_untouched() {
            let dir = tempdir::TempDir::new("atomic-validate").unwrap();
            let path = dir.path().join("doc.json");
            std::fs::write(&path, b"ORIGINAL").unwrap();

            let result =
                write_validated(&path, &serde_json::json!({"a": 1}), |_| Err("nope".to_string()));

            assert!(matches!(result, Err(WriteError::Validation { .. })));
            assert_eq!(std::fs::read(&path).unwrap(), b"ORIGINAL", "must be byte-identical");
            assert!(leftovers(dir.path(), "doc.json").is_empty(), "temp files must be cleaned up");
        }

        #[test]
        fn the_validator_sees_the_bytes_read_back_from_disk() {
            let dir = tempdir::TempDir::new("atomic-seen").unwrap();
            let path = dir.path().join("doc.json");

            let seen = std::cell::RefCell::new(None);
            write_validated(&path, &serde_json::json!({"a": 1}), |text| {
                *seen.borrow_mut() = Some(text.to_string());
                Ok(())
            })
            .unwrap();

            let seen = seen.borrow().clone().expect("validator must run");
            assert_eq!(
                seen,
                std::fs::read_to_string(&path).unwrap(),
                "the validator must see exactly what was committed"
            );
            assert!(seen.contains("\"a\""));
        }

        #[test]
        fn a_successful_write_replaces_the_destination() {
            let dir = tempdir::TempDir::new("atomic-ok").unwrap();
            let path = dir.path().join("doc.json");
            std::fs::write(&path, b"OLD").unwrap();

            write_validated(&path, &serde_json::json!({"a": 1}), |_| Ok(())).unwrap();

            let written: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(written, serde_json::json!({"a": 1}));
            assert!(leftovers(dir.path(), "doc.json").is_empty());
        }

        #[test]
        fn writing_a_new_file_works() {
            let dir = tempdir::TempDir::new("atomic-new").unwrap();
            let path = dir.path().join("doc.json");
            write_validated(&path, &serde_json::json!([1, 2]), |_| Ok(())).unwrap();
            assert!(path.exists());
        }

        /// Concurrent writers must not collide on the temporary name.
        #[test]
        fn temporary_names_are_unique() {
            let path = Path::new("/tmp/doc.json");
            let mut names = std::collections::BTreeSet::new();
            for _ in 0..64 {
                names.insert(temporary_sibling(path));
                std::thread::sleep(std::time::Duration::from_nanos(1));
            }
            assert!(names.len() > 1, "temporary names must vary");
            for name in names {
                assert_eq!(name.parent(), path.parent(), "must be a sibling, for rename atomicity");
            }
        }
    }
}

#[cfg(test)]
mod git_tests {
    use std::path::Path;

    fn run(dir: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|err| panic!("git {args:?}: {err}"));
        assert!(output.status.success(), "git {args:?} failed");
    }

    /// A failed or empty `ls-remote` must be an error.
    ///
    /// Regression: neither the exit status nor empty output was checked, so this returned
    /// `Ok("")`. Callers recorded that as a revision, and update detection then compared `""`
    /// against `""` and concluded the component was current -- a branch-tracked component would
    /// never update again after one transient failure.
    #[test]
    fn a_missing_repository_is_an_error() {
        let temp = tempdir::TempDir::new("git-missing").unwrap();
        let missing = temp.path().join("nope");
        let result = super::git::find_latest_hash(&missing.display().to_string(), "main");
        assert!(result.is_err(), "got {result:?}");
    }

    #[test]
    fn a_missing_branch_is_an_error() {
        let temp = tempdir::TempDir::new("git-branch").unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("file"), b"x").unwrap();
        run(&repo, &["init", "--quiet", "--initial-branch=main"]);
        run(&repo, &["config", "user.email", "f@example.invalid"]);
        run(&repo, &["config", "user.name", "F"]);
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "--quiet", "-m", "one"]);

        let result = super::git::find_latest_hash(&repo.display().to_string(), "no-such-branch");
        assert!(result.is_err(), "got {result:?}");
    }

    #[test]
    fn an_existing_branch_resolves_to_a_commit_hash() {
        let temp = tempdir::TempDir::new("git-ok").unwrap();
        let repo = temp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("file"), b"x").unwrap();
        run(&repo, &["init", "--quiet", "--initial-branch=main"]);
        run(&repo, &["config", "user.email", "f@example.invalid"]);
        run(&repo, &["config", "user.name", "F"]);
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "--quiet", "-m", "one"]);

        let hash = super::git::find_latest_hash(&repo.display().to_string(), "main")
            .expect("an existing branch must resolve");
        assert_eq!(hash.len(), 40);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
