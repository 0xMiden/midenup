//! Executing an [crate::plan::InstallationPlan].
//!
//! Everything here is deliberately decision-free: the plan says what to do, and these modules do
//! exactly that. No filtering, no name resolution, no manifest access.

pub mod archive;
pub mod cargo;
pub mod download;
pub mod extract;
pub mod stage;

pub use self::{
    archive::ArchiveError,
    cargo::{CargoError, argv_for, build as cargo_build},
    download::{ExecError, acquire},
    extract::{ExtractError, extract, render_script},
    stage::{Realized, Seed, StageError, execute, prepare, seed, verify},
};
