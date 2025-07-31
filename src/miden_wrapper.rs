use std::{ffi::OsString, str::FromStr, string::ToString};

use anyhow::{Context, anyhow, bail};
use colored::Colorize;
use strum::{EnumMessage, IntoEnumIterator};
use strum_macros::{Display, EnumIter, EnumMessage, EnumString};

pub use crate::config::Config;
use crate::{
    channel::{Channel, InstalledFile},
    manifest::Manifest,
    toolchain::Toolchain,
};

/// These are the know help messages variants that midenup is aware of.
enum HelpMessage {
    /// Show the default help message, similar to the one you would get with
    /// clap's "--help" flag.
    Default,

    /// Show a help message specific to the current active [[Toolchain]].
    /// NOTE: This help message *could* trigger an install if the active
    /// [[Toolchain]] is not installed.
    Toolchain,

    /// This variant represents a "fallback" option where we save the user's
    /// input so that we later on try to map it to a [[Component]].  This
    /// mapping is dependent on the currently active [[Toolchain]].
    /// NOTE: This help message *could* trigger an install if the active
    /// [[Toolchain]] is not installed.
    Resolve(String),
}

#[derive(Debug, EnumIter, Display, EnumMessage, EnumString)]
#[strum(serialize_all = "snake_case")]
/// All the known, hard-coded, "aliases". These are subcommands that serve as a
/// short form version of a different command from a specific component.
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

/// These are the know mappings from [[MidenAliases]].
enum AliasCommands {
    MidenComponent {
        /// This is the name of the component. NOTE: This is *NOT* the name of
        /// the underlying executable, that information is obtained from the
        /// locally installed [[Manifest]]. To get the name of the executable,
        /// use the [[CommandToExecute::get_exe]] function.
        name: String,
        prefix: Vec<String>,
    },
    /// This represents a command whose binary is not handled by the
    /// [[Manifest]], for instance, `miden new` maps to a call to `cargo`.
    ExternalCommand { name: String, arguments: Vec<String> },
}

impl AliasCommands {
    fn get_exe(self, channel: &Channel) -> anyhow::Result<(String, Vec<String>)> {
        match self {
            AliasCommands::MidenComponent { name, prefix: arguments } => {
                // SAFETY: This could only get triggered if there's an error in
                // hardcoded mappings present in [[MidenAliases::resolve]].
                let component = channel.get_component(&name).with_context(|| {
                    format!(
                        "Component named {} is not present in toolchain version {}",
                        name, channel.name
                    )
                })?;

                let InstalledFile::Executable { binary_name: binary } =
                    component.get_installed_file()
                else {
                    bail!(
                        "Can't execute component {}; since it is not an executable ",
                        component.name
                    )
                };

                Ok((binary, arguments))
            },
            AliasCommands::ExternalCommand { name, arguments } => Ok((name, arguments)),
        }
    }
}
impl MidenAliases {
    /// This functions returns the underlying command corresponding to a given
    /// alias. For more information about the resulting mapping, see
    /// [[AliasCommands]].
    /// NOTE: The [[Channel]] argument is left in case one of these
    /// functionalities migrates [[Component]].  If that ever happens, then the
    /// mapping can be conditioned over the [[Channel::name]] (a.k.a. the version).
    fn resolve(&self, _channel: &Channel) -> AliasCommands {
        match self {
            MidenAliases::Account => AliasCommands::MidenComponent {
                name: String::from("client"),
                prefix: vec![String::from("account")],
            },
            MidenAliases::Faucet => AliasCommands::MidenComponent {
                name: String::from("client"),
                prefix: vec![String::from("mint")],
            },
            MidenAliases::New => AliasCommands::ExternalCommand {
                name: String::from("cargo"),
                arguments: vec![String::from("miden"), String::from("new")],
            },
            MidenAliases::Build => AliasCommands::ExternalCommand {
                name: String::from("cargo"),
                arguments: vec![String::from("miden"), String::from("build")],
            },
            MidenAliases::Test => todo!(),
            MidenAliases::Deploy => AliasCommands::MidenComponent {
                name: String::from("client"),
                prefix: vec![String::from("new-wallet"), String::from("--deploy")],
            },
            MidenAliases::Call => AliasCommands::MidenComponent {
                name: String::from("client"),
                prefix: vec![String::from("call"), String::from("--show")],
            },
            MidenAliases::Send => AliasCommands::MidenComponent {
                name: String::from("client"),
                prefix: vec![String::from("send")],
            },
            MidenAliases::Simulate => AliasCommands::MidenComponent {
                name: String::from("client"),
                prefix: vec![String::from("exec")],
            },
        }
    }
}

/// These are the possible types of subcommands that `miden` is aware of.
enum MidenSubcommand {
    /// Aliases that correspond to a tuple of a known component + a set of
    /// prefixed arguments. For more information, see [[MidenAliases]].
    /// NOTE: This command *could* trigger an install if the active
    /// [[Toolchain]] is not installed.
    Alias(MidenAliases),
    /// Aliases that correspond to a tuple of a known component + a set of
    /// prefixed arguments. For more information, see [[MidenAliases]].
    /// NOTE: With the exception of [[HelpMessage::Default]], this command
    /// *could* trigger an install if the active [[Toolchain]] is not installed.
    Help(HelpMessage),
    /// The user passed in a subcommand that needs to be resolved using the
    /// currently active [[Toolchain]].
    /// NOTE: This command *could* trigger an install if the active
    /// [[Toolchain]] is not installed.
    Resolve(String),
}

