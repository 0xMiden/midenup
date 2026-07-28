//! Building a component with `cargo install`.
//!
//! # On `CARGO_HOME`
//!
//! midenup deliberately does **not** override `CARGO_HOME` for builds, and `Config::cargo_home` is
//! not consulted here. The two things `cargo install` is told about location serve different
//! purposes:
//!
//! * `--root` decides where the built binary and Cargo's install bookkeeping go. That *is*
//!   isolated: every build targets the staging directory for the installation being produced.
//! * `CARGO_HOME` holds the registry index, the crate cache, git checkouts and credentials. Those
//!   are shared caches. Isolating them would mean re-downloading the crates.io index for every
//!   `MIDENUP_HOME`, which is slow and buys nothing -- a cache entry is identical whoever fetched
//!   it, and credentials are the user's.
//!
//! Concurrency is handled by the `MIDENUP_HOME` advisory lock rather than by cache isolation.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use crate::plan::{PlanStep, ResolvedAuthority};

#[derive(Debug, thiserror::Error)]
pub enum CargoError {
    #[error("failed to run cargo: {0}")]
    Spawn(String),
    #[error("`cargo {argv}` exited with {status}")]
    Failed { argv: String, status: String },
    #[error(
        "the build of '{crate_name}' did not produce the expected binary '{expected}' at '{dest}'"
    )]
    MissingBinary {
        crate_name: String,
        expected: String,
        dest: PathBuf,
    },
    #[error(
        "the build of '{crate_name}' produced unexpected binaries: {}",
        unexpected.join(", ")
    )]
    UnexpectedBinaries {
        crate_name: String,
        unexpected: Vec<String>,
    },
}

/// Cargo's own install bookkeeping.
///
/// These are installer-temporary: they describe what Cargo put in a `--root`, which is not the
/// same thing as what midenup owns. The receipt is the ownership record, so these are removed
/// before publication rather than shipped inside a toolchain.
const CARGO_BOOKKEEPING: &[&str] = &[".crates.toml", ".crates2.json"];

