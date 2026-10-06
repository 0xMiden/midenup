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

    let fixture = common::harness::OfflineFixture::create(test_env.tmp_dir.path(), "0.15.0");
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

#[test]
fn integration_gc_reclaims_cargo_caches_without_publication_orphans() {
    let test_env = environment_setup("integration_gc_cargo_cache");
    let fixture = common::harness::OfflineFixture::create(test_env.tmp_dir.path(), "0.15.0");
    let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);
    let orphan = paths::cargo_build_cache(&test_env.midenup_home).join("a".repeat(64));
    std::fs::create_dir_all(&orphan).unwrap();
    std::fs::write(orphan.join("artifact"), "obsolete build output").unwrap();

    Midenup::try_parse_from(["midenup", "gc"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .expect("gc failed");

    assert!(!orphan.exists(), "gc must reclaim caches even without publication orphans");
}

/// Two variants may share a source cache. Removing one does not make that cache disposable;
/// changing the last user's source does, unless a pending installation still needs it.
#[test]
fn integration_gc_retains_shared_and_pending_cargo_caches_and_preserves_user_data() {
    use midenup::{
        channel::UserChannel,
        identity::CustomToolchain,
        publish::journal::{self, JournalEntry},
        state::PublicationId,
        version::Authority,
    };

    let test_env = environment_setup("integration_gc_shared_cache");
    let fixture = common::harness::OfflineFixture::create(test_env.tmp_dir.path(), "0.15.0");
    let (mut state, config) = test_setup(&test_env, &fixture.manifest_uri);
    Midenup::try_parse_from(["midenup", "install", "0.15.0"])
        .unwrap()
        .execute_with_state(&config, &mut state)
        .unwrap();

    let source = test_env.tmp_dir.path().join("patch-source");
    std::fs::create_dir(&source).unwrap();
    let source = source.canonicalize().unwrap();
    std::fs::write(source.join("source.rs"), "user source").unwrap();
    let newer_source = source.with_file_name("new-patch-source");
    std::fs::create_dir(&newer_source).unwrap();
    let newer_source = newer_source.canonicalize().unwrap();
    let cache_root = paths::cargo_build_cache(&test_env.midenup_home);
    let cache = paths::cargo_source_cache(&cache_root, &source);
    let newer_cache = paths::cargo_source_cache(&cache_root, &newer_source);
    for directory in [&cache, &newer_cache] {
        std::fs::create_dir_all(directory).unwrap();
        std::fs::write(directory.join("artifact"), "build output").unwrap();
    }
    // Only owned directories are collected: a symlink must never expose a source to removal.
    std::os::unix::fs::symlink(&source, cache_root.join("source-link")).unwrap();
    let runtime = paths::var_dir(&test_env.midenup_home, &"custom:second".parse().unwrap());
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::write(runtime.join("data"), "user data").unwrap();

    let base = state.get(&semver::Version::new(0, 15, 0)).unwrap().clone();
    let named = |name: &str, path: &std::path::Path| {
        let mut installation = base.clone();
        installation.custom = Some(CustomToolchain {
            name: name.parse().unwrap(),
            channel: UserChannel::Version(base.channel.clone()),
        });
        installation.components[0].version = Authority::Path {
            path: path.to_path_buf(),
            last_modification: None,
        };
        installation
    };
    let first = named("first", &source);
    let second = named("second", &source);
    state.upsert(first.clone());
    state.upsert(second.clone());

    midenup::commands::gc(&config, &state).unwrap();
    assert!(cache.join("artifact").is_file());
    assert!(!newer_cache.exists(), "unused source cache is collectible");

    state.remove_by_id(&first.id());
    midenup::commands::gc(&config, &state).unwrap();
    assert!(cache.is_dir(), "the second variant still needs the shared cache");

    state.upsert(named("second", &newer_source));
    let pending = JournalEntry::install(base.channel, None, PublicationId::generate(), first);
    journal::prepare(&test_env.midenup_home, &pending).unwrap();
    // Missing source trees do not invalidate recorded references to their caches.
    std::fs::rename(&source, source.with_file_name("temporarily-moved-source")).unwrap();
    midenup::commands::gc(&config, &state).unwrap();
    assert!(cache.is_dir(), "the journal's target still needs the shared cache");
    std::fs::rename(source.with_file_name("temporarily-moved-source"), &source).unwrap();

    std::fs::remove_file(journal::entry_path(&test_env.midenup_home, &pending.id)).unwrap();
    midenup::commands::gc(&config, &state).unwrap();
    assert!(!cache.exists(), "source replacement releases the last cache reference");
    assert_eq!(std::fs::read_to_string(source.join("source.rs")).unwrap(), "user source");
    assert_eq!(std::fs::read_to_string(runtime.join("data")).unwrap(), "user data");
    assert!(
        cache_root
            .join("source-link")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
