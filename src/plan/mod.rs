//! Turning a resolved component set into an exact, target-specific installation plan.

pub mod build;
pub mod destination;
pub mod key;

pub use self::{
    build::{InstallationPlan, PlanError, PlanStep, SymlinkSpec, build as build_plan},
    destination::{
        Destination, DestinationError, InvalidArtifactId, MODE_DATA, MODE_EXECUTABLE,
        destination_for, validate_artifact_id,
    },
    key::{ComponentInputs, KeyInputs, PlanKey, compute as compute_plan_key},
};
