use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use anyhow::{Context, bail};
use clap::{Parser, Subcommand, builder::ArgPredicate};
use midenup::{
    channel::{self, UserChannel},
    manifest::{Component, ComponentKind, Manifest, VersionedManifest},
    profile::Profile,
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
    Format {
        /// Writes the formatted manifest to stdout, rather than rewriting the file
        #[arg(long, default_value_t = false)]
        stdout: bool,
    },
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
        /// The component kind and associated metadata
        #[arg(long, value_name = "SPEC", value_parser)]
        kind: ComponentKind,
        /// Specify other components this component implicitly requires
        #[arg(long, value_delimiter = ',', value_name = "VERSION")]
        requires: Vec<String>,
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
        /// The component kind and associated metadata
        #[arg(long, value_name = "SPEC", value_parser)]
        kind: Option<serde_json::Value>,
        /// Adds profiles that should include this component by default
        #[arg(long, value_name = "SPEC", value_parser)]
        profiles: Vec<Profile>,
        #[arg(
            hide(true),
            long,
            default_value = "true",
            default_value_if("profiles", ArgPredicate::IsPresent, Some("false"))
        )]
        keep_existing_profiles: bool,
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
        let mut manifest = VersionedManifest::load_from_file(&self.manifest_path)?;
        match &self.command {
            Command::Check => {
                // Resolve every channel in full. Unlike the previous `component_graph` call, this
                // topologically sorts the result, so a requirement cycle is actually detected --
                // `check` used to build the graph and never sort it, and so accepted cycles.
                for channel in manifest.get_channels() {
                    midenup::resolve::resolve(
                        channel,
                        &midenup::resolve::Intent::new(&[Profile::Complete], &[]),
                    )
                    .with_context(|| format!("channel {} is not installable", channel.name))?;
                }
                Ok(())
            },
            Command::Format { stdout: false } => write_manifest(&manifest, &self.manifest_path),
            Command::Format { stdout: true } => write_manifest_to_stdout(&manifest),
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
            Command::AddComponent { channel, name, authority, kind, requires } => {
                let Some(channel) = manifest.get_channel_mut(channel) else {
                    bail!("unknown toolchain '{channel}'")
                };
                if channel.get_component(name.as_str()).is_some() {
                    bail!(
                        "component '{name}' already exists for toolchain '{}' - use \
                         update-component to modify it",
                        channel.name
                    );
                }
                let component = Component {
                    name: name.clone().into(),
                    version: authority.clone(),
                    kind: kind.clone(),
                    profiles: vec![],
                    requires: requires.clone(),
                    artifacts: Default::default(),
                    // A newly authored component has no fields this build does not understand.
                    extra: Default::default(),
                };
                for required in requires {
                    if channel.get_component(required).is_none() {
                        bail!(
                            "cannot require componennt '{required}': unknown component for \
                             toolchain '{}'",
                            channel.name
                        );
                    }
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
                    bail!("unknown component '{name}' for toolchain '{}'", channel.name);
                }
                channel.components.retain_mut(|c| c.name != name.as_str());
                manifest.update_last_modified();
                write_manifest(&manifest, &self.manifest_path)
            },
            Command::UpdateComponent {
                channel,
                name,
                authority,
                kind,
                profiles,
                keep_existing_profiles,
                requires,
                keep_existing_requires,
            } => {
                let Some(channel) = manifest.get_channel_mut(channel) else {
                    bail!("unknown toolchain '{channel}'")
                };
                for required in requires {
                    if channel.get_component(required).is_none() {
                        bail!(
                            "cannot require componennt '{required}': unknown component for \
                             toolchain '{}'",
                            channel.name
                        );
                    }
                }
                let Some(component) = channel.get_component_mut(name.as_str()) else {
                    bail!(
                        "unknown component '{name}' for toolchain '{}' - use add-component to \
                         create it",
                        channel.name
                    );
                };
                component.version = authority.clone();
                if !*keep_existing_profiles {
                    component.profiles = profiles.clone();
                }
                if !*keep_existing_requires {
                    component.requires = requires.clone();
                }
                if let Some(mut kind) = kind.clone() {
                    let prev = serde_json::to_value(component.kind.clone())?;
                    json_patch::merge(&mut kind, &prev);
                    match serde_json::from_value::<ComponentKind>(kind) {
                        Ok(merged) => {
                            component.kind = merged;
                        },
                        Err(err) => {
                            bail!(
                                "invalid component update: modified json failed to parse with: \
                                 {err}"
                            );
                        },
                    }
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

fn write_manifest_to_stdout(manifest: &Manifest) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout();
    serde_json::to_writer_pretty(&mut stdout, manifest).context("failed to format manifest")
}
