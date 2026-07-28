//! Executing an [crate::plan::InstallationPlan].
//!
//! Everything here is deliberately decision-free: the plan says what to do, and these modules do
//! exactly that. No filtering, no name resolution, no manifest access.

pub mod cargo;
pub mod download;

pub use self::{
    cargo::{CargoError, argv_for, build as cargo_build},
    download::{ExecError, acquire},
};
