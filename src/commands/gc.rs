use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::Context;
use colored::Colorize;

use crate::{config::Config, paths, publish::journal, state::LocalState, version::Authority};

/// Reclaims publications and path-build caches nothing refers to any more.
///
/// Replaced publications remain intact because a process may still execute out of them. Cargo
/// build outputs remain reusable while any installed component uses their source. Explicit GC
/// reclaims both, under the same home lock as installation, preserving state and journal
/// references.
pub fn gc(config: &Config, state: &LocalState) -> anyhow::Result<()> {
    let publications = crate::publish::unreferenced(&config.midenup_home, state)?;
    let caches = unreferenced_cargo_caches(&config.midenup_home, state)?;

    if publications.is_empty() && caches.is_empty() {
        crate::info!("nothing to reclaim");
        return Ok(());
    }

    for (orphans, singular, plural) in [
        (publications, "publication", "publications"),
        (caches, "Cargo build cache", "Cargo build caches"),
    ] {
        if orphans.is_empty() {
            continue;
        }
        for orphan in &orphans {
            crate::info!("removing {}", orphan.display());
            std::fs::remove_dir_all(orphan)
                .with_context(|| format!("failed to remove '{}'", orphan.display()))?;
        }
        crate::info!(
            "reclaimed {} {}",
            orphans.len().to_string().bold(),
            if orphans.len() == 1 { singular } else { plural }
        );
    }

    Ok(())
}

/// Live installations and the next installation in the journal share ownership of source caches.
/// Never walk source trees or follow symlinks within the cache namespace.
fn unreferenced_cargo_caches(home: &Path, state: &LocalState) -> anyhow::Result<Vec<PathBuf>> {
    let root = paths::cargo_build_cache(home);
    let pending = journal::read(home)?;
    let referenced: HashSet<PathBuf> = state
        .installations
        .iter()
        .chain(pending.iter().filter_map(|entry| entry.target_installation.as_ref()))
        .flat_map(|installation| &installation.components)
        .filter_map(|component| match &component.version {
            Authority::Path { path, .. } => {
                // Older records may contain a noncanonical absolute path. Retain the recorded
                // key if a source has temporarily disappeared rather than failing collection.
                let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                Some(paths::cargo_source_cache(&root, &canonical))
            },
            _ => None,
        })
        .collect();

    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read '{}'", root.display()));
        },
    };
    let mut orphans = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("failed to read '{}'", root.display()))?;
        let path = entry.path();
        if entry
            .file_type()
            .with_context(|| format!("failed to inspect '{}'", path.display()))?
            .is_dir()
            && !referenced.contains(&path)
        {
            orphans.push(path);
        }
    }
    orphans.sort();
    Ok(orphans)
}
