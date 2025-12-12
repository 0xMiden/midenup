use std::{ffi::OsString, string::ToString};

use anyhow::{Context, anyhow, bail};
use colored::Colorize;

pub use crate::config::Config;
use crate::{
    channel::{AliasPipeline, AliasStep, Channel, Component, InstalledFile, resolve_command},
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
enum MidenArgument {
    /// The passed argument was an Alias stored in the local [[Manifest]]. [[AliasResolution]]
    /// represents the list of commands that need to be executed. NOTE: Some of these might need
    /// to get resolved.
    Alias(AliasPipeline, ChannelOrigin),
    /// The argument was the name of a component stored in the [[Manifest]].
    Component(Box<Component>, ChannelOrigin),
}

#[derive(Debug)]
struct ResolvedCommand {
    program: String,
    args: Vec<String>,
}

enum EnvironmentError {
    UnkownArgument(String),
}

/// Tracks where a resolution came from so we can warn when falling back to an installed channel.
#[derive(Debug, Clone, Copy)]
enum ChannelOrigin {
    Active,
    Installed,
}

#[derive(Debug)]
struct ToolchainEnvironment<'a> {
    /// We use the original channel as a fallback to
    /// [[ToolchainEnvironment::active_channel]]. If the active channel does not
    /// contain a requested component, for convenience's sake, we check if it
    /// exists in the original_channel. If it does, we execute it, after
    /// displaying a warning message.
    installed_channel: &'a Channel,

    /// This is the channel that is currently active. This *might* differ
    /// slightly from the original upstream channel equivalent in some
    /// scenarios, like:
    /// - The user only selected a subset of components for downloads.
    active_channel: Option<Channel>,
}
impl<'a> ToolchainEnvironment<'a> {
    fn new(installed_channel: &'a Channel, active_channel: Option<Channel>) -> Self {
        ToolchainEnvironment { active_channel, installed_channel }
    }

    /// This is the channel that is currently active. This *might* differ
    /// slightly from the original upstream channel equivalent in some
    /// scenarios, like:
    /// - The user only selected a subset of components for downloads.
    fn get_active_channel(&self) -> &Channel {
        if let Some(active_channel) = self.active_channel.as_ref() {
            active_channel
        } else {
            self.installed_channel
        }
    }

    fn resolve(&self, argument: String) -> Result<MidenArgument, EnvironmentError> {
        if let Some(active_channel) = self.active_channel.as_ref()
            && let Some(pipeline) = active_channel.aliases.get(&argument)
        {
            return Ok(MidenArgument::Alias(pipeline.clone(), ChannelOrigin::Active));
        }

        if let Some(pipeline) = self.installed_channel.aliases.get(&argument) {
            println!(
                "{}: '{}' is available in the installed toolchain but not in the current active selection.",
                "WARNING".yellow().bold(),
                argument,
            );
            return Ok(MidenArgument::Alias(pipeline.clone(), ChannelOrigin::Installed));
        }

        if let Some(active_channel) = self.active_channel.as_ref()
            && let Some(component) = active_channel.get_component(&argument)
        {
            return Ok(MidenArgument::Component(
                Box::new(component.clone()),
                ChannelOrigin::Active,
            ));
        }

        if let Some(component) = self.installed_channel.get_component(&argument) {
            println!(
                "{}: {} is installed, but it is not part of the current active toolchain.",
                "WARNING".yellow().bold(),
                component.name,
            );
            return Ok(MidenArgument::Component(
                Box::new(component.clone()),
                ChannelOrigin::Installed,
            ));
        }

        Err(EnvironmentError::UnkownArgument(argument))
    }

    fn get_executables_display(&self) -> String {
        self.get_active_channel()
            .components
            .iter()
            .filter(|c| matches!(c.get_installed_file(), InstalledFile::Executable { .. }))
            .map(|c| format!("  {}\n", c.name.bold()))
            .collect::<String>()
    }

    fn get_libraries_display(&self) -> String {
        self.get_active_channel()
            .components
            .iter()
            .filter_map(|comp| match comp.get_installed_file() {
                InstalledFile::Library { library_name, .. } => {
                    let display_name = format!("  {}\n", library_name);
                    Some(display_name)
                },
                _ => None,
            })
            .collect::<String>()
    }

    fn get_aliases_display(&self) -> String {
        let aliases = self.get_active_channel().get_aliases();
        let mut keys: Vec<_> = aliases.keys().collect();
        keys.sort();
        keys.iter().map(|alias| format!("  {}\n", alias.bold())).collect::<String>()
    }

