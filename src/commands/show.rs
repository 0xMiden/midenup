use clap::Subcommand;
use colored::Colorize;

use crate::{
    config::Config,
    manifest::Manifest,
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
    pub fn execute(&self, config: &Config, local_manifest: &Manifest) -> anyhow::Result<()> {
        match self {
            Self::Current { verbose } => {
                let (toolchain, justification) = Toolchain::current(config)?;

                if !verbose {
                    println!("{}", &toolchain.channel);
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
                    println!("The current active toolchain is {}", &toolchain.channel);
                }

                Ok(())
            },
            Self::Home => {
                println!("{}", config.midenup_home.display());

                Ok(())
            },
            Self::List => {
                let channels = local_manifest.get_channels();
                let stable_toolchain = config.manifest.get_latest_stable();

                let toolchains_display: Vec<_> = channels
                    .map(|channel| {
                        (
                            &channel.name,
                            stable_toolchain
                                .as_ref()
                                .is_some_and(|stable| stable.name == channel.name),
                        )
                    })
                    .map(|(channel_name, is_stable)| match (channel_name, is_stable) {
                        (name, false) => format!("{name}"),
                        (name, true) => format!("{name} {}", "(stable)".bold()),
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