fn process_input(subcommand: &str, argv: &[OsString]) -> MidenSubcommand {
    if subcommand == "help" {
        match argv.get(2).and_then(|c| c.to_str()) {
            None => MidenSubcommand::Help(HelpMessage::Default),
            Some("toolchain") => MidenSubcommand::Help(HelpMessage::Toolchain),
            Some(other) => MidenSubcommand::Help(HelpMessage::Resolve(other.to_string())),
        }
    } else if let Ok(alias) = MidenAliases::from_str(subcommand) {
        MidenSubcommand::Alias(alias)
    } else {
        MidenSubcommand::Resolve(subcommand.to_string())
    }
}

pub fn miden_wrapper(
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

    let parsed_subcommand = process_input(subcommand, &argv);

    // NOTE: We handle this case first to avoid triggering an install when
    // `miden help` gets run.
    if matches!(parsed_subcommand, MidenSubcommand::Help(HelpMessage::Default)) {
        std::println!("{}", default_help());
        return Ok(());
    }

    // Make sure we know the current toolchain so we can modify the PATH appropriately
    let toolchain = Toolchain::ensure_current_is_installed(config, local_manifest)?;
    let channel = local_manifest
        .get_channel(&toolchain.channel)
        .context("Couldn't find active toolchain in the manifest.")?;

    let (target_exe, prefix_args, include_rest_of_args) = match parsed_subcommand {
        MidenSubcommand::Help(message) => {
            match message {
                // Handled in the matches! above.
                HelpMessage::Default => unreachable!(),
                HelpMessage::Toolchain => {
                    let help = toolchain_help(channel);

                    std::println!("{help}");

                    return Ok(());
                },
                HelpMessage::Resolve(component) => {
                    let component = channel.get_component(&component).with_context(|| {
                        format!(
                            "Couldn't find component {} in the current channel: {}.",
                            component, channel.name
                        )
                    })?;

                    let installed_file = component.get_installed_file();
                    let InstalledFile::Executable { binary_name: binary } = installed_file else {
                        bail!(
                            "Can't show help for {} since it is not an executable.",
                            component.name
                        )
                    };

                    // NOTE: We rely on the different component's CLI interfaces
                    // to recognize the "--help" flag. Currently, this relies on
                    // the fact that clap recognizes said flag by
                    // default. Source:
                    // https://github.com/clap-rs/clap/blob/583ba4ad9a4aea71e5b852b142715acaeaaaa050/src/_features.rs#L10
                    (binary, vec!["--help".to_string()], false)
                },
            }
        },
        MidenSubcommand::Alias(alias) => {
            let command = alias.resolve(channel);
            let (target_exe, prefix_args) = command.get_exe(channel)?;

            (target_exe, prefix_args, true)
        },
        MidenSubcommand::Resolve(component) => {
            let component = channel.get_component(component);
            let Some(component) = component else {
                bail!(
                    "Unknown subcommand: {}. \
            To get a full list of available commands, run:\
            miden help",
                    subcommand
                );
            };

            let installed_file = component.get_installed_file();
            let InstalledFile::Executable { binary_name: binary } = installed_file else {
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
        // We don't want to pass the rest of the CLI arguments to the subshell
        // in this case.
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

fn toolchain_help(channel: &Channel) -> String {
    let available_components: String = channel
        .components
        .iter()
        .filter(|c| !matches!(c.get_installed_file(), InstalledFile::Library { .. }))
        .map(|c| format!("  {}\n", c.name.bold()))
        .collect();
    format!(
        "The Miden toolchain porcelain

The currently available components are:

{} {} <COMPONENT>

{}
{}

{}
  help                   Print this help message
  help components        Print this help message {}
  help <COMPONENT>       Print <COMPONENTS>'s help message {}

{}: These commands will install the currently present toolchain if not installed.
",
        "Usage:".bold().underline(),
        "miden".bold(),
        "Available components:".bold().underline(),
        available_components,
        "Help:".bold().underline(),
        "*".bold(),
        "*".bold(),
        "*".bold(),
    )
}

fn default_help() -> String {
    // SAFETY: This unwrap is safe under the assumption that the MidenAliases
    // enum has at least one variant
    let longest_alias = MidenAliases::iter()
        .map(|a| a.to_string())
        .max_by(|x, y| x.len().cmp(&y.len()))
        .unwrap_or_else(|| panic!("ERROR: MidenAliases enum is empty"));

    let aliases: String = MidenAliases::iter()
        .map(|a| {
            (
                a.to_string(),
                a.get_documentation()
                    // SAFETY: This unwrap is safe as long as every
                    // [[MidenAliases]] variant has a doc comment
                    .unwrap_or_else(|| panic!("Enum {a} is lacking a doc comment")),
            )
        })
        .map(|(alias, description)| {
            let spacing = longest_alias.len() - alias.len();
            // NOTE: This value was added in order to both:
            // - Emulate clap's padding
            // - Improve readability
            let padding = 3;
            let spaces = String::from(' ').repeat(spacing + padding);
            format!("  {}{}{}\n", alias.bold(), spaces, description)
        })
        .collect();

    format!(
        "The Miden toolchain porcelain

{} {} <ALIAS>

{}
{}

{}
  help                   Print this help message
  help toolchain         Print help about the current toolchain {}
  help <COMPONENT>       Print <COMPONENTS>'s help message {}

{}: These commands will install the currently present toolchain if not installed.
",
        "Usage:".bold().underline(),
        "miden".bold(),
        "Available aliases:".bold().underline(),
        aliases,
        "Help:".bold().underline(),
        "*".bold(),
        "*".bold(),
        "*".bold(),
    )
}
