use crate::{config::ensure_midenup_home_exists, manifest::Manifest, Config};

/// This is the first command the user runs after first installing the midenup. It performs the
/// following tasks:
///
/// - Bootstrap the `midenup` environment (create directories, default config, etc.), if not already
///   done. (done in [ensure_midenup_home_exists]).
pub fn init(config: &Config, _local_manifest: &mut Manifest) -> anyhow::Result<()> {
    ensure_midenup_home_exists(config)?;

    println!(
        "midenup was successfully initialized in:
{}",
        config.midenup_home.as_path().display()
    );

    Ok(())
}
