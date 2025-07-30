mod channel;
mod commands;
mod config;
mod manifest;
mod toolchain;
mod utils;
mod version;

use std::{ffi::OsString, path::PathBuf, str::FromStr};

use anyhow::{anyhow, bail, Context};
use channel::{Channel, InstalledFile};
use clap::{Args, FromArgMatches, Parser, Subcommand};
use colored::Colorize;
use commands::INSTALLABLE_COMPONENTS;

pub use self::config::Config;
use self::{
    channel::UserChannel,
    manifest::{Manifest, ManifestError},
    toolchain::Toolchain,
};

#[derive(Debug, Parser)]
#[command(name = "midenup")]
#[command(multicall(true))]
#[command(author, version, about = "The Miden toolchain installer", long_about = None)]
pub struct Midenup {
    #[command(subcommand)]
    behavior: Behavior,
}

#[derive(Debug, Subcommand)]
enum Behavior {
    /// The Miden toolchain installer
    Midenup {
        #[command(flatten)]
        config: GlobalArgs,
        #[command(subcommand)]
        command: Commands,
    },
    /// Invoke components of the current Miden toolchain
    #[command(external_subcommand)]
    Miden(Vec<OsString>),
}

enum AliasError {
    UnrecognizedSubcommand,
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
        arguments: Vec<String>,
    },
    ExternalCommand {
        name: String,
        arguments: Vec<String>,
    },
}

