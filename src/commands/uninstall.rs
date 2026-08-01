use anyhow::{Context, bail};
use colored::Colorize;

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
        let channel_name = channel.to_string();

        if let Ok(entries) = std::fs::read_dir(paths::toolchains_dir(home)) {
            for dir_entry in entries.flatten() {
                let Ok(target) = std::fs::read_link(dir_entry.path()) else {
                    continue;
                };
                // A network link's target is the bare version. The channel's own link points into
                // `publications/`, so this never matches it and it needs no special case.
                if target == std::path::Path::new(&channel_name) {
                    std::fs::remove_file(dir_entry.path())?;
                }
            }
        }
    }

    // The commit point: after this the channel is uninstalled, and an interrupted run is completed
    // rather than rolled back.
    crate::publish::journal::commit_symlink(home, &entry)?;

    // `default` is the user's `midenup override` choice, not a derived link, so nothing would
    // recompute it -- which is why it is removed after the commit point rather than with the
    // network links. Either override form dangles once the channel is gone: one names a network
    // link that has just been removed, the other names the toolchain directory the commit
    // tombstoned. Testing for dangling covers both, and repairs one left over from any earlier
    // cause.
    let default = paths::toolchains_dir(home).join("default");
    if std::fs::symlink_metadata(&default).is_ok() && default.canonicalize().is_err() {
        std::fs::remove_file(&default)?;
        println!(
            "{}: removed the 'default' override, which named {channel}. Set a new one with:\n    \
             midenup override <channel>",
            "info".white().bold()
        );
    }

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
