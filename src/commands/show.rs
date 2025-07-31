use clap::Subcommand;
use colored::Colorize;

use crate::{Config, manifest::Manifest, toolchain::Toolchain};

#[derive(Debug, Subcommand)]
pub enum ShowCommand {
    /// Show the active toolchain
    #[command(name = "active-toolchain")]
    Current,
    /// Display the computed value of MIDENUP_HOME
    Home,
    /// List installed toolchains
    List,
}

impl ShowCommand {
    pub fn execute(&self, config: &Config, local_manifest: &Manifest) -> anyhow::Result<()> {
        match self {
            Self::Current => {
                let toolchain = Toolchain::current(config)?;

                println!("{}", &toolchain.channel);

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
