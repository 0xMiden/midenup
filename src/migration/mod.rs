use crate::{config::Config, manifest::Manifest};

mod atomic_installation;

/// Runs every known toolchain migration against the local environment,
/// dispatching each based on the local manifest version.
pub fn run_toolchain_migration(config: &Config, local_manifest: &Manifest) -> anyhow::Result<()> {
    // Versions with known migrations
    const ATOMIC_INSTALLATION: semver::Version = semver::Version::new(1, 0, 1);

    let latest_local_version = &local_manifest.manifest_version;
    let upstream_version = &config.manifest.manifest_version;
    if upstream_version >= &ATOMIC_INSTALLATION && latest_local_version < &ATOMIC_INSTALLATION {
        atomic_installation::migrate_toolchain(config, local_manifest)?;
    }

    Ok(())
}
