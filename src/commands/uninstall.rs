use anyhow::{Context, bail};

use crate::{
    channel::UserChannel,
    config::Config,
    paths,
    publish::JournalEntry,
    state::{LocalState, PublicationRef},
};

/// Removes an installed channel.
///
/// Uses the same journalled sequence as install (spec section 9.5), with one difference: the commit
/// point replaces `toolchains/<channel>` with a **tombstone** rather than repointing it. That is
/// what lets recovery tell a removal that was committed and interrupted apart from a toolchain
/// somebody deleted by hand -- the first is completed, the second is reported.
///
/// The publication is removed wholesale. There is no per-component removal pass: the publication
/// directory contains exactly what its receipt says it does and nothing else, so walking components
/// to delete their files individually could only ever remove less than the directory itself.
///
/// `var/<channel>` is **kept** unless `purge` is given. It is the user's data -- the client's
/// database lives there -- and removing a toolchain is not a request to delete it. The user is told
/// where it was left.
pub fn uninstall(
    config: &Config,
    requested: &UserChannel,
    state: &mut LocalState,
    purge: bool,
) -> anyhow::Result<()> {
    // Resolved against local state, never upstream: a channel that has been withdrawn upstream is
    // precisely one a user needs to be able to remove (spec section 12.3).
    let installed = config.local_channel(requested).and_then(|version| state.get(&version));

    let Some(installation) = installed else {
        bail!("channel {requested} is not installed, nothing to uninstall");
    };
    let channel = installation.channel.clone();
    let publication = match &installation.publication {
        PublicationRef::Managed { id, .. } => Some(id.clone()),
        // Carried over from v1: nothing describes what it owns, so there is no publication to
        // reclaim. The state record still goes.
        PublicationRef::NeedsReinstall => None,
    };

    let home = &config.midenup_home;
    let entry = JournalEntry::uninstall(channel.clone(), publication);
    crate::publish::journal::prepare(home, &entry)?;

    // Network links are derived, so removing them before the commit costs nothing if the operation
    // is discarded: the next install or update recomputes them from upstream.
    //
    // Found by scanning rather than by asking upstream which networks name this channel. Uninstall
    // has to work offline, and a network may have moved upstream since this machine last looked --
    // in which case upstream would not name the link that is actually here.
    {
        let toolchains = paths::toolchains_dir(home);
        let toolchain_link = paths::toolchain_link(home, &channel);
        let channel_name = channel.to_string();
        let mut removed_links = Vec::new();

        if let Ok(entries) = std::fs::read_dir(&toolchains) {
            for entry in entries.flatten() {
                let path = entry.path();
                // The channel's own link points into `publications/`; a network link's target is
                // the bare version. That difference is what distinguishes them.
                if path == toolchain_link {
                    continue;
                }
                let Ok(target) = std::fs::read_link(&path) else {
                    continue;
                };
                if target == std::path::Path::new(&channel_name) {
                    std::fs::remove_file(&path)?;
                    if let Some(name) = entry.file_name().to_str() {
                        removed_links.push(name.to_string());
                    }
                }
            }
        }

        // `default` may point at a network link, so that it follows the network as it moves, or
        // straight at the toolchain directory when a version was pinned. Both dangle once this
        // channel is gone, and neither used to be handled -- which is what `midenup override
        // stable` followed by an uninstall used to leave behind.
        let default = toolchains.join("default");
        if let Ok(target) = std::fs::read_link(&default) {
            let names_removed_link = target
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| removed_links.iter().any(|removed| removed == name));
            let names_this_toolchain = target == toolchain_link
                || target.file_name() == std::path::Path::new(&channel_name).file_name();

            if names_removed_link || names_this_toolchain {
                std::fs::remove_file(&default)?;
            }
        }
    }

    // The commit point: after this the channel is uninstalled, and an interrupted run is completed
    // rather than rolled back.
    crate::publish::journal::commit_symlink(home, &entry)?;

    // Removes the state record, reclaims the publication, clears the tombstone, deletes the
    // journal.
    crate::publish::journal::finish(home, &entry, state)?;

    // Only now, and only if asked. Deliberately after the commit: this is the one part of an
    // uninstall that cannot be undone by reinstalling.
    let var = paths::var_dir(home, &channel);
    if var.is_dir() {
        if purge {
            std::fs::remove_dir_all(&var)
                .with_context(|| format!("failed to remove '{}'", var.display()))?;
        } else {
            println!(
                "kept your data for {channel} at {}; remove it with `midenup uninstall {channel} \
                 --purge`",
                var.display()
            );
        }
    }

    Ok(())
}
