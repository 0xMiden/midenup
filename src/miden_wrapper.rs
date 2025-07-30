use std::{ffi::OsString, path::PathBuf, str::FromStr};

use anyhow::{anyhow, bail, Context};
use colored::Colorize;

pub use crate::config::Config;
use crate::{
    channel::{Channel, Component, InstalledFile, UserChannel},
    commands::INSTALLABLE_COMPONENTS,
    manifest::{Manifest, ManifestError},
    toolchain::Toolchain,
};

enum AliasError {
    UnrecognizedSubcommand,
}

/// This is used to encapsulate the different mechanisms used to display a help
/// messgage. Currently, there are only two.
enum HelpMessage {
    // /// This variant is used when the display message is obtained by shelling
    // /// out to a miden component. For instance: `miden-client account --help`.
    // ShellOut {
    //     target_exe: String,
    //     prefix_args: Vec<String>,
    // },
    Toolchain,
    // /// This other variant is used when shelling out to the shell is not
    // /// possible. This is mainly done to display the help message of:
    // /// - The `.masp` libraries
    // /// - `miden`'s own 'help' message
    // Internal {
    //     help_message: String,
    // },
    Default,

    Other(String),
}

#[derive(Debug)]
/// Enum of all the known "aliases". These are subcommands that have
/// "abbreviated" versions; these are then mapped to the corresponding "full"
/// command.
enum MidenAliases {
    /// Create local account
    Account,
    /// Fund account via faucet
    Faucet,
    /// Create new project
    New,
    /// Build project
    Build,
    /// Test project
    Test,
    // Node,
    /// Deploy contract
    Deploy,
    // Scan,
    /// Call view function (read-only)
    Call,
    /// Send transaction (state-changing)
    Send,
    /// Simulate transaction (no commit)
    Simulate,
}

impl FromStr for MidenAliases {
    type Err = AliasError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "account" => Ok(MidenAliases::Account),
            "faucet" => Ok(MidenAliases::Faucet),
            "new" => Ok(MidenAliases::New),
            "build" => Ok(MidenAliases::Build),
            "test" => Ok(MidenAliases::Test),
            "deploy" => Ok(MidenAliases::Deploy),
            "call" => Ok(MidenAliases::Call),
            "send" => Ok(MidenAliases::Send),
            "simulate" => Ok(MidenAliases::Simulate),
            _ => Err(AliasError::UnrecognizedSubcommand),
        }
    }
}

type ComponentName = String;
type OmittedSubcommand = Vec<String>;
enum CommandToExecute {
    MidenComponent {
        /// This is the name of the component. NOTE: This is *NOT* the name of
        /// the underlying executable, that information is obtained from the
        /// Manifest.
        name: String,
        prefix: Vec<String>,
    },
    ExternalCommand {
        name: String,
        arguments: Vec<String>,
    },
}

