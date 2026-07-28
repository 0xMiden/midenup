//! Executing an [crate::plan::InstallationPlan].
//!
//! Everything here is deliberately decision-free: the plan says what to do, and these modules do
//! exactly that. No filtering, no name resolution, no manifest access.

pub mod download;

pub use self::download::{ExecError, acquire};
