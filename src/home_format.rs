//! The home format envelope, `$MIDENUP_HOME/home-format.json`.
//!
//! One integer format covers everything under the home: state, layout, receipts and journals.
//! Every entry point checks it before reading or changing anything else, and refuses a format it
//! does not know without changing the home.
//!
//! ```json
//! { "format": 1 }
//! { "format": 1, "conversion": "released-v1" }
//! ```
//!
//! A home without an envelope is either empty, which gets one on its first write, or a home written
//! by a midenup release that predates it, which is preflighted and converted. Anything else is
//! refused unchanged.
//!
//! Conversion has one commit point: replacing the envelope with the target format and the
//! conversion it is about to run. The steps after it are idempotent, so the `conversion` marker is
//! the whole journal: a run that finds it repeats the steps and then clears it.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{lock, paths, publish, state::LocalState};

/// The home format this build reads and writes.
pub const HOME_FORMAT: u64 = 1;

/// How to start over without touching a home that is refused.
const CLEAN_INSTALL: &str = "Nothing in it was changed. To start over without touching it, use a \
                             separate home: MIDENUP_HOME=<new dir> midenup init";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub format: u64,
    /// A committed conversion that has not finished yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversion: Option<Conversion>,
}

/// A conversion recorded at the commit point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Conversion {
    /// A home written by a midenup release that predates the envelope.
    ReleasedV1,
}

#[derive(Debug, thiserror::Error)]
pub enum HomeFormatError {
    #[error("failed to read '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("'{path}' is not a home format midenup can read: {reason}. {}", CLEAN_INSTALL)]
    Malformed { path: PathBuf, reason: String },
    #[error(
        "'{path}' declares home format {found}, but this midenup supports format {}. Upgrade \
         midenup with the documented installer or `cargo install --locked --force midenup` to use \
         this home. {}",
        HOME_FORMAT,
        CLEAN_INSTALL
    )]
    Unsupported { path: PathBuf, found: u64 },
    #[error(
        "'{home}' is not a midenup home this version can use: {reason}. {}",
        CLEAN_INSTALL
    )]
    Unrecognized { home: PathBuf, reason: String },
    #[error(
        "'{home}' was written by a pre-1.0 midenup ({reason}), and this midenup cannot convert \
         it. To keep its installations, run any command with midenup v1.3.0 (`cargo install \
         --locked --force midenup --version 1.3.0`) once to convert it, then upgrade again. {}",
        CLEAN_INSTALL
    )]
    AlphaEra { home: PathBuf, reason: String },
    #[error(transparent)]
    Lock(#[from] lock::LockError),
    #[error("failed to convert '{home}': {reason}")]
    Convert { home: PathBuf, reason: String },
    #[error("failed to write '{path}': {reason}")]
    Write { path: PathBuf, reason: String },
    #[error(transparent)]
    Injected(#[from] crate::fault::InjectedFault),
}

/// Checks the home format and returns the shared lease to hold until the command finishes.
///
/// A home predating the envelope is converted first; the flag says whether that happened, so a
/// caller holding state read before this call knows to reload it.
pub fn enter(home: &Path) -> Result<(lock::FormatLease, bool), HomeFormatError> {
    let mut converted = false;
    loop {
        // Refuse before the lease file is created, so a refused home is left as it was. The
        // converter checks again under its locks.
        if read(home)?.is_none() && !is_empty(home)? {
            preflight(home)?;
        }
        let lease = lock::shared_lease(home)?;
        match read(home)? {
            Some(Envelope { conversion: None, .. }) => return Ok((lease, converted)),
            // Initialized by its first writer: see `initialize_if_empty`.
            None if is_empty(home)? => return Ok((lease, converted)),
            _ => {},
        }
        drop(lease);
        convert(home)?;
        converted = true;
    }
}

/// Writes the envelope into an empty home. Writers call this once they hold `HomeLock`.
pub fn initialize_if_empty(home: &Path) -> Result<(), HomeFormatError> {
    if read(home)?.is_none() && is_empty(home)? {
        write(home, &Envelope { format: HOME_FORMAT, conversion: None })?;
    }
    Ok(())
}

/// Reads the envelope, refusing a format other than [`HOME_FORMAT`] and anything malformed.
pub fn read(home: &Path) -> Result<Option<Envelope>, HomeFormatError> {
    let path = paths::home_format_path(home);
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(HomeFormatError::Io { path, source }),
    };
    let malformed = |reason: String| HomeFormatError::Malformed { path: path.clone(), reason };

    let value: serde_json::Value =
        serde_json::from_str(&contents).map_err(|err| malformed(err.to_string()))?;
    let found = value
        .get("format")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| malformed("it has no integer 'format'".to_string()))?;
    if found != HOME_FORMAT {
        return Err(HomeFormatError::Unsupported { path, found });
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|err| malformed(err.to_string()))
}

