use clap::Subcommand;
use colored::Colorize;

use crate::{
    config::Config,
    state::LocalState,
    toolchain::{Toolchain, ToolchainJustification},
};

#[derive(Debug, Subcommand)]
pub enum ShowCommand {
    /// Show the active toolchain
    #[command(name = "active-toolchain")]
    Current {
        #[arg(long, action)]
        verbose: bool,
    },
    /// Display the computed value of MIDENUP_HOME
    Home,
    /// List installed toolchains
    List,
}

impl ShowCommand {
    pub fn execute(&self, config: &Config, state: &LocalState) -> anyhow::Result<()> {
        match self {
            Self::Current { verbose } => {
                let (toolchain, justification) = Toolchain::current(config)?;

                if !verbose {
                    println!("{}", toolchain.channel);
                } else {
                    match justification {
                        ToolchainJustification::MidenToolchainFile { path } => {
                            println!(
                                "{}: found a miden-toolchain.toml file in {}",
                                "info".white().bold(),
                                path.display()
                            )
                        },
                        ToolchainJustification::Override => {
                            println!(
                                "{}: system default has been overridden via `midenup override`",
                                "info".white().bold(),
                            )
                        },
                        ToolchainJustification::Default => {
                            println!(
                                "{}: current toolchain is system default",
                                "info".white().bold()
                            );
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
                // Upstream only adds *markers* -- which channel is stable, which installations are
                // partial or no longer published -- so when it is unavailable they are simply
                // omitted rather than guessed at (spec section 8.6).
                let upstream = config.upstream_manifest().ok();
                let stable_toolchain = upstream.and_then(|manifest| manifest.get_latest_stable());

                let toolchains_display: Vec<_> = state
                    .installations
                    .iter()
                    .map(|installation| {
                        let name = &installation.channel;
                        let mut line = format!("{name}");

                        if stable_toolchain.as_ref().is_some_and(|stable| &stable.name == name) {
                            line.push_str(&format!(" {}", "(stable)".bold()));
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
