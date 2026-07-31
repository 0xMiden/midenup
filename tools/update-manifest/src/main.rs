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
    /// Set or clear a channel's alias
    ///
    /// This is how a channel is promoted: `clone-toolchain` never copies an alias, so a new channel
    /// holds no pointer until one is bound here. Binding `stable` moves it off whichever channel
    /// held it before.
    SetAlias {
        /// The channel to set the alias on
        #[arg(long, required(true), value_name = "CHANNEL", value_parser)]
        channel: channel::UserChannel,
        /// The alias to apply: `stable`, or an ad-hoc tag. Omit to clear the alias.
        #[arg(long, value_name = "ALIAS", value_parser)]
        alias: Option<channel::ChannelAlias>,
    },
    /// Point a network at a channel, or clear the channel's network
    SetNetwork {
        /// The channel to point the network at
        #[arg(long, required(true), value_name = "CHANNEL", value_parser)]
        channel: channel::UserChannel,
        /// The network this channel targets: `devnet`, `testnet`, `mainnet`. Omit to clear it.
        #[arg(long, value_name = "NETWORK", value_parser)]
        network: Option<channel::Network>,
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
        /// Profiles that should include this component by default
        #[arg(long = "profile", value_name = "PROFILE", value_parser)]
        profiles: Vec<Profile>,
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
        /// The component kind and associated metadata, as a JSON merge patch
        ///
        /// The parser is explicit because clap resolves `serde_json::Value` through its
        /// `From<String>` impl -- which yields `Value::String` -- before ever considering
        /// `FromStr`. A string patch then *replaces* the target under RFC 7386 rather than
        /// merging into it.
        #[arg(long, value_name = "SPEC", value_parser = parse_json_object)]
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

/// Parses a CLI argument as a JSON object.
fn parse_json_object(raw: &str) -> Result<serde_json::Value, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|err| format!("invalid JSON: {err}"))?;
    if !value.is_object() {
        return Err(format!(
            "expected a JSON object, got {}",
            match &value {
                serde_json::Value::Null => "null",
                serde_json::Value::Bool(_) => "a boolean",
                serde_json::Value::Number(_) => "a number",
                serde_json::Value::String(_) => "a string",
                serde_json::Value::Array(_) => "an array",
                serde_json::Value::Object(_) => unreachable!(),
            }
        ));
    }
    Ok(value)
}

fn main() -> ExitCode {
    use clap::FromArgMatches;

    let cli = <Cli as clap::CommandFactory>::command();
    let matches = cli.get_matches();
    let cli = Cli::from_arg_matches(&matches).map_err(|err| err.exit()).unwrap();

    match cli.execute() {
        Ok(_) => ExitCode::SUCCESS,
        Err(err) => {
            // `{err:#}` renders the whole context chain. Plain `{err}` prints only the outermost
            // message, which routinely reduced a precise diagnostic to "failed to write manifest".
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        },
    }
}

