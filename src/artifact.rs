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
        target: &TargetTriple,
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
pub struct TargetTriple(String);

impl Artifact {
    /// Returns the triplet that it is pointing towards.  It should be rare for
    /// this function to error since all the miden artifacts should follow the
    /// same standardized format. This format is secribed in [[Artifact]].
    ///
    /// NOTE: The component name is only required to separate the triplet from the
    /// filename in the URI.
    fn inspect_triplet(&self, component_name: &str) -> Result<TargetTriple, TargetTripleError> {
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

        Ok(TargetTriple(triplet.to_string()))
    }
}

#[derive(Error, Debug, Clone)]
pub enum TargetTripleError {
    #[error("Triplet not found in: {0}")]
    TripletNotFound(String),
    #[error("URI not found in: {0}")]
    UnrecognizedUri(String),
    #[error("Failed to deserialize TargetTriplet because: {0}")]
    UnrecognizedTargetTriple(String),
}
