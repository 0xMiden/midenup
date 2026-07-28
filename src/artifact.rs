use std::{
    borrow::Cow,
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    manifest::{Component, ComponentKind},
    version::Authority,
};

#[derive(Debug, thiserror::Error)]
pub enum InvalidArtifactError {
    #[error("invalid artifact: no uri scheme for artifact {id} of {component}")]
    MissingScheme { id: String, component: String },
    #[error(
        "invalid artifact: invalid scheme for artifact {id} of {component}: expected file://, http://, or https://, got '{scheme}'"
    )]
    InvalidScheme {
        id: String,
        component: String,
        scheme: String,
    },
    #[error(
        "invalid artifact: %version substitution is invalid for artifact {id} of {component}: \
         component has no known semantic version"
    )]
    VersionUnavailable { id: String, component: String },
    #[error("invalid artifact: {substitution} is not defined for artifact {id} of {component}")]
    UndefinedSubstitution {
        id: String,
        component: String,
        substitution: &'static str,
    },
    #[error(
        "invalid artifact: uri is missing %target substitution for artifact {id} of {component} \
         when target is '{target}'"
    )]
    MissingTarget {
        id: String,
        component: String,
        target: String,
    },
}

/// All the artifacts that the [Component] contains.
#[derive(Serialize, Deserialize, Default, Debug, Clone, Hash)]
#[serde(transparent)]
pub struct Artifacts {
    pub(crate) artifacts: BTreeMap<String, Artifact>,
}

impl Artifacts {
    pub fn is_empty(&self) -> bool {
        self.artifacts.is_empty()
    }

    pub fn insert(&mut self, id: String, artifact: Artifact) -> bool {
        self.artifacts.insert(id, artifact).is_none()
    }

    /// Returns the `(artifact id, uri)` pairs that should be installed for `target`.
    ///
    /// The artifact id is the exact filename the artifact is installed as, so callers must use it
    /// to compute a destination path rather than inferring one from the URI.
    pub fn get_default_artifacts_for_target<'a>(
        &'a self,
        target: &str,
        component: &'a Component,
    ) -> Result<Vec<(&'a str, ArtifactUri)>, InvalidArtifactError> {
        match component.kind() {
            ComponentKind::Asset { .. } => self.get_artifacts_for_target(target, component),
            ComponentKind::CargoExtension { spec, .. } | ComponentKind::Executable { spec, .. } => {
                let id = spec.installed_executable.as_str();
                let artifact = self.get_artifact_for_target(id, target, component)?;
                Ok(artifact.into_iter().map(|uri| (id, uri)).collect())
            },
            // Never selected for installation, so it has no artifacts to resolve. The resolver
            // is the gate that rejects it; this arm just keeps the match total.
            ComponentKind::Command { .. } | ComponentKind::Unsupported { .. } => Ok(vec![]),
            ComponentKind::LegacyPackage { .. } | ComponentKind::Package => {
                self.get_artifacts_for_target(target, component)
            },
        }
    }

    /// Returns every declared `(artifact id, uri)` pair available for `target`.
    pub fn get_artifacts_for_target(
        &self,
        target: &str,
        component: &Component,
    ) -> Result<Vec<(&str, ArtifactUri)>, InvalidArtifactError> {
        let mut artifacts = Vec::with_capacity(self.artifacts.len());
        for (id, artifact) in self.artifacts.iter() {
            if let Some(uri) = artifact.get_uri_for(id, target, component)? {
                artifacts.push((id.as_str(), uri));
            }
        }

        Ok(artifacts)
    }

    pub fn get_artifact_for_target(
        &self,
        id: &str,
        target: &str,
        component: &Component,
    ) -> Result<Option<ArtifactUri>, InvalidArtifactError> {
        let Some(artifact) = self.artifacts.get(id) else {
            return Ok(None);
        };
        artifact.get_uri_for(id, target, component)
    }
}

/// Holds a URI used to fetch an artifact.
///
/// These URIs have the following format:
///
/// * `(https://|file://)<path>/<component name>(-<triplet>)?(<extension>)`
#[derive(Serialize, Deserialize, Debug, Clone, Hash)]
#[serde(untagged, rename_all = "kebab-case")]
pub enum Artifact {
    /// An artifact compiled for a specific target
    TargetSpecific {
        /// The URI from which to fetch this artifact, containing substitution patterns for the
        /// variable/target-sensitive portions of the artifact file.
        ///
        /// The basic URI format should use one of two schemes: `file://` or `http(s)://`, e.g.:
        ///
        /// ```
        /// file://path/to/artifacts/%basename-%target.%extension
        /// ```
        ///
        /// ```
        /// https://github.com/org/repo/releases/%version/download/%basename-%target.%extension
        /// ```
        ///
        /// The following substitutions and their values are available:
        ///
        /// * `%version` - the version of the containing component. This requires that the
        ///   component has an associated semantic version declared in the manifest, or an error
        ///   will be produced
        /// * `%basename` - the value of `basename` if present, defaults to the component name
        /// * `%extension` - the value of `extension`, if present
        /// * `%target` - the current target triple
        ///
        /// The only substitution required to be present is `%target` - the other substituions are
        /// optional, and can be used at your convenience.
        uri: String,
        /// Substitutions that apply to all targets
        #[serde(flatten, skip_serializing_if = "Option::is_none")]
        substitutions: Option<Substitutions>,
        /// The supported target triples for this artifact, and their target-specific substitutions
        targets: BTreeMap<String, Substitutions>,
    },
    /// A non-executable/target-agnostic asset
    TargetAgnostic {
        /// The URI for this artifact, including the filename component
        ///
        /// The URI may contain the `%version` substitution string, which will be replaced with
        /// the version of the containing component. Note that `%version` requires that the
        /// component have an associated semantic version, or an error will be produced.
        uri: String,
    },
}

