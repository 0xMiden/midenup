use serde::{Deserialize, Serialize};

/// All the artifacts that the [[Component]] contains.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Artifacts {
    artifacts: Vec<Artifact>,
}

impl Artifacts {
    /// Get a URI to download an artifact that's valid for [target].
    pub fn get_uri_for(&self, target: &TargetTriple) -> Option<String> {
        self.artifacts.iter().find_map(|artifact| artifact.get_uri_for(target))
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

impl TargetTriple {
    fn get_uri_extension(&self) -> String {
        match &self {
            Self::MidenVM => String::from(".masp"),
            Self::Custom(triplet) => triplet.to_string(),
        }
    }
}

impl Artifact {
    /// Returns the URI for the specified component + triplet if it has it.
    ///
    /// NOTE: The component name is required to separate the triplet from the
    /// filename in the URI.
    fn get_uri_for(&self, target: &TargetTriple) -> Option<String> {
        let path = if let Some(file_path) = self.0.strip_prefix("file://") {
            file_path
        } else if let Some(url_path) = self.0.strip_prefix("https://") {
            url_path
        } else {
            return None;
        };

        // <component name>(-<triplet>|.masp)
        let uri_extension = path.split("/").last()?;

        let wanted_uri_extension = target.get_uri_extension();

        if uri_extension.contains(&wanted_uri_extension) {
            Some(self.0.clone())
        } else {
            None
        }
    }
}
