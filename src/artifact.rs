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
    #[error(
        "invalid artifact: artifact {id} of {component} is packaged as '{format}', which this \
         version of midenup cannot read"
    )]
    UnsupportedArchiveFormat {
        id: String,
        component: String,
        format: String,
    },
}

/// How an artifact is packaged at its URI.
///
/// The archive must hold exactly one file, which is the artifact. An archive holding several is an
/// error. An unsupported format would still parse; it is rejected when an installation is planned
/// for it.
#[derive(Debug, Clone, Hash, PartialEq)]
pub struct Archive {
    pub format: ArchiveFormat,
    /// Fields declared by a newer schema that this build does not recognize.
    pub extra: crate::manifest::v3::unknown::Extra,
}

/// The typed shape of an archive, without the unknown-field capture.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
struct ArchiveFields {
    format: ArchiveFormat,
}

/// Serialized as a bare format string when that is all it is, which is every archive this build
/// declares itself.
impl Serialize for Archive {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;

        if self.extra.is_empty() {
            return self.format.serialize(serializer);
        }
        crate::manifest::v3::unknown::merge_extra(
            &ArchiveFields { format: self.format.clone() },
            &self.extra,
        )
        .map_err(S::Error::custom)?
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Archive {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;

        let value = serde_json::Value::deserialize(deserializer)?;

        // The shape every archive needs today; the object form exists so a newer schema can add
        // fields beside the format without this build losing them.
        if let serde_json::Value::String(format) = value {
            return Ok(Self {
                format: ArchiveFormat::parse(&format),
                extra: Default::default(),
            });
        }

        let (fields, extra) = crate::manifest::v3::unknown::split_extra::<ArchiveFields>(value)
            .map_err(D::Error::custom)?;
        Ok(Self { format: fields.format, extra })
    }
}

/// A format this build has a reader for.
///
/// Constructible only by [ArchiveFormat::supported], so an unsupported format cannot be expressed
/// past resolution: the plan and the executor take this type, and therefore have no unsupported
/// case to handle.
///
/// Adding a format is a variant here, an entry in [SupportedFormat::ALL], an arm in
/// [SupportedFormat::as_str], and a reader in [crate::install::archive].
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum SupportedFormat {
    /// A gzip-compressed tar archive.
    TarGz,
}

impl SupportedFormat {
    /// Every format this build can read. The sole table: parsing, validation and the round-trip
    /// test all derive from it.
    pub const ALL: &'static [Self] = &[Self::TarGz];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
        }
    }
}

impl fmt::Display for SupportedFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The format an artifact declares itself packaged in, supported by this build or not.
///
/// This is the wire type: it is what a manifest can say, which is not the same as what this build
/// can do. [ArchiveFormat::supported] is the one way across that gap.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum ArchiveFormat {
    /// A format this build can read.
    Supported(SupportedFormat),
    /// A format this build cannot read. Recorded verbatim so it round-trips; rejected before an
    /// installation is planned for it.
    Unsupported(String),
}

impl ArchiveFormat {
    /// Interprets a declared spelling. An unrecognized one is a value, not a failure: see
    /// [Archive].
    pub fn parse(spelling: &str) -> Self {
        match SupportedFormat::ALL.iter().find(|format| format.as_str() == spelling) {
            Some(format) => Self::Supported(*format),
            None => Self::Unsupported(spelling.to_string()),
        }
    }

    /// The reader for this format, if this build has one.
    pub fn supported(&self) -> Option<SupportedFormat> {
        match self {
            Self::Supported(format) => Some(*format),
            Self::Unsupported(_) => None,
        }
    }

    pub fn is_supported(&self) -> bool {
        self.supported().is_some()
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Supported(format) => format.as_str(),
            Self::Unsupported(spelling) => spelling,
        }
    }

    /// Every spelling an author may write.
    pub fn supported_spellings() -> impl Iterator<Item = &'static str> {
        SupportedFormat::ALL.iter().map(SupportedFormat::as_str)
    }
}

impl fmt::Display for ArchiveFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ArchiveFormat {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ArchiveFormat {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::parse(&String::deserialize(deserializer)?))
    }
}

