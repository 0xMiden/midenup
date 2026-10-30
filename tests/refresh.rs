//! Refreshing the upstream manifest, its cache, and local commands that need neither.

use std::path::Path;

mod common;

use common::{harness::OfflineFixture, *};

const UNREACHABLE: &str = "https://127.0.0.1:1/nope.json";

/// The fixture's manifest for `channel`, with `edit` applied to its JSON.
fn manifest(root: &Path, channel: &str, edit: impl FnOnce(&mut serde_json::Value)) -> String {
    let fixture = OfflineFixture::create(root, channel);
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fixture.manifest_path).unwrap()).unwrap();
    edit(&mut manifest);
    manifest.to_string()
}

#[derive(Clone, Copy, Debug)]
enum Upstream {
    Compatible,
    RaisedMinimum,
    UnsupportedSchema,
    Malformed,
    Unavailable,
}

#[derive(Clone, Copy, Debug)]
enum Cache {
    Valid,
    Absent,
    /// Written before `min_client_version` existed.
    Unusable,
}

/// `list` against every kind of upstream, with every kind of cache.
///
/// Upstream publishes 0.15.0 and the cache 0.14.0, so the listing shows which one was used.
#[test]
fn integration_list_reports_each_refresh_failure_and_keeps_the_cache() {
    use Upstream::*;

    let _guard = common::harness::mutating_test_guard();
    for upstream in [Compatible, RaisedMinimum, UnsupportedSchema, Malformed, Unavailable] {
        for cache in [Cache::Valid, Cache::Absent, Cache::Unusable] {
            let case = format!("{upstream:?} upstream, {cache:?} cache");
            let env = environment_setup("refresh_matrix");
            let root = env.tmp_dir.path();

            let upstream_path = root.join("upstream.json");
            let contents = match upstream {
                Compatible => Some(manifest(&root.join("up"), "0.15.0", |_| {})),
                RaisedMinimum => Some(manifest(&root.join("up"), "0.15.0", |m| {
                    m["min_client_version"] = "999.0.0".into();
                })),
                UnsupportedSchema => Some(manifest(&root.join("up"), "0.15.0", |m| {
                    m["manifest_version"] = "4.0.0".into();
                })),
                Malformed => Some("{ not json".to_string()),
                Unavailable => None,
            };
            if let Some(contents) = &contents {
                std::fs::write(&upstream_path, contents).unwrap();
            }

            let cache_path = midenup::paths::manifest_cache(&env.midenup_home);
            let cached = match cache {
                Cache::Valid => Some(manifest(&root.join("cached"), "0.14.0", |_| {})),
                Cache::Absent => None,
                Cache::Unusable => Some(manifest(&root.join("cached"), "0.14.0", |m| {
                    m.as_object_mut().unwrap().remove("min_client_version");
                })),
            };
            if let Some(cached) = &cached {
                std::fs::create_dir_all(&env.midenup_home).unwrap();
                std::fs::write(&cache_path, cached).unwrap();
            }

            let output =
                run_midenup(&env, &format!("file://{}", upstream_path.display()), &["list"]);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let cache_after = std::fs::read_to_string(&cache_path).ok();

            if let Compatible = upstream {
                assert!(output.status.success(), "{case}: {stderr}");
                assert!(stdout.contains("0.15.0"), "{case}: upstream must be listed: {stdout}");
                assert_eq!(cache_after, contents, "{case}: the fetched bytes must be cached");
                continue;
            }

            let expected: &[&str] = match upstream {
                Compatible => unreachable!(),
                RaisedMinimum => &[
                    "requires a minimum client version of `999.0.0`",
                    "cannot be used by midenup",
                    "cargo install --locked --force midenup",
                ],
                UnsupportedSchema => &[
                    "v4.0.0 requires a newer version of midenup",
                    "cannot be used by midenup",
                    "cargo install --locked --force midenup",
                ],
                Malformed => &["is malformed"],
                Unavailable => &["could not fetch the manifest"],
            };
            for phrase in expected {
                assert!(stderr.contains(phrase), "{case}: expected '{phrase}' in: {stderr}");
            }
            assert_eq!(cache_after, cached, "{case}: rejected content must not touch the cache");

            if let Cache::Valid = cache {
                assert!(output.status.success(), "{case}: the cache must stand in: {stderr}");
                assert!(stderr.contains("using the cached manifest"), "{case}: {stderr}");
                assert!(stdout.contains("0.14.0"), "{case}: the cache must be listed: {stdout}");
            } else {
                assert!(!output.status.success(), "{case}: must fail without a usable cache");
                assert!(stderr.contains("no usable cached manifest"), "{case}: {stderr}");
            }
        }
    }
}