/// The value of user-defined substitutions in [Artifact] definitions
#[derive(Serialize, Deserialize, Default, Debug, Clone, Hash)]
pub struct Substitutions {
    /// The basename to use for the artifact, e.g. `foo` in `foo-aarch64-apple-darwin.ext`
    ///
    /// If not provided, the component name is used as the basename.
    ///
    /// The basename is only relevant if actually used via `%basename`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub basename: Option<String>,
    /// The extension to use for this artifact, e.g. `masp`
    ///
    /// The default extension is determined by the component type
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
    /// Fields declared by a newer schema that this build does not recognize.
    #[serde(flatten)]
    pub extra: crate::manifest::v2::unknown::Extra,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactUri {
    File(PathBuf),
    Http(String),
}

impl ArtifactUri {
    pub fn file_name(&self) -> Option<&Path> {
        match self {
            Self::File(path) => path.file_name().map(Path::new),
            Self::Http(uri) => {
                let (_, last) = uri.rsplit_once('/')?;
                Some(Path::new(last.trim()))
            },
        }
    }
}

impl fmt::Display for ArtifactUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(path) => write!(f, "file://{}", path.display()),
            Self::Http(uri) => f.write_str(uri),
        }
    }
}

impl Artifact {
    pub fn get_uris_for(
        &self,
        id: &str,
        component: &Component,
    ) -> Result<Vec<ArtifactUri>, InvalidArtifactError> {
        let mut artifacts = vec![];
        match self {
            Self::TargetAgnostic { .. } => {
                if let Some(artifact) = self.get_uri_for(id, "", component)? {
                    artifacts.push(artifact);
                }
            },
            Self::TargetSpecific { targets, .. } => {
                for target in targets.keys() {
                    if let Some(artifact) = self.get_uri_for(id, target, component)? {
                        artifacts.push(artifact);
                    }
                }
            },
        }

        Ok(artifacts)
    }

    /// Returns the URI for the specified target, with the provided component details for use in
    /// substitution patterns:
    pub fn get_uri_for(
        &self,
        id: &str,
        target: &str,
        component: &Component,
    ) -> Result<Option<ArtifactUri>, InvalidArtifactError> {
        match self {
            Self::TargetAgnostic { uri } => {
                let Some((scheme, rest)) = uri.split_once("://") else {
                    return Err(InvalidArtifactError::MissingScheme {
                        id: id.to_string(),
                        component: component.name.to_string(),
                    });
                };
                let rest = if rest.contains("%version") {
                    let Authority::Registry { version } = &component.version else {
                        return Err(InvalidArtifactError::VersionUnavailable {
                            id: id.to_string(),
                            component: component.name.to_string(),
                        });
                    };
                    Cow::Owned(rest.replace("%version", &version.to_string()))
                } else {
                    Cow::Borrowed(rest)
                };
                match scheme {
                    "file" => Ok(Some(ArtifactUri::File(PathBuf::from(rest.into_owned())))),
                    "http" | "https" => Ok(Some(ArtifactUri::Http(format!("{scheme}://{rest}")))),
                    scheme => Err(InvalidArtifactError::InvalidScheme {
                        id: id.to_string(),
                        component: component.name.to_string(),
                        scheme: scheme.to_string(),
                    }),
                }
            },
            Self::TargetSpecific { uri, substitutions, targets } => {
                let Some(target_subs) = targets.get(target) else {
                    return Ok(None);
                };
                let basename = target_subs
                    .basename
                    .as_deref()
                    .or(substitutions.as_ref().and_then(|subs| subs.basename.as_deref()))
                    .unwrap_or(component.name.as_ref());
                let extension = target_subs
                    .extension
                    .as_deref()
                    .or(substitutions.as_ref().and_then(|subs| subs.extension.as_deref()));
                let Some((scheme, rest)) = uri.split_once("://") else {
                    return Err(InvalidArtifactError::MissingScheme {
                        id: id.to_string(),
                        component: component.name.to_string(),
                    });
                };
                if !rest.contains("%target") {
                    return Err(InvalidArtifactError::MissingTarget {
                        id: id.to_string(),
                        component: component.name.to_string(),
                        target: target.to_string(),
                    });
                }
                let rest = rest.replace("%target", target);
                let rest = if rest.contains("%version") {
                    let Authority::Registry { version } = &component.version else {
                        return Err(InvalidArtifactError::VersionUnavailable {
                            id: id.to_string(),
                            component: component.name.to_string(),
                        });
                    };
                    rest.replace("%version", &version.to_string())
                } else {
                    rest
                };
                let rest = if rest.contains("%extension") {
                    let extension =
                        extension.ok_or_else(|| InvalidArtifactError::UndefinedSubstitution {
                            id: id.to_string(),
                            component: component.name.to_string(),
                            substitution: "%extension",
                        })?;
                    rest.replace("%extension", extension)
                } else {
                    rest
                };
                let rest = rest.replace("%basename", basename);
                match scheme {
                    "file" => Ok(Some(ArtifactUri::File(PathBuf::from(rest)))),
                    "http" | "https" => Ok(Some(ArtifactUri::Http(format!("{scheme}://{rest}")))),
                    scheme => Err(InvalidArtifactError::InvalidScheme {
                        id: id.to_string(),
                        component: component.name.to_string(),
                        scheme: scheme.to_string(),
                    }),
                }
            },
        }
    }
}
