//! The `$MIDENUP_HOME` layout, in one place.
//!
//! Every path under `MIDENUP_HOME` is named by exactly one function here, so that `install` and
//! `uninstall` cannot disagree about where anything lives. A layout path spelled inline is a
//! second answer to that question, with nothing connecting it to this one.
//!
//! ```text
//! $MIDENUP_HOME/
//! |- state.json                          local installation state
//! |- publications/
//! |  |- <channel>-<publication-id>/       immutable; named opaquely
//! |     |- receipt.json
//! |     |- bin/ lib/ etc/ opt/
//! |- toolchains/
//! |  |- <channel>   -> ../publications/<channel>-<publication-id>
//! |  |- <network>   -> <channel>          one per network naming this channel
//! |  |- default     -> <channel> | <network>
//! |- var/
//! |  |- <selector>/                       mutable state, keyed by what the user selected:
//! |                                       a network name, or a pinned version
//! |- opt            -> toolchains/<active>/opt
//! ```

use std::path::{Path, PathBuf};

use crate::{channel::UserChannel, state::PublicationId};

/// Local installation state: the sole logical authority on what is installed.
pub fn state_path(home: &Path) -> PathBuf {
    home.join("state").with_extension("json")
}

/// The last upstream manifest that was successfully fetched, cached verbatim.
///
/// Consulted only when a fetch fails: an operation that needs upstream can then proceed against a
/// copy that is known to have been real, and say that it is doing so, rather than failing outright
/// because a network was briefly unavailable.
pub fn manifest_cache(home: &Path) -> PathBuf {
    home.join("channel-manifest").with_extension("json")
}

/// The directory of `toolchains/<channel>` symlinks, plus the derived network links and `default`.
pub fn toolchains_dir(home: &Path) -> PathBuf {
    home.join("toolchains")
}

/// The stable name for a channel: a symlink into `publications/`.
///
/// This is what every consumer -- `PATH`, `MIDEN_SYSROOT`, `%lib`, `%etc` -- refers to, which is
/// what lets the publication behind it be replaced atomically.
pub fn toolchain_link(home: &Path, channel: &semver::Version) -> PathBuf {
    toolchains_dir(home).join(channel.to_string())
}

/// A network's symlink: `toolchains/<network>` -> `<channel>`.
///
/// Records the last answer upstream gave about this network *that this machine acted on*. It is
/// deliberately not repointed at a channel that is not installed, which would leave it dangling;
/// `midenup update <network>` is what advances it.
pub fn network_link(home: &Path, network: &str) -> PathBuf {
    toolchains_dir(home).join(network)
}

/// Where installed trees live.
pub fn publications_dir(home: &Path) -> PathBuf {
    home.join("publications")
}

/// Mutable, component-owned state: `%var`. The Miden client's database lives here (`%var(data)`).
///
/// Outside the publication, because a publication is replaced wholesale on every change and this
/// must survive that.
///
/// **Keyed by the toolchain selector the user chose, not by the channel it resolves to.** A network
/// is a moving name, so `mainnet` and `testnet` are distinct stores even in the periods when both
/// name one channel -- which is the shipped default, and which mainnet accounts and testnet notes
/// must never be pooled by. The selector is also stable under a pointer move: when `mainnet`
/// advances to a new channel its store is still `var/mainnet`, so there is nothing to carry and no
/// window in which a pointer and a store disagree. A pinned `0.15.0` keys on the version, and that
/// too is the identity the user chose.
///
/// Nothing may move or delete this: not install, not update, not republication, not a pointer move.
/// The three exceptions are explicit and are the whole list -- `uninstall --purge`; channel
/// migration, where the channel a pinned user selected ceases to exist and their data has to follow
/// it; and the one-time conversion of a pre-network home
/// ([`crate::migrate_networks`]), which hands the default network the single store such a home kept
/// under its channel's version.
pub fn var_dir(home: &Path, selector: &UserChannel) -> PathBuf {
    home.join("var").join(selector.to_string())
}

/// Where an in-flight physical operation records its intent.
///
/// Holds at most one entry, and only while an operation is running: its presence at startup means
/// a previous one was interrupted.
pub fn journal_dir(home: &Path) -> PathBuf {
    home.join("journal")
}

/// One immutable publication.
///
/// The channel is in the name for human legibility only; the publication id is what makes it
/// unique. Nothing may parse identity back out of this path.
pub fn publication_dir(home: &Path, channel: &semver::Version, id: &PublicationId) -> PathBuf {
    publications_dir(home).join(format!("{channel}-{id}"))
}
