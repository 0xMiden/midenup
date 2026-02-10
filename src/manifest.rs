use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    path::Path,
    time::SystemTime,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    channel::{Channel, ChannelAlias, UserChannel},
    version::Authority,
};

const MANIFEST_VERSION: &str = "1.0.0";
const HTTP_ERROR_CODES: std::ops::Range<u32> = 400..500;

/// The global manifest of all known channels and their toolchains
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Manifest {
    /// This version is used to handle breaking changes in the manifest format itself
    manifest_version: Cow<'static, str>,
    /// The UTC timestamp at which this manifest was generated
    date: i64,
    /// The channels described in this manifest
    channels: Vec<Channel>,
}

impl Default for Manifest {
    fn default() -> Self {
        let date = chrono::Utc::now().timestamp();
        Self {
            manifest_version: Cow::Borrowed(MANIFEST_VERSION),
            date,
            channels: vec![],
        }
    }
}

#[derive(Error, Debug)]
pub enum ManifestError {
    #[error("Manifest file is empty")]
    Empty,
    #[error("Webpage {0} is empty")]
    EmptyWebpage(String),
    #[error("Webpage returned error. Does {0} exist?")]
    WebpageError(String),
    #[error("Manifest file is not present in `{0}`")]
    Missing(String),
    #[error("Invalid channel manifest in URI: `{0}`")]
    Invalid(String),
    #[error("Couldn't reach webpage: `{0}`")]
    InternalCurlError(String),
    #[error("unsupported channel manifest URI: `{0}`")]
    Unsupported(String),
    #[error("Failed to update time in manifest: `{0}`")]
    InvalidTime(String),
    #[error("invalid UTF-8: `{0}`")]
    InvalidUtf8(String),
    #[error("failed to serialize manifest: `{0}`")]
    Serialization(String),
}

impl Manifest {
    pub const PUBLISHED_MANIFEST_URI: &str =
        "https://0xmiden.github.io/midenup/channel-manifest.json";
    pub const LOCAL_MANIFEST_URI: &str = "https://0xmiden.github.io/midenup/channel-manifest.json";

    /// Loads a [Manifest] from the given URI.
    pub fn load_from(uri: impl AsRef<str>) -> Result<Manifest, ManifestError> {
        let uri = uri.as_ref();
        let manifest = if let Some(manifest_path) = uri.strip_prefix("file://") {
            let path = Path::new(manifest_path);
            let contents = std::fs::read_to_string(path)
                .map_err(|_| ManifestError::Missing(path.display().to_string()))?;
            // This could potentially be valid if we are parsing the local manifest
            if contents.is_empty() {
                return Err(ManifestError::Empty);
            }

            serde_json::from_str::<Manifest>(&contents).map_err(|e| {
                ManifestError::Invalid(format!(
                    "Invalid channel manifest in {}: {e}",
                    path.display()
                ))
            })
        } else if uri.starts_with("https://") {
            let mut data = Vec::new();
            let mut handle = curl::easy::Easy::new();
            handle.url(uri).map_err(|error| {
                let mut err = format!("Error code {}: ", error.code());
                err.push_str(error.description());
                ManifestError::InternalCurlError(err)
            })?;
            {
                let response_code = handle.response_code().map_err(|_| {
                    ManifestError::InternalCurlError(String::from(
                        "Failed to get response code; despite HTTP protocol supporting it.",
                    ))
                })?;
                if HTTP_ERROR_CODES.contains(&response_code) {
                    return Err(ManifestError::WebpageError(uri.to_string()));
                }

                let mut transfer = handle.transfer();
                transfer
                    .write_function(|new_data| {
                        data.extend_from_slice(new_data);
                        Ok(new_data.len())
                    })
                    .unwrap();
                transfer.perform().map_err(|error| {
                    let mut err = format!("Error code {}: ", error.code());
                    err.push_str(error.description());
                    ManifestError::InternalCurlError(err)
                })?
            }
            if data.is_empty() {
                return Err(ManifestError::EmptyWebpage(uri.to_string()));
            }
            serde_json::from_slice::<Manifest>(&data).map_err(|_| {
                let text = String::from_utf8(data.clone()).unwrap_or_default();
                ManifestError::Invalid(format!(
                    "Invalid channel manifest
{text}"
                ))
            })
        } else {
            return Err(ManifestError::Unsupported(uri.to_string()));
        }?;

        Ok(manifest)
    }

