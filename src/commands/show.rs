use clap::Subcommand;

use crate::{Config, toolchain::Toolchain};

#[derive(Debug, Subcommand)]
pub enum ShowCommand {
    /// Show the active toolchain
    #[command(name = "active-toolchain")]
    Current,
    /// Display the computed value of MIDENUP_HOME
    Home,
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
        }
    }
}
