use serde::{Deserialize, Serialize};

use semver;
use std::env::Args as CLIArgs;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
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

    #[error("ERROR: Couldn't find HOME directory")]
    CouldNotFindHome,

    #[error(
        "ERROR: .miden directory missing. Try running
miden-up init
"
    )]
    MidenDirMissing,

    #[error(
        "ERROR: No such toolchain available. The available toolchains are:
{0}"
    )]
    NoSuchToolChainAvailable(String),

    #[error("ERROR: Unrecognized channel: {0}")]
    NoSuchChannel(String),
}

// TODO: Implement differentiator between these two
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
enum Version {
    // A specific version of a package following a semantic or quasi-semantic
    // versioning scheme
    Semantic(String),
    // A version that simply points to a git repository. Analogous to
    // "nightly"/"trunk".
    // Git(String),
}

impl ToString for Version {
    fn to_string(&self) -> String {
        match &self {
            Version::Semantic(version) => version.to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Stdlib {
    version: Version,
}
// TODO: Make this a macro
impl Stdlib {
    fn as_cargo_dependency(&self) -> String {
        match &self.version {
            Version::Semantic(version) => format!("miden-stdlib = {{ version = \"{version}\" }}"),
        }
    }
}

impl MidenLib {
    fn as_cargo_dependency(&self) -> String {
        match &self.version {
            Version::Semantic(version) => format!("miden-lib = {{ version = \"{version}\" }}"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct MidenLib {
    version: Version,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Midenc {
    version: Version,
}

#[derive(Serialize, Deserialize, Debug)]
struct Toolchain {
    // This is the version that identifies the toolchain itself. Each component
    // from the toolchain will have its own version separately.
    version: String,

    stdlib: Stdlib,
    miden_lib: MidenLib,
    midenc: Midenc,
}

impl Toolchain {
    fn generate_cargo_toml(&self) -> String {
        let mut full_toml = String::new();

        let stdlib_link = self.stdlib.as_cargo_dependency();
        full_toml.push_str(stdlib_link.as_str());
        full_toml.push('\n');

        let miden_lib_link = self.miden_lib.as_cargo_dependency();
        full_toml.push_str(miden_lib_link.as_str());
        full_toml.push('\n');

        full_toml
    }
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

    fn get_toolchain(&self, toolchain_version: &str) -> Result<&Toolchain, MidenUpError> {
        let toolchain = self
            .stable
            .iter()
            .find(|toolchain| toolchain.version == toolchain_version);

        // TODO: Refactor using inspect_err
        if let Some(toolchain) = toolchain {
            Ok(toolchain)
        } else {
            let err_string = self
                .stable
                .iter()
                .fold(String::new(), |mut acc, toolchain| {
                    acc.push_str("- ");
                    acc.push_str(&toolchain.version);
                    acc.push('\n');
                    acc
                });
            return Err(MidenUpError::NoSuchToolChainAvailable(err_string));
        }
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

/// This is the first command the user runs after first installing the midenup. It:
/// - Install the current stable library
fn midenup_init(ctx: &mut Context) -> Result<(), MidenUpError> {
    // MIDENC_SYSROOT is where all the toolchains will live
    let miden_dir = &ctx.miden_dir;
    // Create the miden directory if are not already present
    fs::create_dir_all(&miden_dir)
        .map_err(|_| MidenUpError::CreateDirError(miden_dir.to_path_buf()))?;

    Ok(())
}

/// This is the first command the user runs. It:
/// - Install the current stable library
fn midenup_install(ctx: &mut Context) -> Result<(), MidenUpError> {
    let channel = ctx.args.next().ok_or(MidenUpError::MissingArgs)?;
    let chosen_toolchain = match channel.as_str() {
        "stable" => ctx.manifest.get_stable()?,
        chosen_version if semver::Version::parse(chosen_version).is_ok() => {
            ctx.manifest.get_toolchain(chosen_version)?
        }
        unrecognized => return Err(MidenUpError::NoSuchChannel(unrecognized.to_string())),
    };

    let version_string = &chosen_toolchain.version;

    let miden_dir = &ctx.miden_dir;
    let toolchain_dir = miden_dir.join(format!("toolchain-{version_string}"));
    unsafe {
        std::env::set_var("MIDENC_SYSROOT", miden_dir);
    }

    if !Path::new(miden_dir).exists() {
        return Err(MidenUpError::MidenDirMissing);
    }

    // Create the miden and toolchain directory if they are not already present
    fs::create_dir_all(&toolchain_dir)
        .map_err(|_| MidenUpError::CreateDirError(toolchain_dir.clone()))?;

    // Create install directory and script
    let install_dir = toolchain_dir.join("install");
    fs::create_dir_all(&install_dir)
        .map_err(|_| MidenUpError::CreateDirError(toolchain_dir.clone()))?;

    let install_file_path = install_dir.join("install").with_extension("rs");
    let mut install_file = fs::File::create(&install_file_path)
        .map_err(|_| MidenUpError::CreateFileError(install_file_path.clone()))?;

    let install_script_contents = generate_install_script(chosen_toolchain);
    install_file
        .write_all(&install_script_contents.into_bytes())
        .unwrap();

    let _output = Command::new("cargo")
        .args(["+nightly", "-Zscript", install_file_path.to_str().unwrap()])
        .output()
        .expect("failed to execute process");

    Ok(())
}

impl MidenupSubcommand {
    fn execute(&self, ctx: &mut Context) -> Result<(), MidenUpError> {
        match &self {
            MidenupSubcommand::Init => midenup_init(ctx),
            MidenupSubcommand::Install => midenup_install(ctx),
            MidenupSubcommand::Update => todo!(),
        }
    }
}

struct Context {
    // Latest available manifest
    manifest: Manifest,
    // Cli arguments
    args: CLIArgs,
    // Miden dir
    miden_dir: PathBuf,
}
impl Context {
    fn new(manifest: Manifest, args: CLIArgs) -> Result<Self, MidenUpError> {
        // MIDENC_SYSROOT is where all the toolchains will live
        let miden_dir = std::env::var("MIDENC_SYSROOT")
            .as_ref()
            .map(std::path::PathBuf::from)
            .unwrap_or(
                dirs::home_dir()
                    .ok_or(MidenUpError::CouldNotFindHome)?
                    .join(".miden"),
            );
        Ok(Context {
            manifest,
            args,
            miden_dir,
        })
    }
}

fn main() -> Result<(), MidenUpError> {
    // Ideally, this should be lazy. Maybe
    let manifest = fetch_miden_manifest().unwrap();
    let args = std::env::args();
    let mut context = Context::new(manifest, args)?;

    let command = context.args.next().ok_or(MidenUpError::MissingArgs)?;
    #[cfg(debug_assertions)]
    let command = command.split("/").last().expect(
        "Failed to remove path from executable. That get 'miden-up' from './target/debug/miden-up'. This is only a temporary messure.",
    );

    let subcommand = context.args.next().ok_or(MidenUpError::MissingArgs)?;
    let subcommand = MidenupSubcommand::from_str(&subcommand)?;

    subcommand
        .execute(&mut context)
        .inspect_err(|result| println!("{result}"))
}

// NOTE: Currenltly this function is mocked, in reality this file will be download from a github page available in the miden organization
fn fetch_miden_manifest() -> Result<Manifest, MidenUpError> {
    let manifest_file = std::path::Path::new("channel-miden.json");
    let contents =
        fs::read_to_string(manifest_file).map_err(|_| MidenUpError::ManifestUnreachable)?;
    let manifest: Manifest = serde_json::from_str(&contents).unwrap();
    Ok(manifest)
}

fn generate_install_script(install: &Toolchain) -> String {
    let repos = install.generate_cargo_toml();

    let toolchain_version = &install.version;

    format!("#!/usr/bin/env cargo
---cargo
[dependencies]
{repos}
---

use std::process::Command;

// NOTE: This file was generated by midenup. Do not edit by hand

fn main() {{
    // MIDENC_SYSROOT is set by the compiler when invoking this script, and will contain
    // the resolved (and prepared) sysroot path of `$XDG_DATA_DIR/miden`
    let miden_dir = std::path::Path::new(env!(\"MIDENC_SYSROOT\"));
    let toolchain_dir = miden_dir.join(format!(\"toolchain-{toolchain_version}\"));
    let lib_dir = toolchain_dir.join(\"lib\");

    // Write transaction kernel to $XDG_DATA_DIR/miden/<toolchain>/tx.masl
    let tx = miden_lib::MidenLib::default();
    let tx = tx.as_ref();
    (*tx)
        .write_to_file(lib_dir.join(\"miden-lib\").with_extension(\"masl\"))
        .unwrap();

    // Write stdlib to $XDG_DATA_DIR/miden/<toolchain>/std.masl
    let stdlib = miden_lib::StdLibrary::default();
    // let stdlib = miden_stdlib::StdLibrary::default(); //NOTE: Both imports work, which one should be used?
    let stdlib = stdlib.as_ref();
    (*stdlib)
        .write_to_file(lib_dir.join(\"std\").with_extension(\"masl\"))
        .unwrap();

    // NOTE: Commenting this out simply to save time.
    // // Install midenc
    //
    // Command::new(\"cargo\")
    //     .args([\"install\", \"midenc\", \"--root\", toolchain_dir.to_str().unwrap()])
    //     .output()
    //     .expect(\"failed to install compiler \");
}}
")
}
