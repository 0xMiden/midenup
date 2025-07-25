use std::{ffi::OsString, str::FromStr};

use anyhow::{anyhow, bail, Context};
use colored::Colorize;

use crate::{commands::INSTALLABLE_COMPONENTS, manifest::Manifest, toolchain::Toolchain, Config};

#[derive(Debug)]
/// Miden Components managed by Midenup
enum MidenComponents {
    /// Standard Library in .masp format
    Std,
    /// Base Library/Transaction Kernel in .masp format
    Base,
    /// Miden Client (executable)
    Client,
    /// Miden VM (executable)
    VM,
    /// Miden Compiler (executable)
    Compiler,
    /// Miden Compiler Cargo extension (executable)
    CargoMiden,
}

impl MidenComponents {
    fn help_command(&self) -> HelpMessage {
        match self {
            MidenComponents::Std => {
                // Taken from: https://github.com/0xMiden/miden-vm?tab=readme-ov-file#project-structure
                let help_message = String::from(
                    "The Miden standard library in masp format.\
                         Provides highly-optimized and battle-tested implementations of commonly-used primitives.",
                );
                HelpMessage::Internal { help_message }
            },
            MidenComponents::Base => {
                // Taken from: https://github.com/0xMiden/miden-base?tab=readme-ov-file#project-structure
                let help_message = String::from(
                    "The Miden base library in masp format.\
                        Contains the code of the Miden rollup kernels and standardized smart contracts.",
                );
                HelpMessage::Internal { help_message }
            },
            MidenComponents::Client => HelpMessage::ShellOut {
                target_exe: String::from("miden-client"),
                prefix_args: vec![String::from("help")],
            },
            MidenComponents::VM => HelpMessage::ShellOut {
                target_exe: String::from("miden-vm"),
                prefix_args: vec![String::from("help")],
            },
            MidenComponents::Compiler => HelpMessage::ShellOut {
                target_exe: String::from("midenc"),
                prefix_args: vec![String::from("help")],
            },
            MidenComponents::CargoMiden => HelpMessage::ShellOut {
                target_exe: String::from("cargo-miden"),
                prefix_args: vec![String::from("help")],
            },
        }
    }
}

impl FromStr for MidenComponents {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "std" => Ok(MidenComponents::Std),
            "base" => Ok(MidenComponents::Base),
            "client" => Ok(MidenComponents::Client),
            "vm" => Ok(MidenComponents::VM),
            "compiler" | "midenc" => Ok(MidenComponents::Compiler),
            "cargo-miden" | "cargomiden" | "cargo" => Ok(MidenComponents::CargoMiden),
            _ => bail!("Unknown component {s}"),
        }
    }
}

#[derive(Debug)]
/// Enum of all the known "aliases". These are subcommands that have
/// "abbreviated" versions; these are then mapped to the corresponding "full"
/// command.
enum MidenAliasses {
    Account,
    Faucet,
    New,
    Build,
    Test,
    // Node,
    Deploy,
    // Scan,
    Call,
    Send,
    Simulate,
}

impl FromStr for MidenAliasses {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "account" => Ok(MidenAliasses::Account),
            "faucet" => Ok(MidenAliasses::Faucet),
            "new" => Ok(MidenAliasses::New),
            "build" => Ok(MidenAliasses::Build),
            "test" => Ok(MidenAliasses::Test),
            "deploy" => Ok(MidenAliasses::Deploy),
            "call" => Ok(MidenAliasses::Call),
            "send" => Ok(MidenAliasses::Send),
            "simulate" => Ok(MidenAliasses::Simulate),
            _ => bail!("Unknown subcommand {s}"),
        }
    }
}

