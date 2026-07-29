//! Turning a resolved component set into an exact, target-specific installation plan.

pub mod authority;
pub mod build;
pub mod destination;
pub mod key;

pub use self::{
    authority::{PinError, ResolvedAuthority, pin, recheck_path},
    build::{
        InstallationPlan, PlanError, PlanStep, SymlinkSpec, build as build_plan, component_key,
    },
    destination::{
        Destination, DestinationError, InvalidArtifactId, MODE_DATA, MODE_EXECUTABLE,
        destination_for, validate_artifact_id,
    },
    key::{ComponentInputs, KeyInputs, PlanKey, compute as compute_plan_key},
};
