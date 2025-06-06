use serde::{Deserialize, Serialize};

use std::fs;
use std::path::PathBuf;

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

fn main() {
    let manifest = fetch_miden_manifest().unwrap();

    std::dbg!(manifest);
}

// NOTE: Currenltly this function is mocked, in reality this file will be download from a github page available in the miden organization
fn fetch_miden_manifest() -> Result<Manifest, MidenUpError> {
    let manifest_file = std::path::Path::new("channel-miden.json");
    let contents =
        fs::read_to_string(manifest_file).map_err(|_| MidenUpError::ManifestUnreachable)?;
    let manifest: Manifest = serde_json::from_str(&contents).unwrap();
    Ok(manifest)
}
