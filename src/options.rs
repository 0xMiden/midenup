use std::collections::BTreeMap;

use clap::{Parser, ValueEnum};

use crate::{
    identity::InstallationId, manifest::Component, profile::Profile, resolve::Intent,
    toolchain::Patch,
};

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
    /// The identity and source of a named installation.
    #[arg(skip)]
    pub custom: Option<crate::identity::CustomToolchain>,
    /// The profile to install (default: minimal; the project's profile if CHANNEL is omitted)
    #[arg(long, short)]
    pub profile: Option<Profile>,
    /// Components to install in addition to the profile's members
    #[arg(long = "component", value_name = "COMPONENT")]
    pub components: Vec<String>,
    /// Components whose files must be re-acquired rather than carried forward from the previous
    /// publication.
    ///
    /// Empty for a fresh install: there is nothing to carry forward. An update fills it with the
    /// components it determined have actually changed. The install itself adds every component
    /// whose patch differs from the installed one.
    #[arg(skip)]
    pub stale: Vec<String>,
    /// Components to record exactly as they are already installed, rather than as upstream
    /// describes them.
    ///
    /// Only `--path-update=off`/`interactive` produces these. Recording the upstream definition
    /// for a component the user declined to update would mark it up to date without having
    /// rebuilt it, so the next update would stop offering.
    #[arg(skip)]
    pub held_back: Vec<Component>,
    /// How this installation affects the recorded selection.
    ///
    /// `None` means "derive a `Replace` from the profile and components given on the command
    /// line", which is what a direct `midenup install` wants. Callers that are not the CLI set
    /// this explicitly.
    #[arg(skip)]
    pub intent_update: Option<IntentUpdate>,
    /// The network the user named, which `toolchains/<network>` will point at the installed
    /// channel. `None` when a version was requested directly: no network link is written.
    #[arg(skip)]
    pub network: Option<String>,
    /// The toolchain file patches applied to the channel being installed.
    #[arg(skip)]
    pub patches: BTreeMap<String, Patch>,
}

impl InstallationOptions {
    /// The local identity being installed, independently of its upstream version.
    pub(crate) fn installation_id(&self, version: &semver::Version) -> InstallationId {
        match &self.custom {
            Some(custom) => InstallationId::Custom(custom.name.clone()),
            None => InstallationId::Version(version.clone()),
        }
    }
}

/// Optional update settings.
#[derive(Default, Debug, Parser, Clone, Copy)]
pub struct UpdateOptions {
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
    /// Rebuild the component from its source whenever the source or its definition changed
    All,
    /// Prompt the user to determine how to proceed
    Interactive,
}

impl From<InstallationOptions> for UpdateOptions {
    fn from(_value: InstallationOptions) -> Self {
        UpdateOptions::default()
    }
}

impl From<UpdateOptions> for InstallationOptions {
    fn from(_value: UpdateOptions) -> Self {
        InstallationOptions {
            custom: None,
            profile: None,
            components: Vec::new(),
            stale: Vec::new(),
            held_back: Vec::new(),
            // An update re-resolves what is already recorded; it does not restate intent.
            intent_update: Some(IntentUpdate::Preserve),
            network: None,
            patches: BTreeMap::new(),
        }
    }
}
