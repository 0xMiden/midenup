//! Bringing a `$MIDENUP_HOME` written by an alpha `midenup` up to the network layout.
//!
//! Cheap, because of a decision made earlier: local state records channel versions and never
//! aliases, so `state.json` needs no change at all and `state_version` stays where it is. What
//! needs attention is only what is derived on disk.
//!
//! Runs alongside [`crate::migrate_v1::migrate_if_needed`], and *without* the home lock: it is on
//! the `miden` dispatch path, which must not wait on that lock, so every operation here is written
//! to tolerate another process having done it first.
//!
//! Idempotent, and two `stat` calls when there is nothing to do, because the manifest cache is
//! only read when a home is actually being converted.
//!
//! Deletable once alpha installations are gone.

use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::{channel::DEFAULT_NETWORK, paths};

/// The name an alpha `midenup` gave the link that `mainnet` now owns.
const LEGACY_LINK: &str = "stable";

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    NothingToDo,
    Migrated,
}

/// Renames the legacy `stable` link, repoints `default` at it, and drops a stale manifest cache.
pub fn migrate_if_needed(home: &Path) -> anyhow::Result<Outcome> {
    let mut migrated = false;

    let legacy = paths::network_link(home, LEGACY_LINK);
    let mainnet = paths::network_link(home, DEFAULT_NETWORK);

    // `symlink_metadata`, not `exists`: a link whose target has been removed still has to be
    // renamed, and `exists` follows the link and answers false for it.
    if std::fs::symlink_metadata(&legacy).is_ok() && std::fs::symlink_metadata(&mainnet).is_err() {
        match std::fs::rename(&legacy, &mainnet) {
            Ok(()) => {
                // Only an alpha home can be holding a cache this build cannot read, and only here
                // do we know we are looking at one. Checking on every startup would mean parsing
                // the cached manifest twice per command on the dispatch path, for an answer that
                // is almost always "nothing to do".
                drop_stale_manifest_cache(home)?;
                migrated = true;
            },
            // Another process migrated between the check and here. Deliberately unlocked: dispatch
            // must not wait on the home lock, so losing a race is expected rather than exceptional.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {},
            Err(err) => {
                return Err(err).context("failed to rename the legacy 'stable' link");
            },
        }
    }

    // Not conditional on the rename above: a run interrupted between the two would otherwise leave
    // `default` dangling forever. One `read_link` when there is nothing to do.
    if repoint_default(home)? {
        migrated = true;
    }

    Ok(if migrated {
        Outcome::Migrated
    } else {
        Outcome::NothingToDo
    })
}

/// `midenup override stable` pointed `default` at the `stable` link rather than at a toolchain
/// directory, so that it would follow the channel as it moved. Renaming that link without
/// repointing `default` leaves it dangling.
///
/// Returns whether it changed anything.
fn repoint_default(home: &Path) -> anyhow::Result<bool> {
    let default = paths::toolchains_dir(home).join("default");
    let Ok(target) = std::fs::read_link(&default) else {
        return Ok(false);
    };
    if target.file_name().and_then(|name| name.to_str()) != Some(LEGACY_LINK) {
        return Ok(false);
    }

    let replacement = if target.is_absolute() {
        paths::network_link(home, DEFAULT_NETWORK)
    } else {
        PathBuf::from(DEFAULT_NETWORK)
    };
    crate::utils::fs::replace_symlink(&default, &replacement)?;
    Ok(true)
}

