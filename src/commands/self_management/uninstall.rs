//! `midenup self uninstall`: removing the executable, its `miden` link, and the whole home.
//!
//! Everything to be removed is resolved and checked before the user is asked, and nothing is
//! created or changed until they confirm. The home is then taken exclusively through the format
//! lease, which every other `midenup` and `miden` process holds shared while it runs, so uninstall
//! refuses while anything is using the home instead of deleting files out from under it.
//!
//! Removal is not a transaction. External entries go first, while the home is still in place; the
//! home is then renamed into a reserved sibling directory and deleted there. After that rename,
//! nothing touches the home's path again, so a home a new process creates there meanwhile is never
//! removed. A failure reports what is left and where.

use std::{
    io::{BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};

use crate::{config, lock};

/// Everything an uninstall removes, resolved before anything is removed.
#[derive(Debug)]
struct Removal {
    /// The real home directory, if there is one.
    home: Option<PathBuf>,
    /// The configured home, when it is a symlink to [Removal::home].
    home_link: Option<PathBuf>,
    /// The running executable, unless it lives inside the home and goes with it.
    executable: Option<PathBuf>,
    /// `miden` links that resolve to the running executable.
    links: Vec<PathBuf>,
}

pub fn uninstall(config: &config::Config, force: bool) -> anyhow::Result<()> {
    let executable = super::running_executable()?;
    let removal = resolve(config, &executable)?;

    if !force && !confirm(&removal)? {
        crate::info!("uninstall cancelled; nothing was removed");
        return Ok(());
    }

    // Reserved before anything is taken or removed: if the home cannot be moved aside, nothing else
    // may go either, and nothing is left behind.
    let reserved = removal.home.as_deref().map(Reservation::beside).transpose()?;

    // Held until the home is gone. Its file moves with the home, which is fine: what matters is
    // that no other process can hold it while this one removes the home.
    let _lease = match &removal.home {
        Some(home) => Some(lock::exclusive_lease(home).with_context(|| {
            format!("refusing to uninstall while '{}' is in use", home.display())
        })?),
        None => None,
    };

    for link in &removal.links {
        remove_file(link)?;
    }
    if let Some(executable) = &removal.executable {
        remove_file(executable)?;
    }
    if let Some(link) = &removal.home_link {
        remove_file(link)?;
    }
    if let (Some(home), Some(reserved)) = (&removal.home, reserved) {
        reserved.remove_home(home)?;
    }

    crate::info!("midenup has been uninstalled");
    Ok(())
}

/// Resolves what to remove and refuses a home whose removal would take more than midenup's data.
fn resolve(config: &config::Config, executable: &Path) -> anyhow::Result<Removal> {
    let configured = std::path::absolute(&config.midenup_home).with_context(|| {
        format!("failed to resolve the midenup home '{}'", config.midenup_home.display())
    })?;
    let is_link = configured.symlink_metadata().is_ok_and(|meta| meta.file_type().is_symlink());
    let home = match configured.canonicalize() {
        Ok(home) => Some(home),
        // No home, or a link to one that is gone: the link, if any, is all there is to remove.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => {
            return Err(err).with_context(|| {
                format!("failed to resolve the midenup home '{}'", configured.display())
            });
        },
    };

    if let Some(home) = &home {
        let user_home = dirs::home_dir().map(|dir| resolve_lenient(&dir));
        let cargo_home = resolve_lenient(&config.cargo_home);
        check_removable(home, user_home.as_deref(), &cargo_home)?;
    }

    let inside_home = |path: &Path| home.as_ref().is_some_and(|home| path.starts_with(home));

    let mut links = Vec::new();
    let candidates =
        [config.cargo_home.join("bin").join("miden"), executable.with_file_name("miden")];
    for candidate in candidates {
        let candidate = std::path::absolute(&candidate).unwrap_or(candidate);
        let is_symlink =
            candidate.symlink_metadata().is_ok_and(|meta| meta.file_type().is_symlink());
        let ours = is_symlink && candidate.canonicalize().is_ok_and(|target| target == executable);
        // Resolved by its parent, so the same entry reached through two spellings is removed once.
        let resolved = candidate
            .parent()
            .and_then(|parent| parent.canonicalize().ok())
            .map(|parent| parent.join("miden"));
        if let Some(resolved) = resolved
            && ours
            && !inside_home(&resolved)
            && !links.contains(&resolved)
        {
            links.push(resolved);
        }
    }

    Ok(Removal {
        home_link: is_link.then_some(configured),
        executable: (!inside_home(executable)).then(|| executable.to_path_buf()),
        home,
        links,
    })
}

/// Refuses a home that is the filesystem root, or that is or contains the user's home directory or
/// `CARGO_HOME`. Applies even with `--force`: these are configuration mistakes, and deleting the
/// home would delete far more than midenup owns.
fn check_removable(home: &Path, user_home: Option<&Path>, cargo_home: &Path) -> anyhow::Result<()> {
    if home.parent().is_none() {
        bail!("refusing to uninstall: the midenup home is the filesystem root");
    }
    if let Some(user_home) = user_home
        && user_home.starts_with(home)
    {
        bail!(
            "refusing to uninstall: the midenup home '{}' contains your home directory",
            home.display()
        );
    }
    if cargo_home.starts_with(home) {
        bail!(
            "refusing to uninstall: the midenup home '{}' contains CARGO_HOME '{}'",
            home.display(),
            cargo_home.display()
        );
    }
    Ok(())
}

