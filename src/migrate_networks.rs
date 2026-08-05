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
        // Before the rename, deliberately: the legacy link is the only marker that this home
        // predates network-keyed `var/`, it names the version the store is under, and it survives
        // only until the rename. Doing this first means a run interrupted in between is retried in
        // full by the next one, and that a home with no legacy link is never touched at all.
        if adopt_var_for_default_network(home, &legacy)? {
            migrated = true;
        }

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

/// Gives the default network the store that a pre-network home wrote under the channel key.
///
/// `var/` is keyed by the toolchain selector ([`paths::var_dir`]), so a network's state lives at
/// `var/<network>`. Such a home has its state at `var/<version>` instead, and would otherwise
/// present the default network with an empty store while the real one sat under a key nothing
/// selects.
///
/// **Only the default network.** Such a home had exactly one store no matter how many networks
/// named the channel, and under this model that one store is the default network's -- it is what
/// the user was actually working against. The others were never separate and start empty, which is
/// the honest outcome: giving each a copy would present one set of accounts as three independent
/// stores.
///
/// `legacy` is where the version comes from, and its absence is proof that a home is not one to
/// convert -- see the call site.
///
/// **Its presence proves less.** An alpha `midenup` wrote that link for whichever channel was the
/// latest stable, whatever the user typed, so it is there for someone who pinned the newest release
/// just as it is for someone who asked for `stable`. Such a store is already keyed the way the user
/// selected it and moving it would hide it. Two forms of that pin, two treatments:
///
/// * `midenup override <version>` is recorded in the home, as a `default` naming the version rather
///   than the legacy link, and is detected here: the adoption is skipped entirely.
/// * A project's `miden-toolchain.toml` is not visible from the home at all, so the message this
///   prints on success says how to put the store back.
///
/// Returns whether it changed anything.
fn adopt_var_for_default_network(home: &Path, legacy: &Path) -> anyhow::Result<bool> {
    let Ok(named) = std::fs::read_link(legacy) else {
        return Ok(false);
    };
    let Some(named) = named.file_name() else {
        return Ok(false);
    };
    if default_pins_a_version(home) {
        return Ok(false);
    }

    let source = home.join("var").join(named);
    let destination = home.join("var").join(DEFAULT_NETWORK);
    // Both conditions are the idempotency check as well as the precondition: a second run finds no
    // source, and a home that already has a network-keyed store is never overwritten by one.
    if !source.is_dir() || destination.exists() {
        return Ok(false);
    }

    match std::fs::rename(&source, &destination) {
        Ok(()) => {
            println!(
                "moved your {DEFAULT_NETWORK} data from {} to {}: it is now keyed by the network \
                 rather than by the toolchain version.\nIf a project of yours pins {} in \
                 miden-toolchain.toml, move it back with:  mv {} {}",
                source.display(),
                destination.display(),
                named.to_string_lossy(),
                destination.display(),
                source.display(),
            );
            Ok(true)
        },
        // Another process got there first, or between the check and here: the source is gone, or
        // the destination it just created is in the way. Deliberately unlocked; see the rename in
        // `migrate_if_needed`.
        Err(err)
            if matches!(
                err.kind(),
                std::io::ErrorKind::NotFound
                    | std::io::ErrorKind::AlreadyExists
                    | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(false)
        },
        Err(err) => Err(err).context("failed to move the default network's data"),
    }
}