impl CommandToExecute {
    fn get_exe(self, channel: &Channel) -> anyhow::Result<(String, Vec<String>)> {
        match self {
            CommandToExecute::MidenComponent { name, arguments } => {
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
                arguments: vec![String::from("account")],
            },
            MidenAliases::Faucet => CommandToExecute::MidenComponent {
                name: String::from("client"),
                arguments: vec![String::from("mint")],
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
                arguments: vec![String::from("new-wallet"), String::from("--deploy")],
            },
            MidenAliases::Call => CommandToExecute::MidenComponent {
                name: String::from("call"),
                arguments: vec![String::from("new-wallet"), String::from("--show")],
            },
            MidenAliases::Send => CommandToExecute::MidenComponent {
                name: String::from("client"),
                arguments: vec![String::from("send")],
            },
            MidenAliases::Simulate => CommandToExecute::MidenComponent {
                name: String::from("client"),
                arguments: vec![String::from("exec")],
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

#[derive(Debug, Subcommand)]
/// All the available Midenup Commands
enum Commands {
    /// Bootstrap the `midenup` environment.
    ///
    /// This initializes the `MIDEN_HOME` directory layout and configuration.
    Init,
    /// Install a Miden toolchain
    Install {
        /// The channel or version to install, e.g. `stable` or `0.15.0`
        #[arg(required(true), value_name = "CHANNEL", value_parser)]
        channel: UserChannel,
    },
    /// Uninstall a Miden toolchain
    Uninstall {
        /// The channel or version to install, e.g. `stable` or `0.15.0`
        #[arg(required(true), value_name = "CHANNEL", value_parser)]
        channel: UserChannel,
    },
    /// Show information about the midenup environment
    #[command(subcommand)]
    Show(commands::ShowCommand),
    /// Sets the current active miden toolchain for the current project.
    /// This creates a miden-toolchain.toml file in the present working directory.
    Set {
        /// The channel or version to set, e.g. `stable` or `0.15.0`
        #[arg(required(true), value_name = "CHANNEL", value_parser)]
        channel: UserChannel,
    },
    /// Sets the system's default toolchain.
    Override {
        /// The channel or version to set, e.g. `stable` or `0.15.0`
        #[arg(required(true), value_name = "CHANNEL", value_parser)]
        channel: UserChannel,
    },
    /// Update your installed Miden toolchains.
    Update {
        /// `midenup update`'s behavior differs depending on the specified [CHANNEL]
        /// - If provided, updates only the specified channel.
        /// - If left blank, then midenup will check for updates in all the downloaded toolchains.
        /// - If [CHANNEL] = stable, then it will look for the newest available toolchain and set
        ///   that to be stable.
        #[arg(value_name = "CHANNEL", value_parser)]
        channel: Option<UserChannel>,
    },
}

const DEFAULT_USER_DATA_DIR: &str = "XDG_DATA_HOME";

const MIDENUP_MANIFEST_URI_ENV: &str = "MIDENUP_MANIFEST_URI";
/// Global configuration options for `midenup`
#[derive(Debug, Args)]
struct GlobalArgs {
    /// The location of the Miden toolchain root
    #[arg(long, hide(true), value_name = "DIR", env = "MIDENUP_HOME")]
    midenup_home: Option<PathBuf>,
    /// The URI from which we should load the global toolchain manifest
    #[arg(
        long,
        hide(true),
        value_name = "FILE",
        env = MIDENUP_MANIFEST_URI_ENV,
        default_value = manifest::Manifest::PUBLISHED_MANIFEST_URI
    )]
    manifest_uri: String,
}

impl Commands {
    /// Execute the requested subcommand
    fn execute(&self, config: &Config, local_manifest: &mut Manifest) -> anyhow::Result<()> {
        match &self {
            Self::Init => commands::init(config),
            Self::Install { channel, .. } => {
                let Some(channel) = config.manifest.get_channel(channel) else {
                    bail!("channel '{}' doesn't exist or is unavailable", channel);
                };
                commands::install(config, channel, local_manifest)
            },
            Self::Uninstall { channel, .. } => {
                let Some(channel) = config.manifest.get_channel(channel) else {
                    bail!("channel '{}' doesn't exist or is unavailable", channel);
                };
                commands::uninstall(config, channel, local_manifest)
            },
            Self::Update { channel } => commands::update(config, channel.as_ref(), local_manifest),
            Self::Show(cmd) => cmd.execute(config, local_manifest),
            Self::Set { channel } => commands::set(config, channel),
            Self::Override { channel } => commands::r#override(config, channel),
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

fn main() -> anyhow::Result<()> {
    curl::init();

    let cli = <Midenup as clap::CommandFactory>::command();
    let matches = cli.get_matches();
    let cli = Midenup::from_arg_matches(&matches).map_err(|err| err.exit()).unwrap();

    let config = match cli.behavior {
        Behavior::Miden(_) => {
            // Always respect XDG dirs if set
            let midenup_home = std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .map(|dir| dir.join("midenup"))
                .or_else(|| dirs::data_dir().map(|dir| dir.join("midenup")))
                // If for whatever reason, we can't access the data dir, we fall
                // back to .local/share
                .or_else(|| {
                    std::env::home_dir()
                        .map(|home| home.join(".local").join("share"))
                })
                .ok_or_else(||
                            anyhow!("Failed to set midenup directory.\
                                     Consider setting a value for XDG_DATA_HOME in your shell's profile"
                            )
                )?;

            let manifest_uri = std::env::var(MIDENUP_MANIFEST_URI_ENV)
                .unwrap_or(manifest::Manifest::PUBLISHED_MANIFEST_URI.to_string());
            Config::init(midenup_home, manifest_uri)?
        },
        Behavior::Midenup { ref config, .. } => {
            let midenup_home = config
                .midenup_home
                .clone()
                .or_else(|| {
                    // Always respect XDG dirs if set
                    std::env::var_os("XDG_DATA_HOME")
                        .map(PathBuf::from)
                        .map(|dir| dir.join("midenup"))
                })
                .or_else(|| dirs::data_dir().map(|dir| dir.join("midenup")))
                // If for whatever reason, we can't access the data dir, we fall
                // back to .local/share
                .or_else(|| {
                    std::env::home_dir()
                        .map(|home| home.join(".local").join("share"))
                })
                .ok_or_else(||
                            anyhow!("Failed to set midenup directory.\
                                     Consider setting a value for XDG_DATA_HOME in your shell's profile"
                            )
                )?;

            Config::init(midenup_home, &config.manifest_uri)?
        },
    };
    std::dbg!(&config.manifest);

    // Manifest that stores locally installed toolchains
    let mut local_manifest = {
        let local_manifest_path = config.midenup_home.join("manifest").with_extension("json");
        let local_manifest_uri = format!(
            "file://{}",
            local_manifest_path.to_str().context("Couldn't convert miden directory")?,
        );
        match Manifest::load_from(local_manifest_uri) {
            Ok(manifest) => Ok(manifest),
            Err(ManifestError::Empty | ManifestError::Missing(_)) => Ok(Manifest::default()),
            Err(err) => Err(err),
        }
        .context("Error parsing local manifest")
    }?;

    match cli.behavior {
        Behavior::Miden(argv) => {
            // // Make sure we know the current toolchain so we can modify the PATH appropriately
            // let toolchain = Toolchain::ensure_current_is_installed(&config, &mut local_manifest)?;
            // Extract the target binary to execute from argv[1]
            let subcommand = {
                let subcommand = argv.get(1).ok_or(anyhow!(
                    "No arguments were passed to `miden`. To get a list of available commands, run:
miden help"
                ))?;
                subcommand.to_str().expect("Invalid command name: {subcommand}")
            };

            let (target_exe, prefix_args, include_rest_of_args): (String, Vec<String>, _) = {
                if subcommand == "help" {
                    match argv.get(2).and_then(|c| c.to_str()) {
                        None => {
                            std::println!("{}", default_help());
                            return Ok(());
                        },
                        Some("components") => {
                            // Make sure we know the current toolchain so we can modify the PATH appropriately
                            let toolchain = Toolchain::ensure_current_is_installed(
                                &config,
                                &mut local_manifest,
                            )?;
                            let channel = local_manifest
                                .get_channel(&toolchain.channel)
                                .context("Couldn't find active toolchain in the manifest.")?;

                            let help = components_help(channel);

                            std::println!("{}", help);

                            return Ok(());
                        },
                        Some(component) => {
                            // Make sure we know the current toolchain so we can modify the PATH appropriately
                            let toolchain = Toolchain::ensure_current_is_installed(
                                &config,
                                &mut local_manifest,
                            )?;
                            let channel = local_manifest
                                .get_channel(&toolchain.channel)
                                .context("Couldn't find active toolchain in the manifest.")?;

                            let component =
                                channel.get_component(component).with_context(|| {
                                    format!(
                                        "Couldn't find component {} in the current channel: {}.",
                                        component, channel.name
                                    )
                                })?;

                            let installed_file = component.get_installed_file();
                            let InstalledFile::InstalledExecutable(binary) = installed_file else {
                                bail!(
                                    "Can't execute component {}; since it is not an executable ",
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
                } else {
                    // Make sure we know the current toolchain so we can modify the PATH appropriately
                    let toolchain =
                        Toolchain::ensure_current_is_installed(&config, &mut local_manifest)?;
                    let channel = local_manifest
                        .get_channel(&toolchain.channel)
                        .context("Couldn't find active toolchain in the manifest.")?;

                    if let Ok(alias) = MidenAliases::from_str(subcommand) {
                        let command = alias.resolve(channel);
                        let (target_exe, prefix_args) = command.get_exe(channel)?;

                        (target_exe, prefix_args, true)
                    } else if let Some(component) = channel.get_component(subcommand) {
                        std::dbg!(&channel);
                        let installed_file = component.get_installed_file();
                        let InstalledFile::InstalledExecutable(binary) = installed_file else {
                            bail!(
                                "Can't execute component {}; since it is not an executable ",
                                component.name
                            )
                        };
                        (binary, vec![], true)
                    } else {
                        bail!(
                            "Unknown subcommand: {}. \
                         To get a full list of available commmands, run:\
                         miden help",
                            subcommand
                        );
                    }
                }
            };

            let toolchain = Toolchain::ensure_current_is_installed(&config, &mut local_manifest)?;
            std::dbg!(&target_exe, &prefix_args, &include_rest_of_args);

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
        },
        Behavior::Midenup { command: subcommand, .. } => {
            subcommand.execute(&config, &mut local_manifest)
        },
    }
}

fn handle_miden_command(args: Vec<OsString>) {}

/// Wrapper function that handles help messaging dispatch
// fn handle_help(component: Option<&str>) -> anyhow::Result<HelpMessage> {
//     if let Some(component) = component {
//         // if let Ok(component) = MidenComponents::from_str(component) {
//         //     Ok(component.help_command())
//         // } else
//         if let Ok(command) = MidenAliases::from_str(component) {
//             Ok(command.help_command())
//         } else {
//             bail!(
//                 "Unrecognized command {}. To see available commands, run:
// miden help",
//                 component
//             )
//         }
//     } else {
//         Ok(HelpMessage::Internal { help_message: default_help() })
//     }
// }
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
  help components        Print this help message
  help <COMPONENT>       Print <COMPONENTS>'s help message
",
        "Usage:".bold().underline(),
        "miden".bold(),
        available_components,
    )
}

fn default_help() -> String {
    // Note:
    let aliases: String = [
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

Available aliases:
{}

Help:
  help                   Print this help message
  help <COMPONENT>       Print <COMPONENTS>'s help message
",
        "Usage:".bold().underline(),
        "miden".bold(),
        available_components,
        aliases
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::version::Authority;
    type LocalManifest = Manifest;
    use crate::{channel::*, manifest::*, *};

    /// Simple auxiliary function to setup a midneup directory environment in
    /// tests.
    fn test_setup(midenup_home: &Path, manifest_uri: &str) -> (LocalManifest, Config) {
        let local_manifest = {
            let local_manifest_path = midenup_home.join("manifest").with_extension("json");
            let local_manifest_uri = format!(
                "file://{}",
                local_manifest_path.to_str().expect("Couldn't convert miden directory"),
            );

            match Manifest::load_from(local_manifest_uri) {
                Ok(manifest) => Ok(manifest),
                Err(ManifestError::Empty | ManifestError::Missing(_)) => Ok(Manifest::default()),
                Err(err) => Err(err),
            }
            .unwrap_or_else(|_| {
                panic!("Failed to parse manifest {}", local_manifest_path.display())
            })
        };

        let config =
            Config::init(midenup_home.to_path_buf().clone(), manifest_uri).unwrap_or_else(|_| {
                panic!(
                    "Failed construct config from manifest {} and midenup_home at {}",
                    manifest_uri,
                    midenup_home.display(),
                )
            });

        (local_manifest, config)
    }

    #[test]
    /// Tries to install the "stable" toolchain from the present manifest.
    fn integration_install_stable() {
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();
        let midenup_home = tmp_home_path.join("midenup");

        const FILE: &str = "file://manifest/channel-manifest.json";

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE);

        let install = Commands::Install { channel: UserChannel::Stable };
        install.execute(&config, &mut local_manifest).expect("Failed to install stable");

        // After install is executed, the local manifest should be present
        let manifest = midenup_home.join("manifest").with_extension("json");
        assert!(manifest.exists());

        let stable_dir = midenup_home.join("toolchains").join("stable");
        assert!(stable_dir.exists());
        assert!(stable_dir.is_symlink());

        let stable_channel = local_manifest
            .get_latest_stable()
            .expect("No stable channel found; despite having installed stable");

        // We test if the in-memory representation of the local manifest
        // contains the stable alias
        assert_eq!(stable_channel.alias, Some(ChannelAlias::Stable));

        // We read the filesystem again, to check that the "stable" alias was
        // correclty saved
        assert_eq!(
            local_manifest
                .get_channels()
                .next()
                .expect(
                    "ERROR: The local_manifest in the filesystem has no alias, when it should have stable alias"
                )
                .alias.as_ref().expect("ERROR: The installed stable toolchain should be marked as stable in the local manifest"),
            &ChannelAlias::Stable
        );

        tmp_home.close().expect("Couldn't delete tmp midenup home directory");
    }

    #[test]
    /// First, use a manifest file to install the stable toolchain under version
    /// 0.14.0. Then, update said manifest and try to update stable to the newer
    /// version
    fn integration_update_stable() {
        // NOTE: Currentlty "update stable" maintains the old stable toolchain
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();

        let midenup_home = tmp_home_path.join("midenup");

        const FILE_PRE_UPDATE: &str = "file://tests/data/update-stable/manifest-pre-update.json";

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE_PRE_UPDATE);

        let install = Commands::Install { channel: UserChannel::Stable };
        install.execute(&config, &mut local_manifest).expect("Failed to install stable");
        // After install is executed, the local manifest should be present
        let manifest = midenup_home.join("manifest").with_extension("json");
        assert!(manifest.exists());
        let stable_dir = midenup_home.join("toolchains").join("stable");
        assert!(stable_dir.exists());
        assert!(stable_dir.is_symlink());

        const FILE_POST_UPDATE: &str = "file://tests/data/update-stable/manifest-post-update.json";

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE_POST_UPDATE);

        let update = Commands::Update { channel: Some(UserChannel::Stable) };
        update.execute(&config, &mut local_manifest).expect("Failed to update stable");

        // Now there should be two channels. The old stable (no longer marked as
        // such) and the new stable channel
        assert_eq!(local_manifest.get_channels().count(), 2);
        let old_stable = local_manifest
            .get_channel(&UserChannel::Version(semver::Version::new(0, 14, 0)))
            .expect("Couldn't find old stable channel via version");
        assert_eq!(old_stable.alias, None);

        let new_stable = local_manifest
            .get_channel(&UserChannel::Version(semver::Version::new(0, 15, 0)))
            .expect("Couldn't find old stable channel via version");
        assert_eq!(new_stable.alias, Some(ChannelAlias::Stable));

        // Now we check if the structure is correclty saved in the filesystem
        let (local_manifest, _) = test_setup(&midenup_home, FILE_POST_UPDATE);
        let old_stable = local_manifest
            .get_channel(&UserChannel::Version(semver::Version::new(0, 14, 0)))
            .expect("Couldn't find old stable channel via version");
        assert_eq!(old_stable.alias, None);

        let new_stable = local_manifest
            .get_channel(&UserChannel::Version(semver::Version::new(0, 15, 0)))
            .expect("Couldn't find old stable channel via version");
        assert_eq!(new_stable.alias, Some(ChannelAlias::Stable));

        let toolchain_dir = midenup_home.join("toolchains");
        let _old_stable = toolchain_dir.join("0.14.0");
        let new_stable = toolchain_dir.join("0.15.0");
        let stable_symlink = toolchain_dir.join("stable");

        assert!(stable_symlink.exists());
        assert!(stable_symlink.is_symlink());

        let stable_dir = std::fs::read_link(stable_symlink.as_path())
            .expect("Couldn't obtain directory where the stable directory is pointing to");
        assert_eq!(stable_dir, new_stable);

        tmp_home.close().expect("Couldn't delete tmp midenup home directory");
    }

    #[test]
    /// First, use a manifest file to install the version 0.14.0.  Then, use a
    /// newer manifest to display an update in the std component and a downgrade
    /// in base. After triggering an update, check if those components got
    /// updated successfully.
    fn integration_update_specific_component() {
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();

        let midenup_home = tmp_home_path.join("midenup");

        const FILE_PRE_UPDATE: &str =
            "file://tests/data/update-specific/manifest-pre-component-update.json";

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE_PRE_UPDATE);

        let install = Commands::Install {
            channel: UserChannel::Version(semver::Version::new(0, 14, 0)),
        };
        install.execute(&config, &mut local_manifest).expect("Failed to install 0.14.0");
        // After install is executed, the local manifest should be present
        let manifest = midenup_home.join("manifest").with_extension("json");
        assert!(manifest.exists());
        let version = semver::Version::new(0, 14, 0);
        let old_std = local_manifest
            .get_channel(&UserChannel::Version(version.clone()))
            .expect("Local manifest didn't register version 0.14.0 despite having being installed")
            .get_component("std").expect("Local manifest didn't save the std component despite being present in the upstream manifest");
        if let Authority::Cargo { version, .. } = old_std.version.clone() {
            // 0.13.0 is the version of the std library saved in FILE_PRE_UPDATE
            assert_eq!(version, semver::Version::new(0, 13, 0))
        } else {
            panic!("The old std's authority is not Cargo, despite having been installed with it");
        }

        // This is used for debugging purposes in case the test fails.
        let mut show_toolchain_dir = std::process::Command::new("tree")
            .arg(tmp_home_path)
            .stderr(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .spawn()
            .expect("Couldn't execute tree command");
        let _ = show_toolchain_dir.wait().expect("Failed to execute tree");

        const FILE_POST_UPDATE: &str =
            "file://tests/data/update-specific/manifest-post-component-update.json";

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE_POST_UPDATE);

        let update = Commands::Update {
            channel: Some(UserChannel::Version(semver::Version::new(0, 14, 0))),
        };
        update.execute(&config, &mut local_manifest).expect("Failed to update stable");
        let new_std = local_manifest
            .get_channel(&UserChannel::Version(version.clone()))
            .expect("Local manifest didn't register version 0.14.0 despite having being installed")
            .get_component("std").expect("Local manifest didn't save the std component despite being present in the upstream manifest");

        if let Authority::Cargo { version, .. } = new_std.version.clone() {
            // 0.14.0 is the newer version
            assert_eq!(version, semver::Version::new(0, 14, 0))
        } else {
            panic!(
                "The updated std's authority is not Cargo, despite having been installed with it"
            );
        }
    }

    #[test]
    /// Install a specific component and then try to check if midenup update
    /// registers it got rolled back
    fn integration_rollback_specific_component() {
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();
        let midenup_home = tmp_home_path.join("midenup");

        const FILE_PRE_UPDATE: &str =
            "file://tests/data/rollback-component/manifest-pre-component-rollback.json";

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE_PRE_UPDATE);

        let install = Commands::Install {
            channel: UserChannel::Version(semver::Version::new(0, 14, 0)),
        };
        install.execute(&config, &mut local_manifest).expect("Failed to install 0.14.0");
        // After install is executed, the local manifest should be present
        let manifest = midenup_home.join("manifest").with_extension("json");
        assert!(manifest.exists());

        let toolchain_path = midenup_home.join("toolchains").join("0.14.0");
        assert!(toolchain_path.join("installation-successful").exists());
        assert!(toolchain_path.exists());

        // This is used for debugging purposes in case the test fails.
        let mut show_toolchain_dir = std::process::Command::new("tree")
            .arg(tmp_home_path)
            .stderr(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .spawn()
            .expect("Couldn't execute tree command");

        let _ = show_toolchain_dir.wait().expect("Failed to execute tree");

        let version = semver::Version::new(0, 14, 0);
        let std = local_manifest
            .get_channel(&UserChannel::Version(version.clone()))
            .expect("Local manifest didn't register version 0.14.0 despite having being installed")
            .get_component("std").expect("Local manifest didn't save the std component despite being present in the upstream manifest");
        if let Authority::Cargo { version, .. } = std.version.clone() {
            // 0.13.0 is the version of the std library saved in FILE_PRE_UPDATE
            assert_eq!(version, semver::Version::new(0, 14, 0))
        } else {
            panic!("The old std's authority is not Cargo, despite having been installed with it");
        }

        const FILE_POST_UPDATE: &str =
            "file://tests/data/rollback-component/manifest-post-component-rollback.json";

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE_POST_UPDATE);

        let update = Commands::Update {
            channel: Some(UserChannel::Version(semver::Version::new(0, 14, 0))),
        };
        update.execute(&config, &mut local_manifest).expect("Failed to update stable");
        let rolled_back_std = local_manifest
            .get_channel(&UserChannel::Version(version.clone()))
            .expect("Local manifest didn't register version 0.14.0 despite having being installed")
            .get_component("std").expect("Local manifest didn't save the std component despite being present in the upstream manifest");

        if let Authority::Cargo { version, .. } = rolled_back_std.version.clone() {
            // 0.14.0 is the newer version
            assert_eq!(version, semver::Version::new(0, 13, 0))
        } else {
            panic!(
                "The updated std's authority is not Cargo, despite having been installed with it"
            );
        }
    }

    #[test]
    #[should_panic]
    /// This 'midenc' component present in this manifest is lacking its required
    /// 'rustup_channel" and thus should fail to compile.
    fn midenup_catches_installation_failure() {
        let tmp_home = tempdir::TempDir::new("midenup").expect("Couldn't create temp-dir");
        let tmp_home_path = tmp_home.path();
        let midenup_home = tmp_home_path.join("midenup");

        const FILE_PRE_UPDATE: &str = "file://tests/data/manifest-uncompilable-midenc.json";

        let (mut local_manifest, config) = test_setup(&midenup_home, FILE_PRE_UPDATE);

        let install = Commands::Install { channel: UserChannel::Stable };
        install.execute(&config, &mut local_manifest).expect("Failed to install stable");
        // After install is executed, the local manifest should be present
        let manifest = midenup_home.join("manifest").with_extension("json");
        assert!(manifest.exists());
    }
}