/// Removes a cached manifest this build cannot read.
///
/// It would be rejected by the version check anyway; dropping it turns a confusing version error on
/// the first offline command into an ordinary refetch.
///
/// A v1 cache is *not* stale: [`crate::manifest::VersionedManifest::parse_str`] runs it through the
/// v1 converter, so deleting it would cost the offline capability this exists to preserve.
fn drop_stale_manifest_cache(home: &Path) -> anyhow::Result<()> {
    let cache = paths::manifest_cache(home);
    let Ok(contents) = std::fs::read_to_string(&cache) else {
        return Ok(());
    };

    let readable = crate::manifest::version::read_version_header(&contents, "manifest_version")
        .is_ok_and(|header| {
            header.version.major == crate::manifest::v3::MANIFEST_VERSION.major
                || header.version == crate::manifest::v1::MANIFEST_VERSION
        });
    if readable {
        return Ok(());
    }

    match std::fs::remove_file(&cache) {
        Ok(()) => Ok(()),
        // Another process dropped it first; see the rename in `migrate_if_needed`.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).context("failed to remove the stale manifest cache"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `$MIDENUP_HOME` as an alpha `midenup` left it.
    fn alpha_home() -> (tempdir::TempDir, PathBuf) {
        let temp = tempdir::TempDir::new("migrate-networks").unwrap();
        let home = temp.path().join("midenup");
        let toolchains = crate::paths::toolchains_dir(&home);
        std::fs::create_dir_all(&toolchains).unwrap();

        std::fs::create_dir_all(toolchains.join("0.15.0")).unwrap();
        std::os::unix::fs::symlink("0.15.0", toolchains.join("stable")).unwrap();
        std::os::unix::fs::symlink(toolchains.join("stable"), toolchains.join("default")).unwrap();
        std::fs::write(crate::paths::manifest_cache(&home), r#"{"manifest_version":"2.0.0"}"#)
            .unwrap();

        (temp, home)
    }

    #[test]
    fn the_stable_link_becomes_mainnet() {
        let (_temp, home) = alpha_home();
        assert_eq!(migrate_if_needed(&home).unwrap(), Outcome::Migrated);

        assert_eq!(
            std::fs::read_link(crate::paths::network_link(&home, "mainnet")).unwrap(),
            PathBuf::from("0.15.0")
        );
        assert!(
            std::fs::symlink_metadata(crate::paths::network_link(&home, "stable")).is_err(),
            "the old name must not be left behind"
        );
    }

    /// `midenup override stable` pointed `default` at the `stable` link so it would follow updates.
    /// Renaming the target without repointing `default` leaves it dangling.
    #[test]
    fn the_default_link_follows_the_rename() {
        let (_temp, home) = alpha_home();
        migrate_if_needed(&home).unwrap();

        let default = crate::paths::toolchains_dir(&home).join("default");
        let target = std::fs::read_link(&default).expect("default must still be a symlink");
        assert_eq!(target.file_name().unwrap(), "mainnet");
        assert!(default.canonicalize().is_ok(), "default must not dangle");
    }

    /// A v2 cache parses under no build that reads v3, and leaving it makes the first offline
    /// command fail with a version error instead of simply refetching.
    #[test]
    fn a_stale_manifest_cache_is_dropped() {
        let (_temp, home) = alpha_home();
        migrate_if_needed(&home).unwrap();
        assert!(!crate::paths::manifest_cache(&home).exists());
    }

    #[test]
    fn a_current_manifest_cache_is_kept() {
        let (_temp, home) = alpha_home();
        std::fs::write(crate::paths::manifest_cache(&home), r#"{"manifest_version":"3.0.0"}"#)
            .unwrap();
        migrate_if_needed(&home).unwrap();
        assert!(crate::paths::manifest_cache(&home).exists());
    }

    /// A v1 cache reads perfectly well through the v1 converter, so dropping it would strictly lose
    /// offline capability for the alpha user this migration exists to help.
    #[test]
    fn a_v1_manifest_cache_is_kept() {
        let (_temp, home) = alpha_home();
        std::fs::write(crate::paths::manifest_cache(&home), r#"{"manifest_version":"1.0.1"}"#)
            .unwrap();
        migrate_if_needed(&home).unwrap();
        assert!(crate::paths::manifest_cache(&home).exists());
    }

    /// `exists` follows the link and answers false for one whose target is gone, which would leave
    /// the legacy name in place forever.
    #[test]
    fn a_dangling_legacy_link_is_still_renamed() {
        let temp = tempdir::TempDir::new("migrate-networks-dangling").unwrap();
        let home = temp.path().join("midenup");
        let toolchains = crate::paths::toolchains_dir(&home);
        std::fs::create_dir_all(&toolchains).unwrap();
        std::os::unix::fs::symlink("0.15.0", toolchains.join("stable")).unwrap();

        assert_eq!(migrate_if_needed(&home).unwrap(), Outcome::Migrated);
        assert!(
            std::fs::symlink_metadata(crate::paths::network_link(&home, "mainnet")).is_ok(),
            "the link must be renamed even though its target is gone"
        );
    }

    /// `default` may name the legacy link relatively, in which case the replacement must stay
    /// relative rather than becoming an absolute path into this home.
    #[test]
    fn a_relative_default_link_follows_the_rename() {
        let (_temp, home) = alpha_home();
        let default = crate::paths::toolchains_dir(&home).join("default");
        std::fs::remove_file(&default).unwrap();
        std::os::unix::fs::symlink("stable", &default).unwrap();

        migrate_if_needed(&home).unwrap();
        assert_eq!(std::fs::read_link(&default).unwrap(), PathBuf::from("mainnet"));
    }

    /// `midenup override 0.15.0` pins a version directly; that is not the link being renamed.
    #[test]
    fn a_default_link_naming_a_version_is_left_alone() {
        let (_temp, home) = alpha_home();
        let default = crate::paths::toolchains_dir(&home).join("default");
        std::fs::remove_file(&default).unwrap();
        std::os::unix::fs::symlink("0.15.0", &default).unwrap();

        migrate_if_needed(&home).unwrap();
        assert_eq!(std::fs::read_link(&default).unwrap(), PathBuf::from("0.15.0"));
    }

    #[test]
    fn migration_is_idempotent() {
        let (_temp, home) = alpha_home();
        migrate_if_needed(&home).unwrap();
        let after_first = std::fs::read_link(crate::paths::network_link(&home, "mainnet")).unwrap();

        assert_eq!(migrate_if_needed(&home).unwrap(), Outcome::NothingToDo);
        assert_eq!(
            std::fs::read_link(crate::paths::network_link(&home, "mainnet")).unwrap(),
            after_first
        );
    }

    /// An installation that already has a mainnet link is not an alpha installation, and its
    /// mainnet link is the authority.
    #[test]
    fn an_existing_mainnet_link_is_not_overwritten() {
        let (_temp, home) = alpha_home();
        let toolchains = crate::paths::toolchains_dir(&home);
        std::fs::create_dir_all(toolchains.join("0.16.0")).unwrap();
        std::os::unix::fs::symlink("0.16.0", toolchains.join("mainnet")).unwrap();

        migrate_if_needed(&home).unwrap();
        assert_eq!(
            std::fs::read_link(crate::paths::network_link(&home, "mainnet")).unwrap(),
            PathBuf::from("0.16.0")
        );
    }

    #[test]
    fn a_home_that_does_not_exist_is_not_an_error() {
        let temp = tempdir::TempDir::new("migrate-networks-empty").unwrap();
        assert_eq!(migrate_if_needed(temp.path()).unwrap(), Outcome::NothingToDo);
    }
}