/// `path` with symlinks resolved as far as it exists, so a directory that does not exist yet is
/// still compared by where it would be.
fn resolve_lenient(path: &Path) -> PathBuf {
    let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut missing = Vec::new();
    let mut existing = path.as_path();
    loop {
        if let Ok(resolved) = existing.canonicalize() {
            return missing.iter().rev().fold(resolved, |dir, name| dir.join(name));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name.to_os_string());
                existing = parent;
            },
            _ => return path,
        }
    }
}

/// Asks once for the whole uninstall. Only `y` or `yes` confirms; anything else, including an empty
/// answer or a closed input, cancels.
fn confirm(removal: &Removal) -> anyhow::Result<bool> {
    let mut prompt = String::from("This will uninstall midenup and remove:\n");
    if let Some(home) = &removal.home {
        let shown = removal.home_link.as_ref().unwrap_or(home);
        prompt +=
            &format!("  - {} (all toolchains, configuration, caches and data)\n", shown.display());
    }
    if let Some(executable) = &removal.executable {
        prompt += &format!("  - {}\n", executable.display());
    }
    for link in &removal.links {
        prompt += &format!("  - {}\n", link.display());
    }
    prompt += "Continue? [y/N] ";

    let mut stderr = std::io::stderr().lock();
    stderr
        .write_all(prompt.as_bytes())
        .context("failed to show the confirmation prompt")?;
    stderr.flush().context("failed to show the confirmation prompt")?;
    drop(stderr);

    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .context("failed to read the confirmation")?;
    // A typed answer ends the prompt's line; one that was piped in is not echoed.
    if !std::io::stdin().is_terminal() {
        eprintln!();
    }
    let answer = answer.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}

fn remove_file(path: &Path) -> anyhow::Result<()> {
    std::fs::remove_file(path).with_context(|| {
        format!(
            "failed to remove '{}'; uninstall stopped before removing the home",
            path.display()
        )
    })?;
    crate::info!("removed {}", path.display());
    Ok(())
}

/// A private, empty directory beside the home, which the home is moved into for removal.
///
/// Reserving it is the last step that can fail for lack of permission on the home's parent, so it
/// happens before anything is removed. An unused reservation is removed when dropped.
#[derive(Debug)]
struct Reservation {
    dir: PathBuf,
}

impl Reservation {
    fn beside(home: &Path) -> anyhow::Result<Self> {
        let (Some(parent), Some(name)) = (home.parent(), home.file_name()) else {
            bail!("refusing to uninstall: cannot remove '{}'", home.display());
        };
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos())
            .unwrap_or(0);
        let dir = parent.join(format!(
            ".{}.uninstall-{}-{nanos}",
            name.to_string_lossy(),
            std::process::id()
        ));
        std::fs::create_dir(&dir).with_context(|| {
            format!(
                "failed to prepare removing '{}' in '{}'; nothing was removed",
                home.display(),
                parent.display()
            )
        })?;
        Ok(Self { dir })
    }

    /// Moves `home` into the reservation, then deletes it. The rename detaches the home in one
    /// step; from then on only the reservation is touched.
    fn remove_home(self, home: &Path) -> anyhow::Result<()> {
        let name = home.file_name().expect("checked when reserved");
        std::fs::rename(home, self.dir.join(name)).with_context(|| {
            format!("failed to move '{}' aside for removal; it was left in place", home.display())
        })?;

        std::fs::remove_dir_all(&self.dir).with_context(|| {
            format!(
                "removed midenup from '{}', but could not delete all of it; remove '{}' manually",
                home.display(),
                self.dir.display()
            )
        })?;
        crate::info!("removed {}", home.display());
        Ok(())
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        // Only an empty reservation goes: one still holding the home is reported, not deleted.
        let _ = std::fs::remove_dir(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_home_holding_more_than_midenup_data_is_refused() {
        let user = Path::new("/home/user");
        let cargo = Path::new("/home/user/.cargo");
        for home in ["/", "/home", "/home/user", "/home/user/.cargo"] {
            let home = Path::new(home);
            assert!(check_removable(home, Some(user), cargo).is_err(), "{}", home.display());
        }
        // CARGO_HOME inside the midenup home.
        let home = Path::new("/home/user/.local/share/midenup");
        assert!(check_removable(home, Some(user), &home.join("cargo")).is_err());
    }

    #[test]
    fn an_ordinary_home_is_removable() {
        let user = Path::new("/home/user");
        let cargo = Path::new("/home/user/.cargo");
        for home in [
            "/home/user/.local/share/midenup",
            "/home/user/Library/Application Support/midenup",
            // A sibling whose name merely starts with the user's home is not inside it.
            "/home/user-midenup",
            // Inside CARGO_HOME is fine: removing it leaves Cargo's own files alone.
            "/home/user/.cargo/midenup",
        ] {
            check_removable(Path::new(home), Some(user), cargo).expect(home);
        }
    }

    #[test]
    fn missing_paths_resolve_through_their_existing_ancestors() {
        let temp = tempdir::TempDir::new("resolve-lenient").unwrap();
        let real = temp.path().canonicalize().unwrap();
        assert_eq!(resolve_lenient(&temp.path().join("a/b")), real.join("a/b"));
    }
}