impl MidenAliasses {
    fn help_command(&self) -> HelpMessage {
        match self {
            MidenAliasses::Account => HelpMessage::ShellOut {
                target_exe: String::from("miden-client"),
                prefix_args: vec![String::from("account"), String::from("--help")],
            },
            MidenAliasses::Faucet => HelpMessage::ShellOut {
                target_exe: String::from("miden-client"),
                prefix_args: vec![String::from("mint"), String::from("--help")],
            },
            MidenAliasses::New => HelpMessage::ShellOut {
                target_exe: String::from("cargo"),
                prefix_args: vec![
                    String::from("miden"),
                    String::from("new"),
                    String::from("--help"),
                ],
            },
            MidenAliasses::Build => HelpMessage::ShellOut {
                target_exe: String::from("cargo"),
                prefix_args: vec![
                    String::from("miden"),
                    String::from("build"),
                    String::from("--help"),
                ],
            },
            MidenAliasses::Test => todo!(),
            // NOTE: This help message displays help for every flag.
            // Maybe return a filter lambda to parse these messages?
            MidenAliasses::Deploy => HelpMessage::ShellOut {
                target_exe: String::from("miden-client"),
                prefix_args: vec![String::from("new-wallet"), String::from("--help")],
            },
            // NOTE: This help message displays help for every flag.
            // Maybe return a filter lambda to parse these messages?
            MidenAliasses::Call => HelpMessage::ShellOut {
                target_exe: String::from("miden-client"),
                prefix_args: vec![String::from("new-wallet"), String::from("--help")],
            },

            MidenAliasses::Send => HelpMessage::ShellOut {
                target_exe: String::from("miden-client"),
                prefix_args: vec![String::from("send"), String::from("--help")],
            },
            MidenAliasses::Simulate => HelpMessage::ShellOut {
                target_exe: String::from("miden-client"),
                prefix_args: vec![String::from("exec"), String::from("--help")],
            },
        }
    }

    /// Get the corresponding executable target executable and prefix arguments
    /// for each known [MidenCommands]. These can later be used in a subshell to
    /// execute the underlying component.
    fn get_command_exec(&self) -> (String, Vec<String>) {
        match self {
            MidenAliasses::Account => (String::from("miden-client"), vec![String::from("account")]),
            MidenAliasses::Faucet => (String::from("miden-client"), vec![String::from("mint")]),
            MidenAliasses::New => {
                (String::from("cargo"), vec![String::from("miden"), String::from("new")])
            },
            MidenAliasses::Build => {
                (String::from("cargo"), vec![String::from("miden"), String::from("build")])
            },
            MidenAliasses::Test => todo!(),
            MidenAliasses::Deploy => (
                String::from("miden-client"),
                vec![String::from("new-wallet"), String::from("--deploy")],
            ),
            MidenAliasses::Call => {
                (String::from("miden-client"), vec![String::from("account"), String::from("-s")])
            },
            MidenAliasses::Send => (String::from("miden-client"), vec![String::from("send")]),
            MidenAliasses::Simulate => (String::from("miden-client"), vec![String::from("exec")]),
        }
    }
}

/// This is used to encapsulate the different mechanisms used to display a help
/// messgage. Currently, there are only two.
enum HelpMessage {
    /// This variant is used when the display message is obtained by shelling
    /// out to a miden component. For instance: `miden-client account --help`.
    ShellOut {
        target_exe: String,
        prefix_args: Vec<String>,
    },
    /// This other variant is used when shelling out to the shell is not
    /// possible. This is mainly done to display the help message of:
    /// - The `.masp` libraries
    /// - `miden`'s own 'help' message
    Internal { help_message: String },
}

