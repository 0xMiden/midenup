//! The `$MIDENUP_HOME` layout, in one place.
//!
//! Every path under `MIDENUP_HOME` is named by exactly one function here. Spelling a layout path
//! inline is how `install` and `uninstall` drifted apart: one wrote `lib/<artifact>` and the other
//! looked for `<artifact>`, and nothing connected the two.
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
//! |  |- stable      -> <channel>
//! |  |- default     -> <channel>
//! |- opt            -> toolchains/<active>/opt
//! ```

use std::path::{Path, PathBuf};

use crate::state::PublicationId;

/// Local installation state: the sole logical authority on what is installed.
pub fn state_path(home: &Path) -> PathBuf {
    home.join("state").with_extension("json")
}

/// The directory of `toolchains/<channel>` symlinks, plus the derived `stable` and `default`.
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

/// Where installed trees live.
pub fn publications_dir(home: &Path) -> PathBuf {
    home.join("publications")
}

/// Mutable, component-owned state for a channel: `%var`.
///
/// Outside the publication, and keyed by channel rather than by publication, because a publication
/// is replaced wholesale on every change. The Miden client's database lives here (`%var(data)`);
/// with `var/` inside the publication, every toolchain update destroyed it.
///
/// Install, update and republication never read, write, move or delete this. The only exception is
/// channel migration, which renames it so client data follows the toolchain.
pub fn var_dir(home: &Path, channel: &semver::Version) -> PathBuf {
    home.join("var").join(channel.to_string())
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
