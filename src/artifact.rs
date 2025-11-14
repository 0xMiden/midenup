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
            .find(|artifact| artifact.contains(target, component_name))
            .map(|arti| arti.uri.clone())
    }
}

/// Represents a mapping from a given [target] to the [url] which contains it.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct Artifact {
    /// URI of the form:
    /// - 'https://<link>/<component name>(-<triplet>|.masp)'
    /// - 'file://<path>/<component name>(-<triplet>|.masp)'
    uri: String,
}

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
    /// Returns the triplet that it is pointing towards.  It should be rare for
    /// this function to error since all the miden artifacts should follow the
    /// same standardized format. This format is secribed in [[Artifact]].
    ///
    /// NOTE: The component name is only required to separate the triplet from the
    /// filename in the URI.
    fn contains(&self, target: &TargetTriple, component_name: &str) -> bool {
        let path = if let Some(file_path) = self.uri.strip_prefix("file://") {
            file_path
        } else if let Some(url_path) = self.uri.strip_prefix("https://") {
            url_path
        } else {
            return false;
        };

        // <component name>(-<triplet>|.masp)
        let suffix = if let Some(suffix) =
            path.split("/").last().and_then(|suffix| suffix.strip_prefix(component_name))
        {
            suffix
        } else {
            return false;
        };

        match suffix {
            ".masp" => {
                matches!(target, &TargetTriple::MidenVM)
            },
            dash_triplet if suffix.starts_with("-") => {
                // Safety: This is safe since this only executed if the dash
                // starts with "-".
                let triplet = {
                    let triplet = dash_triplet.strip_prefix("-").unwrap();
                    TargetTriple::Custom(String::from(triplet))
                };

                *target == triplet
            },
            _ => false,
        }
    }
}
