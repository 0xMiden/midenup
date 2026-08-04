#![allow(unused)]

pub mod harness;

use std::path::{Path, PathBuf};

use midenup::config;
use tempdir::TempDir;

#[macro_export]
macro_rules! full_path_manifest {
    ($file:expr) => {
        concat!("file://", full_path!($file))
    };
}

#[macro_export]
macro_rules! full_path {
    ($file:expr) => {
        concat!(env!("CARGO_MANIFEST_DIR"), "/", $file)
    };
}

pub type LocalManifest = midenup::state::LocalState;

/// Runs the real `midenup` binary against a test environment.
pub fn run_midenup(
    env: &TestEnvironment,
    manifest_uri: &str,
    args: &[&str],
) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_midenup"))
        .args(args)
        .current_dir(&env.present_working_dir)
        .env("MIDENUP_HOME", &env.midenup_home)
        .env("CARGO_HOME", &env.cargo_home)
        .env("MIDENUP_MANIFEST_URI", manifest_uri)
        .output()
        .expect("failed to run midenup")
}

pub fn test_setup(env: &TestEnvironment, manifest_uri: &str) -> (LocalManifest, config::Config) {
    let state = midenup::state::LocalState::load(&env.midenup_home.join("state.json"))
        .unwrap_or_else(|err| panic!("failed to load local state: {err}"));

    let config = config::Config::init(
        env.present_working_dir.clone(),
        env.midenup_home.clone(),
        env.cargo_home.clone(),
        manifest_uri,
        true,
    )
    .unwrap_or_else(|err| {
        panic!(
            "Failed to construct config from manifest {} and midenup_home at {}.
Error: {}",
            manifest_uri,
            env.midenup_home.display(),
            err,
        )
    });

    (state, config)
}

// NOTE: We save this variables in this struct because if they ever go out of scope, the created
// directory get deleted.
pub struct TestEnvironment {
    pub tmp_dir: TempDir,
    pub midenup_home: PathBuf,
    pub cargo_home: PathBuf,
    pub present_working_dir: PathBuf,
}

/// Simple auxiliary function to setup a midneup directory environment in tests.
///
/// Additionally, it changes the PWD to a new temp dir to isolate test execution.
pub fn environment_setup(test_name: &str) -> TestEnvironment {
    let tmp_dir =
        tempdir::TempDir::new(&format!("midenup-{test_name}")).expect("Couldn't create temp-dir");

    let tmp_present_working_directory = tmp_dir.path().join("test-working-directory");

    let tmp_midenup_home = tmp_dir.path().join("midenup");

    let tmp_cargo_home = tmp_dir.path().join("cargo");

    std::fs::create_dir(&tmp_present_working_directory).unwrap();

    TestEnvironment {
        tmp_dir,
        midenup_home: tmp_midenup_home,
        cargo_home: tmp_cargo_home,
        present_working_dir: tmp_present_working_directory,
    }
}