/// An artifact resolved for one target: where to fetch it, and what it is packaged as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedArtifact {
    pub uri: ArtifactUri,
    /// Present when the fetched bytes are an archive rather than the file itself.
    pub archive: Option<SupportedFormat>,
}

/// All the artifacts that the [Component] contains.
#[derive(Serialize, Deserialize, Default, Debug, Clone, Hash, PartialEq)]
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

    /// Returns the `(artifact id, resolved artifact)` pairs that should be installed for `target`.
    ///
    /// The artifact id is the exact filename the artifact is installed as, so callers must use it
    /// to compute a destination path rather than inferring one from the URI.
    pub fn get_default_artifacts_for_target<'a>(
        &'a self,
        target: &str,
        component: &'a Component,
    ) -> Result<Vec<(&'a str, ResolvedArtifact)>, InvalidArtifactError> {
        match component.kind() {
            ComponentKind::Asset => self.get_artifacts_for_target(target, component),
            ComponentKind::CargoExtension { spec, .. } | ComponentKind::Executable { spec, .. } => {
                let id = spec.installed_executable.as_str();
                let artifact = self.get_artifact_for_target(id, target, component)?;
                Ok(artifact.into_iter().map(|resolved| (id, resolved)).collect())
            },
            // Never selected for installation, so it has no artifacts to resolve. The resolver
            // is the gate that rejects it; this arm just keeps the match total.
            ComponentKind::Command { .. } | ComponentKind::Unsupported { .. } => Ok(vec![]),
            ComponentKind::LegacyPackage { .. } | ComponentKind::Package => {
                self.get_artifacts_for_target(target, component)
            },
        }
    }

    /// Returns every declared `(artifact id, resolved artifact)` pair available for `target`.
    pub fn get_artifacts_for_target(
        &self,
        target: &str,
        component: &Component,
    ) -> Result<Vec<(&str, ResolvedArtifact)>, InvalidArtifactError> {
        let mut artifacts = Vec::with_capacity(self.artifacts.len());
        for (id, artifact) in self.artifacts.iter() {
            if let Some(resolved) = artifact.resolve_for(id, target, component)? {
                artifacts.push((id.as_str(), resolved));
            }
        }

        Ok(artifacts)
    }

    pub fn get_artifact_for_target(
        &self,
        id: &str,
        target: &str,
        component: &Component,
    ) -> Result<Option<ResolvedArtifact>, InvalidArtifactError> {
        let Some(artifact) = self.artifacts.get(id) else {
            return Ok(None);
        };
        artifact.resolve_for(id, target, component)
    }
}

/// Holds a URI used to fetch an artifact.
///
/// These URIs have the following format:
///
/// * `(https://|file://)<path>/<component name>(-<triplet>)?(<extension>)`
///
/// # Forward compatibility
///
/// Like every other schema type, an artifact preserves fields this build does not recognize
/// (spec section 4.4). It needs hand-written `Serialize`/`Deserialize` for the same reason
/// [crate::manifest::Component] does: `TargetSpecific` already flattens `substitutions`, so a
/// catch-all `#[serde(flatten)]` would capture the keys that flatten consumed and emit them twice.
///
/// Deserialization also dispatches on the presence of `targets` rather than falling back through an
/// untagged enum. An untagged fallback reports "data did not match any variant", which says nothing
/// about which field was actually wrong.
#[derive(Debug, Clone, Hash, PartialEq)]
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
        substitutions: Option<Substitutions>,
        /// The supported target triples for this artifact, and their target-specific substitutions
        targets: BTreeMap<String, Substitutions>,
        /// An optional content digest. Recorded and round-tripped, never verified. See [Digest].
        digest: Option<Digest>,
        /// How the artifact is packaged at that URI, if it is not the bare file. See [Archive].
        archive: Option<Archive>,
        /// Fields declared by a newer schema that this build does not recognize.
        extra: crate::manifest::v3::unknown::Extra,
    },
    /// A non-executable/target-agnostic asset
    TargetAgnostic {
        /// The URI for this artifact, including the filename component
        ///
        /// The URI may contain the `%version` substitution string, which will be replaced with
        /// the version of the containing component. Note that `%version` requires that the
        /// component have an associated semantic version, or an error will be produced.
        uri: String,
        /// An optional content digest. Recorded and round-tripped, never verified. See [Digest].
        digest: Option<Digest>,
        /// How the artifact is packaged at that URI, if it is not the bare file. See [Archive].
        archive: Option<Archive>,
        /// Fields declared by a newer schema that this build does not recognize.
        extra: crate::manifest::v3::unknown::Extra,
    },
}

