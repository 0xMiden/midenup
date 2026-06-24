use clap::{Parser, ValueEnum};

use crate::channel::Component;

pub const DEFAULT_USER_DATA_DIR: &str = "XDG_DATA_HOME";

/// Optional installation settings.
#[derive(Default, Debug, Parser, Clone)]
pub struct InstallationOptions {
    /// Displays the entirety of cargo's output when performing installations.
    #[clap(long, short, default_value = "false")]
    pub verbose: bool,
    /// These are the components that will be uninstalled before re-installation.
    #[clap(skip)]
    pub components_to_uninstall: Vec<Component>,
}

/// Optional update settings.
#[derive(Default, Debug, Parser, Clone, Copy)]
pub struct UpdateOptions {
    /// Displays the entirety of cargo's output when performing installations.
    #[clap(long, short, default_value = "false")]
    pub verbose: bool,
    /// Determines how midenup will handle updates for components installed from a path
    #[clap(value_enum, short, long, default_value = "off")]
    pub path_update: PathUpdate,
}

/// Represents the behavior chosen when a component being updated was installed from a path
#[derive(Default, Debug, Parser, Clone, Copy, ValueEnum)]
pub enum PathUpdate {
    /// Skip updating the component
    #[default]
    Off,
    /// Force the component to be updated
    ///
    /// TODO(pauls): Clarify the semantics of what this option does
    All,
    /// Prompt the user to determine how to proceed
    Interactive,
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
            components_to_uninstall: Vec::new(),
        }
    }
}
