use clap::{Parser, ValueEnum};

use crate::{manifest::Component, profile::Profile, resolve::Intent};

/// How an installation affects the selection persisted for a channel.
///
/// Installing and *recording what the user wants* are separate concerns. A toolchain-file
/// activation installs a narrowed set but must only ever add to the recorded selection; a direct
/// install records exactly what was asked for, and is allowed to shrink it.
#[derive(Debug, Clone)]
pub enum IntentUpdate {
    /// Replace the recorded selection. A direct `midenup install`.
    Replace(Intent),
    /// Merge into the recorded selection, never removing. Toolchain-file activation.
    Union(Intent),
    /// Leave the recorded selection alone. An update re-resolves existing intent rather than
    /// restating it.
    Preserve,
}

pub const DEFAULT_USER_DATA_DIR: &str = "XDG_DATA_HOME";

/// Optional installation settings.
#[derive(Default, Debug, Parser, Clone)]
pub struct InstallationOptions {
    /// The toolchain profile to install
    #[arg(long, short, default_value = "minimal")]
    pub profile: Profile,
    /// Displays the entirety of cargo's output when performing installations.
    #[arg(long, short, default_value = "false")]
    pub verbose: bool,
    /// Components to install in addition to the profile's members
    #[arg(long = "component", value_name = "COMPONENT")]
    pub components: Vec<String>,
    /// These are the components that will be uninstalled before re-installation.
    #[arg(skip)]
    pub components_to_uninstall: Vec<Component>,
    /// How this installation affects the recorded selection.
    ///
    /// `None` means "derive a `Replace` from the profile and components given on the command
    /// line", which is what a direct `midenup install` wants. Callers that are not the CLI set
    /// this explicitly.
    #[arg(skip)]
    pub intent_update: Option<IntentUpdate>,
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
            profile: Profile::Minimal,
            verbose: value.verbose,
            components: Vec::new(),
            components_to_uninstall: Vec::new(),
            // An update re-resolves what is already recorded; it does not restate intent.
            intent_update: Some(IntentUpdate::Preserve),
        }
    }
}