    pub fn remove_channel(&mut self, channel_name: semver::Version) {
        //
        self.channels.retain(|c| c.name != channel_name);
    }

    pub fn add_channel(&mut self, channel: Channel) {
        // Before adding the new stable channel, remove the stable alias from
        // all the channels that have it.
        // NOTE: This should be only a single channel, we check for multiple
        // just in case.
        if self.is_latest_stable(&channel) {
            for channel in self
                .channels
                .iter_mut()
                .filter(|c| c.alias.as_ref().is_some_and(|a| matches!(a, ChannelAlias::Stable)))
            {
                channel.alias = None
            }
        }

        // NOTE: If the channel already exists in the manifest, remove the old
        // version. This happens when updating
        self.channels.retain(|c| c.name != channel.name);

        self.channels.push(channel);
    }

    /// Determines whether the `channel` is the latest stable version. This can
    /// only be determined by the [Manifest], since this definition is dependant
    /// on all the other present [Channels]
    pub fn is_latest_stable(&self, channel: &Channel) -> bool {
        self.channels.iter().filter(|c| c.is_stable()).all(|c| {
            let comparison = channel.name.cmp_precedence(&c.name);
            matches!(comparison, std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
        })
    }

    /// Attempts to fetch the version corresponding to the `stable` [Channel],
    /// by definition this is the latest version.
    /// WARNING: This method is mainly intended to be used with the *Upstream*
    /// Manifest, not the Local Manifest.  This is because, stable is simply
    /// defined to be "the latest non-nightly" channel in the [Manifest].
    /// Therefore, in order to have a unified vision of what "stable" refers,
    /// refer to the upstream [Manifest].
    pub fn get_latest_stable(&self) -> Option<&Channel> {
        self.channels
            .iter()
            .find(|c| matches!(c.alias, Some(ChannelAlias::Stable)))
            .or_else(|| {
                self.channels
                    .iter()
                    .filter(|c| c.is_stable())
                    .max_by(|x, y| x.name.cmp_precedence(&y.name))
            })
    }

    pub fn get_latest_nightly(&self) -> Option<&Channel> {
        self.channels.iter().find(|c| c.is_latest_nightly()).or_else(|| {
            self.channels
                .iter()
                .filter(|c| c.is_nightly())
                .max_by(|x, y| x.name.cmp_precedence(&y.name))
        })
    }

    pub fn get_named_nightly(&self, name: impl AsRef<str>) -> Option<&Channel> {
        self.channels.iter().find(|c| {
            c.alias.as_ref().is_some_and(
                |alias| matches!(alias, ChannelAlias::Nightly(Some(tag)) if tag == name.as_ref()),
            )
        })
    }

    pub fn get_channel_by_name(&self, ver: &semver::Version) -> Option<&Channel> {
        self.channels.iter().find(|c| &c.name == ver)
    }

    /// Attempts to fetch the [Channel] corresponding to the given [ChannelType]
    pub fn get_channel(&self, channel: &UserChannel) -> Option<&Channel> {
        match channel {
            UserChannel::Version(v) => self.channels.iter().find(|c| &c.name == v),
            UserChannel::Stable => self.get_latest_stable(),
            UserChannel::Nightly => self.get_latest_nightly(),
            UserChannel::Other(tag) => match tag.strip_prefix("nightly-") {
                Some(suffix) => self.get_named_nightly(suffix),
                None => self.channels.iter().find(|c| {
                    c.alias.as_ref().is_some_and(|alias| {
                        matches!(alias, ChannelAlias::Tag(t) if t ==
            tag.as_ref())
                    })
                }),
            },
        }
    }

    pub fn get_channels(&self) -> impl Iterator<Item = &Channel> {
        self.channels.iter()
    }

    fn set_date(&mut self, new_date: SystemTime) -> Result<(), ManifestError> {
        let time = new_date
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|dur| dur.as_secs())
            .and_then(|secs| i64::try_from(secs).ok())
            .ok_or(ManifestError::InvalidTime(format!(
                "Failed to set {:?} as the Manifest's time",
                new_date
            )))?;

        self.date = time;
        Ok(())
    }

