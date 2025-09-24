use std::{
    ffi::OsStr,
    fmt::Display,
    ops::Deref,
    path::{Path, PathBuf},
};

use anyhow::Context;

use crate::{
    channel::{Channel, Component, InstalledFile},
    manifest::Manifest,
};

#[allow(clippy::enum_variant_names)]
pub enum ToolchainInstallationStatus {
    FullyInstalled(File),
    PartiallyInstalled(File),
    NotInstalled,
}

#[derive(Debug)]
pub struct File(PathBuf);
impl Deref for File {
    type Target = PathBuf;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
// Implement AsRef<Path> for your struct
impl AsRef<Path> for File {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

impl AsRef<OsStr> for File {
    fn as_ref(&self) -> &OsStr {
        self.0.as_os_str()
    }
}

#[derive(Debug)]
pub struct Directory(PathBuf);
impl Deref for Directory {
    type Target = PathBuf;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
// Implement AsRef<Path> for your struct
impl AsRef<Path> for Directory {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}
// Implement AsRef<Path> for your struct
impl AsRef<OsStr> for Directory {
    fn as_ref(&self) -> &OsStr {
        self.0.as_os_str()
    }
}

/// Struct that handles Midneup's home
#[derive(Debug)]
pub struct Home {
    midenup_home: PathBuf,
}

impl Home {
    fn new(midenup_home: PathBuf) -> Self {
        Home { midenup_home }
    }

    /// Function that ensures that the Midenup Home directory exists. Returns
    /// whether it already existed or not.
    pub fn ensure_exists(&self) -> anyhow::Result<bool> {
        if !self.midenup_home.exists() {
            std::fs::create_dir_all(&self.midenup_home).with_context(|| {
                format!(
                    "failed to initialize MIDENUP_HOME directory: '{}'",
                    self.midenup_home.display()
                )
            })?;
            Ok(false)
        } else {
            Ok(true)
        }
    }

    /// Check to what degree a [[Toolchain]] has been installed.
    pub fn check_toolchain_installation(&self, channel: &Channel) -> ToolchainInstallationStatus {
        let channel_dir = self.midenup_home.join("toolchains").join(format!("{}", channel.name));
        let installation_complete = channel_dir.join("installation-successful");
        let installation_in_progress = channel_dir.join(".installation-in-progress");

        if installation_complete.exists() {
            ToolchainInstallationStatus::FullyInstalled(File(installation_complete))
        } else if installation_in_progress.exists() {
            ToolchainInstallationStatus::PartiallyInstalled(File(installation_in_progress))
        } else {
            ToolchainInstallationStatus::NotInstalled
        }
    }

    pub fn get_manifest(&self) -> File {
        File(self.midenup_home.join("manifest").with_extension("json"))
    }

    /// The location of the toolchains/ directory, where all the toolchains are
    /// installed.
    pub fn get_toolchains_dir(&self) -> File {
        File(self.midenup_home.join("toolchains"))
    }

    /// The location of Midenup's bin directory.
    pub fn get_bin_dir(&self) -> Directory {
        Directory(self.midenup_home.join("bin"))
    }

    /// Get the toolchain directory associated with a specific [[Channel]].
    pub fn get_toolchain_dir(&self, channel: &Channel) -> Directory {
        let installed_toolchains_dir = self.midenup_home.join("toolchains");
        Directory(installed_toolchains_dir.join(format!("{}", channel.name)))
    }

    /// Get the [[Channel]]'s bin directory
    pub fn get_bin_dir_from(&self, channel: &Channel) -> Directory {
        Directory(self.get_toolchain_dir(channel).join("bin"))
    }

    /// Get the toolchain directory associated with a specific [[Channel]].
    pub fn get_installed_channel(&self, channel: &Channel) -> File {
        File(self.get_toolchain_dir(channel).join(".installed_channel.json"))
    }

    /// The location of the stable symlink
    pub fn get_stable_dir(&self) -> Directory {
        Directory(self.get_toolchains_dir().join("stable"))
    }

    /// The location of the stable symlink
    pub fn get_default_dir(&self) -> Directory {
        Directory(self.get_toolchains_dir().join("default"))
    }

    /// Get the intall.rs file for a corresponding channel.
    pub fn get_installer(&self, channel: &Channel) -> File {
        File(self.get_toolchain_dir(channel).join("install").with_extension("rs"))
    }

    /// The location of the stable symlink
    pub fn get_installed_file(&self, channel: &Channel, component: &Component) -> File {
        let toolchain_dir = self.get_toolchain_dir(channel);

        let installed_file = match component.get_installed_file() {
            InstalledFile::Executable { binary_name } => {
                toolchain_dir.join("bin").join(binary_name)
            },
            InstalledFile::Library { library_name } => toolchain_dir.join("lib").join(library_name),
        };

        File(installed_file)
    }

    pub fn get_miden_executable(&self) -> File {
        File(self.get_bin_dir().join("miden"))
    }
}

impl Display for Home {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.midenup_home.as_path().display())
    }
}

impl AsRef<std::ffi::OsStr> for Home {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.midenup_home.as_os_str()
    }
}

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
    // pub midenup_home: PathBuf,
    pub midenup_home_2: Home,
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
    /// This flag is used to detect/distinguish when midenup is being used in
    /// tests. At the time of writing, this is mostly done to install debug
    /// builds of the various miden components to speed tests up.
    pub debug: bool,
}

impl Config {
    pub fn init(
        midenup_home: PathBuf,
        manifest_uri: impl AsRef<str>,
        debug: bool,
    ) -> anyhow::Result<Config> {
        let manifest = Manifest::load_from(manifest_uri)?;
        let working_directory =
            std::env::current_dir().context("Could not obtain present working directory")?;

        let config = Config {
            working_directory,
            midenup_home_2: Home::new(midenup_home),
            manifest,
            debug,
        };

        Ok(config)
    }
}
