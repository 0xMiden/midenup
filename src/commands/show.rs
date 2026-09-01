use clap::Subcommand;
use colored::Colorize;

use super::Flags;
use crate::{
    config::Config,
    report,
    state::LocalState,
    toolchain::{Toolchain, ToolchainJustification},
};

#[derive(Debug, Subcommand)]
pub enum ShowCommand {
    /// Show the active toolchain.
    #[command(name = "active-toolchain")]
    Current {
        #[clap(flatten)]
        flags: Flags,
    },
    /// Display the computed value of MIDENUP_HOME
    Home,
    /// List installed toolchains
    List {
        #[clap(flatten)]
        flags: Flags,
    },
}

impl ShowCommand {
    pub fn execute(&self, config: &Config, state: &LocalState) -> anyhow::Result<()> {
        use core::fmt::Write;

        match self {
            Self::Current { .. } => {
                let (toolchain, justification) = Toolchain::current(config, None)?;

                // The justification is commentary, not the result, so it goes to stderr.
                if report::verbosity() >= report::Verbosity::Debug {
                    match justification {
                        ToolchainJustification::MidenToolchainFile { path } => {
                            crate::info!("found a miden-toolchain.toml file in {}", path.display())
                        },
                        ToolchainJustification::Override => {
                            crate::info!(
                                "system default has been overridden via `midenup override`"
                            )
                        },
                        ToolchainJustification::Requested => {
                            crate::info!("explicitly requested by user")
                        },
                        ToolchainJustification::Default => {
                            crate::info!("current toolchain is system default")
                        },
                    }
                }
                println!("{}", toolchain.channel);

                Ok(())
            },
            Self::Home => {
                println!("{}", config.midenup_home.display());

                Ok(())
            },
            Self::List { .. } => {
                // Installed toolchains are recorded locally, so this works with no network at all.
                // Upstream only adds *markers* -- which networks name a channel, which
                // installations are partial or no longer published -- so when it is unavailable
                // they are simply omitted rather than guessed at.
                let upstream = config.upstream_manifest().ok();
                // The upstream lookup may have emitted a report to stderr, so restore stdout's
                // color policy immediately before rendering the result.
                let use_color = report::prepare_stdout_color();

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
                            if use_color {
                                write!(
                                    &mut line,
                                    " {}",
                                    format!("({})", networks.join(", ")).bold()
                                )
                                .unwrap();
                            } else {
                                write!(&mut line, " ({})", networks.join(", ")).unwrap();
                            }
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
                                    if use_color {
                                        write!(&mut line, " {}", marker.yellow()).unwrap();
                                    } else {
                                        line.push(' ');
                                        line.push_str(&marker);
                                    }
                                }
                            }
                        }

                        // A migrated record describes a pre-publication tree that no receipt
                        // covers, so midenup will not execute against it. Saying so is the whole
                        // point: the user's toolchain still works, but only after it is installed
                        // properly, and they should not have to infer that from a failure.
                        if !installation.is_managed() {
                            if use_color {
                                write!(&mut line, " {}", "(needs reinstallation)".yellow())
                                    .unwrap();
                            } else {
                                line.push_str(" (needs reinstallation)");
                            }
                            write!(&mut line, " -- run `midenup install {name}`").unwrap();
                        }

                        if let Some(manifest) = upstream {
                            match manifest.get_channel_by_name(name) {
                                Some(channel)
                                    if installation
                                        .as_channel()
                                        .is_partially_installed(channel) =>
                                {
                                    if use_color {
                                        write!(&mut line, " {}", "(partially installed)".yellow())
                                            .unwrap();
                                    } else {
                                        line.push_str(" (partially installed)");
                                    }
                                },
                                Some(_) => {},
                                // Retained, not deleted: the user may still want `var/` and an
                                // explicit uninstall (spec section 12.3).
                                None if use_color => {
                                    write!(&mut line, " {}", "(unavailable upstream)".yellow())
                                        .unwrap()
                                },
                                None => line.push_str(" (unavailable upstream)"),
                            }
                        }

                        line
                    })
                    .collect();

                if use_color {
                    println!("{}", "Installed toolchains:".bold().underline());
                } else {
                    println!("Installed toolchains:");
                }
                for toolchain in toolchains_display {
                    println!("{toolchain}");
                }

                Ok(())
            },
        }
    }
}
