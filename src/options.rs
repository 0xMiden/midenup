use clap::{Parser, ValueEnum};

use crate::commands::Update;

pub const DEFAULT_USER_DATA_DIR: &str = "XDG_DATA_HOME";

// type ComponentName = Cow<'static, str>;
/// Optional installation settings.
#[derive(Debug, Parser, Clone)]
pub struct InstallationOptions {
    #[clap(long, short, default_value = "false")]
    /// Displays the entirety of cargo's output when performing installations.
    pub verbose: bool,
    #[clap(skip)]
    /// These are components that are already installed but need to be updated
    /// via a re-install.
    pub components_to_update: Vec<Update>,
}

#[allow(clippy::derivable_impls)]
impl Default for InstallationOptions {
    fn default() -> Self {
        Self {
            verbose: false,
            components_to_update: Vec::new(),
        }
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
        InstallationOptions {
            verbose: value.verbose,
            components_to_update: Vec::new(),
        }
    }
}
