//! Restart recovery at every labelled point in the publication protocol.
//!
//! Requires the `fault-injection` feature, which compiles the abort points:
//!
//! ```bash
//! make recovery-test
//! ```
//!
//! Without it the whole file compiles away, because a build that cannot be made to stop mid-publish
//! has nothing to prove here.
#![cfg(feature = "fault-injection")]

use clap::Parser;
use midenup::{
    commands::Midenup,
    fault::{FAULT_POINT_ENV, FaultPoint},
    paths,
    state::{LocalState, PublicationRef},
};

mod common;

use common::*;

/// Runs `midenup install 0.15.0` with `point` armed, from a freshly loaded state.
///
/// Loading state from disk each time is the point: it is what a restarted process does, and it is
/// the only way an in-process test can observe what the *filesystem* was left in rather than what
/// the previous call happened to leave in memory.
fn install_aborting_at(
    env: &TestEnvironment,
    manifest_uri: &str,
    point: Option<FaultPoint>,
) -> anyhow::Result<()> {
    // Safety: integration tests run one process per test under nextest, and the mutating guard
    // serializes the rest. Nothing else in this process reads the variable concurrently.
    unsafe {
        match point {
            Some(point) => std::env::set_var(FAULT_POINT_ENV, point.as_str()),
            None => std::env::remove_var(FAULT_POINT_ENV),
        }
    }

    let (mut state, config) = test_setup(env, manifest_uri);
    Midenup::try_parse_from(["midenup", "install", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .map(|_| ())
}

/// Runs a command that does nothing but let recovery happen, as a restart would.
fn restart(env: &TestEnvironment, manifest_uri: &str) -> LocalState {
    unsafe {
        std::env::remove_var(FAULT_POINT_ENV);
    }

    let (mut state, config) = test_setup(env, manifest_uri);
    Midenup::try_parse_from(["midenup", "show", "active-toolchain"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .ok();

    LocalState::load(&paths::state_path(&env.midenup_home)).expect("state must load after recovery")
}

/// For every labelled point in the publication protocol, stopping there and restarting must leave
/// exactly one consistent state: either the install happened completely, or it did not happen at
/// all. Never a half-published toolchain, and never a state record with nothing behind it.
#[test]
fn integration_recovery_is_deterministic_at_every_publication_step() {
    let _guard = common::harness::mutating_test_guard();
    let channel = semver::Version::new(0, 15, 0);

    for point in FaultPoint::PUBLICATION {
        let env = environment_setup(&format!("recovery_{point}"));
        let fixture = common::harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");

        let aborted = install_aborting_at(&env, &fixture.manifest_uri, Some(point));
        assert!(aborted.is_err(), "the install must stop at {point}");

        let state = restart(&env, &fixture.manifest_uri);

        // Whatever happened, no operation may still be pending afterwards.
        assert!(
            midenup::publish::journal::read(&env.midenup_home).unwrap().is_none(),
            "{point}: the journal must be resolved by recovery"
        );

        let link = paths::toolchain_link(&env.midenup_home, &channel);
        match state.get(&channel) {
            // The commit point had been passed: the install must be complete and usable.
            Some(installation) => {
                assert!(
                    matches!(
                        point,
                        FaultPoint::PostCommit | FaultPoint::PostRecord | FaultPoint::PostDerive
                    ),
                    "{point}: an install recorded as complete must have passed the commit point"
                );

                let PublicationRef::Managed { id, .. } = &installation.publication else {
                    panic!("{point}: a recovered install must reference its publication");
                };
                let publication = paths::publication_dir(&env.midenup_home, &channel, id);

                assert!(
                    publication.join("bin").join("miden-vm").exists(),
                    "{point}: the recorded publication must contain what it claims"
                );
                assert_eq!(
                    std::fs::canonicalize(&link).unwrap(),
                    std::fs::canonicalize(&publication).unwrap(),
                    "{point}: the toolchain link must resolve to the recorded publication"
                );
            },
            // The commit point had not been reached: nothing may remain of the attempt.
            None => {
                assert!(
                    matches!(
                        point,
                        FaultPoint::PostPrepare | FaultPoint::PostStage | FaultPoint::PostVerify
                    ),
                    "{point}: an install past the commit point must not be discarded"
                );
                assert!(
                    std::fs::symlink_metadata(&link).is_err(),
                    "{point}: a discarded install must leave no toolchain link"
                );

                let publications = paths::publications_dir(&env.midenup_home);
                let leftovers: Vec<_> = std::fs::read_dir(&publications)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|entry| entry.file_name())
                    .collect();
                assert!(
                    leftovers.is_empty(),
                    "{point}: the staged publication must be discarded, found {leftovers:?}"
                );
            },
        }
    }
}

/// A second attempt after an abort must succeed, and must leave nothing of the abandoned attempt
/// behind.
///
/// This is the case a user actually hits: the install failed, so they run it again. A discarded
/// staging tree is deleted immediately -- unlike a *replaced* publication, nothing can ever have
/// been executing out of one that was never published.
#[test]
fn integration_recovery_allows_the_operation_to_be_retried() {
    let _guard = common::harness::mutating_test_guard();
    let channel = semver::Version::new(0, 15, 0);

    for point in FaultPoint::PUBLICATION {
        let env = environment_setup(&format!("retry_{point}"));
        let fixture = common::harness::OfflineFixture::create(env.tmp_dir.path(), "0.15.0");

        install_aborting_at(&env, &fixture.manifest_uri, Some(point))
            .expect_err("the install must stop");
        // Recovery runs at the start of the retry, not only on a read-only command.
        install_aborting_at(&env, &fixture.manifest_uri, None).expect("the retry must succeed");

        let state = LocalState::load(&paths::state_path(&env.midenup_home)).unwrap();
        let installation = state.get(&channel).expect("the retry must record the install");
        let PublicationRef::Managed { id, .. } = &installation.publication else {
            panic!("expected a managed publication");
        };

        let publications: Vec<_> = std::fs::read_dir(paths::publications_dir(&env.midenup_home))
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            publications.contains(&format!("0.15.0-{id}")),
            "{point}: the recorded publication must be on disk; found {publications:?}"
        );

        // How many are left says which side of the commit point the abort was on. A staging tree
        // that was never published is discarded outright -- nothing can have been running from it.
        // One that *was* published is replaced by the retry and left for `midenup gc`, because
        // another process may still be executing out of it.
        let expected = match point {
            FaultPoint::PostPrepare | FaultPoint::PostStage | FaultPoint::PostVerify => 1,
            FaultPoint::PostCommit | FaultPoint::PostRecord | FaultPoint::PostDerive => 2,
            // Not reachable: this loop walks `PUBLICATION`, and migration has its own commit
            // point and its own test.
            FaultPoint::PreMigrationCommit => unreachable!("not a publication step"),
        };
        assert_eq!(
            publications.len(),
            expected,
            "{point}: unexpected publications on disk: {publications:?}"
        );
    }
}
