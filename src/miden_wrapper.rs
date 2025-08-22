use std::{collections::HashMap, ffi::OsString, string::ToString};

use anyhow::{anyhow, bail, Context};
use colored::Colorize;

pub use crate::config::Config;
use crate::{
    channel::{Alias, CLICommand, Channel, Component, InstalledFile},
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
    /// mapping is dependent on the currently active [[Toolchain]]. These will
    /// try to be resolved into a [[MidenArgument]].
    /// NOTE: This help message *could* trigger an install if the active
    /// [[Toolchain]] is not installed.
    Resolve(String),
}

/// The possible non-help commands that a user's input can be resolved into.
enum MidenArgument<'a> {
    /// The passed argument was an Alias stored in the local [[Manifest]]. [[AliasResolution]]
    /// represents the list of commands that need to be executed. NOTE: Some of these might need
    /// to get resolved.
    Alias(&'a Component, CLICommand),
    /// The argument was the name of a component stored in the [[Manifest]].
    Component(&'a Component),
}

enum EnvironmentError {
    UnkownArgument,
}
struct ToolchainEnvironment {
    aliases: HashMap<Alias, CLICommand>,
    components: Vec<Component>,
}
impl ToolchainEnvironment {
    fn new(channel: &Channel) -> Self {
        let aliases = channel.get_aliases();
        let components = channel.components.clone();
        ToolchainEnvironment { aliases, components }
    }

    fn resolve(&self, argument: String) -> Result<MidenArgument<'_>, EnvironmentError> {
        if let Some(component) = self.components.iter().find(|c| c.aliases.contains_key(&argument))
        {
            let resolution = component.aliases.get(&argument).unwrap();
            Ok(MidenArgument::Alias(component, resolution.clone()))
        } else if let Some(component) = self.components.iter().find(|c| c.name == argument) {
            Ok(MidenArgument::Component(component))
        } else {
            Err(EnvironmentError::UnkownArgument)
        }
    }

    fn get_components_display(&self) -> String {
        self.components
            .iter()
            .filter(|c| !matches!(c.get_installed_file(), InstalledFile::Library { .. }))
            .map(|c| format!("  {}\n", c.name.bold()))
            .collect::<String>()
    }

    fn get_aliases_display(&self) -> String {
        let mut aliases: Vec<_> = self.aliases.keys().collect();
        aliases.sort();
        aliases.iter().map(|alias| format!("  {}\n", alias.bold())).collect::<String>()
    }
}

/// These are the possible types of subcommands that `miden` is aware of.
enum MidenSubcommand {
    /// Aliases that correspond to a tuple of a known component + a set of
    /// prefixed arguments. For more information, see [[MidenAliases]].
    /// NOTE: With the exception of [[HelpMessage::Default]], this command
    /// *could* trigger an install if the active [[Toolchain]] is not installed.
    Help(HelpMessage),
    /// Displays midenup cargo version ang git revision hash.
    Version,
    /// The user passed in a subcommand that needs to be resolved using the
    /// currently active [[Toolchain]]. Resolution can result in one of the
    /// following elements:
    /// - An alias
    /// - A [[Component]]
    ///
    /// If it's none of those, then we error out.
    ///
    /// NOTE: This command *could* trigger an install if the active
    /// [[Toolchain]] is not installed.
    Resolve(String),
}