pub fn execute_miden(
    argv: Vec<OsString>,
    config: &Config,
    local_manifest: &mut Manifest,
) -> anyhow::Result<()> {
    // Extract the target binary to execute from argv[1]
    let subcommand = {
        let subcommand = argv.get(1).ok_or(anyhow!(
            "No arguments were passed to `miden`. To get a list of available commands, run:
miden help"
        ))?;
        subcommand.to_str().expect("Invalid command name: {subcommand}")
    };
    let aliased_command = MidenAliasses::from_str(subcommand);

    let (target_exe, prefix_args, include_rest_of_args) = match aliased_command.ok() {
        // These are know miden aliasses.
        Some(alias) => {
            let (target_exe, prefix_args) = alias.get_command_exec();
            (target_exe, prefix_args, true)
        },
        None => {
            if subcommand == "help" {
                // NOTE: This could either be a [MidenCommands] or a
                // [MidenComponents].
                let component = argv.get(2).and_then(|c| c.to_str());
                let help_message = handle_help(component)?;
                match help_message {
                    HelpMessage::Internal { help_message } => {
                        std::println!("{help_message}");
                        return Ok(());
                    },
                    HelpMessage::ShellOut { target_exe, prefix_args } => {
                        (target_exe, prefix_args, false)
                    },
                }
            } else {
                let command = match subcommand {
                    "client" => "miden-client",
                    "vm" => "miden-vm",
                    "midenc" | "compiler" => "midenc",
                    "cargo-miden" | "cargo" => "cargo-miden",
                    other => {
                        bail!(
                            "Unrecognized command {other}. To see available commands, run:
miden help"
                        )
                    },
                };
                (command.to_string(), vec![], true)
            }
        },
    };

    // Make sure we know the current toolchain so we can modify the PATH appropriately
    let toolchain = Toolchain::ensure_current_is_installed(&config, local_manifest)?;

    // Compute the effective PATH for this command
    let toolchain_bin = config
        .midenup_home
        .join("toolchains")
        .join(toolchain.channel.to_string())
        .join("bin");
    let path = match std::env::var_os("PATH") {
        Some(prev_path) => {
            let mut path = OsString::from(format!("{}:", toolchain_bin.display()));
            path.push(prev_path);
            path
        },
        None => toolchain_bin.into_os_string(),
    };

    let rest_of_args = if include_rest_of_args {
        argv.iter().skip(2)
    } else {
        // We don't want to pass the rest of the CLI arguments to the subshell in this case.
        // This is equivalent to std::iter::empty::<OsString>()
        argv.iter().skip(argv.len())
    };

    let mut output = std::process::Command::new(target_exe)
        .env("MIDENUP_HOME", &config.midenup_home)
        .env("PATH", path)
        .args(prefix_args)
        .args(rest_of_args)
        .stderr(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .spawn()
        .with_context(|| format!("failed to run 'miden {subcommand}'"))?;

    let status = output.wait().with_context(|| {
        format!("error occurred while waiting for 'miden {subcommand}' to finish executing")
    })?;

    if status.success() {
        Ok(())
    } else {
        bail!("'miden {}' failed with status {}", subcommand, status.code().unwrap_or(1))
    }
}

/// Wrapper function that handles help messaging dispatch
fn handle_help(component: Option<&str>) -> anyhow::Result<HelpMessage> {
    if let Some(component) = component {
        if let Ok(component) = MidenComponents::from_str(component) {
            Ok(component.help_command())
        } else if let Ok(command) = MidenAliasses::from_str(component) {
            Ok(command.help_command())
        } else {
            bail!(
                "Unrecognized command {}. To see available commands, run:
miden help",
                component
            )
        }
    } else {
        Ok(HelpMessage::Internal { help_message: default_help() })
    }
}

fn default_help() -> String {
    // Note:
    let aliasses: String = [
        "account", "faucet", "new", "build", "test", "deploy", "call", "send", "simulate",
    ]
    .iter()
    .map(|alias| format!("  {}\n", alias.bold()))
    .collect();

    let available_components: String = INSTALLABLE_COMPONENTS
        .iter()
        .map(|c| {
            let component_name = c.replace("miden-", "");
            format!("  {}\n", component_name.bold())
        })
        .collect();
    format!(
        "The Miden toolchain porcelain

{} {} <COMPONENT>

Available components:
{}

Available aliasses:
{}

Help:
  help                   Print this help message
  help <COMPONENT>       Print <COMPONENTS>'s help message
",
        "Usage:".bold().underline(),
        "miden".bold(),
        available_components,
        aliasses
    )
}
