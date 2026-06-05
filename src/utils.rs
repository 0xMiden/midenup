//! This module contains some general purpose functions.

pub mod git {
    use std::{path::PathBuf, string::FromUtf8Error};

    use thiserror::Error;

    #[derive(Error, Debug)]
    pub enum GitError {
        #[error("failed to fetch latest git rev-hash from branch {branch}, is git installed?")]
        FetchRevHash { branch: String, source: std::io::Error },

        #[error(
            "failed to format latest git rev-hash from branch {branch}, does the branch exist?"
        )]
        InvalidRevHash { branch: String, source: FromUtf8Error },

        #[error("failed to create directory: '{}'", path.display())]
        CreateDirectory { path: PathBuf, source: std::io::Error },

        #[error("failed to spawn shell for git command")]
        SpawnFailed { source: std::io::Error },

        #[error("{message}")]
        CommandFailed { message: String, source: std::io::Error },
    }

    pub fn find_latest_hash(repository_url: &str, branch_name: &str) -> Result<String, GitError> {
        let check_revision_hash = std::process::Command::new("git")
            .arg("ls-remote")
            .arg(repository_url)
            .arg("--branch")
            .arg(branch_name)
            .stderr(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::piped())
            .output()
            .map_err(|source| GitError::FetchRevHash { branch: branch_name.to_string(), source })?;

        // This returns a string of the form:
        //
        // sym_ref\tref_name
        //
        // Source: https://github.com/git/git/blob/41905d60226a0346b22f0d0d99428c746a5a3b14/builtin/ls-remote.c#L169
        let revision_hash: String = String::from_utf8(check_revision_hash.stdout)
            .map_err(|source| GitError::InvalidRevHash { branch: branch_name.to_string(), source })?
            .chars()
            .take_while(|&c| c != '\t')
            .collect();

        Ok(revision_hash)
    }

    // Used in tests
    #[allow(dead_code)]
    pub fn clone_specific_revision(
        repository_url: &str,
        revision: &str,
        dir: &PathBuf,
    ) -> Result<(), GitError> {
        std::fs::create_dir(dir)
            .map_err(|source| GitError::CreateDirectory { path: dir.clone(), source })?;

        std::process::Command::new("git")
            .args(["-C", dir.to_str().unwrap()])
            .arg("init")
            .stderr(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map_err(|source| GitError::SpawnFailed { source })?
            .wait()
            .map_err(|source| GitError::CommandFailed {
                message: "failed to run git init command".to_string(),
                source,
            })?;
        std::process::Command::new("git")
            .args(["-C", dir.to_str().unwrap()])
            .args(["remote", "add", "origin", repository_url])
            .spawn()
            .map_err(|source| GitError::SpawnFailed { source })?
            .wait()
            .map_err(|source| GitError::CommandFailed {
                message: format!("failed to set {repository_url} as remote"),
                source,
            })?;
        std::process::Command::new("git")
            .args(["-C", dir.to_str().unwrap()])
            .args(["fetch", "origin", "--depth=1"])
            .arg(revision)
            .spawn()
            .map_err(|source| GitError::SpawnFailed { source })?
            .wait()
            .map_err(|source| GitError::CommandFailed {
                message: format!("failed to fetch {revision} from {repository_url}"),
                source,
            })?;
        std::process::Command::new("git")
            .args(["-C", dir.to_str().unwrap()])
            .args(["reset", "--hard", "FETCH_HEAD"])
            .spawn()
            .map_err(|source| GitError::SpawnFailed { source })?
            .wait()
            .map_err(|source| GitError::CommandFailed {
                message: format!("failed to reset {} to {revision}", dir.display()),
                source,
            })?;
        Ok(())
    }
}

pub mod fs {
    use std::{fs, path::PathBuf, time::SystemTime};

    use thiserror::Error;

    #[derive(Error, Debug)]
    pub enum FsError {
        #[error("could not create symlink")]
        SymlinkFailed { source: std::io::Error },

        #[error("failed to read any file")]
        NoModificationFound,
    }

    #[cfg(unix)]
    pub fn symlink(from: &std::path::Path, to: &std::path::Path) -> Result<(), FsError> {
        std::os::unix::fs::symlink(to, from).map_err(|source| FsError::SymlinkFailed { source })
    }

    #[cfg(windows)]
    pub fn symlink(from: &std::path::Path, to: &std::path::Path) -> Result<(), FsError> {
        std::os::windows::fs::symlink_file(to, from)
            .map_err(|source| FsError::SymlinkFailed { source })
    }

    const ENTRY_LIMIT: u32 = u32::MAX;

    /// Returns the latest registered modification time inside a directory, including its
    /// subdirectories.
    ///
    /// This is intended as a "best effort" approximation, if it encounters any errors while reading
    /// an entry, it simply skips it. Additionally, as a safety net, the `ENTRY_LIMIT` sets an upper
    /// bound on the number of entries the function can check before returning.
    pub fn latest_modification(dir: &PathBuf) -> Result<(SystemTime, PathBuf), FsError> {
        fn traverse_directories(
            dir: &PathBuf,
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
            .map(|metadata| (metadata, dir.clone()))
            .ok();

        let (latest_found_modification, _) =
            traverse_directories(dir, directory_last_modification, 0);

        // This should only be an error if every single metadata read failed, which should be
        // unlikely.
        latest_found_modification.ok_or(FsError::NoModificationFound)
    }
}
