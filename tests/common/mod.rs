#![allow(unused)]

use std::path::{Path, PathBuf};

use midenup::{config, manifest};
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

pub type LocalManifest = manifest::Manifest;

/// Simple auxiliary function to setup a midneup directory environment in tests.
///
/// Additionally, it changes the PWD to a new temp dir to isolate test execution.
pub fn test_setup(midenup_home: &Path, manifest_uri: &str) -> (LocalManifest, config::Config) {
    let local_manifest = {
        let local_manifest_path = midenup_home.join("manifest").with_extension("json");
        let local_manifest_uri = format!(
            "file://{}",
            local_manifest_path.to_str().expect("Couldn't convert miden directory"),
        );

        match manifest::Manifest::load_from(local_manifest_uri) {
            Ok(manifest) => Ok(manifest),
            Err(manifest::ManifestError::Empty | manifest::ManifestError::Missing(_)) => {
                Ok(manifest::Manifest::default())
            },
            Err(err) => Err(err),
        }
        .unwrap_or_else(|_| panic!("Failed to parse manifest {}", local_manifest_path.display()))
    };

    let config = config::Config::init(midenup_home.to_path_buf().clone(), manifest_uri, true)
        .unwrap_or_else(|err| {
            panic!(
                "Failed to construct config from manifest {} and midenup_home at {}.
Error: {}",
                manifest_uri,
                midenup_home.display(),
                err,
            )
        });

    (local_manifest, config)
}

// NOTE: We save this variables in this struct because if they ever go out of scope, the created
// directory get deleted.
pub struct TestEnvironment {
    pub tmp_dir: TempDir,
    pub midenup_dir: PathBuf,
    pub cargo_home: PathBuf,
    pub present_working_dir: PathBuf,
}

pub fn environment_setup(test_name: &str) -> TestEnvironment {
    let tmp_dir =
        tempdir::TempDir::new(&format!("midenup-{test_name}")).expect("Couldn't create temp-dir");

    let tmp_present_working_directory = tmp_dir.path().join("test-working-directory");

    let tmp_midenup_home = tmp_dir.path().join("midenup");

    let tmp_cargo_home = tmp_dir.path().join("cargo");

    std::fs::create_dir(&tmp_present_working_directory).unwrap();

    // Set CARGO_HOME in order to not use the system's default `.cargo/bin` directory.
    unsafe {
        std::env::set_var("CARGO_HOME", &tmp_cargo_home);
    }

    std::env::set_current_dir(&tmp_present_working_directory).unwrap_or_else(|err| {
        panic!(
            "Failed to switch to {}, because of {err}",
            tmp_present_working_directory.display()
        )
    });

    TestEnvironment {
        tmp_dir,
        midenup_dir: tmp_midenup_home,
        cargo_home: tmp_cargo_home,
        present_working_dir: tmp_present_working_directory,
    }
}