/// The typed shape of an artifact, without the unknown-field capture.
///
/// Kept in lockstep with [Artifact] by construction: it is what the hand-written impls below
/// serialize through, so "known" means exactly "what this emits".
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged, rename_all = "kebab-case")]
enum ArtifactFields {
    TargetSpecific {
        uri: String,
        #[serde(flatten, skip_serializing_if = "Option::is_none")]
        substitutions: Option<Substitutions>,
        targets: BTreeMap<String, Substitutions>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        digest: Option<Digest>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        archive: Option<Archive>,
    },
    TargetAgnostic {
        uri: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        digest: Option<Digest>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        archive: Option<Archive>,
    },
}

impl Artifact {
    fn fields(&self) -> ArtifactFields {
        match self {
            Self::TargetSpecific {
                uri,
                substitutions,
                targets,
                digest,
                archive,
                ..
            } => ArtifactFields::TargetSpecific {
                uri: uri.clone(),
                substitutions: substitutions.clone(),
                targets: targets.clone(),
                digest: digest.clone(),
                archive: archive.clone(),
            },
            Self::TargetAgnostic { uri, digest, archive, .. } => ArtifactFields::TargetAgnostic {
                uri: uri.clone(),
                digest: digest.clone(),
                archive: archive.clone(),
            },
        }
    }

    fn extra(&self) -> &crate::manifest::v3::unknown::Extra {
        match self {
            Self::TargetSpecific { extra, .. } | Self::TargetAgnostic { extra, .. } => extra,
        }
    }
}

impl Serialize for Artifact {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;

        crate::manifest::v3::unknown::merge_extra(&self.fields(), self.extra())
            .map_err(S::Error::custom)?
            .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Artifact {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;

        let value = serde_json::Value::deserialize(deserializer)?;

        // `targets` is what distinguishes the two shapes, and saying so produces a better
        // diagnostic than the untagged fallback's "data did not match any variant" -- which names
        // no field at all.
        if value.get("targets").is_some_and(|targets| !targets.is_object()) {
            return Err(D::Error::custom(
                "an artifact's `targets` must be a map of target triple to substitutions",
            ));
        }

        let (fields, extra) = crate::manifest::v3::unknown::split_extra::<ArtifactFields>(value)
            .map_err(D::Error::custom)?;

        Ok(match fields {
            ArtifactFields::TargetSpecific {
                uri,
                substitutions,
                targets,
                digest,
                archive,
            } => Self::TargetSpecific {
                uri,
                substitutions,
                targets,
                digest,
                archive,
                extra,
            },
            ArtifactFields::TargetAgnostic { uri, digest, archive } => {
                Self::TargetAgnostic { uri, digest, archive, extra }
            },
        })
    }
}

impl Artifact {
    /// The declared content digest, if any. Never verified -- see [Digest].
    ///
    /// For an archived artifact it describes the archive as fetched, not the file installed out of
    /// it.
    pub fn digest(&self) -> Option<&Digest> {
        match self {
            Self::TargetSpecific { digest, .. } | Self::TargetAgnostic { digest, .. } => {
                digest.as_ref()
            },
        }
    }

    /// How the artifact is packaged at its URI, if it is not the bare file.
    pub fn archive(&self) -> Option<&Archive> {
        match self {
            Self::TargetSpecific { archive, .. } | Self::TargetAgnostic { archive, .. } => {
                archive.as_ref()
            },
        }
    }
}

