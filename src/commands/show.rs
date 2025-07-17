use anyhow::Context;
use clap::Subcommand;

use crate::{toolchain::Toolchain, Config};

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
    pub fn execute(&self, config: &Config) -> anyhow::Result<()> {
        match self {
            Self::Current => {
                let toolchain = Toolchain::current()?;

                println!("{}", &toolchain.channel);

                Ok(())
            },
            Self::Home => {
                println!("{}", config.midenup_home.display());

                Ok(())
            },
            Self::List => {
                let toolchains_dir = config.midenup_home.join("toolchains");
                let toolchains = std::fs::read_dir(toolchains_dir)
                    .context("Couldn't read toolchains directory")?;
                for toolchain in toolchains {
                    println!("{}", toolchain.unwrap().file_name().display());
                }

                Ok(())
            },
        }
    }
}
