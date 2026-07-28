//! Deliberate failure at a named point in the publication protocol.
//!
//! The protocol's whole claim is that an operation interrupted *anywhere* leaves exactly one
//! consistent outcome. That claim is only worth as much as the evidence for it, and the evidence
//! cannot be gathered by inspection: it needs the process to actually stop between two specific
//! filesystem operations, repeatedly, at every labelled point.
//!
//! So the labelled points are real code, compiled only under the `fault-injection` feature and
//! armed with `MIDENUP_FAULT_POINT`. Without the feature every call is a no-op that optimizes away;
//! a released binary contains no way to reach them.
//!
//! ```bash
//! MIDENUP_FAULT_POINT=post-commit midenup install 0.15.0   # stops after the commit point
//! midenup show                                             # recovery completes it
//! ```

use std::str::FromStr;

/// The environment variable that arms a fault.
pub const FAULT_POINT_ENV: &str = "MIDENUP_FAULT_POINT";

/// A labelled point in the publication protocol (spec section 9.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultPoint {
    /// After the journal entry is written, before anything is staged.
    PostPrepare,
    /// After the staging tree is built, before it is verified.
    PostStage,
    /// After verification and the receipt, before the commit point.
    PostVerify,
    /// Immediately after the commit point: the symlink is repointed, nothing else is done.
    PostCommit,
    /// After `state.json` is committed, before the derived symlinks are rebuilt.
    PostRecord,
    /// After the derived symlinks, before the old publication and journal are cleaned up.
    PostDerive,
}

impl FaultPoint {
    /// Every point, in protocol order.
    pub const ALL: [FaultPoint; 6] = [
        Self::PostPrepare,
        Self::PostStage,
        Self::PostVerify,
        Self::PostCommit,
        Self::PostRecord,
        Self::PostDerive,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PostPrepare => "post-prepare",
            Self::PostStage => "post-stage",
            Self::PostVerify => "post-verify",
            Self::PostCommit => "post-commit",
            Self::PostRecord => "post-record",
            Self::PostDerive => "post-derive",
        }
    }
}

impl std::fmt::Display for FaultPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FaultPoint {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        FaultPoint::ALL
            .into_iter()
            .find(|point| point.as_str() == value)
            .ok_or_else(|| format!("unknown fault point '{value}'"))
    }
}

#[derive(Debug, thiserror::Error)]
#[error("aborted at the injected fault point '{point}'")]
pub struct InjectedFault {
    pub point: FaultPoint,
}

/// Fails if `point` is the armed fault point.
#[cfg(feature = "fault-injection")]
pub fn fail_at(point: FaultPoint) -> Result<(), InjectedFault> {
    let armed = std::env::var(FAULT_POINT_ENV).ok();
    match armed.as_deref().map(FaultPoint::from_str) {
        Some(Ok(armed)) if armed == point => Err(InjectedFault { point }),
        _ => Ok(()),
    }
}

/// Without the feature there is no way to arm a fault, so this is a no-op.
#[cfg(not(feature = "fault-injection"))]
#[inline(always)]
pub fn fail_at(_point: FaultPoint) -> Result<(), InjectedFault> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_point_round_trips_through_its_name() {
        for point in FaultPoint::ALL {
            assert_eq!(point.as_str().parse::<FaultPoint>().unwrap(), point);
        }
        assert!("post-nothing".parse::<FaultPoint>().is_err());
    }

    /// Without the feature the hook must be inert, whatever the environment says.
    #[cfg(not(feature = "fault-injection"))]
    #[test]
    fn faults_cannot_be_armed_in_a_release_build() {
        for point in FaultPoint::ALL {
            assert!(fail_at(point).is_ok());
        }
    }
}