/// The value of user-defined substitutions in [Artifact] definitions
#[derive(Serialize, Deserialize, Default, Debug, Clone, Hash, PartialEq)]
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
    pub extra: crate::manifest::v3::unknown::Extra,
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
                if let Some(artifact) = self.resolve_for(id, "", component)? {
                    artifacts.push(artifact.uri);
                }
            },
            Self::TargetSpecific { targets, .. } => {
                for target in targets.keys() {
                    if let Some(artifact) = self.resolve_for(id, target, component)? {
                        artifacts.push(artifact.uri);
                    }
                }
            },
        }

        Ok(artifacts)
    }

    /// Resolves the artifact for `target`: its URI, and what it is packaged as.
    pub fn resolve_for(
        &self,
        id: &str,
        target: &str,
        component: &Component,
    ) -> Result<Option<ResolvedArtifact>, InvalidArtifactError> {
        match self {
            Self::TargetAgnostic { uri, archive, .. } => {
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
                let archive = supported_archive(archive.as_ref(), id, component)?;
                match scheme {
                    "file" => Ok(Some(ResolvedArtifact {
                        uri: ArtifactUri::File(PathBuf::from(rest.into_owned())),
                        archive,
                    })),
                    "http" | "https" => Ok(Some(ResolvedArtifact {
                        uri: ArtifactUri::Http(format!("{scheme}://{rest}")),
                        archive,
                    })),
                    scheme => Err(InvalidArtifactError::InvalidScheme {
                        id: id.to_string(),
                        component: component.name.to_string(),
                        scheme: scheme.to_string(),
                    }),
                }
            },
            Self::TargetSpecific { uri, substitutions, targets, archive, .. } => {
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
                let archive = supported_archive(archive.as_ref(), id, component)?;
                match scheme {
                    "file" => Ok(Some(ResolvedArtifact {
                        uri: ArtifactUri::File(PathBuf::from(rest)),
                        archive,
                    })),
                    "http" | "https" => Ok(Some(ResolvedArtifact {
                        uri: ArtifactUri::Http(format!("{scheme}://{rest}")),
                        archive,
                    })),
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

/// Narrows a declared format to one this build can read, rejecting the rest.
///
/// Here rather than at execution because it is a fact about the manifest -- known before a single
/// byte is fetched.
fn supported_archive(
    archive: Option<&Archive>,
    id: &str,
    component: &Component,
) -> Result<Option<SupportedFormat>, InvalidArtifactError> {
    let Some(archive) = archive else {
        return Ok(None);
    };
    archive.format.supported().map(Some).ok_or_else(|| {
        InvalidArtifactError::UnsupportedArchiveFormat {
            id: id.to_string(),
            component: component.name.to_string(),
            format: archive.format.to_string(),
        }
    })
}

/// A content digest declared for an artifact, e.g. `sha256:9f86d081...`.
///
/// Reserved, not enforced: the value is validated for shape, recorded, and round-tripped, but no
/// verification is performed against downloaded bytes. Turning verification on later is then a
/// behavior change rather than a schema break.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Digest {
    pub algorithm: String,
    pub hex: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InvalidDigestError {
    #[error("invalid digest '{0}': expected the form '<algorithm>:<hex>'")]
    Malformed(String),
    #[error("invalid digest '{0}': the digest value must be non-empty lowercase hex")]
    NotHex(String),
}

impl core::str::FromStr for Digest {
    type Err = InvalidDigestError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let Some((algorithm, hex)) = s.split_once(':') else {
            return Err(InvalidDigestError::Malformed(s.to_string()));
        };
        if algorithm.is_empty() {
            return Err(InvalidDigestError::Malformed(s.to_string()));
        }
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()) {
            return Err(InvalidDigestError::NotHex(s.to_string()));
        }
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(InvalidDigestError::NotHex(s.to_string()));
        }
        Ok(Self {
            algorithm: algorithm.to_string(),
            hex: hex.to_string(),
        })
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.algorithm, self.hex)
    }
}

impl Serialize for Digest {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod digest_tests {
    use super::*;

    #[test]
    fn digest_parses_and_round_trips() {
        let d: Digest = "sha256:9f86d081884c7d659a2feaa0c55ad015".parse().unwrap();
        assert_eq!(d.algorithm, "sha256");
        assert_eq!(d.to_string(), "sha256:9f86d081884c7d659a2feaa0c55ad015");
    }