/// Whether `toolchains/default` names a toolchain directory rather than the legacy link.
///
/// That is what `midenup override <version>` wrote, and it is the one deliberate pin a home records
/// about itself. `default` naming the legacy link is `midenup override stable`, and no `default` at
/// all is a user who never overrode -- neither says anything about a version.
fn default_pins_a_version(home: &Path) -> bool {
    let default = paths::toolchains_dir(home).join("default");
    let Ok(target) = std::fs::read_link(&default) else {
        return false;
    };
    target.file_name().and_then(|name| name.to_str()) != Some(LEGACY_LINK)
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

    /// Seeds the store a pre-network home would have written, under the channel key.
    fn seed_var(home: &Path, key: &str) {
        let dir = home.join("var").join(key);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("store.sqlite3"), key.as_bytes()).unwrap();
    }

    fn var_store(home: &Path, key: &str) -> Option<Vec<u8>> {
        std::fs::read(home.join("var").join(key).join("store.sqlite3")).ok()
    }

    /// The one store such a home has is the default network's, and it has to arrive at the key the
    /// default network is now resolved under or it would be invisible.
    #[test]
    fn the_single_store_becomes_the_default_networks() {
        let (_temp, home) = alpha_home();
        seed_var(&home, "0.15.0");

        assert_eq!(migrate_if_needed(&home).unwrap(), Outcome::Migrated);
        assert_eq!(var_store(&home, DEFAULT_NETWORK).as_deref(), Some(&b"0.15.0"[..]));
        assert!(
            !home.join("var").join("0.15.0").exists(),
            "the old key must not be left holding a second copy"
        );
    }

    /// The other networks were never separate stores in such a home, so they start empty rather
    /// than each receiving a copy of the same accounts.
    #[test]
    fn the_other_networks_start_empty() {
        let (_temp, home) = alpha_home();
        seed_var(&home, "0.15.0");
        migrate_if_needed(&home).unwrap();

        for network in ["testnet", "devnet"] {
            assert!(
                !home.join("var").join(network).exists(),
                "{network} must not be given a copy of the default network's store"
            );
        }
    }

    #[test]
    fn adopting_the_store_is_idempotent() {
        let (_temp, home) = alpha_home();
        seed_var(&home, "0.15.0");
        migrate_if_needed(&home).unwrap();

        assert_eq!(migrate_if_needed(&home).unwrap(), Outcome::NothingToDo);
        assert_eq!(var_store(&home, DEFAULT_NETWORK).as_deref(), Some(&b"0.15.0"[..]));
    }

    /// A home that already has a network-keyed store is not a home that needs converting, and the
    /// store it has is the authority.
    #[test]
    fn an_existing_network_store_is_not_overwritten() {
        let (_temp, home) = alpha_home();
        seed_var(&home, "0.15.0");
        seed_var(&home, DEFAULT_NETWORK);

        migrate_if_needed(&home).unwrap();
        assert_eq!(var_store(&home, DEFAULT_NETWORK).as_deref(), Some(&b"mainnet"[..]));
        assert_eq!(
            var_store(&home, "0.15.0").as_deref(),
            Some(&b"0.15.0"[..]),
            "and the one that could not move stays where the user can find it"
        );
    }

    /// The hard half of the one below: the pinned version is the one the legacy link names, because
    /// an alpha `midenup` wrote that link for whatever was the latest stable. `default` naming the
    /// version rather than the link is what says the user chose it, and their store is already
    /// under the key that selects it.
    #[test]
    fn a_store_under_a_pinned_default_is_left_alone() {
        let (_temp, home) = alpha_home();
        let default = crate::paths::toolchains_dir(&home).join("default");
        std::fs::remove_file(&default).unwrap();
        std::os::unix::fs::symlink("0.15.0", &default).unwrap();
        seed_var(&home, "0.15.0");

        migrate_if_needed(&home).unwrap();
        assert_eq!(var_store(&home, "0.15.0").as_deref(), Some(&b"0.15.0"[..]));
        assert!(
            !home.join("var").join(DEFAULT_NETWORK).exists(),
            "a deliberately pinned store must not be moved out from under the pin"
        );
    }

    /// A store under a version no network names was selected by pinning that version, and pinning
    /// keys on the version -- so it is already where it belongs.
    #[test]
    fn a_pinned_store_is_left_alone() {
        let (_temp, home) = alpha_home();
        seed_var(&home, "0.9.0");

        migrate_if_needed(&home).unwrap();
        assert_eq!(var_store(&home, "0.9.0").as_deref(), Some(&b"0.9.0"[..]));
        assert!(!home.join("var").join(DEFAULT_NETWORK).exists());
    }

    #[test]
    fn a_home_that_does_not_exist_is_not_an_error() {
        let temp = tempdir::TempDir::new("migrate-networks-empty").unwrap();
        assert_eq!(migrate_if_needed(temp.path()).unwrap(), Outcome::NothingToDo);
    }
}
