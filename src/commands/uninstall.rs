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
    let installed = config.local_channel(requested, state).and_then(|version| state.get(&version));

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

    // Derived pointers (`stable`, a network name) are recomputed by the next install or update, so
    // removing them before the commit costs nothing if the operation is discarded.
    //
    // Which names pointed here is *not* knowable from local state, which records channel versions
    // and not names, and upstream may be unreachable. So rather than guessing a list, this removes
    // whichever pointers actually resolve to the channel being uninstalled. `default` is excluded
    // because it is the user's explicit choice rather than something derived.
    {
        let toolchain_link = paths::toolchain_link(home, &channel);
        let toolchains_dir = paths::toolchains_dir(home);
        let target = toolchain_link.canonicalize().ok();

        if let (Some(target), Ok(entries)) = (target, std::fs::read_dir(&toolchains_dir)) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();

                // Skip the channel's own version-named link -- the commit point handles that -- and
                // never touch `default`.
                if path == toolchain_link || name == "default" {
                    continue;
                }
                if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
                    continue;
                }
                if path.canonicalize().ok().as_ref() == Some(&target) {
                    std::fs::remove_file(&path)?;
                }
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