/// Builds `step`, leaving the binary at its planned destination.
pub fn build(
    step: &PlanStep,
    staging_root: &Path,
    verbose: bool,
    debug: bool,
) -> Result<(), CargoError> {
    let PlanStep::CargoBuild { crate_name, expect_binary, dest, .. } = step else {
        return Ok(());
    };

    let argv = argv_for(step, staging_root, verbose, debug);
    let rendered = argv
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");

    let status = std::process::Command::new("cargo")
        .args(&argv)
        .stderr(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .status()
        .map_err(|err| CargoError::Spawn(err.to_string()))?;

    if !status.success() {
        return Err(CargoError::Failed {
            argv: rendered,
            status: status.to_string(),
        });
    }

    verify_outputs(staging_root, crate_name, expect_binary, dest)
}

/// The exact argument vector for a build step.
///
/// Split out so the shape of the command can be asserted without running a compiler.
pub fn argv_for(step: &PlanStep, staging_root: &Path, verbose: bool, debug: bool) -> Vec<OsString> {
    let PlanStep::CargoBuild {
        crate_name,
        authority,
        features,
        rustup_channel,
        expect_binary,
        ..
    } = step
    else {
        return Vec::new();
    };

    let mut argv: Vec<OsString> = Vec::new();

    // Optional arguments are *omitted* when unset. The previous implementation rendered them as
    // empty strings, so cargo received a stray "" argument.
    if let Some(channel) = rustup_channel {
        argv.push(format!("+{channel}").into());
    }

    argv.push("install".into());
    argv.push("--locked".into());
    argv.push("--profile".into());
    argv.push(if debug { "dev".into() } else { "release".into() });
    if !verbose {
        argv.push("--quiet".into());
    }

    // Always name the binary. A multi-binary crate would otherwise deposit every binary it
    // defines into the toolchain, and the receipt would not describe what actually landed.
    argv.push("--bin".into());
    argv.push(expect_binary.into());

    match authority {
        ResolvedAuthority::Registry { version } => {
            argv.push(crate_name.into());
            argv.push("--version".into());
            argv.push(version.to_string().into());
        },
        ResolvedAuthority::Git { url, revision, .. } => {
            argv.push("--git".into());
            argv.push(url.into());
            // Always a concrete commit: the authority was pinned during planning.
            argv.push("--rev".into());
            argv.push(revision.into());
            argv.push(crate_name.into());
        },
        ResolvedAuthority::Path { canonical, .. } => {
            argv.push("--path".into());
            argv.push(canonical.into());
        },
    }

    if !features.is_empty() {
        argv.push("--features".into());
        argv.push(features.join(",").into());
    }

    argv.push("--root".into());
    argv.push(staging_root.into());

    argv
}

/// Confirms the build produced exactly what was planned, and clears Cargo's bookkeeping.
fn verify_outputs(
    staging_root: &Path,
    crate_name: &str,
    expect_binary: &str,
    dest: &Path,
) -> Result<(), CargoError> {
    if !dest.exists() {
        return Err(CargoError::MissingBinary {
            crate_name: crate_name.to_string(),
            expected: expect_binary.to_string(),
            dest: dest.to_path_buf(),
        });
    }

    // `--bin` should already prevent this, but a crate can define a binary under a different
    // name than its target, and an unexpected file in `bin/` would be published as though
    // midenup owned it.
    let bin_dir = staging_root.join("bin");
    let unexpected: Vec<String> = std::fs::read_dir(&bin_dir)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != expect_binary)
        .collect();

    if !unexpected.is_empty() {
        return Err(CargoError::UnexpectedBinaries {
            crate_name: crate_name.to_string(),
            unexpected,
        });
    }

    for name in CARGO_BOOKKEEPING {
        let _ = std::fs::remove_file(staging_root.join(name));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(authority: ResolvedAuthority, features: &[&str], channel: Option<&str>) -> PlanStep {
        PlanStep::CargoBuild {
            crate_name: "miden-vm".to_string(),
            authority,
            features: features.iter().map(|f| f.to_string()).collect(),
            rustup_channel: channel.map(|c| c.to_string()),
            expect_binary: "miden-vm".to_string(),
            dest: PathBuf::from("/staging/bin/miden-vm"),
            owner: "vm".to_string(),
        }
    }

    fn registry() -> ResolvedAuthority {
        ResolvedAuthority::Registry { version: semver::Version::new(0, 15, 0) }
    }

    fn rendered(argv: &[OsString]) -> Vec<String> {
        argv.iter().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    /// Regression: unset optional arguments were rendered as empty strings, so cargo received a
    /// stray "" for both the toolchain and the verbosity flag.
    #[test]
    fn unset_optional_arguments_are_omitted_entirely() {
        let argv =
            rendered(&argv_for(&step(registry(), &[], None), Path::new("/staging"), false, true));

        assert!(!argv.iter().any(|a| a.is_empty()), "no empty argv entries: {argv:?}");
        assert!(!argv.iter().any(|a| a == "+"), "no bare toolchain flag: {argv:?}");
        assert!(!argv.iter().any(|a| a == "--features"), "no features flag when none declared");
    }

    /// A multi-binary crate would otherwise deposit every binary it defines into the toolchain.
    #[test]
    fn the_expected_binary_is_always_named() {
        let argv =
            rendered(&argv_for(&step(registry(), &[], None), Path::new("/staging"), false, true));
        let index = argv.iter().position(|a| a == "--bin").expect("--bin must be passed");
        assert_eq!(argv[index + 1], "miden-vm");
    }

    #[test]
    fn a_rustup_channel_is_passed_first() {
        let argv = rendered(&argv_for(
            &step(registry(), &[], Some("nightly")),
            Path::new("/staging"),
            false,
            true,
        ));
        assert_eq!(argv[0], "+nightly", "the toolchain selector must precede the subcommand");
    }

    #[test]
    fn verbosity_suppresses_the_quiet_flag() {
        let quiet = rendered(&argv_for(&step(registry(), &[], None), Path::new("/s"), false, true));
        assert!(quiet.iter().any(|a| a == "--quiet"));

        let loud = rendered(&argv_for(&step(registry(), &[], None), Path::new("/s"), true, true));
        assert!(!loud.iter().any(|a| a == "--quiet"));
    }

    #[test]
    fn the_debug_flag_selects_the_profile() {
        let dev = rendered(&argv_for(&step(registry(), &[], None), Path::new("/s"), false, true));
        let index = dev.iter().position(|a| a == "--profile").unwrap();
        assert_eq!(dev[index + 1], "dev");

        let release =
            rendered(&argv_for(&step(registry(), &[], None), Path::new("/s"), false, false));
        let index = release.iter().position(|a| a == "--profile").unwrap();
        assert_eq!(release[index + 1], "release");
    }

    #[test]
    fn features_are_comma_joined() {
        let argv = rendered(&argv_for(
            &step(registry(), &["std", "concurrent"], None),
            Path::new("/s"),
            false,
            true,
        ));
        let index = argv.iter().position(|a| a == "--features").unwrap();
        assert_eq!(argv[index + 1], "std,concurrent");
    }

    #[test]
    fn a_registry_authority_passes_the_crate_and_version() {
        let argv = rendered(&argv_for(&step(registry(), &[], None), Path::new("/s"), false, true));
        assert!(argv.contains(&"miden-vm".to_string()));
        let index = argv.iter().position(|a| a == "--version").unwrap();
        assert_eq!(argv[index + 1], "0.15.0");
    }

    /// The authority was pinned during planning, so a git build always names a concrete commit.
    #[test]
    fn a_git_authority_passes_a_concrete_revision() {
        let authority = ResolvedAuthority::Git {
            url: "https://example.invalid/r.git".to_string(),
            revision: "abc123".to_string(),
            subpath: None,
        };
        let argv = rendered(&argv_for(&step(authority, &[], None), Path::new("/s"), false, true));

        let index = argv.iter().position(|a| a == "--rev").expect("--rev must be passed");
        assert_eq!(argv[index + 1], "abc123");
        assert!(!argv.iter().any(|a| a == "--branch"), "a branch must never reach cargo");
    }

    #[test]
    fn a_path_authority_passes_the_canonical_path() {
        let authority = ResolvedAuthority::Path {
            canonical: PathBuf::from("/src/miden-vm"),
            mtime: None,
        };
        let argv = rendered(&argv_for(&step(authority, &[], None), Path::new("/s"), false, true));
        let index = argv.iter().position(|a| a == "--path").unwrap();
        assert_eq!(argv[index + 1], "/src/miden-vm");
    }

    #[test]
    fn the_staging_root_is_always_passed() {
        let argv =
            rendered(&argv_for(&step(registry(), &[], None), Path::new("/staging"), false, true));
        let index = argv.iter().position(|a| a == "--root").expect("--root must be passed");
        assert_eq!(argv[index + 1], "/staging");
    }

    #[test]
    fn a_missing_expected_binary_is_an_error() {
        let dir = tempdir::TempDir::new("cargo-missing").unwrap();
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();

        let err = verify_outputs(dir.path(), "c", "miden-vm", &dir.path().join("bin/miden-vm"))
            .expect_err("must fail");
        assert!(matches!(err, CargoError::MissingBinary { .. }), "{err}");
    }

    /// An unexpected file in `bin/` would be published as though midenup owned it.
    #[test]
    fn unexpected_binaries_are_rejected() {
        let dir = tempdir::TempDir::new("cargo-extra").unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("miden-vm"), b"x").unwrap();
        std::fs::write(bin.join("stowaway"), b"x").unwrap();

        let err = verify_outputs(dir.path(), "c", "miden-vm", &bin.join("miden-vm"))
            .expect_err("must fail");
        assert!(
            matches!(&err, CargoError::UnexpectedBinaries { unexpected, .. }
                if unexpected == &["stowaway".to_string()]),
            "{err}"
        );
    }

    /// Cargo's bookkeeping describes what Cargo put in a `--root`, not what midenup owns.
    #[test]
    fn cargo_bookkeeping_is_removed_before_publication() {
        let dir = tempdir::TempDir::new("cargo-bookkeeping").unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("miden-vm"), b"x").unwrap();
        for name in CARGO_BOOKKEEPING {
            std::fs::write(dir.path().join(name), b"{}").unwrap();
        }

        verify_outputs(dir.path(), "c", "miden-vm", &bin.join("miden-vm")).expect("should succeed");

        for name in CARGO_BOOKKEEPING {
            assert!(!dir.path().join(name).exists(), "{name} must not be published");
        }
    }

    #[test]
    fn a_non_build_step_produces_no_argv() {
        let step = PlanStep::Download {
            uri: "https://example.invalid/x".to_string(),
            dest: PathBuf::from("/tmp/x"),
            mode: 0o755,
            owner: "x".to_string(),
            digest: None,
        };
        assert!(argv_for(&step, Path::new("/s"), false, true).is_empty());
        build(&step, Path::new("/s"), false, true).expect("a download is not a build");
    }
}
