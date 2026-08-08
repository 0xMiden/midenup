//! Which channel each network names on this machine, and whether upstream still agrees.
//!
//! `toolchains/<network>` records the last answer upstream gave that this machine acted on.

use std::{collections::BTreeMap, path::Path};

use crate::paths;

/// Every `toolchains/<network>` link on this machine, with the channel it names.
///
/// The inverse of [`paths::network_link`]. An unreadable or absent `toolchains/` is an empty map
/// rather than an error: every caller is reporting, and has nothing to say about a home with no
/// links in it.
pub fn links(home: &Path) -> BTreeMap<String, semver::Version> {
    let mut links = BTreeMap::new();

    let Ok(entries) = std::fs::read_dir(paths::toolchains_dir(home)) else {
        return links;
    };

    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        // `midenup override` sets "default", it is not a network link.
        if name == "default" {
            continue;
        }
        let Ok(target) = std::fs::read_link(entry.path()) else {
            continue;
        };

        // A network link names a version that lives directly under `toolchains/`, written either
        // bare or as a full path -- both spellings are in use, the latter by a home carried over
        // from before the network layout. Both halves of the rule are needed: a tombstone names
        // `.uninstalled`, which is no version at all, while a channel's own link names
        // `../publications/<channel>-<id>`, whose file name *does* parse as a version -- one with
        // the publication id as its prerelease -- so the directory is what rules it out.
        let Some(channel) = target
            .file_name()
            .and_then(|channel| channel.to_str())
            .and_then(|channel| semver::Version::parse(channel).ok())
        else {
            continue;
        };
        // Compared as text rather than resolved, so a link whose channel has been removed is still
        // recognized as the network link it is. A bare `<version>` has an empty parent.
        let inside_toolchains = target
            .parent()
            .is_some_and(|dir| dir.as_os_str().is_empty() || dir == paths::toolchains_dir(home));
        if !inside_toolchains {
            continue;
        }

        links.insert(name, channel);
    }

    links
}

/// Report to show a user whose network link no longer agrees with upstream.
///
/// `upstream` is what `networks[network]` names now, and `None` means the network is no longer
/// declared at all. Returns the parenthesized marker for `midenup show list`. A published rollback
/// is reported too. Only the wording differs.
pub fn drift(
    network: &str,
    local: &semver::Version,
    upstream: Option<&semver::Version>,
) -> Option<String> {
    let Some(upstream) = upstream else {
        // No remedy is offered: `midenup update <network>` resolves the name against the manifest,
        // so it would fail with "unknown channel" rather than fixing anything.
        return Some(format!("({network} is no longer declared upstream)"));
    };

    if upstream == local {
        return None;
    }

    let movement = if upstream > local {
        format!("is now {upstream}")
    } else {
        format!("has moved back to {upstream}")
    };

    Some(format!("({network} {movement} upstream -- run `midenup update {network}`)"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A `$MIDENUP_HOME` whose `toolchains/` holds one symlink per `(name, target)` pair.
    fn home_with(entries: &[(&str, &str)]) -> (tempdir::TempDir, PathBuf) {
        let temp = tempdir::TempDir::new("networks").unwrap();
        let home = temp.path().join("midenup");
        std::fs::create_dir_all(paths::toolchains_dir(&home)).unwrap();

        for (name, target) in entries {
            std::os::unix::fs::symlink(target, paths::network_link(&home, name)).unwrap();
        }

        (temp, home)
    }

    /// [`links`] as `(network, channel)` strings, so expectations read as they do on disk.
    fn links_of(home: &Path) -> Vec<(String, String)> {
        links(home)
            .into_iter()
            .map(|(network, channel)| (network, channel.to_string()))
            .collect()
    }

    /// Every other kind of entry `toolchains/` holds must be excluded, and several networks naming
    /// one channel -- the state after a promotion -- is normal.
    #[test]
    fn only_network_links_are_reported() {
        let (_temp, home) = home_with(&[
            ("0.15.0", "../publications/0.15.0-abc123"), // the channel's own link
            ("0.14.0", ".uninstalled"),                  // an interrupted uninstall's tombstone
            ("default", "mainnet"),                      // `midenup override mainnet`
            ("mainnet", "0.15.0"),
            ("testnet", "0.15.0"),
        ]);
        std::fs::write(paths::toolchains_dir(&home).join("notes.txt"), b"whatever").unwrap();

        assert_eq!(
            links_of(&home),
            vec![
                ("mainnet".to_string(), "0.15.0".to_string()),
                ("testnet".to_string(), "0.15.0".to_string()),
            ]
        );
    }

    /// A home carried over from before the network layout spells its links absolutely, and the
    /// channel one of those names is running just as much as any other. A channel's own link
    /// written the same way stays excluded: it names an entry in `publications/`, not in
    /// `toolchains/`.
    #[test]
    fn an_absolute_link_names_its_channel() {
        let (_temp, home) = home_with(&[]);
        let toolchains = paths::toolchains_dir(&home);
        std::os::unix::fs::symlink(toolchains.join("0.14.0"), toolchains.join("mainnet")).unwrap();
        std::os::unix::fs::symlink(
            home.join("publications").join("0.14.0-abc123"),
            toolchains.join("0.14.0"),
        )
        .unwrap();

        assert_eq!(links_of(&home), vec![("mainnet".to_string(), "0.14.0".to_string())]);
    }

    /// `midenup override 0.15.0` points `default` at the toolchain directory, absolutely. Parsing
    /// the target's file name rather than the whole target would read this as a network called
    /// `default` naming 0.15.0.
    #[test]
    fn a_version_override_is_not_mistaken_for_a_network() {
        let (_temp, home) = home_with(&[]);
        let toolchains = paths::toolchains_dir(&home);
        std::os::unix::fs::symlink(toolchains.join("0.15.0"), toolchains.join("default")).unwrap();

        assert!(links_of(&home).is_empty(), "`default` is never a network link");
    }

    /// [`drift`] over versions written the way the report reads them back.
    fn report(local: &str, upstream: Option<&str>) -> Option<String> {
        let parse = |version: &str| semver::Version::parse(version).unwrap();
        let upstream = upstream.map(parse);
        drift("mainnet", &parse(local), upstream.as_ref())
    }

    /// Both directions name the channel to move to and the command that moves there: a user on a
    /// channel upstream has rolled back from is in the same position as one who is behind. Only the
    /// wording differs, since "is now" would describe a move forwards that did not happen.
    #[test]
    fn a_moved_network_names_the_new_channel_and_the_remedy() {
        let ahead = report("0.14.0", Some("0.15.0")).expect("a moved network must be reported");
        assert!(ahead.contains("is now 0.15.0"), "{ahead}");
        assert!(ahead.contains("midenup update mainnet"), "{ahead}");

        let back = report("0.15.0", Some("0.14.0")).expect("a rollback must be reported");
        assert!(back.contains("moved back to 0.14.0"), "{back}");
        assert!(back.contains("midenup update mainnet"), "{back}");
    }

    /// `midenup update <network>` resolves the name against the manifest, so suggesting it for a
    /// network the manifest no longer declares would send the user to an "unknown channel" error.
    #[test]
    fn an_undeclared_network_is_reported_without_a_remedy() {
        let reported = report("0.15.0", None).expect("a withdrawn network must still be reported");

        assert!(reported.contains("no longer declared upstream"), "{reported}");
        assert!(!reported.contains("midenup update"), "there is nothing to run: {reported}");
    }
}
