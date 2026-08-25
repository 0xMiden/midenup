use anyhow::Context;
use colored::Colorize;

use crate::{config::Config, state::LocalState};

/// Reclaims publications nothing refers to any more.
///
/// Every change to an installed channel produces a new publication and leaves the old one on disk,
/// unreferenced but intact -- a process that was already running may still be executing out of it
/// (spec section 3.1). Nothing else ever reclaims those, so this is not housekeeping: without it,
/// every update costs another copy of a toolchain.
///
/// Idempotent, and never removes a publication that is referenced by `state.json` or named by an
/// in-flight operation.
pub fn gc(config: &Config, state: &LocalState) -> anyhow::Result<()> {
    let orphans = crate::publish::unreferenced(&config.midenup_home, state)?;

    if orphans.is_empty() {
        println!("nothing to reclaim");
        return Ok(());
    }

    for orphan in &orphans {
        crate::info!("removing {}", orphan.display());
        std::fs::remove_dir_all(orphan)
            .with_context(|| format!("failed to remove '{}'", orphan.display()))?;
    }

    println!(
        "reclaimed {} {}",
        orphans.len().to_string().bold(),
        if orphans.len() == 1 {
            "publication"
        } else {
            "publications"
        }
    );

    Ok(())
}