    #[test]
    fn malformed_digest_is_rejected() {
        assert!("9f86d081".parse::<Digest>().is_err(), "missing algorithm");
        assert!(":abc".parse::<Digest>().is_err(), "empty algorithm");
        assert!("sha256:".parse::<Digest>().is_err(), "empty hex");
        assert!("sha256:zzzz".parse::<Digest>().is_err(), "non-hex");
        assert!("sha256:ABCD".parse::<Digest>().is_err(), "uppercase hex");
    }

    /// The digest survives a manifest round-trip and is not verified against anything.
    #[test]
    fn digest_round_trips_through_a_manifest() {
        let src = serde_json::json!({
            "manifest_version": "3.0.0",
            "date": 1735689600,
            "channels": [{"name": "0.15.0", "components": [{
                "name": "core",
                "version": {"kind": "registry", "version": "0.23.4"},
                "kind": "package",
                "artifacts": {"core.masp": {
                    "uri": "https://example.invalid/core.masp",
                    "digest": "sha256:deadbeef"
                }}
            }]}]
        })
        .to_string();

        let m = crate::manifest::VersionedManifest::parse_str(&src).expect("parse");
        let out: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(
            out["channels"][0]["components"][0]["artifacts"]["core.masp"]["digest"],
            serde_json::json!("sha256:deadbeef")
        );
    }

    #[test]
    fn a_malformed_digest_fails_the_parse() {
        let src = serde_json::json!({
            "manifest_version": "3.0.0",
            "date": 1735689600,
            "channels": [{"name": "0.15.0", "components": [{
                "name": "core",
                "version": {"kind": "registry", "version": "0.23.4"},
                "kind": "package",
                "artifacts": {"core.masp": {
                    "uri": "https://example.invalid/core.masp",
                    "digest": "not-a-digest"
                }}
            }]}]
        })
        .to_string();

        assert!(crate::manifest::VersionedManifest::parse_str(&src).is_err());
    }
}

#[cfg(test)]
mod archive_tests {
    use std::borrow::Cow;

    use super::*;
    use crate::{
        manifest::{ComponentKind, ExecutableComponent, InstallationMethod},
        profile::Profile,
    };

    const TARGET: &str = "aarch64-apple-darwin";

    fn component() -> Component {
        Component {
            name: Cow::Borrowed("vm"),
            version: Authority::Registry { version: semver::Version::new(0, 16, 0) },
            kind: ComponentKind::Executable {
                installation_method: InstallationMethod::Prebuilt,
                spec: ExecutableComponent {
                    installed_executable: "miden-vm".to_string(),
                    ..Default::default()
                },
            },
            profiles: vec![Profile::Minimal],
            requires: vec![],
            artifacts: Artifacts::default(),
            extra: Default::default(),
        }
    }

    fn parse(source: serde_json::Value) -> Artifact {
        serde_json::from_value(source).expect("must parse")
    }

    /// Every format this build reads must survive the manifest: declared by its spelling, it parses
    /// to that format and serializes back to the same document.
    #[test]
    fn every_supported_format_round_trips() {
        for format in SupportedFormat::ALL {
            let source = serde_json::json!({
                "uri": "https://example.invalid/vm.tar.gz",
                "archive": format.as_str()
            });
            let artifact = parse(source.clone());
            assert_eq!(
                artifact.archive().unwrap().format.supported(),
                Some(*format),
                "'{format}' must parse back to itself"
            );
            assert_eq!(serde_json::to_value(&artifact).unwrap(), source);
        }
    }

    #[test]
    fn an_archived_artifact_resolves_to_its_format() {
        let artifact = parse(serde_json::json!({
            "uri": "https://example.invalid/v%version/%basename-%target.%extension",
            "archive": "tar.gz",
            "basename": "miden-vm",
            "extension": "tar.gz",
            "targets": {TARGET: {}}
        }));

        let resolved = artifact
            .resolve_for("miden-vm", TARGET, &component())
            .expect("must resolve")
            .expect("the target is declared");

        assert_eq!(
            resolved.uri.to_string(),
            format!("https://example.invalid/v0.16.0/miden-vm-{TARGET}.tar.gz")
        );
        assert_eq!(resolved.archive, Some(SupportedFormat::TarGz));
    }

