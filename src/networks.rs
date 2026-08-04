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
        // `midenup override` sets "default", is not a network link.
        if name == "default" {
            continue;
        }
        let Ok(target) = std::fs::read_link(entry.path()) else {
            continue;
        };

        // The whole target is parsed as a version, which is what makes this rule sufficient on
        // its own: a version string cannot contain a separator, so this admits only a
        // single-segment relative target. Everything else in the directory is excluded by
        // it -- a channel's own link names `../publications/<channel>-<id>`, a tombstone
        // names `.uninstalled`, and `default` is absolute under either override form.
        let Some(channel) = target.to_str().and_then(|target| semver::Version::parse(target).ok())
        else {
            continue;
        };

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