impl CommandToExecute {
    fn get_exe(self, channel: &Channel) -> anyhow::Result<(String, Vec<String>)> {
        match self {
            CommandToExecute::MidenComponent { name, prefix: arguments } => {
                // In reality, this error shouldn't occur since the components are baked into the compiler
                let component = channel.get_component(&name).with_context(|| {
                    format!(
                        "Component named {} is not present in toolchain version {}",
                        name, channel.name
                    )
                })?;

                let InstalledFile::InstalledExecutable(binary) = component.get_installed_file()
                else {
                    bail!(
                        "Can't execute component {}; since it is not an executable ",
                        component.name
                    )
                };

                Ok((binary, arguments))
            },
            CommandToExecute::ExternalCommand { name, arguments } => Ok((name, arguments)),
        }
    }
}
impl MidenAliases {
    // The channel is left as a parameter just in case one of these
    // functionalitites changes components.  If that ever happens, then the
    // mapping from Alias to component name can be conditioned over the Channel
    // Version.
    fn resolve(&self, _channel: &Channel) -> CommandToExecute {
        match self {
            MidenAliases::Account => CommandToExecute::MidenComponent {
                name: String::from("client"),
                prefix: vec![String::from("account")],
            },
            MidenAliases::Faucet => CommandToExecute::MidenComponent {
                name: String::from("client"),
                prefix: vec![String::from("mint")],
            },
            MidenAliases::New => CommandToExecute::ExternalCommand {
                name: String::from("cargo"),
                arguments: vec![String::from("miden"), String::from("new")],
            },
            MidenAliases::Build => CommandToExecute::ExternalCommand {
                name: String::from("cargo"),
                arguments: vec![String::from("miden"), String::from("build")],
            },
            MidenAliases::Test => todo!(),
            MidenAliases::Deploy => CommandToExecute::MidenComponent {
                name: String::from("client"),
                prefix: vec![String::from("new-wallet"), String::from("--deploy")],
            },
            MidenAliases::Call => CommandToExecute::MidenComponent {
                name: String::from("call"),
                prefix: vec![String::from("new-wallet"), String::from("--show")],
            },
            MidenAliases::Send => CommandToExecute::MidenComponent {
                name: String::from("client"),
                prefix: vec![String::from("send")],
            },
            MidenAliases::Simulate => CommandToExecute::MidenComponent {
                name: String::from("client"),
                prefix: vec![String::from("exec")],
            },
        }
    }
    fn help_command(&self) -> String {
        let help_argument = match self {
            MidenAliases::Account => "--help",
            MidenAliases::Faucet => "--help",
            MidenAliases::New => "--help",
            MidenAliases::Build => "--help",
            MidenAliases::Test => todo!(),
            // NOTE: This help message displays help for every flag.
            // Maybe return a filter lambda to parse these messages?
            MidenAliases::Deploy => "--help",
            // NOTE: This help message displays help for every flag.
            // Maybe return a filter lambda to parse these messages?
            MidenAliases::Call => "--help",
            MidenAliases::Send => "--help",
            MidenAliases::Simulate => "--help",
        };
        help_argument.to_string()
    }

    /// Get the corresponding executable target executable and prefix arguments
    /// for each known [MidenCommands]. These can later be used in a subshell to
    /// execute the underlying component.
    fn get_command_exec(&self) -> (String, Vec<String>) {
        match self {
            MidenAliases::Account => (String::from("miden-client"), vec![String::from("account")]),
            MidenAliases::Faucet => (String::from("miden-client"), vec![String::from("mint")]),
            MidenAliases::New => {
                (String::from("cargo"), vec![String::from("miden"), String::from("new")])
            },
            MidenAliases::Build => {
                (String::from("cargo"), vec![String::from("miden"), String::from("build")])
            },
            MidenAliases::Test => todo!(),
            MidenAliases::Deploy => (
                String::from("miden-client"),
                vec![String::from("new-wallet"), String::from("--deploy")],
            ),
            MidenAliases::Call => {
                (String::from("miden-client"), vec![String::from("account"), String::from("-s")])
            },
            MidenAliases::Send => (String::from("miden-client"), vec![String::from("send")]),
            MidenAliases::Simulate => (String::from("miden-client"), vec![String::from("exec")]),
        }
    }
}

enum MidenWrapper {
    Help(HelpMessage),
    MidenComponent(Component),
    Alias(MidenAliases),
}

type MidenSubcommand = String;
fn process_input(
    argv: &Vec<OsString>,
    config: &Config,
    local_manifest: &mut Manifest,
) -> anyhow::Result<(MidenSubcommand, MidenWrapper)> {
    // Extract the target binary to execute from argv[1]
    let subcommand = {
        let subcommand = argv.get(1).ok_or(anyhow!(
            "No arguments were passed to `miden`. To get a list of available commands, run:
miden help"
        ))?;
        subcommand.to_str().expect("Invalid command name: {subcommand}")
    };

    let command = if subcommand == "help" {
        match argv.get(2).and_then(|c| c.to_str()) {
            None => MidenWrapper::Help(HelpMessage::Default),
            Some("toolchain") => MidenWrapper::Help(HelpMessage::Toolchain),
            Some(other) => MidenWrapper::Help(HelpMessage::Other(other.to_string())),
        }
    } else {
        // Make sure we know the current toolchain so we can modify the PATH appropriately
        let toolchain = Toolchain::ensure_current_is_installed(config, local_manifest)?;
        let channel = local_manifest
            .get_channel(&toolchain.channel)
            .context("Couldn't find active toolchain in the manifest.")?;

        if let Ok(alias) = MidenAliases::from_str(subcommand) {
            MidenWrapper::Alias(alias)
        } else if let Some(component) = channel.get_component(subcommand) {
            MidenWrapper::MidenComponent(component.clone())
        } else {
            bail!(
                "Unknown subcommand: {}. \
            To get a full list of available commmands, run:\
            miden help",
                subcommand
            );
        }
    };

    Ok((subcommand.to_string(), command))
}