/// Atomically replaces the envelope and syncs the home directory, so the replacement is durable.
fn write(home: &Path, envelope: &Envelope) -> Result<(), HomeFormatError> {
    let path = paths::home_format_path(home);
    let failed = |reason: String| HomeFormatError::Write { path: path.clone(), reason };
    std::fs::create_dir_all(home).map_err(|err| failed(err.to_string()))?;
    crate::utils::atomic::write_validated(&path, envelope, |written| {
        serde_json::from_str::<Envelope>(written)
            .map(|_| ())
            .map_err(|err| err.to_string())
    })
    .map_err(|err| failed(err.to_string()))?;
    std::fs::File::open(home)
        .and_then(|dir| dir.sync_all())
        .map_err(|err| failed(err.to_string()))
}

/// Whether the home records nothing installed: no state, and only caches, empty directories, lock
/// files, temporaries and entries midenup does not write.
fn is_empty(home: &Path) -> Result<bool, HomeFormatError> {
    for name in visible_entries(home)? {
        let empty = match name.as_str() {
            "toolchains" | "publications" | "journal" => {
                visible_entries(&home.join(&name))?.is_empty()
            },
            "state.json" | "manifest.json" | "var" | "opt" => false,
            _ => true,
        };
        if !empty {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The names in `dir`, without lock files and temporaries; none when it does not exist.
fn visible_entries(dir: &Path) -> Result<Vec<String>, HomeFormatError> {
    let io = |source| HomeFormatError::Io { path: dir.to_path_buf(), source };
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(io(source)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let name = entry.map_err(io)?.file_name().to_string_lossy().into_owned();
        if !name.starts_with('.') {
            names.push(name);
        }
    }
    Ok(names)
}

/// Converts the home under the exclusive lease and `HomeLock`, or resumes a committed conversion.
fn convert(home: &Path) -> Result<(), HomeFormatError> {
    let _lease = lock::exclusive_lease(home)?;
    let _lock = lock::acquire(home)?;

    let conversion = match read(home)? {
        Some(Envelope { conversion: None, .. }) => return Ok(()),
        // Committed by an earlier run: finish it from the record alone.
        Some(Envelope { conversion: Some(conversion), .. }) => conversion,
        None if is_empty(home)? => return Ok(()),
        None => {
            let conversion = preflight(home)?;
            write(
                home,
                &Envelope {
                    format: HOME_FORMAT,
                    conversion: Some(conversion),
                },
            )?;
            crate::fault::fail_at(crate::fault::FaultPoint::PostFormatCommit)?;
            conversion
        },
    };

    match conversion {
        Conversion::ReleasedV1 => convert_released_v1(home)?,
    }
    write(home, &Envelope { format: HOME_FORMAT, conversion: None })?;
    crate::info!("converted '{}' to home format {HOME_FORMAT}", home.display());
    Ok(())
}

/// Removes carried-forward toolchains' alpha directories and rewrites `state.json` at the current
/// state version. Everything else already reads as it is.
fn convert_released_v1(home: &Path) -> Result<(), HomeFormatError> {
    let path = paths::state_path(home);
    if !path.exists() {
        return Ok(());
    }
    let failed = |reason: String| HomeFormatError::Convert { home: home.to_path_buf(), reason };
    let state = LocalState::load(&path).map_err(|err| failed(err.to_string()))?;
    for channel in carried_forward(&state) {
        let dir = paths::toolchains_dir(home).join(channel);
        if std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir()) {
            std::fs::remove_dir_all(&dir)
                .map_err(|err| failed(format!("failed to remove '{}': {err}", dir.display())))?;
        }
    }
    state.save(&path).map_err(|err| failed(err.to_string()))
}

/// Checks a home without an envelope as a whole, before anything in it changes.
///
/// Read-only. Accepts only what releases that predate the envelope leave behind.
fn preflight(home: &Path) -> Result<Conversion, HomeFormatError> {
    let unrecognized =
        |reason: String| HomeFormatError::Unrecognized { home: home.to_path_buf(), reason };
    let alpha = |reason: &str| HomeFormatError::AlphaEra {
        home: home.to_path_buf(),
        reason: reason.to_string(),
    };

    let state_path = paths::state_path(home);
    let state = LocalState::load(&state_path).map_err(|err| unrecognized(err.to_string()))?;

    for name in visible_entries(home)? {
        match name.as_str() {
            "state.json"
            | "channel-manifest.json"
            | "toolchains"
            | "publications"
            | "var"
            | "journal"
            | "opt"
            | "cache" => {},
            // A leftover beside real state is inert; without it, it is the only record.
            "manifest.json" if !state.installations.is_empty() => {},
            "manifest.json" => return Err(alpha("it holds 'manifest.json' and no installations")),
            // Not midenup's, and the conversion leaves it alone.
            _ => {},
        }
    }

    let carried_forward = carried_forward(&state);
    check_links(home, &paths::toolchains_dir(home), true, &carried_forward)?;

    // Every release that leaves anything installed writes it.
    if !state_path.exists() {
        return Err(unrecognized("it has no 'state.json'".to_string()));
    }

    for installation in &state.installations {
        let crate::state::PublicationRef::Managed { id, .. } = &installation.publication else {
            continue;
        };
        // A missing publication is divergence, which recovery reports with a remedy.
        let publication = paths::publication_dir(home, &installation.channel, id);
        if publish::receipt_path(&publication).exists() {
            publish::read_receipt(&publication).map_err(|err| unrecognized(err.to_string()))?;
        }
    }

    publish::journal::read(home).map_err(|err| unrecognized(err.to_string()))?;

    Ok(Conversion::ReleasedV1)
}

/// The toolchains v1.x carried forward from an alpha home, whose alpha directory it left in place.
fn carried_forward(state: &LocalState) -> BTreeSet<String> {
    state
        .installations
        .iter()
        .filter(|i| i.custom.is_none())
        .filter(|i| matches!(i.publication, crate::state::PublicationRef::NeedsReinstall))
        .map(|i| i.channel.to_string())
        .collect()
}

/// Every toolchain entry must be a link, except a carried-forward toolchain's alpha directory;
/// only the top level may hold the `custom` directory.
fn check_links(
    home: &Path,
    dir: &Path,
    top: bool,
    carried_forward: &BTreeSet<String>,
) -> Result<(), HomeFormatError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(HomeFormatError::Io { path: dir.to_path_buf(), source }),
    };
    for entry in entries {
        let entry =
            entry.map_err(|source| HomeFormatError::Io { path: dir.to_path_buf(), source })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = entry
            .file_type()
            .map_err(|source| HomeFormatError::Io { path: entry.path(), source })?;
        if name.starts_with('.') {
            continue;
        }
        if top && name == "stable" {
            return Err(HomeFormatError::AlphaEra {
                home: home.to_path_buf(),
                reason: "it has a 'toolchains/stable' link".to_string(),
            });
        }
        if top && name == "custom" && file_type.is_dir() {
            check_links(home, &entry.path(), false, carried_forward)?;
        } else if top && file_type.is_dir() && carried_forward.contains(&name) {
            // Reinstalled on first use, which needs the link's place.
        } else if !file_type.is_symlink() {
            return Err(HomeFormatError::AlphaEra {
                home: home.to_path_buf(),
                reason: format!("'{}' is not a link", entry.path().display()),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> (tempdir::TempDir, PathBuf) {
        let temp = tempdir::TempDir::new("home-format").unwrap();
        let home = temp.path().join("midenup");
        (temp, home)
    }

    #[test]
    fn an_empty_home_is_initialized_by_its_first_writer_only() {
        let (_temp, home) = home();
        let (lease, converted) = enter(&home).unwrap();
        assert!(!converted);
        assert_eq!(read(&home).unwrap(), None, "entering alone must not write an envelope");
        drop(lease);

        initialize_if_empty(&home).unwrap();
        assert_eq!(read(&home).unwrap(), Some(Envelope { format: 1, conversion: None }));
    }

    #[test]
    fn unknown_and_malformed_envelopes_are_refused() {
        let (_temp, home) = home();
        std::fs::create_dir_all(&home).unwrap();
        let path = paths::home_format_path(&home);
        for (contents, unsupported) in [
            (r#"{"format":2}"#, true),
            (r#"{"format":0}"#, true),
            (r#"{"format":"1"}"#, false),
            (r#"{"format":1.5}"#, false),
            (r#"{"format":1,"conversion":"from-the-future"}"#, false),
            (r#"{"format":1,"extra":true}"#, false),
            ("not json", false),
        ] {
            std::fs::write(&path, contents).unwrap();
            let err = enter(&home).expect_err(contents);
            assert_eq!(
                matches!(err, HomeFormatError::Unsupported { .. }),
                unsupported,
                "{contents}: {err}"
            );
            assert!(err.to_string().contains("MIDENUP_HOME=<new dir> midenup init"), "{err}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), contents);
        }
    }

    #[test]
    fn a_newer_format_names_both_formats() {
        let (_temp, home) = home();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(paths::home_format_path(&home), r#"{"format":7}"#).unwrap();
        let message = enter(&home).unwrap_err().to_string();
        assert!(message.contains("format 7") && message.contains("format 1"), "{message}");
    }

    #[test]
    fn entries_midenup_does_not_write_are_left_alone() {
        let (_temp, home) = home();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("notes.txt"), "mine").unwrap();
        let (lease, converted) = enter(&home).unwrap();
        assert!(!converted, "a home holding only someone else's file is empty");
        drop(lease);

        std::fs::write(paths::state_path(&home), r#"{"state_version":"1.0.0","installations":[]}"#)
            .unwrap();
        let (_lease, converted) = enter(&home).unwrap();
        assert!(converted);
        assert_eq!(std::fs::read_to_string(home.join("notes.txt")).unwrap(), "mine");
    }

    #[test]
    fn a_pre_publication_toolchain_directory_is_refused() {
        let (_temp, home) = home();
        std::fs::create_dir_all(paths::toolchains_dir(&home).join("0.15.0")).unwrap();
        assert!(matches!(enter(&home), Err(HomeFormatError::AlphaEra { .. })));
        assert_eq!(read(&home).unwrap(), None);
    }

    #[test]
    fn a_carried_forward_toolchain_directory_is_removed_by_the_conversion() {
        let (_temp, home) = home();
        let alpha = paths::toolchains_dir(&home).join("0.15.0");
        std::fs::create_dir_all(alpha.join("bin")).unwrap();
        std::fs::write(alpha.join("bin").join("miden-vm"), "alpha").unwrap();
        let other = paths::toolchains_dir(&home).join("0.16.0");
        let mut state = LocalState::default();
        state.upsert(crate::state::Installation {
            channel: semver::Version::new(0, 15, 0),
            custom: None,
            intent: Default::default(),
            components: Vec::new(),
            publication: crate::state::PublicationRef::NeedsReinstall,
            installed_at: 1,
            midenup_version: None,
            patches: Default::default(),
        });
        state.save(&paths::state_path(&home)).unwrap();

        std::fs::create_dir_all(&other).unwrap();
        assert!(
            matches!(enter(&home), Err(HomeFormatError::AlphaEra { .. })),
            "a directory no carried-forward record names is still refused"
        );
        std::fs::remove_dir(&other).unwrap();

        let (_lease, converted) = enter(&home).unwrap();
        assert!(converted);
        assert!(!alpha.exists(), "its place is needed for the reinstall's link");
    }

    #[test]
    fn a_home_holding_only_caches_and_empty_directories_is_empty() {
        let (_temp, home) = home();
        std::fs::create_dir_all(paths::toolchains_dir(&home)).unwrap();
        std::fs::create_dir_all(paths::publications_dir(&home)).unwrap();
        std::fs::write(paths::manifest_cache(&home), "{}").unwrap();
        let (lease, converted) = enter(&home).unwrap();
        assert!(!converted);
        drop(lease);
        initialize_if_empty(&home).unwrap();
        assert_eq!(read(&home).unwrap(), Some(Envelope { format: 1, conversion: None }));
    }

    #[test]
    fn a_home_with_links_and_no_state_is_refused() {
        let (_temp, home) = home();
        std::fs::create_dir_all(paths::toolchains_dir(&home)).unwrap();
        std::os::unix::fs::symlink("0.15.0", paths::network_link(&home, "mainnet")).unwrap();
        assert!(matches!(enter(&home), Err(HomeFormatError::Unrecognized { .. })));
        assert_eq!(read(&home).unwrap(), None);
    }

    #[test]
    fn a_recorded_conversion_is_resumed() {
        let (_temp, home) = home();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(paths::state_path(&home), r#"{"state_version":"1.0.0","installations":[]}"#)
            .unwrap();
        write(
            &home,
            &Envelope {
                format: 1,
                conversion: Some(Conversion::ReleasedV1),
            },
        )
        .unwrap();

        let (_lease, converted) = enter(&home).unwrap();
        assert!(converted);
        assert_eq!(read(&home).unwrap(), Some(Envelope { format: 1, conversion: None }));
        let state = std::fs::read_to_string(paths::state_path(&home)).unwrap();
        assert!(state.contains("2.0.0"), "{state}");
    }
}
