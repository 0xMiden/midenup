use serde::{Deserialize, Serialize};

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
    /// - 'https://<link>/<component name>-<triplet>'
    /// - 'file://<path>/<component name>-<triplet>'
    uri: String,
}

#[derive(Debug, PartialEq)]
pub enum TargetTriple {
    Custom(String),
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

        // <component name>-<triplet>
        let component_dash_triplet = if let Some(component_dash_triplet) = path.split("/").last() {
            component_dash_triplet
        } else {
            return false;
        };

        let triplet = if let Some(triplet) = component_dash_triplet
            .strip_prefix(component_name)
            .and_then(|dash_triplet| dash_triplet.strip_prefix("-"))
        {
            triplet
        } else {
            return false;
        };

        match target {
            TargetTriple::Custom(target_triplet) => triplet == target_triplet,
            TargetTriple::MidenVM => true,
        }
    }
}