    fn resolve_component_for_step(
        &self,
        component_name: &str,
        preferred: ChannelOrigin,
    ) -> anyhow::Result<(&Channel, &Component)> {
        // Prefer the originating channel, but warn if we have to fall back to the other installed
        // channel.
        let (primary, fallback, warning_origin) = match preferred {
            ChannelOrigin::Active => (
                self.active_channel.as_ref(),
                Some(self.installed_channel),
                "current active toolchain",
            ),
            ChannelOrigin::Installed => (
                Some(self.installed_channel),
                self.active_channel.as_ref(),
                "installed toolchain",
            ),
        };

        if let Some(primary_channel) = primary
            && let Some(component) = primary_channel.get_component(component_name)
        {
            return Ok((primary_channel, component));
        }

        if let Some(fallback_channel) = fallback
            && let Some(component) = fallback_channel.get_component(component_name)
        {
            println!(
                "{}: {} is not present in the {}; using the other installed channel definition.",
                "WARNING".yellow().bold(),
                component.name,
                warning_origin,
            );
            return Ok((fallback_channel, component));
        }

        bail!("Component '{}' is not available in the current toolchain", component_name)
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
    let (toolchain, _, partial_channel) =
        Toolchain::ensure_current_is_installed(config, local_manifest)?;

    let toolchain_environment = {
        let installed_channel = local_manifest
            .get_channel(&toolchain.channel)
            .context("Couldn't find active toolchain in the manifest.")?;

        ToolchainEnvironment::new(installed_channel, partial_channel)
    };

    let (help_flag, user_args, resolve_target) = match parsed_subcommand {
        MidenSubcommand::Help(HelpMessage::Default) => unreachable!(),
        MidenSubcommand::Help(HelpMessage::Toolchain) => {
            let help = toolchain_help(&toolchain_environment);

            println!("{help}");

            return Ok(());
        },
        MidenSubcommand::Help(HelpMessage::Resolve(resolve)) => {
            // NOTE: We rely on the different component's CLI interfaces to
            // recognize the "--help" flag. Currently, this relies on the fact
            // that clap recognizes said flag by default.
            // Source: https://github.com/clap-rs/clap/blob/583ba4ad9a4aea71e5b852b142715acaeaaaa050/src/_features.rs#L10
            (Some(String::from("--help")), vec![], resolve)
        },
        MidenSubcommand::Resolve(resolve) => {
            // argv is of the form:
            // miden <alias|component> ...
            // So we skip the first two and pass the rest to the underlying executable.
            (None, argv.iter().skip(2).cloned().collect(), resolve)
        },
        _ => unreachable!(),
    };

    let resolved_argument = match toolchain_environment.resolve(resolve_target.clone()) {
        Ok(argument) => argument,
        Err(EnvironmentError::UnkownArgument(argument)) => {
            let help_message = toolchain_help(&toolchain_environment);
            let err_msg = format!(
                "Failed to resolve '{}': Neither known alias or component.

{}",
                argument, help_message
            );
            bail!(err_msg);
        },
    };

    let mut resolved_commands: Vec<ResolvedCommand> = match resolved_argument {
        MidenArgument::Alias(alias_pipeline, origin) => alias_pipeline
            .iter()
            .map(|step| {
                resolve_to_command(step, &toolchain_environment, origin, config).with_context(
                    || format!("failed to resolve alias step for '{}'", resolve_target),
                )
            })
            .collect::<anyhow::Result<_>>()?,
        MidenArgument::Component(component, origin) => {
            let step = AliasStep {
                component: component.name.to_string(),
                command: component.get_call_format(),
            };
            vec![resolve_to_command(&step, &toolchain_environment, origin, config)?]
        },
    };

    if let Some(help_flag) = help_flag
        && let Some(first) = resolved_commands.first_mut()
    {
        first.args.push(help_flag);
    }

    for (idx, resolved_command) in resolved_commands.iter().enumerate() {
        let mut args: Vec<OsString> = resolved_command.args.iter().map(OsString::from).collect();
        if idx == 0 {
            args.extend(user_args.iter().cloned());
        }

        let mut command = config
            .execute_command(
                toolchain_environment.installed_channel,
                &resolved_command.program,
                &args,
            )
            .with_context(|| format!("failed to run 'miden {subcommand}'"))?;

        let status = command.wait().with_context(|| {
            format!("error occurred while waiting for 'miden {subcommand}' to finish executing")
        })?;

        if !status.success() {
            bail!(
                "'miden {}' failed while running step {} with status {}",
                subcommand,
                idx + 1,
                status.code().unwrap_or(1)
            )
        }
    }

    Ok(())
}

fn resolve_to_command(
    step: &AliasStep,
    toolchain_environment: &ToolchainEnvironment,
    preferred_origin: ChannelOrigin,
    config: &Config,
) -> anyhow::Result<ResolvedCommand> {
    let (channel, component) =
        toolchain_environment.resolve_component_for_step(&step.component, preferred_origin)?;
    let resolution = resolve_command(&step.command, channel, component, config)?;
    let (program, args) = resolution
        .split_first()
        .ok_or_else(|| anyhow!("Resolved command for {} is empty", component.name))?;

    Ok(ResolvedCommand {
        program: program.clone(),
        args: args.to_vec(),
    })
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
    let usage = "Usage:".bold().underline();
    let miden = "miden".bold();
    let asterisk = "*".bold();

    let available_aliases_text = "Available aliases:".bold().underline();
    let available_aliases: String = toolchain_environment.get_aliases_display();

    let available_components_text = "Available components:".bold().underline();
    let available_components: String = toolchain_environment.get_executables_display();

    let available_libraries_text = "Available libraries:".bold().underline();
    let available_libraries: String = toolchain_environment.get_libraries_display();

    let help = "Help:".bold().underline();

    format!(
        "The Miden toolchain porcelain

{usage} {miden} <ALIAS|COMPONENT>

{available_aliases_text}
{available_aliases}
{available_components_text}
{available_components}
{available_libraries_text}
{available_libraries}

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
