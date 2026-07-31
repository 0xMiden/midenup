//! Two-stage schema version dispatch.
//!
//! Every document `midenup` reads declares its schema version in a single top-level field. That
//! field is parsed *first*, on its own, and only then is the rest of the document parsed with the
//! matching schema.
//!
//! This matters for forward compatibility. Deriving the version as part of deserializing the whole
//! document ties version detection to the shape of everything else, so a document from a newer
//! `midenup` fails to parse before we can produce the one diagnostic that would actually help
//! ("you need a newer midenup"). Reading a minimal header first decouples the two.

use serde::Deserialize;

use super::ManifestError;

/// The schema version declared by a document, read in isolation from the rest of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionHeader {
    pub version: semver::Version,
}

/// Whether this build can read a document declaring a given schema version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Compatibility {
    /// Same major version: readable. Unknown fields introduced by a newer minor are preserved.
    Supported,
    /// Major version above what this build knows.
    RequiresNewer { found: semver::Version },
    /// Major version below what this build reads natively. May still be migratable.
    TooOld { found: semver::Version },
}

/// Reads just the `field` version header from `content`.
///
/// Deliberately tolerant of everything else in the document: sibling fields may be absent,
/// unrecognized, or malformed. Only `field` has to be present and parseable as a semantic version.
pub fn read_version_header(content: &str, field: &str) -> Result<VersionHeader, ManifestError> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(flatten)]
        fields: serde_json::Map<String, serde_json::Value>,
    }

    let envelope = serde_json::from_str::<Envelope>(content)
        .map_err(|err| ManifestError::Invalid(format!("failed to parse document: {err}")))?;

    let Some(raw) = envelope.fields.get(field) else {
        return Err(ManifestError::MissingVersion(field.to_string()));
    };
    let Some(raw) = raw.as_str() else {
        return Err(ManifestError::MissingVersion(field.to_string()));
    };
    let version = raw
        .parse::<semver::Version>()
        .map_err(|_| ManifestError::MissingVersion(field.to_string()))?;

    Ok(VersionHeader { version })
}

/// Classifies a declared version against the major version this build reads natively.
///
/// Compatibility is evaluated on the **major** component only: a newer minor or patch is additive
/// by construction, so it is readable.
pub fn classify(found: &semver::Version, supported_major: u64) -> Compatibility {
    match found.major.cmp(&supported_major) {
        std::cmp::Ordering::Equal => Compatibility::Supported,
        std::cmp::Ordering::Greater => Compatibility::RequiresNewer { found: found.clone() },
        std::cmp::Ordering::Less => Compatibility::TooOld { found: found.clone() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_parses_without_requiring_other_fields() {
        // A newer manifest may add or remove sibling fields; version detection must not depend on
        // `date` or `channels` being present or well-formed.
        let h = read_version_header(r#"{"manifest_version":"3.1.0"}"#, "manifest_version").unwrap();
        assert_eq!(h.version, semver::Version::new(3, 1, 0));

        let h = read_version_header(
            r#"{"manifest_version":"2.0.0","channels":"not-an-array","future":{"a":1}}"#,
            "manifest_version",
        )
        .unwrap();
        assert_eq!(h.version, semver::Version::new(2, 0, 0));
    }

    #[test]
    fn header_reads_the_named_field_so_one_impl_serves_both_documents() {
        let h = read_version_header(r#"{"state_version":"1.0.0"}"#, "state_version").unwrap();
        assert_eq!(h.version, semver::Version::new(1, 0, 0));
    }

    #[test]
    fn classify_uses_major_only() {
        assert_eq!(classify(&semver::Version::new(2, 0, 0), 2), Compatibility::Supported);
        assert_eq!(classify(&semver::Version::new(2, 9, 3), 2), Compatibility::Supported);
        assert!(matches!(
            classify(&semver::Version::new(3, 0, 0), 2),
            Compatibility::RequiresNewer { .. }
        ));
        assert!(matches!(
            classify(&semver::Version::new(1, 0, 1), 2),
            Compatibility::TooOld { .. }
        ));
    }

    #[test]
    fn missing_or_malformed_version_field_is_an_error() {
        for bad in [
            r#"{"date":1}"#,                        // absent
            r#"{"manifest_version":2}"#,            // not a string
            r#"{"manifest_version":"not-semver"}"#, // unparseable
        ] {
            assert!(
                matches!(
                    read_version_header(bad, "manifest_version"),
                    Err(ManifestError::MissingVersion(_))
                ),
                "should reject {bad}"
            );
        }
    }

    #[test]
    fn malformed_json_is_reported_as_such() {
        assert!(matches!(
            read_version_header("{not json", "manifest_version"),
            Err(ManifestError::Invalid(_))
        ));
    }
}