    fn fetch_latest_releases(&self) -> Result<LatestReleases, ManifestError> {
        let mut crates = HashMap::new();

        for channel in self.get_channels() {
            for component in &channel.components {
                let Authority::Cargo { package, .. } = &component.version else {
                    continue;
                };

                let crate_name = package.as_deref().unwrap_or(&component.name);

                // Skip if already fetched.
                if crates.contains_key(crate_name) {
                    continue;
                }

                let entries = Manifest::fetch_released_versions(crate_name)?;

                let mut mapping: HashMap<MajorMinor, IndexEntry> = HashMap::new();
                for entry in entries {
                    let key = MajorMinor {
                        major: entry.version.major,
                        minor: entry.version.minor,
                    };
                    mapping
                        .entry(key)
                        .and_modify(|existing| {
                            if entry.version.patch > existing.version.patch {
                                *existing = entry.clone();
                            }
                        })
                        .or_insert(entry);
                }

                crates.insert(crate_name.to_string(), CrateVersions { mapping });
            }
        }

        Ok(LatestReleases { crates })
    }

    /// The sparse's index format can be found here:
    /// https://doc.rust-lang.org/cargo/reference/registry-index.html#index-files
    fn sparse_index_url(crate_name: &str) -> String {
        let path = match crate_name.len() {
            // The first 3 cases are only kept for compatibility/future proofing.
            // Miden packages will probably not fall under these cases.
            1 => format!("1/{crate_name}"),
            2 => format!("2/{crate_name}"),
            3 => format!("3/{}/{crate_name}", &crate_name[0..1]),
            _ => {
                let first_two = &crate_name[0..2];
                let next_two = &crate_name[2..4];
                format!("{first_two}/{next_two}/{crate_name}")
            },
        };
        format!("https://index.crates.io/{path}")
    }