fn parse_subcommand(subcommand: &str, argv: &[OsString]) -> MidenSubcommand {
    if subcommand == "help" {
        match argv.get(2).and_then(|c| c.to_str()) {
            None => MidenSubcommand::Help(HelpMessage::Default),
            Some("toolchain") => MidenSubcommand::Help(HelpMessage::Toolchain),
            Some(other) => MidenSubcommand::Help(HelpMessage::Resolve(other.to_string())),
        }
    } else if subcommand == "--version" {
        MidenSubcommand::Version
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
        let subcommand = argv.get(1).with_context(|| {
            format!(
                "
{}: '{}' requires a subcommand but one was not provided

{} {} <ALIAS|COMMAND>

For more information, try 'miden help'.
",
                "error:".red().bold(),
                "miden".yellow().bold(),
                "Usage".bold().underline(),
                "miden".bold(),
            )
        })?;
        subcommand.to_str().expect("Invalid command name: {subcommand}")
    };

    let parsed_subcommand = parse_subcommand(subcommand, &argv);

    // NOTE: We handle these case first to avoid triggering an install when help
    // related commands are run.
    match parsed_subcommand {
        MidenSubcommand::Help(HelpMessage::Default) => {
            println!("{}", default_help());
            return Ok(());
        },
        MidenSubcommand::Version => {
            println!("{}", display_version(config));
            return Ok(());
        },
        _ => (),
    }

    // Make sure we know the current toolchain so we can modify the PATH appropriately
    let toolchain = Toolchain::ensure_current_is_installed(config, local_manifest)?;
    let channel = local_manifest
        .get_channel(&toolchain.channel)
        .context("Couldn't find active toolchain in the manifest.")?;
    let toolchain_environment = ToolchainEnvironment::new(channel);

    let (extra_arguments, include_rest_of_args) = match parsed_subcommand {
        MidenSubcommand::Help(HelpMessage::Default) => unreachable!(),
        MidenSubcommand::Help(HelpMessage::Toolchain) => {
            let help = toolchain_help(&toolchain_environment);

            println!("{help}");

            return Ok(());
        },
        MidenSubcommand::Help(HelpMessage::Resolve(_)) => {
            // NOTE: We rely on the different component's CLI interfaces to
            // recognize the "--help" flag. Currently, this relies on the fact
            // that clap recognizes said flag by default.
            // Source: https://github.com/clap-rs/clap/blob/583ba4ad9a4aea71e5b852b142715acaeaaaa050/src/_features.rs#L10
            (vec!["--help".to_string()], false)
        },
        _ => (vec![], true),
    };

    // We obtain the target executable and prefixes that are associated with the
    // passed subcommand.
    let (target_exe, mut prefix_args) = match parsed_subcommand {
        MidenSubcommand::Version => unreachable!(),
        MidenSubcommand::Help(HelpMessage::Default) => unreachable!(),
        MidenSubcommand::Help(HelpMessage::Toolchain) => unreachable!(),
        // Resolution, either for help or for actual execution is the same. The
        // only difference is wheter you append "--help" at the end and if we
        // process additional arguments.
        MidenSubcommand::Help(HelpMessage::Resolve(resolve))
        | MidenSubcommand::Resolve(resolve) => {
            match toolchain_environment.resolve(resolve.clone()) {
                Ok(MidenArgument::Alias(component, alias_resolutions)) => {
                    let commands = alias_resolutions
                        .iter()
                        .map(|description| description.resolve_command(channel, component, config))
                        .collect::<Result<Vec<String>, _>>()?;

                    // SAFETY: Safe under the assumption that every alias has an
                    // associated command.
                    let command = commands.first().unwrap().clone();
                    let aliased_arguments: Vec<String> = commands.into_iter().skip(1).collect();

                    (command, aliased_arguments)
                },
                Ok(MidenArgument::Component(component)) => {
                    let call_convention = component
                        .get_call_format()
                        .iter()
                        .map(|argument| argument.resolve_command(channel, component, config))
                        .collect::<Result<Vec<String>, _>>()?;

                    // SAFETY: Safe under the assumption that every call_format has at least one
                    // argument
                    let command = call_convention.first().unwrap().clone();
                    let args: Vec<String> = call_convention.into_iter().skip(1).collect();

                    (command, args)
                },
                Err(_) => {
                    let aliases = toolchain_environment.get_aliases_display();
                    let components = toolchain_environment.get_components_display();
                    bail!(
                        "Failed to resolve {}: Neither known alias or component.

These are the known aliases:
{aliases}
And these are the known components:
{components}

        ",
                        resolve.clone(),
                    );
                },
            }
        },
    };

    // Now that executable resolution is done, we append the extra arguments we
    // obtained in the beginning.
    prefix_args.extend(extra_arguments);

    // Compute the effective PATH for this command
    let toolchain_bin = config.midenup_home_2.get_bin_dir_from(channel);

    let path = std::env::var_os("PATH")
        .map(|prev_path| {
            let mut path = OsString::from(format!("{}:", toolchain_bin.display()));
            path.push(prev_path);
            path
        })
        .unwrap_or(toolchain_bin.as_os_str().into());
    // {
    //     Some(prev_path) => {
    //         let mut path = OsString::from(format!("{}:", toolchain_bin.display()));
    //         path.push(prev_path);
    //         path
    //     },
    //     None => toolchain_bin.into_os_string(),
    // };

    let rest_of_args = if include_rest_of_args {
        argv.iter().skip(2)
    } else {
        // We don't want to pass the rest of the CLI arguments to the subshell
        // in this case.
        // This is equivalent to std::iter::empty::<OsString>()
        argv.iter().skip(argv.len())
    };

    let mut output = std::process::Command::new(target_exe)
        .env("MIDENUP_HOME", &config.midenup_home_2)
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

fn display_version(config: &Config) -> String {
    // NOTE: These files are generated in the project's build.rs.

    let compiled_cargo_version = include_str!(concat!(env!("OUT_DIR"), "/cargo_version.in"));

    let git_revision = include_str!(concat!(env!("OUT_DIR"), "/git_revision.in"));

    let midenup_version = env!(
        "CARGO_PKG_VERSION",
        "CARGO_PKG_VERSION environment variable not set.\
                 This should be set by cargo by default; however, if not, it can be manually set using the `version` field in the Cargo.toml file"
    );
    let cargo_version = {
        std::process::Command::new("cargo")
            .arg("--version")
            .output()
            .map_err(|err| anyhow::anyhow!("failed to run 'cargo --version' because of {err}"))
            .and_then(|output| {
                String::from_utf8(output.stdout).map_err(|err| {
                    anyhow::anyhow!("failed to parse cargo version because of: {err}")
                })
            })
            .inspect_err(|e| {
                println!("Failed to obtain cargo version:");
                println!("{}", e);
                println!("Leaving as unknown")
            })
            .unwrap_or("unknown".to_string())
    };
    let cargo_version = cargo_version.trim();

    let toolchain_version = Toolchain::current(config)
        .and_then(|(toolchain, _)| {
            config
                .manifest
                .get_channel(&toolchain.channel)
                .map(|channel| channel.name.to_string())
                .ok_or(anyhow!("channel: {} doesn't exist or isn't available ", toolchain.channel))
        })
        .inspect_err(|err| {
            println!(
                "failed to obtain current toolchain error because of: {err}, leaving as unknown"
            )
        })
        .unwrap_or("unknown".to_string());

    let github_issue = {
        let short_body = format!(
            "<!--- (leave this at the bottom) --> midenup:{midenup_version}, toolchain: {toolchain_version}, cargo:{cargo_version}, rev:{git_revision}"
        );
        format!(
            "https://github.com/0xMiden/midenup/issues/new?title=bug:<YOUR_ISSUE>&body={short_body}"
        )
    };

    format!(
        "
The Miden toolchain porcelain:

Environment:
- cargo version: {cargo_version}.

Midenup:
- midenup + miden version: {midenup_version}.
- active toolchain version: {toolchain_version}.
- midenup revision: {git_revision}.
- midenup was compiled with {compiled_cargo_version}.


Found a bug? Create an issue by copying this into your browser:

{github_issue}
"
    )
}

fn toolchain_help(toolchain_environment: &ToolchainEnvironment) -> String {
    let available_components: String = toolchain_environment.get_components_display();
    let available_aliases: String = toolchain_environment.get_aliases_display();

    let usage = "Usage:".bold().underline();
    let miden = "miden".bold();
    let asterisk = "*".bold();
    let available_aliases_text = "Available aliases:".bold().underline();
    let available_components_text = "Available components:".bold().underline();
    let help = "Help:".bold().underline();

    format!(
        "The Miden toolchain porcelain

{usage} {miden} <ALIAS|COMPONENT>

{available_aliases_text}
{available_aliases}
{available_components_text}
{available_components}

{help}
  help                   Print this help message
  help toolchain         Print this help message {asterisk}
  help <COMPONENT>       Print <COMPONENTS>'s help message {asterisk}

{asterisk}: These commands will install the currently present toolchain if not installed.
",
    )
}

fn default_help() -> String {
    let asterisk = "*".bold();
    let help = "Help:".bold().underline();
    format!(
        "The Miden toolchain porcelain

{help}
  help                   Print this help message
  help toolchain         Print help about the currently available aliases and components {asterisk}
  help <COMPONENT>       Print a specific <COMPONENTS>'s help message {asterisk}

{asterisk}: These commands will install the currently present toolchain if not installed.
",
    )
}
