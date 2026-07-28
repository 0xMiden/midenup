use clap::Parser;
use midenup::{commands::Midenup, paths, state::PublicationRef};

mod common;

use common::*;

/// `midenup gc` reclaims publications nothing refers to, and only those.
///
/// This is the only thing that ever reclaims a replaced publication: republishing deliberately
/// leaves its predecessor on disk, because another process may still be executing out of it.
#[test]
fn integration_gc_removes_orphans_and_never_touches_referenced_publications() {
    let _guard = common::harness::mutating_test_guard();
    let test_env = environment_setup("integration_gc");

    let fixture = common::harness::OfflineFixture::build(test_env.tmp_dir.path(), "0.15.0");
    let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);
    let channel = semver::Version::new(0, 15, 0);

    Midenup::try_parse_from(["midenup", "install", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to install");

    let PublicationRef::Managed { id, .. } = &state.get(&channel).unwrap().publication else {
        panic!("expected a managed publication");
    };
    let replaced = paths::publication_dir(&test_env.midenup_home, &channel, id);

    // Republish, which leaves the first publication behind.
    Midenup::try_parse_from(["midenup", "install", "0.15.0", "--profile", "complete"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("failed to republish");

    let PublicationRef::Managed { id, .. } = &state.get(&channel).unwrap().publication else {
        panic!("expected a managed publication");
    };
    let live = paths::publication_dir(&test_env.midenup_home, &channel, id);
    assert!(replaced.is_dir(), "the replaced publication is what gc exists to reclaim");

    // Plus something that was never recorded at all -- a staging tree from a run that died before
    // it could be journalled, say.
    let orphan = paths::publications_dir(&test_env.midenup_home).join("0.15.0-orphaned");
    std::fs::create_dir_all(&orphan).unwrap();

    Midenup::try_parse_from(["midenup", "gc"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("gc failed");

    assert!(!replaced.exists(), "the replaced publication must be reclaimed");
    assert!(!orphan.exists(), "an unrecorded publication must be reclaimed");
    assert!(live.is_dir(), "the referenced publication must survive");
    assert!(
        live.join("bin").join("miden-vm").exists(),
        "and must survive intact, not merely as a directory"
    );

    // Idempotent: a second run finds nothing and changes nothing.
    Midenup::try_parse_from(["midenup", "gc"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("gc must be idempotent");
    assert!(live.is_dir());

    // ...and the toolchain still works afterwards.
    let toolchain = paths::toolchain_link(&test_env.midenup_home, &channel);
    assert!(toolchain.join("opt").join("miden vm").symlink_metadata().is_ok());
}
