use serde::{Deserialize, Serialize};

/// All the artifacts that the [[Component]] contains.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Artifacts {
    artifacts: Vec<Artifact>,
}

impl Artifacts {
    /// Get a URI to download an artifact that's valid for [target].
    pub fn get_uri_for(&self, target: &TargetTriple, component_name: &str) -> Option<String> {
        self.artifacts
            .iter()
            .find_map(|artifact| artifact.get_uri_for(target, component_name))
    }
}

/// Holds a URI used to fetch an artifact. These URIs have the following format:
/// (https://|file://)<path>/<component name>(-<triplet>|.masp)
#[derive(Serialize, Deserialize, Debug, Clone)]
struct Artifact(String);

#[derive(Debug, PartialEq)]
pub enum TargetTriple {
    /// Custom triplet used by cargo. Since we use the same triplets as cargo, we
    /// simply copy them as-is, without any type of parsing.
    Custom(String),
    /// Used for .masp Libraries that are used in the MidenVM. Components that
    /// have these libraries as artifacts only have one entry in
    /// [[Artifacts::artifacts]].
    MidenVM,
}

impl Artifact {
    /// Returns the URI for the specified component + triplet if it has it.
    ///
    /// NOTE: The component name is required to separate the triplet from the
    /// filename in the URI.
    fn get_uri_for(&self, target: &TargetTriple, component_name: &str) -> Option<String> {
        let path = if let Some(file_path) = self.0.strip_prefix("file://") {
            file_path
        } else if let Some(url_path) = self.0.strip_prefix("https://") {
            url_path
        } else {
            return None;
        };

        // <component name>(-<triplet>|.masp)
        let suffix =
            path.split("/").last().and_then(|suffix| suffix.strip_prefix(component_name))?;

        let is_looked_for = match suffix {
            ".masp" => {
                matches!(target, &TargetTriple::MidenVM)
            },
            dash_triplet if suffix.starts_with("-") => {
                // Safety: This is safe since this only executed if dash_triplet
                // starts with "-".
                let triplet = {
                    let triplet = dash_triplet.strip_prefix("-").unwrap();
                    TargetTriple::Custom(String::from(triplet))
                };

                *target == triplet
            },
            _ => false,
        };

        if is_looked_for { Some(self.0.clone()) } else { None }
    }
}
