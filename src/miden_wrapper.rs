use std::{ffi::OsString, fmt::Display, string::ToString};

use anyhow::{Context, anyhow, bail};
use colored::Colorize;

pub use crate::config::Config;
use crate::{
    channel::{Channel, CliCommands, Component, InstalledFile, resolve_command},
    manifest::Manifest,
    toolchain::Toolchain,
};

/// These are the know help messages variants that midenup is aware of.
enum HelpMessage {
    /// Show the default help message, similar to the one you would get with clap's "--help" flag.
    Default,
    /// Show a help message specific to the current active [Toolchain].
    ///
    /// NOTE: This help message *could* trigger an install if the active [Toolchain] is not
    /// installed.
    Toolchain,
    /// This variant represents a "fallback" option where we save the user's input so that we later
    /// on try to map it to a [Component].
    ///
    /// This mapping is dependent on the currently active [Toolchain]. These will try to be resolved
    /// into a [MidenArgument].
    ///
    /// NOTE: This help message *could* trigger an install if the active [Toolchain] is not
    /// installed.
    Resolve(String),
}

/// The possible non-help commands that a user's input can be resolved into.
#[derive(Debug)]
enum MidenArgument {
    /// The passed argument was an alias stored in the local [Manifest].
    ///
    /// [AliasResolution] represents the list of commands that need to be executed.
    ///
    /// NOTE: Some of these might need to get resolved.
    Alias(Component, CliCommands),
    /// The argument was the name of a component stored in the [Manifest].
    Component(Component),
}

/// Struct containing the command to execute and the channel to execute it against.
struct ExecutionEnvironment<'a> {
    argument: MidenArgument,
    active_channel: &'a Channel,
}

enum EnvironmentError {
    UnknownArgument(String),
    LibraryAsExecutable(String),
    AliasOnly(String),
}

impl Display for EnvironmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self {
            EnvironmentError::UnknownArgument(err) => write!(f, "{err}"),
            EnvironmentError::LibraryAsExecutable(err) => write!(f, "{err}"),
            EnvironmentError::AliasOnly(err) => write!(f, "{err}"),
        }
    }
}

#[derive(Debug)]
struct ToolchainEnvironment<'a> {
    /// We use the original channel as a fallback to [`ToolchainEnvironment::active_channel`].
    ///
    /// If the active channel does not contain a requested component, for convenience's sake, we
    /// check if it exists in the original_channel. If it does, we execute it, after displaying a
    /// warning message.
    installed_channel: &'a Channel,
    /// This is the channel that is currently active.
    ///
    /// This *might* differ slightly from the original upstream channel equivalent in some
    /// scenarios, e.g. the user only selected a subset of components for downloads.
    active_channel: Option<Channel>,
}

#[derive(Debug, Clone, Copy)]
enum ChannelType {
    Installed,
    Active,
}
impl<'a> ToolchainEnvironment<'a> {
    fn new(installed_channel: &'a Channel, active_channel: Option<Channel>) -> Self {
        ToolchainEnvironment { installed_channel, active_channel }
    }

    /// This is the channel that is currently active.
    ///
    /// This *might* differ slightly from the original upstream channel equivalent in some
    /// scenarios, e.g. the user only selected a subset of components for downloads.
    fn get_active_channel(&self) -> (&Channel, ChannelType) {
        if let Some(active_channel) = self.active_channel.as_ref() {
            (active_channel, ChannelType::Active)
        } else {
            (self.installed_channel, ChannelType::Installed)
        }
    }

    /// Parses the user's input and returns the required [ExecutionEnvironment] to execute the
    /// requested command.
    fn resolve(&self, argument: String) -> Result<ExecutionEnvironment<'_>, EnvironmentError> {
        // Local function that tries to parse an argument given a channel's state.
        let fallback_motive = if let Some(active_channel) = self.active_channel.as_ref() {
            match resolve_argument(active_channel, &argument) {
                Ok(arg) => return Ok(ExecutionEnvironment { argument: arg, active_channel }),
                Err(EnvironmentError::UnknownArgument(_)) => {
                    FallbackMotive::ArgumentNotInActiveChannel
                },
                Err(e) => return Err(e),
            }
        } else {
            FallbackMotive::NoActiveChannel
        };

