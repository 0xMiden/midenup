use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand, builder::ArgPredicate};
use midenup::{
    channel::{self, Component, UserChannel},
    manifest::Manifest,
    version::Authority,
};

#[derive(Debug, Parser)]
#[command(
    name = "update-manifest",
    author,
    about = "Modify channel-manifest.json safely",
    long_about = None,
    arg_required_else_help(true)
)]
pub struct Cli {
    #[arg(long, required(true), value_name = "PATH")]
    manifest_path: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check that the manifest is valid
    Check,
    /// Format the manifest
    Format,
    /// Updates the timestamp of the manifest to the current time in UTC
    Touch,
    /// Clone the a toolchain to a new toolchain for further modification
    CloneToolchain {
        /// The channel to clone
        #[arg(long, required(true), value_name = "CHANNEL", value_parser)]
        from: channel::UserChannel,
        /// The name of the channel that will be created
        #[arg(long, required(true), value_name = "CHANNEL", value_parser)]
        to: channel::UserChannel,
    },
    /// Add a component to a toolchain
    AddComponent {
        /// The channel to add this component to
        #[arg(long, required(true), value_name = "CHANNEL", value_parser)]
        channel: channel::UserChannel,
        /// The name of the component to add
        #[arg(required(true), value_name = "NAME")]
        name: String,
        /// The version/authority of the new component
        #[arg(long, value_name = "SPEC", value_parser)]
        authority: Authority,
        /// If provided, sets the rustup channel required by this component
        #[arg(long, value_name = "VERSION")]
        rustup_channel: Option<String>,
        /// The set of other components implicitly required by this component
        #[arg(long, value_delimiter = ',', value_name = "VERSION")]
        requires: Vec<String>,
        /// The set of Cargo features required to build/install this component
        #[arg(long, value_delimiter = ',', value_name = "VERSION")]
        features: Vec<String>,
    },
    /// Remove a component from a toolchain
    RemoveComponent {
        /// The channel to remove the component from
        #[arg(long, required(true), value_name = "CHANNEL", value_parser)]
        channel: channel::UserChannel,
        /// The name of the component to remove
        #[arg(required(true), value_name = "NAME")]
        name: String,
    },
    UpdateComponent {
        /// The channel in which to find the component being updated
        #[arg(long, required(true), value_name = "CHANNEL", value_parser)]
        channel: channel::UserChannel,
        /// The name of the component to update
        #[arg(required(true), value_name = "NAME")]
        name: String,
        /// Updates the version/authority of the component
        #[arg(long, value_name = "SPEC", value_parser)]
        authority: Authority,
        /// Marks this component as optional
        #[arg(long, value_name = "SPEC", value_parser)]
        optional: Option<bool>,
        /// Adds other components as implicitly required by this component
        #[arg(long, value_delimiter = ',', value_name = "VERSION")]
        requires: Vec<String>,
        #[arg(
            hide(true),
            long,
            default_value = "true",
            default_value_if("requires", ArgPredicate::IsPresent, Some("false"))
        )]
        keep_existing_requires: bool,
        /// Adds Cargo features required to build/install this component
        #[arg(long, value_delimiter = ',', value_name = "VERSION")]
        features: Vec<String>,
        #[arg(
            hide(true),
            long,
            default_value = "true",
            default_value_if("features", ArgPredicate::IsPresent, Some("false"))
        )]
        keep_existing_features: bool,
    },
}

fn main() -> ExitCode {
    use clap::FromArgMatches;

    let cli = <Cli as clap::CommandFactory>::command();
    let matches = cli.get_matches();
    let cli = Cli::from_arg_matches(&matches).map_err(|err| err.exit()).unwrap();

    match cli.execute() {
        Ok(_) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        },
    }
}