    /// An unreadable container is rejected when the installation is planned, not when the manifest
    /// is parsed -- an unrelated channel must stay parseable.
    #[test]
    fn an_unreadable_format_parses_but_does_not_resolve() {
        let source = serde_json::json!({
            "uri": "https://example.invalid/vm.zip",
            "archive": "zip"
        });
        let artifact = parse(source.clone());
        assert_eq!(
            serde_json::to_value(&artifact).unwrap(),
            source,
            "an unknown format must round-trip verbatim"
        );

        let err = artifact.resolve_for("miden-vm", "", &component()).expect_err("must fail");
        assert!(matches!(err, InvalidArtifactError::UnsupportedArchiveFormat { .. }), "{err}");
        assert!(err.to_string().contains("zip"), "the message must name the format: {err}");
    }

    /// An unarchived artifact emits no `archive` key at all, so existing manifests are untouched.
    #[test]
    fn an_unarchived_artifact_has_no_archive() {
        let source = serde_json::json!({"uri": "https://example.invalid/core.masp"});
        let artifact = parse(source.clone());
        assert!(artifact.archive().is_none());
        assert_eq!(serde_json::to_value(&artifact).unwrap(), source);

        let resolved = artifact.resolve_for("core.masp", "", &component()).unwrap().unwrap();
        assert!(resolved.archive.is_none());
    }
}

#[cfg(test)]
mod forward_compatibility_tests {
    use super::*;

    /// Spec section 4.4: *every* schema type preserves what this build does not understand,
    /// `Artifact` included. Without that, a newer publisher adding a field to an artifact -- a
    /// signature, say -- would lose it on the first `update-manifest` round trip.
    #[test]
    fn unknown_artifact_fields_round_trip() {
        for source in [
            serde_json::json!({
                "uri": "https://example.invalid/%target",
                "targets": {"aarch64-apple-darwin": {"basename": "vm"}},
                "signature": {"alg": "ed25519", "sig": "deadbeef"}
            }),
            serde_json::json!({
                "uri": "https://example.invalid/core.masp",
                "digest": "sha256:9f86d081884c7d659a2feaa0c55ad015",
                "provenance": ["a", "b"]
            }),
        ] {
            let artifact: Artifact = serde_json::from_value(source.clone()).expect("must parse");
            let out = serde_json::to_value(&artifact).expect("must serialize");
            assert_eq!(out, source, "an artifact must round-trip byte-for-byte");
        }
    }

    /// The capture must not duplicate keys that the flattened `substitutions` already consumed --
    /// the failure mode the hand-written impls exist to prevent.
    #[test]
    fn flattened_substitutions_are_not_duplicated_into_the_extras() {
        let source = serde_json::json!({
            "uri": "https://example.invalid/%target.%extension",
            "basename": "miden-vm",
            "extension": "tar.gz",
            "archive": "tar.gz",
            "targets": {"aarch64-apple-darwin": {}}
        });

        let artifact: Artifact = serde_json::from_value(source.clone()).unwrap();
        let out = serde_json::to_value(&artifact).unwrap();

        assert_eq!(out, source);
        assert_eq!(
            out.as_object().unwrap().len(),
            5,
            "no key may appear twice: {}",
            serde_json::to_string(&out).unwrap()
        );
    }

    /// The artifact's own capture only sees the top level, so a nested archive has to preserve its
    /// unknown fields itself.
    #[test]
    fn unknown_fields_inside_an_archive_round_trip() {
        let source = serde_json::json!({
            "uri": "https://example.invalid/vm.tar.gz",
            "archive": {"format": "tar.gz", "strip-components": 1}
        });

        let artifact: Artifact = serde_json::from_value(source.clone()).expect("must parse");
        assert_eq!(serde_json::to_value(&artifact).unwrap(), source);
    }

    /// A malformed artifact names what is wrong with it, rather than reporting that no variant
    /// matched.
    #[test]
    fn a_malformed_targets_map_is_reported_as_such() {
        let err = serde_json::from_value::<Artifact>(serde_json::json!({
            "uri": "https://example.invalid/%target",
            "targets": ["aarch64-apple-darwin"]
        }))
        .expect_err("must reject");

        assert!(err.to_string().contains("targets"), "{err}");
    }
}
