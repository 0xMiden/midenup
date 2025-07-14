use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::manifest::Manifest;

#[derive(Debug)]
/// This struct holds contextual information about the environment in which
/// midenup/miden will operate under. This meant to be a *read-only* data
/// structure.
pub struct Config {
    /// The path to the current working directory in which midenup/miden was
    /// called from.
    pub working_directory: PathBuf,
    /// The path to the midenup's home directory, which holds all the installed
    /// toolchains with their respective libraries and executables.
    ///
    /// By default, it will point to $XDG_DATA_HOME/midenup; although a custom
    /// path can be specified via the MIDENUP_HOME environment variable, like
    /// so:
    ///
    /// MIDENUP_HOME=/path/to/custom/home midenup
    pub midenup_home: PathBuf,
    /// This represents the upstream manifest, which contains the state of all
    /// the available toolchains with their respective components.
    ///
    /// It is usually going to be obtained from cURLing the URI present in
    /// [crate::manifest::Manifest::PUBLISHED_MANIFEST_URI], although it could
    /// also be obtained from a different source (be it a local file or a
    /// different URL) for debugging purposes. The source can be specified via
    /// the MIDENUP_MANIFEST_URI environment variable. For example:
    ///
    /// MIDENUP_MANIFEST_URI=file://your-custom-manifest.json midenup
    ///
    /// For more information about the Manifest's fields and format, see
    /// [Manifest].
    pub manifest: Manifest,
}

impl Config {
    pub fn init(midenup_home: PathBuf, manifest_uri: impl AsRef<str>) -> anyhow::Result<Config> {
        let manifest = Manifest::load_from(manifest_uri)?;
        let working_directory =
            std::env::current_dir().context("Could not obtain present working directory")?;

        let config = Config {
            midenup_home,
            manifest,
            working_directory,
        };

        Ok(config)
    }

    /// This ensures that the midneup directory is properly initialized.
    pub fn ensure_midenup_home_exists(&self) -> anyhow::Result<()> {
        commands::init(self)
    }
}
