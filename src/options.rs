use clap::{Parser, ValueEnum};

pub const DEFAULT_USER_DATA_DIR: &str = "XDG_DATA_HOME";

/// Optional installation settings.
#[derive(Debug, Parser, Clone, Copy)]
pub struct InstallationOptions {
    #[clap(long, short, default_value = "false")]
    /// Displays the entirety of cargo's output when performing installations.
    pub verbose: bool,
}

#[allow(clippy::derivable_impls)]
impl Default for InstallationOptions {
    fn default() -> Self {
        Self { verbose: false }
    }
}

/// Optional update settings.
#[derive(Debug, Parser, Clone, Copy)]
pub struct UpdateOptions {
    #[clap(long, short, default_value = "false")]
    /// Displays the entirety of cargo's output when performing installations.
    pub verbose: bool,

    /// Determines how midenup will handle updates for components installed from a path
    #[clap(value_enum, short, long, default_value = "off")]
    pub path_update: PathUpdate,
}

#[derive(Default, Debug, Parser, Clone, Copy, ValueEnum)]
pub enum PathUpdate {
    #[default]
    Off,
    All,
    Interactive,
}

#[allow(clippy::derivable_impls)]
impl Default for UpdateOptions {
    fn default() -> Self {
        Self {
            verbose: false,
            path_update: PathUpdate::default(),
        }
    }
}

impl From<InstallationOptions> for UpdateOptions {
    fn from(value: InstallationOptions) -> Self {
        UpdateOptions {
            verbose: value.verbose,
            ..Default::default()
        }
    }
}

impl From<UpdateOptions> for InstallationOptions {
    fn from(value: UpdateOptions) -> Self {
        InstallationOptions { verbose: value.verbose }
    }
}
