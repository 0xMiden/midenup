use std::path::PathBuf;

use anyhow::{Context, bail};

use crate::{manifest::Manifest, toolchain::Toolchain, utils};

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

    pub fn update_opt_symlinks(&self) -> anyhow::Result<()> {
        let (current_toolchain, _) = Toolchain::current(self)?;

        // Directory which point to the directory where symlinks are stored
        let opt_dir = self.midenup_home.join("opt");

        let Some(active_channel) = self.manifest.get_channel(&current_toolchain.channel) else {
            bail!("channel '{}' doesn't exist or is unavailable", current_toolchain.channel);
        };

        let update = if let Ok(pointing) = std::fs::read_link(&opt_dir) {
            // If it does exist, update it if it's pointing to a non-active toolchain.
            pointing
                .file_name()
                .and_then(|toolchain_name| toolchain_name.to_str())
                .is_some_and(|toolchain_name| toolchain_name != active_channel.name.to_string())
        } else {
            // If the symlink doesn't exist, update it by creating it.
            true
        };

        if update {
            if opt_dir.exists() {
                std::fs::remove_file(&opt_dir).context("Couldn't remove 'opt' symlink")?;
            }
            let opt_path = active_channel.get_channel_dir(self).join("opt");
            utils::symlink(&opt_dir, &opt_path).with_context(|| {
                format!(
                    "Failed to create opt/ symlink from {} to {}",
                    opt_dir.display(),
                    opt_path.display()
                )
            })?;
        }

        Ok(())
    }
}