        // We know try to resolve the argument with the installed channel.
        {
            let miden_argument = resolve_argument(self.installed_channel, &argument)?;

            let not_found_in_active =
                matches!(fallback_motive, FallbackMotive::ArgumentNotInActiveChannel);

            let warning_message = match (&miden_argument, not_found_in_active) {
                (MidenArgument::Alias(comp, _), true) => Some(format!(
                    "{}: {} is an alias from component {}, which is installed but is not part of \
                     the current active toolchain.",
                    "WARNING".yellow().bold(),
                    argument,
                    comp.name,
                )),
                (MidenArgument::Component(comp), true) => Some(format!(
                    "{}: {} is installed, but it is not part of the current active toolchain.",
                    "WARNING".yellow().bold(),
                    comp.name,
                )),
                _ => None,
            };
            if let Some(warning) = warning_message {
                println!("{warning}")
            };

            Ok(ExecutionEnvironment {
                argument: miden_argument,
                active_channel: self.installed_channel,
            })
        }
    }

    fn get_executables_display(&self) -> String {
        self.get_active_channel()
            .0
            .components
            .iter()
            .filter(|c| {
                matches!(
                    c.get_installed_file(),
                    InstalledFile::Executable { binary_name: _, alias_only: false }
                )
            })
            .map(|c| format!("  {}\n", c.name.bold()))
            .collect::<String>()
    }

    fn get_libraries_display(&self) -> String {
        self.get_active_channel()
            .0
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
        let aliases = self.get_active_channel().0.get_aliases();
        let mut keys: Vec<_> = aliases.keys().collect();
        keys.sort();
        keys.iter().map(|alias| format!("  {}\n", alias.bold())).collect::<String>()
    }
}

/// These are the possible types of subcommands that `miden` is aware of.
enum MidenSubcommand {
    /// Aliases that correspond to a tuple of a known component + a set of prefixed arguments.
    ///
    /// For more information, see [MidenAliases].
    ///
    /// NOTE: With the exception of [`HelpMessage::Default`], this command *could* trigger an
    /// install if the active [Toolchain] is not installed.
    Help(HelpMessage),
    /// Displays midenup cargo version ang git revision hash.
    Version,
    /// The user passed in a subcommand that needs to be resolved using the currently active
    /// [Toolchain].
    ///
    /// Resolution can result in one of the following elements:
    ///
    /// - An alias
    /// - A [Component]
    ///
    /// If it's none of those, then we error out.
    ///
    /// NOTE: This command *could* trigger an install if the active [Toolchain] is not installed.
    Resolve(String),
}

/// Identifies the `--help` flag argument in clap
const CLAP_HELP_FLAG: &str = "help_flag";
/// Identifies the `help` subcommand in clap
const CLAP_HELP_SUBCMD: &str = "help";
/// Identifies the name of the component/alias argument of the `miden help` subcommand
const CLAP_HELP_COMPONENT_ARG: &str = "alias_component";
/// Identifies the `--version` flag argument in clap
const CLAP_VERSION_FLAG: &str = "version";

/// Builds the clap [Command] definition for the `miden` binary.
fn build_miden_command() -> clap::Command {
    clap::Command::new("miden")
        .about("The Miden toolchain porcelain")
        // We disable clap's built-in help flag and version flag because
        // `miden` provides its own custom help and version commands.
        .disable_help_flag(true)
        .disable_help_subcommand(true)
        .disable_version_flag(true)
        // This is what allows `miden` to be dynamic.
        .allow_external_subcommands(true)
        // This adds support for the -h and --help flags.
        .arg(clap::Arg::new(CLAP_HELP_FLAG).short('h').long("help").action(clap::ArgAction::SetTrue))
        // This adds support for `miden help <alias/component>`.
        .subcommand(
            clap::Command::new(CLAP_HELP_SUBCMD)
                .about("Print help information")
                .arg(clap::Arg::new(CLAP_HELP_COMPONENT_ARG).num_args(0..=1)),
        )
        // This adds support for --version.
        .arg(clap::Arg::new(CLAP_VERSION_FLAG).long("version").action(clap::ArgAction::SetTrue))
}

