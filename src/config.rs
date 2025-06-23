use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::manifest::{Manifest, ManifestError};

pub struct Config {
    pub midenup_home: PathBuf,
    pub manifest: Manifest,
    /// Local version of the manifest describing the installed toolchains. If
    /// missing, it starts as [Manifest::default]
    pub local_manifest: Manifest,
}

impl Config {
    pub fn init(midenup_home: PathBuf, manifest_uri: impl AsRef<str>) -> anyhow::Result<Config> {
        let manifest = Manifest::load_from(manifest_uri)?;

        let local_manifest_path = midenup_home.join("manifest").with_extension("json");
        let local_manifest_uri = format!(
            "file://{}",
            local_manifest_path.to_str().context("Couldn't convert miden directory")?,
        );
        let local_manifest = match Manifest::load_from(local_manifest_uri) {
            Ok(manifest) => Ok(manifest),
            Err(ManifestError::EmptyManifest | ManifestError::MissingManifest(_)) => {
                Ok(Manifest::default())
            },
            Err(err) => Err(err),
        }
        .context("Error parsing manifest")?;

        let config = Config { midenup_home, manifest, local_manifest };

        Ok(config)
    }

    pub fn ensure_midenup_home_exists(&self) -> anyhow::Result<&Path> {
        if !self.midenup_home.exists() {
            std::fs::create_dir_all(&self.midenup_home).with_context(|| {
                format!("failed to create MIDENUP_HOME with path: {}", self.midenup_home.display())
            })?;
        }

        Ok(&self.midenup_home)
    }
}
