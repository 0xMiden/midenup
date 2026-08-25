use clap::Subcommand;
use colored::Colorize;

use crate::{
    config::Config,
    report,
    state::LocalState,
    toolchain::{Toolchain, ToolchainJustification},
};

#[derive(Debug, Subcommand)]
pub enum ShowCommand {
    /// Show the active toolchain.
    ///
    /// Reports the channel alone; `-v` also reports why it is the active one.
    #[command(name = "active-toolchain")]
    Current,
    /// Display the computed value of MIDENUP_HOME
    Home,
    /// List installed toolchains
    List,
}

impl ShowCommand {
    pub fn execute(&self, config: &Config, state: &LocalState) -> anyhow::Result<()> {
        match self {
            Self::Current => {
                let (toolchain, justification) = Toolchain::current(config, None)?;

                if report::verbosity() < report::Verbosity::Verbose {
                    println!("{}", toolchain.channel);
                } else {
                    match justification {
                        ToolchainJustification::MidenToolchainFile { path } => {
                            println!(
                                "{}: found a miden-toolchain.toml file in {}",
                                "info".bold(),
                                path.display()
                            )
                        },
                        ToolchainJustification::Override => {
                            println!(
                                "{}: system default has been overridden via `midenup override`",
                                "info".bold(),
                            )
                        },
                        ToolchainJustification::Requested => {
                            println!("{}: explicitly requested by user", "info".bold(),)
                        },
                        ToolchainJustification::Default => {
                            println!("{}: current toolchain is system default", "info".bold());
                        },
                    }
                    println!("The current active toolchain is {}", toolchain.channel);
                }

                Ok(())
            },
            Self::Home => {
                println!("{}", config.midenup_home.display());

                Ok(())
            },
            Self::List => {
                // Installed toolchains are recorded locally, so this works with no network at all.
                // Upstream only adds *markers* -- which networks name a channel, which
                // installations are partial or no longer published -- so when it is unavailable
                // they are simply omitted rather than guessed at.
                let upstream = config.upstream_manifest().ok();

                // Check every `toolchains/<network>` links on this machine to compare with
                // upstream.
                let local_links = upstream
                    .map(|_| crate::networks::links(&config.midenup_home))
                    .unwrap_or_default();

                let toolchains_display: Vec<_> = state
                    .installations
                    .iter()
                    .map(|installation| {
                        let name = &installation.channel;
                        let mut line = format!("{name}");

                        // Several networks may name one channel, so this is a list rather than a
                        // single marker. Omitted entirely when upstream is unavailable: which
                        // networks name a channel is upstream's answer, never one derived from
                        // what happens to be on disk here.
                        let networks: Vec<&str> = upstream
                            .map(|manifest| manifest.networks_for(name).collect())
                            .unwrap_or_default();
                        if !networks.is_empty() {
                            line.push_str(&format!(
                                " {}",
                                format!("({})", networks.join(", ")).bold()
                            ));
                        }

                        // If a network whose link still names this channel while upstream has
                        // moved it, we show it to the user.
                        if let Some(manifest) = upstream {
                            for (network, linked) in &local_links {
                                if linked != name {
                                    continue;
                                }
                                if let Some(marker) = crate::networks::drift(
                                    network,
                                    linked,
                                    manifest.network_version(network),
                                ) {
                                    line.push_str(&format!(" {}", marker.yellow()));
                                }
                            }
                        }

                        // A migrated record describes a pre-publication tree that no receipt
                        // covers, so midenup will not execute against it. Saying so is the whole
                        // point: the user's toolchain still works, but only after it is installed
                        // properly, and they should not have to infer that from a failure.
                        if !installation.is_managed() {
                            line.push_str(&format!(
                                " {} -- run `midenup install {name}`",
                                "(needs reinstallation)".yellow()
                            ));
                        }

                        if let Some(manifest) = upstream {
                            match manifest.get_channel_by_name(name) {
                                Some(channel)
                                    if installation
                                        .as_channel()
                                        .is_partially_installed(channel) =>
                                {
                                    line.push_str(&format!(
                                        " {}",
                                        "(partially installed)".yellow()
                                    ));
                                },
                                Some(_) => {},
                                // Retained, not deleted: the user may still want `var/` and an
                                // explicit uninstall (spec section 12.3).
                                None => line
                                    .push_str(&format!(" {}", "(unavailable upstream)".yellow())),
                            }
                        }

                        line
                    })
                    .collect();

                println!("{}", "Installed toolchains:".bold().underline());
                for toolchain in toolchains_display {
                    println!("{toolchain}");
                }

                Ok(())
            },
        }
    }
}
