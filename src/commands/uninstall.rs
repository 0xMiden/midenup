use anyhow::bail;

use crate::{
    channel::Channel,
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
pub fn uninstall(
    config: &Config,
    upstream_channel: &Channel,
    state: &mut LocalState,
) -> anyhow::Result<()> {
    let Some(installation) = state.get(&upstream_channel.name) else {
        bail!("channel {} is not installed, nothing to uninstall", upstream_channel.name);
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

    // `stable` is derived, so removing it before the commit costs nothing if the operation is
    // discarded: the next install or update recomputes it from upstream.
    {
        let toolchain_link = paths::toolchain_link(home, &channel);
        let stable_symlink = paths::toolchains_dir(home).join("stable");

        // Only remove it if it actually points at the toolchain being uninstalled -- it may have
        // just been created for a channel this one migrated into.
        let points_here = stable_symlink
            .canonicalize()
            .ok()
            .zip(toolchain_link.canonicalize().ok())
            .map(|(stable, toolchain)| stable == toolchain)
            .unwrap_or(false);

        if points_here && std::fs::symlink_metadata(&stable_symlink).is_ok() {
            std::fs::remove_file(&stable_symlink)?;
        }
    }

    // The commit point: after this the channel is uninstalled, and an interrupted run is completed
    // rather than rolled back.
    crate::publish::journal::commit_symlink(home, &entry)?;

    // Removes the state record, reclaims the publication, clears the tombstone, deletes the
    // journal.
    crate::publish::journal::finish(home, &entry, state)?;

    Ok(())
}
