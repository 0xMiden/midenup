//! What `midenup` has installed, as recorded on this machine.

use serde::{Deserialize, Serialize};

use crate::{manifest::Component, plan::PlanKey, resolve::Intent};

/// An opaque identifier for one immutable published installation.
///
/// Deliberately **not** derived from content. Naming a publication after a digest of its inputs
/// invites treating equal names as equal bytes, which nothing here verifies; an opaque id makes
/// that mistake impossible to express.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PublicationId(String);

impl PublicationId {
    /// Generates a fresh identifier.
    ///
    /// Uniqueness only has to hold within one `MIDENUP_HOME`, against operations already
    /// serialized by the advisory lock. A process-local counter makes collisions within a process
    /// impossible rather than merely unlikely -- a clock read alone is not enough, since two calls
    /// can land in the same nanosecond -- and time plus pid separates processes.
    pub fn generate() -> Self {
        use std::{
            hash::{DefaultHasher, Hash, Hasher},
            sync::atomic::{AtomicU64, Ordering},
            time::{SystemTime, UNIX_EPOCH},
        };

        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let mut hasher = DefaultHasher::new();
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            .hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        COUNTER.fetch_add(1, Ordering::Relaxed).hash(&mut hasher);
        Self(format!("{:016x}", hasher.finish()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PublicationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How an installation's physical files relate to what this build manages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PublicationRef {
    /// A publication this build created and owns, described by a receipt.
    Managed {
        id: PublicationId,
        plan_key: PlanKey,
        target: String,
    },
    /// Carried forward from a v1 manifest, with no publication behind it yet.
    ///
    /// The pre-v2 layout is not described by any receipt, so `midenup` cannot know what it owns.
    /// Such a record is never executed against; the next operation touching the channel reinstalls
    /// it. Produced only by migration.
    NeedsReinstall,
}

/// How a component's files were actually acquired.
///
/// Recorded because `prebuilt-with-cargo-fallback` can go either way, and uninstall has to match
/// the path that was really taken rather than the one that was declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RealizedMethod {
    Prebuilt,
    Cargo,
    Extracted,
    /// The component installs no files of its own.
    None,
}

/// One file a publication owns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Output {
    pub path: std::path::PathBuf,
    pub owner: String,
    pub mode: u32,
    pub realized: RealizedMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<crate::artifact::Digest>,
}

/// The immutable record of what a publication contains.
///
/// Written once, inside the publication directory, and thereafter the authority on which files
/// that publication owns. Uninstall and update-seeding both consult it rather than guessing from
/// the manifest, which is how install and uninstall paths drift apart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub publication_id: PublicationId,
    pub plan_key: PlanKey,
    pub target: String,
    pub channel: semver::Version,
    pub outputs: Vec<Output>,
}

/// One installed channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Installation {
    pub channel: semver::Version,
    /// What the user asked for. Re-resolved against upstream on every update.
    pub intent: Intent,
    /// The resolved component set, snapshotted so `miden` can dispatch without the network.
    pub components: Vec<Component>,
    pub publication: PublicationRef,
    /// UTC epoch seconds.
    pub installed_at: i64,
}

impl Installation {
    /// Whether this record describes files that this build manages.
    pub fn is_managed(&self) -> bool {
        matches!(self.publication, PublicationRef::Managed { .. })
    }
}