    /// Get all the available versions from a crate.
    /// For more information about crates.io API and format, see:
    /// - https://crates.io/data-access
    /// - https://doc.rust-lang.org/cargo/reference/registry-index.html
    fn fetch_released_versions(crate_name: &str) -> Result<Vec<IndexEntry>, ManifestError> {
        let url = Manifest::sparse_index_url(crate_name);
        let mut data = Vec::new();

        let mut handle = curl::easy::Easy::new();
        handle.url(&url).map_err(|e| ManifestError::InternalCurlError(e.to_string()))?;
        {
            let mut transfer = handle.transfer();
            transfer
                .write_function(|new_data| {
                    data.extend_from_slice(new_data);
                    Ok(new_data.len())
                })
                .unwrap();
            transfer
                .perform()
                .map_err(|e| ManifestError::InternalCurlError(e.to_string()))?;
        }

        let body =
            String::from_utf8(data).map_err(|e| ManifestError::InvalidUtf8(e.to_string()))?;

        let index_entries = body
            .lines()
            .map(|line| {
                serde_json::from_str::<IndexEntry>(line)
                    .map_err(|e| ManifestError::Serialization(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(index_entries)
    }
}

/// Struct containing information about the update result.
pub struct ManifestUpdateResult {
    /// Set of all the changed packages. If none were changed, it will be empty.
    pub changed_packages: HashSet<String>,

    /// The Manifest's new time. If no packages are updated, the time is left
    /// untouched and this will be None.
    pub new_date: Option<SystemTime>,
}
impl Manifest {
    /// Tries to update the manifest, checking all the registered crates for
    /// newer release.
    pub fn update(&mut self) -> Result<ManifestUpdateResult, ManifestError> {
        let latest_releases = self.fetch_latest_releases()?;

        let mut changed_packages = HashSet::new();
        for channel in &mut self.channels {
            for component in &mut channel.components {
                let Authority::Cargo { package, version } = &mut component.version else {
                    continue;
                };

                let crate_name = package.as_deref().unwrap_or(&component.name);
                let key = MajorMinor {
                    major: version.major,
                    minor: version.minor,
                };

                let Some(crate_versions) = latest_releases.crates.get(crate_name) else {
                    continue;
                };
                let Some(latest) = crate_versions.mapping.get(&key) else {
                    continue;
                };

                if version.patch > latest.version.patch {
                    continue;
                }

                *version = latest.version.clone();
                changed_packages.insert(crate_name.to_string());
            }
        }

        if changed_packages.is_empty() {
            let update_result = ManifestUpdateResult { changed_packages, new_date: None };

            return Ok(update_result);
        }

        let now = std::time::SystemTime::now();
        self.set_date(now)?;

        let update_result = ManifestUpdateResult { changed_packages, new_date: Some(now) };

        Ok(update_result)
    }
}

// Helper structures
/// An entry in the crate's crates.io sparse index
#[derive(Deserialize, Debug, Clone)]
struct IndexEntry {
    /// The crate's version
    #[serde(rename = "vers")]
    version: semver::Version,
    // We ignore the rest of the values.
}

/// Key used in [[CreateVersions]]
#[derive(Debug, PartialEq, Eq, Hash)]
struct MajorMinor {
    major: u64,
    minor: u64,
}

/// Mapping from every major.minor release to its latest crates.io index entry
#[derive(Debug)]
struct CrateVersions {
    mapping: HashMap<MajorMinor, IndexEntry>,
}

/// Stores all the crates present in the manifest and their respective
/// [[CrateVersions]] mappings.
#[derive(Debug)]
struct LatestReleases {
    crates: HashMap<CrateName, CrateVersions>,
}
type CrateName = String;

#[cfg(test)]
mod tests {
    use std::borrow::Cow;

    use super::Manifest;
    use crate::{channel::UserChannel, manifest::ChannelAlias, version::Authority};

    #[test]
    /// Validates that the current channel manifest is parseable.
    fn validate_current_channel_manifest() {
        let manifest = Manifest::load_from("file://manifest/channel-manifest.json")
            .expect("Couldn't load manifest");

        let _stable = manifest
            .get_channel(&UserChannel::Stable)
            .expect("Could not convert UserChannel to internal channel representation");
    }

    #[test]
    /// Validates that the *published* channel manifest is parseable.
    /// NOTE: This test is mainly intended for backwards compatibilty reasons.
    fn validate_published_channel_manifest() {
        let manifest = Manifest::load_from(Manifest::PUBLISHED_MANIFEST_URI)
            .expect("Failed to parse upstream manifest.");

        let _ = manifest
            .get_channel(&UserChannel::Stable)
            .expect("Could not convert UserChannel to internal channel representation");
    }

    #[test]
    /// Validates that non-standard manifest features are parsed correctly, these include:
    /// - Non stable channels (custom tags, nightly)
    /// - Components wwith git and a path as an [[Authority]].
    fn unit_test_manifest_additional() {
        const FILE: &str =
            "file://tests/data/unit_test_manifest_additional/manifest-non-stable.json";
        let manifest = Manifest::load_from(FILE).unwrap();
        {
            let custom_build = manifest
                .get_channel(&UserChannel::Other(Cow::Borrowed("custom-dev-build")))
                .unwrap_or_else(|| {panic!("Could not convert UserChannel to internal channel representation from {FILE}",)
                });

            #[allow(unused_variables)]
            {
                let prerelease = semver::Prerelease::new("custom-build").unwrap();
                assert!(matches!(&custom_build.name, semver::Version { pre: _prerelease, .. }));
            }
            assert_eq!(
                custom_build.alias,
                Some(ChannelAlias::Tag(Cow::Borrowed("custom-dev-build")))
            );
            {
                let std_lib = custom_build
                    .get_component("std")
                    .unwrap_or_else(|| panic!("Could not find standard library in {FILE}",));

                assert!(matches!(std_lib.version, Authority::Path { .. }));
            }
        }
        {
            let nightly = manifest.get_channel(&UserChannel::Nightly).unwrap_or_else(|| {
                panic!(
                    "Could not convert UserChannel to internal channel representation from {FILE}",
                )
            });
            assert_eq!(nightly.alias, Some(ChannelAlias::Nightly(None)));
            {
                let client = nightly
                    .get_component("client")
                    .unwrap_or_else(|| panic!("Could not find standard library in {FILE}",));

                assert!(matches!(client.version, Authority::Git { .. }));
            }
        }
    }
}
