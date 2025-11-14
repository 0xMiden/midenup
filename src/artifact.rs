use std::{fmt::Display, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Artifacts {
    artifacts: Vec<Artifact>,
}

impl Artifacts {
    /// Get a URI to download an artifact that's valid for [target].
    pub fn get_uri_for(
        &self,
        target: &TargetTriple2,
        component_name: &str,
    ) -> Result<String, Vec<TargetTripleError>> {
        let mut errors = Vec::new();
        let uri = self
            .artifacts
            .iter()
            .find(|artifact| {
                artifact
                    .inspect_triplet(component_name)
                    .inspect_err(|err| errors.push(err.clone()))
                    .is_ok_and(|triplet| &triplet == target)
            })
            .map(|arti| arti.uri.clone());

        if let Some(uri) = uri {
            Ok(uri)
        } else {
            Err(errors)
        }
    }
}

/// Represents a mapping from a given [target] to the [url] which contains it.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct Artifact {
    /// URI of the form:
    /// - 'https://<link>/<component name>-<triplet>'
    /// - 'file://<path>/<component name>-<triplet>'
    uri: String,
}

#[derive(Debug, PartialEq)]
pub struct TargetTriple2(String);

impl Artifact {
    /// Returns the triplet that it is pointing towards.  It should be rare for
    /// this function to error since all the miden artifacts should follow the
    /// same standardized format. This format is secribed in [[Artifact]].
    ///
    /// NOTE: The component name is only required to separate the triplet from the
    /// filename in the URI.
    fn inspect_triplet(&self, component_name: &str) -> Result<TargetTriple2, TargetTripleError> {
        let path = if let Some(file_path) = self.uri.strip_prefix("file://") {
            Ok(file_path)
        } else if let Some(url_path) = self.uri.strip_prefix("https://") {
            Ok(url_path)
        } else {
            Err(TargetTripleError::UnrecognizedUri(self.uri.clone()))
        }?;

        // <component name>-<triplet>
        let component_dash_triplet = path
            .split("/")
            .last()
            .ok_or(TargetTripleError::TripletNotFound(self.uri.clone()))?;

        let triplet = component_dash_triplet
            .strip_prefix(component_name)
            .and_then(|dash_triplet| dash_triplet.strip_prefix("-"))
            .ok_or(TargetTripleError::UnrecognizedTargetTriple(self.uri.clone()))?;

        Ok(TargetTriple2(triplet.to_string()))
    }
}

// /// Struct that represents a target architecture by the rust compiler.
// /// There is no universal standadarized way to represent them, however,
// /// according to the
// /// [LLVM documentation](https://llvm.org/doxygen/Triple_8h_source.html),
// /// most triples have one of the following two shapes:
// /// - "ARCHITECTURE-VENDOR-OPERATING_SYSTEM"
// /// - "ARCHITECTURE-VENDOR-OPERATING_SYSTEM-ENVIRONMENT"
// ///
// /// This template does match with two major wellknown targets:
// /// aarch64-apple-darwin and x86_64-unknown-linux-gnu.
// ///
// /// There is one *notable* special case which is the Miden VM. MASP Libraries
// /// are OS/environent-agnostic, since they target the Miden VM itself. So, they
// /// use the following triplet: zkvm-miden-unknown
// #[derive(Debug, PartialEq, Eq, Clone)]
// pub struct TargetTriple {
//     architecture: String,
//     vendor: String,
//     operating_system: String,
//     environment: Option<String>,
// }

// impl serde::ser::Serialize for TargetTriple {
//     fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
//     where
//         S: serde::ser::Serializer,
//     {
//         serializer.serialize_str(&self.to_string())
//     }
// }

// impl<'de> serde::de::Deserialize<'de> for TargetTriple {
//     fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
//     where
//         D: serde::de::Deserializer<'de>,
//     {
//         let s = String::deserialize(deserializer)?;
//         s.parse::<Self>().map_err(serde::de::Error::custom)
//     }
// }

#[derive(Error, Debug, Clone)]
pub enum TargetTripleError {
    #[error("Triplet not found in: {0}")]
    TripletNotFound(String),
    #[error("URI not found in: {0}")]
    UnrecognizedUri(String),
    #[error("Failed to deserialize TargetTriplet because: {0}")]
    UnrecognizedTargetTriple(String),
}
