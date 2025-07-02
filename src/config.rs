use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::manifest::Manifest;

#[derive(Debug)]
pub struct Config {
    pub pwd: PathBuf,
    pub midenup_home: PathBuf,
    pub manifest: Manifest,
}

impl Config {
    pub fn init(midenup_home: PathBuf, manifest_uri: impl AsRef<str>) -> anyhow::Result<Config> {
        let manifest = Manifest::load_from(manifest_uri)?;
        let pwd = std::env::current_dir().context("Could not obtain present working directory")?;

        let config = Config { midenup_home, manifest, pwd };

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
