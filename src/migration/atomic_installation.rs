use std::path::PathBuf;

use anyhow::Context;

use crate::{config::Config, manifest::Manifest, utils};

/// Migrates a pre-1.0.1 environment to the atomic-installation toolchain layout.
pub fn migrate_toolchain(config: &Config, local_manifest: &Manifest) -> anyhow::Result<()> {
    const OBSOLETE_FILES: [&str; 2] = [".installed_channel.json", "installation-successful"];

    let toolchains_dir = config.midenup_home.join("toolchains");
    let installed_toolchains_dir = config.midenup_home.join("installed_toolchains");

    for channel in local_manifest.get_channels() {
        let hash = channel.content_hash();
        let old_toolchain_dir = toolchains_dir.join(channel.name.to_string());
        let is_real_dir = old_toolchain_dir
            .symlink_metadata()
            .map(|metadata| metadata.file_type().is_dir())
            .unwrap_or(false);
        if !is_real_dir {
            continue;
        }

        let install_dir_name = format!("{}-{}", &channel.name, hash);
        let install_dir = installed_toolchains_dir.join(&install_dir_name);

        std::fs::create_dir_all(&install_dir).with_context(|| {
            format!("failed to create install directory: '{}'", install_dir.display())
        })?;

        utils::fs::copy_dir_recursive(&old_toolchain_dir, &install_dir, &OBSOLETE_FILES)
            .with_context(|| {
                format!(
                    "failed to migrate toolchain from '{}' to '{}'",
                    old_toolchain_dir.display(),
                    install_dir.display()
                )
            })?;

        let opt_dir = install_dir.join("opt");
        if opt_dir.exists() {
            for entry in std::fs::read_dir(&opt_dir)
                .with_context(|| format!("failed to read directory '{}'", opt_dir.display()))?
            {
                let entry = entry
                    .with_context(|| format!("failed to read entry in '{}'", opt_dir.display()))?;

                let link = entry.path();
                let old_target = std::fs::read_link(&link)
                    .with_context(|| format!("failed to read symlink '{}'", link.display()))?;
                let binary = old_target.file_name().with_context(|| {
                    format!("symlink target has no file name: '{}'", old_target.display())
                })?;
                let relative_target = std::path::Path::new("..").join("bin").join(binary);

                std::fs::remove_file(&link)
                    .with_context(|| format!("failed to remove symlink '{}'", link.display()))?;
                utils::fs::symlink(&link, &relative_target).with_context(|| {
                    format!(
                        "failed to recreate symlink '{}' -> '{}'",
                        link.display(),
                        relative_target.display()
                    )
                })?;
            }
        }

        let relative_install_target =
            PathBuf::from("..").join("installed_toolchains").join(&install_dir_name);

        std::fs::remove_dir_all(&old_toolchain_dir).with_context(|| {
            format!("failed to remove old toolchain directory: '{}'", old_toolchain_dir.display())
        })?;
        utils::fs::symlink(&old_toolchain_dir, &relative_install_target).with_context(|| {
            format!(
                "failed to symlink '{}' -> '{}'",
                old_toolchain_dir.display(),
                relative_install_target.display()
            )
        })?;
    }

    let stable_symlink = toolchains_dir.join("stable");
    let stable_is_symlink = stable_symlink
        .symlink_metadata()
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false);
    if stable_is_symlink {
        let old_target = std::fs::read_link(&stable_symlink)
            .with_context(|| format!("failed to read symlink '{}'", stable_symlink.display()))?;
        if old_target.is_absolute() {
            let channel_name = old_target.file_name().with_context(|| {
                format!("symlink target has no file name: '{}'", old_target.display())
            })?;
            let relative_target = std::path::Path::new(channel_name);

            std::fs::remove_file(&stable_symlink).with_context(|| {
                format!("failed to remove symlink '{}'", stable_symlink.display())
            })?;
            utils::fs::symlink(&stable_symlink, relative_target).with_context(|| {
                format!(
                    "failed to recreate symlink '{}' -> '{}'",
                    stable_symlink.display(),
                    relative_target.display()
                )
            })?;
        }
    }
    // panic!();

    Ok(())
}