pub fn miden_wrapper(
    argv: Vec<OsString>,
    config: &Config,
    local_manifest: &mut Manifest,
) -> anyhow::Result<()> {
    let (subcommand, command) = process_input(&argv, config, local_manifest)?;

    if matches!(command, MidenWrapper::Help(HelpMessage::Default)) {
        std::println!("{}", default_help());
        return Ok(());
    }

    // Make sure we know the current toolchain so we can modify the PATH appropriately
    let toolchain = Toolchain::ensure_current_is_installed(config, local_manifest)?;
    let channel = local_manifest
        .get_channel(&toolchain.channel)
        .context("Couldn't find active toolchain in the manifest.")?;

    let (target_exe, prefix_args, include_rest_of_args): (String, Vec<String>, _) = match command {
        MidenWrapper::Help(message) => {
            match message {
                // NOTE: We handle the default help message case first. This is
                // done in order to avoid installing a toolchain when a user
                // runs `miden help` (which could happen if
                // [[Toolchain::ensure_current_is_installed]] get called).
                HelpMessage::Default => unreachable!(),
                HelpMessage::Toolchain => {
                    let help = components_help(channel);

                    std::println!("{help}");

                    return Ok(());
                },
                HelpMessage::Other(component) => {
                    let component = channel.get_component(&component).with_context(|| {
                        format!(
                            "Couldn't find component {} in the current channel: {}.",
                            component, channel.name
                        )
                    })?;

                    let installed_file = component.get_installed_file();
                    let InstalledFile::InstalledExecutable(binary) = installed_file else {
                        bail!(
                            "Can't show help for {} since it is not an executable.",
                            component.name
                        )
                    };

                    // NOTE: We rely on the different compponent's CLI
                    // interfaces to recognize the "--help" flag. At the
                    // minute, this relies on the fact that clap, by
                    // default, recognizes said flag. Source:
                    // https://github.com/clap-rs/clap/blob/583ba4ad9a4aea71e5b852b142715acaeaaaa050/src/_features.rs#L10
                    (binary, vec!["--help".to_string()], false)
                },
            }
        },
        MidenWrapper::Alias(alias) => {
            let command = alias.resolve(channel);
            let (target_exe, prefix_args) = command.get_exe(channel)?;

            (target_exe, prefix_args, true)
        },
        MidenWrapper::MidenComponent(component) => {
            let installed_file = component.get_installed_file();
            let InstalledFile::InstalledExecutable(binary) = installed_file else {
                bail!("Can't execute component {}; since it is not an executable ", component.name)
            };
            (binary, vec![], true)
        },
    };

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

fn components_help(channel: &Channel) -> String {
    let available_components: String = channel
        .components
        .iter()
        .map(|c| {
            let component_name = c.name.replace("miden-", "");
            format!("  {}\n", component_name.bold())
        })
        .collect();
    format!(
        "The Miden toolchain porcelain

The currently available components are:

{} {} <COMPONENT>

Available components:
{}

Help:
  help                   Print this help message
  help components        Print this help message {}
  help <COMPONENT>       Print <COMPONENTS>'s help message {}

*: NOTE: These commands will install the currently present toolchain if not installed.
",
        "Usage:".bold().underline(),
        "miden".bold(),
        available_components,
        "*".bold(),
        "*".bold(),
    )
}

fn default_help() -> String {
    let aliases: String = [
        "account", "faucet", "new", "build", "test", "deploy", "call", "send", "simulate",
    ]
    .iter()
    .map(|alias| format!("  {}\n", alias.bold()))
    .collect();

    format!(
        "The Miden toolchain porcelain

{} {} <ALIAS>

Available aliases:
{}

Help:
  help                   Print this help message
  help toolchain         Print help about the current toolchain {}
  help <COMPONENT>       Print <COMPONENTS>'s help message {}

*: NOTE: These commands will install the currently present toolchain if not installed.
",
        "Usage:".bold().underline(),
        "miden".bold(),
        aliases,
        "*".bold(),
        "*".bold(),
    )
}
