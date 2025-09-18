use std::{cmp::max, fs, path::PathBuf, time::SystemTime};

/// This file contains some general purpose functions.
use anyhow::Context;

#[cfg(unix)]
pub fn symlink(from: &std::path::Path, to: &std::path::Path) -> anyhow::Result<()> {
    std::os::unix::fs::symlink(to, from).context("could not create symlink")
}

#[cfg(windows)]
pub fn symlink(from: &std::path::Path, to: &std::path::Path) -> anyhow::Result<()> {
    std::os::windows::fs::symlink_file(to, from).context("could not create symlink")
}

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

    // This returns a string of the form:
    // sym_ref\tref_name
    // Source: https://github.com/git/git/blob/41905d60226a0346b22f0d0d99428c746a5a3b14/builtin/ls-remote.c#L169
    let revision_hash: String = String::from_utf8(check_revision_hash.stdout)
        .context(format!(
            "failed to format latest git rev-hash from branch {branch_name}, does the branch exist?.",
        ))?
        .chars()
        .take_while(|&c| c != '\t')
        .collect();

    Ok(revision_hash)
}

/// Returns the latest registered modification time inside a directory,
/// including its subdirectories. This is intended as a "best effort"
/// aproximation, if it encounters any errors while reading an entry, it simply
/// skips it. Additionally, as a safety net, the [[ENTRY_LIMIT]] sets an upper
/// bound on the number of entries the function can check before returning.
const ENTRY_LIMIT: u32 = u32::MAX;
pub fn latest_modification(dir: PathBuf) -> anyhow::Result<(SystemTime, PathBuf)> {
    fn traverse_directories(
        dir: PathBuf,
        latest: Option<(SystemTime, PathBuf)>,
        current_entry: u32,
    ) -> (Option<(SystemTime, PathBuf)>, u32) {
        let mut local_latest = latest;
        let mut current_entry_count = current_entry;

        let entries = fs::read_dir(&dir);
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
                        traverse_directories(file.path(), local_latest.clone(), current_entry_count)
                    } else {
                        (metadata.modified().ok().map(|metadata| (metadata, file.path())), current_entry_count + 1)
                    };

                current_entry_count = visited_entries;

                local_latest = match (&local_latest, current_entry_latest) {
                    (Some((local_latest_time, path_old)), Some((current_entry_latest, path))) => {
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
        .and_then(|metadata| Ok((metadata, dir.clone())))
        .ok();

    let (latest_found_modification, _) = traverse_directories(dir, directory_last_modification, 0);

    // This should only be an error if every single metadata read failed, which should be unlikely.
    latest_found_modification.context("Failed to read any file")
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Local};

    use super::latest_modification;

    #[test]
    fn latest_modification_test() {
        let result =
            latest_modification("/Users/fabri/Repositories/miden-compiler-lambda-fork".into());
        let date: DateTime<Local> = result.unwrap().clone().into();

        // std::dbg!(result.as_ref());
        std::dbg!(date);
    }
}
