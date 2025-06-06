use serde::{Deserialize, Serialize};

use std::env::Args as CLIArgs;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

use clap::{Parser, Subcommand};
use thiserror::Error;

#[derive(Error, Debug)]
enum MidenUpError {
    #[error("ERROR: data store disconnected {0}")]
    CreateDirError(PathBuf),

    #[error("ERROR: Could not create file in {0}")]
    CreateFileError(PathBuf),

    #[error(
        "ERROR: Missing arguments:
Format is: miden-up <command> <arguments>"
    )]
    MissingArgs,

    #[error("ERROR: Couldn't fetch manifest from <link>")]
    ManifestUnreachable,

    #[error("ERROR: Ill-formated manifest")]
    ManifestFormatError,

    #[error("ERROR: Invalid toolchain selected {0}")]
    ToolchainNotFound(String),

    #[error("ERROR: Unknown subcommand")]
    UnknownSubcommand,

    #[error("ERROR: Empty manifest: The current manifest has no toolchains")]
    EmptyManifest,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
struct Stdlib {
    version: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
struct MidenLib {
    version: String,
}

#[derive(Default, Serialize, Deserialize, Debug, Clone)]
struct Midenc {
    version: String,
}

#[derive(Default, Serialize, Deserialize, Debug)]
struct Toolchain {
    // This is the version that identifies the toolchain itself. Each component
    // from the toolchain will have its own version separately.
    version: String,

    stdlib: Stdlib,
    miden_lib: MidenLib,
    midenc: Midenc,
}

#[derive(Serialize, Deserialize, Debug, Default)]
struct Manifest {
    manifest_version: String,
    date: String,
    stable: Vec<Toolchain>,
}

impl Manifest {
    fn get_stable(&self) -> Result<&Toolchain, MidenUpError> {
        self.stable
            .iter()
            .max_by(|ver_x, ver_y| ver_x.version.cmp(&ver_y.version))
            .ok_or(MidenUpError::EmptyManifest)
    }
}

enum MidenupSubcommand {
    Init,
    Install,
    Update,
}

impl FromStr for MidenupSubcommand {
    type Err = MidenUpError;

    // Required method
    fn from_str(subcommand: &str) -> Result<Self, Self::Err> {
        match subcommand {
            "init" => Ok(MidenupSubcommand::Init),
            "install" => Ok(MidenupSubcommand::Install),
            "update" => Ok(MidenupSubcommand::Update),
            _ => Err(MidenUpError::UnknownSubcommand),
        }
    }
}
// fn process_miden_up_install(args: &mut CLIArgs) -> Result<(), MidenUpError> {
// }

/// This is the first command the user runs. It:
/// - Install the current stable library
fn midenup_init(ctx: &mut Context) -> Result<(), MidenUpError> {
    let stable = ctx.manifest.get_stable()?;
    std::dbg!(stable);
    todo!()
}

impl MidenupSubcommand {
    fn execute(&self, ctx: &mut Context) -> Result<(), MidenUpError> {
        match &self {
            MidenupSubcommand::Init => midenup_init(ctx),
            MidenupSubcommand::Install => todo!(),
            MidenupSubcommand::Update => todo!(),
        }
    }
}

struct Context {
    // Latest available manifest
    manifest: Manifest,
    // Cli arguments
    args: CLIArgs,
}
fn main() -> Result<(), MidenUpError> {
    // Ideally, this should be lazy. Maybe
    let manifest = fetch_miden_manifest().unwrap();
    let args = std::env::args();
    let mut context = Context { manifest, args };

    let command = context.args.next().ok_or(MidenUpError::MissingArgs)?;
    #[cfg(debug_assertions)]
    let command = command.split("/").last().expect(
        "Failed to remove path from executable. That get 'miden-up' from './target/debug/miden-up'. This is only a temporary messure.",
    );

    let subcommand = context.args.next().ok_or(MidenUpError::MissingArgs)?;
    let subcommand = MidenupSubcommand::from_str(&subcommand)?;

    subcommand.execute(&mut context)?;
    Ok(())
}

// NOTE: Currenltly this function is mocked, in reality this file will be download from a github page available in the miden organization
fn fetch_miden_manifest() -> Result<Manifest, MidenUpError> {
    let manifest_file = std::path::Path::new("channel-miden.json");
    let contents =
        fs::read_to_string(manifest_file).map_err(|_| MidenUpError::ManifestUnreachable)?;
    let manifest: Manifest = serde_json::from_str(&contents).unwrap();
    Ok(manifest)
}