impl Cli {
    fn execute(&self) -> anyhow::Result<()> {
        let mut manifest = Manifest::load_from_file(&self.manifest_path)?;
        match &self.command {
            Command::Check => Ok(()),
            Command::Format => write_manifest(&manifest, &self.manifest_path),
            Command::Touch => {
                manifest.update_last_modified();
                write_manifest(&manifest, &self.manifest_path)
            },
            Command::CloneToolchain { from, to } => {
                let Some(mut from) = manifest.get_channel(from).cloned() else {
                    bail!("unknown source toolchain '{from}'")
                };
                let to = match to {
                    UserChannel::Stable | UserChannel::Nightly => {
                        bail!("cannot create toolchains named 'stable' or 'nightly'")
                    },
                    UserChannel::Other(_) => {
                        bail!("target toolchain must be named by its semantic version")
                    },
                    UserChannel::Version(v) => v,
                };
                if manifest.get_channel_by_name(to).is_some() {
                    bail!("toolchain '{to}' already exists");
                }
                from.name = to.clone();
                // Don't clone aliases - that must be done separately
                from.alias = None;
                manifest.add_channel(from);
                manifest.update_last_modified();

                write_manifest(&manifest, &self.manifest_path)
            },
            Command::AddComponent {
                channel,
                name,
                authority,
                rustup_channel,
                requires,
                features,
            } => {
                let Some(channel) = manifest.get_channel_mut(channel) else {
                    bail!("unknown toolchain '{channel}'")
                };
                if channel.get_component(name.as_str()).is_some() {
                    bail!(
                        "component '{name}' already exists for toolchain '{}' - use \
                         update-component to modify it",
                        &channel.name
                    );
                }
                let mut component = Component::new(name.clone(), authority.clone());
                component.rustup_channel = rustup_channel.clone();
                component.optional = true;
                component.features = features.clone();
                for required in requires {
                    if channel.get_component(required).is_none() {
                        bail!(
                            "cannot require componennt '{required}': unknown component for \
                             toolchain '{}'",
                            &channel.name
                        );
                    }
                    component.requires.push(required.clone());
                }
                channel.components.push(component);
                manifest.update_last_modified();
                write_manifest(&manifest, &self.manifest_path)
            },
            Command::RemoveComponent { channel, name } => {
                let Some(channel) = manifest.get_channel_mut(channel) else {
                    bail!("unknown toolchain '{channel}'")
                };
                if channel.get_component(name.as_str()).is_none() {
                    bail!("unknown component '{name}' for toolchain '{}'", &channel.name);
                }
                channel.components.retain_mut(|c| c.name != name.as_str());
                manifest.update_last_modified();
                write_manifest(&manifest, &self.manifest_path)
            },
            Command::UpdateComponent {
                channel,
                name,
                authority,
                optional,
                requires,
                features,
                keep_existing_requires,
                keep_existing_features,
            } => {
                let Some(channel) = manifest.get_channel_mut(channel) else {
                    bail!("unknown toolchain '{channel}'")
                };
                for required in requires {
                    if channel.get_component(required).is_none() {
                        bail!(
                            "cannot require componennt '{required}': unknown component for \
                             toolchain '{}'",
                            &channel.name
                        );
                    }
                }
                let Some(component) = channel.get_component_mut(name.as_str()) else {
                    bail!(
                        "unknown component '{name}' for toolchain '{}' - use add-component to \
                         create it",
                        &channel.name
                    );
                };
                let prev_version = match &component.version {
                    Authority::Cargo { version, .. } => Some(version.clone()),
                    _ => None,
                };
                let version = match authority {
                    Authority::Cargo { version, .. } => Some(version.clone()),
                    _ => None,
                };
                component.version = authority.clone();
                if let Some(prev_version) = prev_version.as_ref()
                    && let Some(version) = version.as_ref()
                    && let Some(artifacts) = component.artifacts.as_mut()
                {
                    artifacts.replace_version(prev_version, version);
                } else if prev_version.is_some() {
                    component.artifacts = None;
                }
                if let Some(optional) = *optional {
                    component.optional = optional;
                }
                if !*keep_existing_features {
                    component.features = features.clone();
                }
                if !*keep_existing_requires {
                    component.requires = requires.clone();
                }
                manifest.update_last_modified();
                write_manifest(&manifest, &self.manifest_path)
            },
        }
    }
}

fn write_manifest(manifest: &Manifest, manifest_path: &Path) -> anyhow::Result<()> {
    let formatted = serde_json::to_vec_pretty(manifest).context("failed to format manifest")?;
    std::fs::write(manifest_path, formatted).context("failed to write manifest")
}