/// Converts clap [ArgMatches] into a [MidenSubcommand].
fn parse_matches(matches: &clap::ArgMatches) -> MidenSubcommand {
    if matches.get_flag(CLAP_HELP_FLAG) {
        return MidenSubcommand::Help(HelpMessage::Default);
    }
    if matches.get_flag(CLAP_VERSION_FLAG) {
        return MidenSubcommand::Version;
    }
    match matches.subcommand() {
        Some((CLAP_HELP_SUBCMD, sub_matches)) => {
            match sub_matches.get_one::<String>(CLAP_HELP_COMPONENT_ARG).map(String::as_str) {
                // `miden help` is the same as `--help`.
                None => MidenSubcommand::Help(HelpMessage::Default),
                // `miden help toolchain`.
                Some("toolchain") => MidenSubcommand::Help(HelpMessage::Toolchain),
                // `miden help <alias/component>`.
                Some(other) => MidenSubcommand::Help(HelpMessage::Resolve(other.to_string())),
            }
        },
        // `miden <alias/compoent>`.
        Some((comp_or_alias, _)) => MidenSubcommand::Resolve(comp_or_alias.to_string()),
        // `miden` alone.
        None => MidenSubcommand::Help(HelpMessage::Default),
    }
}

pub fn miden_wrapper(
    argv: &[OsString],
    config: &Config,
    local_manifest: &mut Manifest,
) -> anyhow::Result<()> {
    let matches = build_miden_command().get_matches_from(argv);

    let parsed_subcommand = parse_matches(&matches);

    // Used in error messages further down.
    let user_input = argv.iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>().join(" ");

    // NOTE: We handle these case first to avoid triggering an install when help related commands
    // are run.
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
    let (toolchain, _justification, partial_channel) =
        Toolchain::ensure_current_is_installed(config, local_manifest)?;

    let toolchain_environment = {
        let installed_channel = local_manifest
            .get_channel(&toolchain.channel)
            .context("Couldn't find active toolchain in the manifest.")?;

        ToolchainEnvironment::new(installed_channel, partial_channel)
    };

    // Whether the user requested help for a specific alias or component (e.g. `miden help
    // compile`). If true, we append "--help" to the resolved command's arguments further down.
    let requested_help = match parsed_subcommand {
        MidenSubcommand::Help(HelpMessage::Default) => unreachable!(),
        MidenSubcommand::Help(HelpMessage::Toolchain) => {
            let help = toolchain_help(&toolchain_environment);

            println!("{help}");

            return Ok(());
        },
        MidenSubcommand::Help(HelpMessage::Resolve(_)) => true,
        _ => false,
    };

    // We obtain the target executable and prefixes that are associated with the passed subcommand.
    let (target_exe, prefix_args, active_channel) = match parsed_subcommand {
        MidenSubcommand::Version
        | MidenSubcommand::Help(HelpMessage::Default)
        | MidenSubcommand::Help(HelpMessage::Toolchain) => unreachable!(),
        // Resolution, either for help or for actual execution is the same. The only difference is
        // wheter we append "--help" at the end and if we process additional arguments.
        MidenSubcommand::Help(HelpMessage::Resolve(resolve))
        | MidenSubcommand::Resolve(resolve) => {
            match toolchain_environment.resolve(resolve.clone()) {
                Ok(ExecutionEnvironment {
                    argument: MidenArgument::Alias(component, alias_resolutions),
                    active_channel,
                }) => {
                    let commands =
                        resolve_command(&alias_resolutions, active_channel, &component, config)?;

                    // SAFETY: Safe under the assumption that every alias has an associated command.
                    let mut commands = std::collections::VecDeque::from(commands);
                    let command = commands.pop_front().unwrap();
                    let aliased_arguments = commands;

                    (command, aliased_arguments, active_channel)
                },
                Ok(ExecutionEnvironment {
                    argument: MidenArgument::Component(component),
                    active_channel,
                }) => {
                    let mut call_convention = std::collections::VecDeque::from(resolve_command(
                        &component.get_call_format(),
                        active_channel,
                        &component,
                        config,
                    )?);

                    // SAFETY: Safe under the assumption that every call_format has at least one
                    // argument
                    let command = call_convention.pop_front().unwrap();
                    let args = call_convention;

                    (command, args, active_channel)
                },
                Err(err) => {
                    let help_message = toolchain_help(&toolchain_environment);
                    let err_msg = format!(
                        "{}

{}",
                        err, help_message
                    );
                    bail!(err_msg);
                },
            }
        },
    };

    // This is either --help in case the user requested for help or the
    // remaining arguments passed by the user.
    let remaining_args = if requested_help {
        vec![std::ffi::OsStr::new("--help").to_os_string()]
    } else {
        matches
        .subcommand()
        // Since we're using "allow_external_subcommands" all the remaining
        // arguments are stored in the empty string "".
        // Source: https://docs.rs/clap/latest/clap/struct.Command.html#method.allow_external_subcommands
        .and_then(|(_, sub_matches)| sub_matches.get_many::<OsString>(""))
        .map(|vals| vals.map(OsString::clone).collect())
        .unwrap_or_default()
    };

    let args = prefix_args.into_iter().chain(remaining_args).collect::<Vec<_>>();

    let mut command = config
        .execute_command(active_channel, &target_exe, &args)
        .with_context(|| format!("failed to run '{user_input}'"))?;

    let status = command.wait().with_context(|| {
        format!("error occurred while waiting for '{user_input}' to finish executing")
    })?;

    if status.success() {
        Ok(())
    } else {
        bail!("'{}' failed with status {}", user_input, status.code().unwrap_or(1))
    }
}

