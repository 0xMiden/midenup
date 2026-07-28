//! Turning a resolved component set into an exact, target-specific installation plan.

pub mod destination;

pub use self::destination::{
    Destination, DestinationError, InvalidArtifactId, MODE_DATA, MODE_EXECUTABLE, destination_for,
    validate_artifact_id,
};
