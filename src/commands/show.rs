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
                let stable_toolchain = config.manifest.get_latest_stable();

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

                        // Retained, not deleted: the user may still want `var/` and an explicit
                        // uninstall (spec section 12.3).
                        if config.manifest.get_channel_by_name(name).is_none() {
                            line.push_str(&format!(" {}", "(unavailable upstream)".yellow()));
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
