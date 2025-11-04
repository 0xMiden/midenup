use std::{fmt::Display, str::FromStr};

use thiserror::Error;

struct Artifacts {
    artifacts: Vec<Artifact>,
}

/// Represents a mapping from a given [target] to the [url] which contains it.
struct Artifact {
    target: TargetTriple,

    uri: String,
}

/// Struct that represents a target architecture by the rust compiler.
/// There is no universal standadarized way to represent them, however,
/// according to the
/// [LLVM documentation](https://llvm.org/doxygen/Triple_8h_source.html),
/// most triples have one of the following two shapes:
/// - "ARCHITECTURE-VENDOR-OPERATING_SYSTEM"
/// - "ARCHITECTURE-VENDOR-OPERATING_SYSTEM-ENVIRONMENT"
///
/// This template does match with two major wellknown targets:
/// aarch64-apple-darwin and x86_64-unknown-linux-gnu.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct TargetTriple {
    architecture: String,
    vendor: String,
    operating_system: String,
    environment: Option<String>,
}

#[derive(Error, Debug)]
pub enum TargetTripleError {
    #[error("Failed to deserialize TargetTriplet because: {0}")]
    UnrecognizedTargetTriple(String),
}

impl FromStr for TargetTriple {
    type Err = TargetTripleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split("-");
        let architecture = parts
            .next()
            .ok_or(TargetTripleError::UnrecognizedTargetTriple(
                "Missing 'architecture' field".into(),
            ))?
            .into();
        let vendor = parts
            .next()
            .ok_or(TargetTripleError::UnrecognizedTargetTriple("Missing 'vendor' field".into()))?
            .into();
        let operating_system = parts
            .next()
            .ok_or(TargetTripleError::UnrecognizedTargetTriple(
                "Missing 'operating_system' field".into(),
            ))?
            .into();
        let environment = parts.next().map(String::from);
        Ok(TargetTriple {
            architecture,
            vendor,
            operating_system,
            environment,
        })
    }
}

impl Display for TargetTriple {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let repr = format!(
            "{}-{}-{}{}",
            self.architecture,
            self.vendor,
            self.operating_system,
            self.environment.as_ref().map(|env| format!("-{}", env)).unwrap_or_default()
        );
        write!(f, "{repr}")
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::TargetTriple;

    #[test]
    /// Test that we can parse triples that we actually support.
    fn parse_wellknown_targets() {
        let mut failed_parsing = Vec::new();
        let well_known_targets = ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu"];
        for target in well_known_targets {
            if let Err(err) = TargetTriple::from_str(target) {
                failed_parsing.push((target, err));
            }
        }
        if failed_parsing.is_empty() {
            return;
        }
        let err_message = failed_parsing.iter().fold(
            String::from("Failed to serialize the following well known targets:"),
            |acc, (target, err)| format!("{acc}\n   - {target}, because {}", err),
        );
        panic!("{}", err_message)
    }
}