impl Cli {
    fn execute(&self) -> anyhow::Result<()> {
        let mut manifest = VersionedManifest::load_from_file(&self.manifest_path)?;
        match &self.command {
            Command::Check => {
                // Structural validation first: it reports every problem in one pass, which is
                // what an authoring tool should do. Loading the manifest does NOT run this --
                // see the module docs on src/manifest/validate.rs.
                if let Err(errors) = midenup::manifest::validate::validate_manifest(&manifest) {
                    for error in errors.iter() {
                        eprintln!("error: {error}");
                    }
                    bail!("manifest failed validation with {} error(s)", errors.len());
                }

                // Then confirm each channel actually resolves. Unlike the previous
                // `component_graph` call this topologically sorts, so requirement cycles are
                // detected -- `check` used to build the graph and never sort it.
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
                    UserChannel::Stable => {
                        bail!("cannot create a toolchain named 'stable'")
                    },
                    // Covers network names too: a channel is identified by its version, and the
                    // names it answers to are bound separately via `set-alias`/`set-network`.
                    UserChannel::Other(_) => {
                        bail!("target toolchain must be named by its semantic version")
                    },
                    UserChannel::Version(v) => v,
                };
                if manifest.get_channel_by_name(to).is_some() {
                    bail!("toolchain '{to}' already exists");
                }
                from.name = to.clone();
                // Don't clone aliases - that must be done separately, via `set-alias`. Nor the
                // network: a clone has not been deployed anywhere.
                from.alias = None;
                from.network = None;
                manifest.add_channel(from);
                manifest.update_last_modified();

                write_manifest(&manifest, &self.manifest_path)
            },
            Command::SetAlias { channel, alias } => {
                let name = manifest
                    .get_channel(channel)
                    .map(|c| c.name.clone())
                    .with_context(|| format!("unknown toolchain '{channel}'"))?;

                // Route through `add_channel` rather than mutating in place, so that the
                // "an alias names exactly one channel" bookkeeping applies here too: setting
                // `stable` on one channel clears it from whichever channel held it before.
                let mut updated = manifest
                    .get_channel_by_name(&name)
                    .cloned()
                    .expect("channel was just resolved by name");
                updated.alias = alias.clone();
                manifest.add_channel(updated);
                manifest.update_last_modified();

                write_manifest(&manifest, &self.manifest_path)
            },
            Command::SetNetwork { channel, network } => {
                let name = manifest
                    .get_channel(channel)
                    .map(|c| c.name.clone())
                    .with_context(|| format!("unknown toolchain '{channel}'"))?;

                let mut updated = manifest
                    .get_channel_by_name(&name)
                    .cloned()
                    .expect("channel was just resolved by name");
                updated.network = network.clone();
                manifest.add_channel(updated);
                manifest.update_last_modified();

                write_manifest(&manifest, &self.manifest_path)
            },
            Command::AddComponent {
                channel,
                name,
                authority,
                kind,
                profiles,
                requires,
            } => {
                // `legacy-package` exists only so that channels 0.9-0.15, which ship packages
                // extracted from Rust crates, remain installable. It is closed to new authoring:
                // packages are prebuilt from here on.
                if matches!(kind, ComponentKind::LegacyPackage { .. }) {
                    bail!(
                        "'legacy-package' is deprecated and closed to new channels - packages \
                         must ship prebuilt artifacts; use 'package' instead"
                    );
                }
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
                    profiles: profiles.clone(),
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

                // Removing a component that something else still requires would leave the channel
                // unresolvable, which install would only discover much later.
                let dependents: Vec<&str> = channel
                    .components
                    .iter()
                    .filter(|c| c.name != name.as_str() && c.requires.iter().any(|r| r == name))
                    .map(|c| c.name.as_ref())
                    .collect();
                if !dependents.is_empty() {
                    bail!(
                        "cannot remove component '{name}': it is still required by {}",
                        dependents.join(", ")
                    );
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
                if let Some(kind) = kind.clone() {
                    // `json_patch::merge(target, patch)` applies `patch` onto `target`. The
                    // existing value is the target and the user's partial is the patch -- the
                    // reverse silently discarded every requested change.
                    let mut merged = serde_json::to_value(component.kind.clone())?;
                    json_patch::merge(&mut merged, &kind);
                    match serde_json::from_value::<ComponentKind>(merged) {
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

/// Writes `manifest`, but only if it parses and validates once read back from disk.
///
/// A plain `fs::write` cannot do this: it commits the bytes before anything has confirmed they are
/// a usable manifest, so a mutation that produces something unparseable destroys the file it was
/// editing. Here the destination is replaced by a single rename, and every failure before that
/// leaves the original byte-for-byte intact.
fn write_manifest(manifest: &Manifest, manifest_path: &Path) -> anyhow::Result<()> {
    midenup::utils::atomic::write_validated(manifest_path, manifest, |written| {
        let reparsed = VersionedManifest::parse_str(written)
            .map_err(|err| format!("the result would not parse as a manifest: {err}"))?;
        midenup::manifest::validate::validate_manifest(&reparsed).map_err(|errors| {
            let detail: Vec<String> = errors.iter().map(|e| format!("\n  - {e}")).collect();
            format!("the result would not be a valid manifest:{}", detail.concat())
        })
    })
    .context("failed to write manifest")
}

fn write_manifest_to_stdout(manifest: &Manifest) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout();
    serde_json::to_writer_pretty(&mut stdout, manifest).context("failed to format manifest")
}