/// `update` refreshes, so a failed fetch fails it even with a valid cache, and changes nothing.
#[test]
fn integration_update_never_runs_on_the_cache() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("refresh_update");
    let fixture = OfflineFixture::create(env.tmp_dir.path(), "0.15.0");

    let installed = run_midenup(&env, &fixture.manifest_uri, &["install", "0.15.0"]);
    assert!(installed.status.success(), "{}", String::from_utf8_lossy(&installed.stderr));

    let raised = env.tmp_dir.path().join("raised.json");
    std::fs::write(
        &raised,
        manifest(&env.tmp_dir.path().join("raised"), "0.15.0", |m| {
            m["min_client_version"] = "999.0.0".into();
        }),
    )
    .unwrap();

    let cache = midenup::paths::manifest_cache(&env.midenup_home);
    let state = midenup::paths::state_path(&env.midenup_home);
    let before = (std::fs::read(&cache).unwrap(), std::fs::read(&state).unwrap());

    for uri in [UNREACHABLE.to_string(), format!("file://{}", raised.display())] {
        let output = run_midenup(&env, &uri, &["update"]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{uri}: update must fail: {stderr}");
        assert!(stderr.contains("requires a fresh manifest"), "{uri}: {stderr}");
        assert!(!stderr.contains("using the cached manifest"), "{uri}: {stderr}");
        assert_eq!(
            (std::fs::read(&cache).unwrap(), std::fs::read(&state).unwrap()),
            before,
            "{uri}: a failed update must change nothing"
        );
    }
}

/// Installed toolchains are used and managed with no upstream and no cache.
#[test]
fn integration_local_commands_need_neither_upstream_nor_a_cache() {
    let _guard = common::harness::mutating_test_guard();
    let env = environment_setup("refresh_local");
    let fixture = OfflineFixture::create(env.tmp_dir.path(), "0.15.0");

    let installed = run_midenup(&env, &fixture.manifest_uri, &["install", "0.15.0"]);
    assert!(installed.status.success(), "{}", String::from_utf8_lossy(&installed.stderr));
    std::fs::remove_file(midenup::paths::manifest_cache(&env.midenup_home)).unwrap();

    let miden = env.tmp_dir.path().join("bin").join("miden");
    std::fs::create_dir_all(miden.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_midenup"), &miden).unwrap();
    let dispatched = midenup_command(&miden, &env, UNREACHABLE).arg("vm").output().unwrap();
    assert!(dispatched.status.success(), "{}", String::from_utf8_lossy(&dispatched.stderr));
    assert!(String::from_utf8_lossy(&dispatched.stdout).contains("miden-vm 0.15.0"));

    let listed = run_midenup(&env, UNREACHABLE, &["show", "list"]);
    assert!(listed.status.success(), "{}", String::from_utf8_lossy(&listed.stderr));
    assert!(String::from_utf8_lossy(&listed.stdout).contains("0.15.0"));

    for args in [&["override", "0.15.0"][..], &["uninstall", "0.15.0"], &["gc"]] {
        let output = run_midenup(&env, UNREACHABLE, args);
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
    }
    assert!(
        !env.midenup_home.join("toolchains").join("0.15.0").exists(),
        "the toolchain must be uninstalled"
    );
}