pub fn display_version(config: &Config) -> String {
    // NOTE: These files are generated in the project's build.rs.

    let compiled_cargo_version = include_str!(concat!(env!("OUT_DIR"), "/cargo_version.in"));

    let git_revision = include_str!(concat!(env!("OUT_DIR"), "/git_revision.in"));

    let midenup_version = env!(
        "CARGO_PKG_VERSION",
        "CARGO_PKG_VERSION environment variable not set.This should be set by cargo by default; \
         however, if not, it can be manually set using the `version` field in the Cargo.toml file"
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
            "<!--- (leave this at the bottom) --> midenup:{midenup_version}, toolchain: \
             {toolchain_version}, cargo:{cargo_version}, rev:{git_revision}"
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

/// Function that tries to resolve `argument` inside the `channel`.
fn resolve_argument(channel: &Channel, argument: &str) -> Result<MidenArgument, EnvironmentError> {
    let mut resolution = Err(EnvironmentError::UnknownArgument(format!(
        "Failed to resolve '{}': Neither known alias or component.",
        argument
    )));

    for comp in channel.components.iter() {
        if let Some(associated_command) = comp.aliases.get(argument) {
            return Ok(MidenArgument::Alias(comp.clone(), associated_command.to_owned()));
        } else if comp.name == argument {
            match comp.get_installed_file() {
                InstalledFile::Executable { alias_only: false, binary_name: _ } => {
                    resolution = Ok(MidenArgument::Component(comp.clone()));
                    break;
                },
                InstalledFile::Executable { alias_only: true, binary_name: _ } => {
                    let aliases = comp
                        .aliases
                        .keys()
                        .map(|alias| format!("'{}'", alias))
                        .collect::<Vec<_>>()
                        .join(", ");
                    resolution = Err(EnvironmentError::AliasOnly(format!(
                        "'{}' is not intended to be called via 'miden', but rather by its \
                         aliases: {aliases}",
                        comp.name
                    )));
                    break;
                },
                InstalledFile::Library { library_name, library_struct: _ } => {
                    return Err(EnvironmentError::LibraryAsExecutable(format!(
                        "'{}' installs the {} library. It is not intended to be executed as a \
                         binary.",
                        comp.name, library_name
                    )));
                },
            }
        }
    }

    resolution
}

/// Why the active channel falls back on the installed channel.
enum FallbackMotive {
    /// There simply is no active channel.
    NoActiveChannel,
    /// There is an active channel, yet the argument wasn't found.
    ArgumentNotInActiveChannel,
}
