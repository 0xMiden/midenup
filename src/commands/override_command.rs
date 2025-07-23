use crate::{Config, channel::UserChannel, manifest::Manifest};

// This function is called override_command because "override" is a reserved
// keyword.
// Source: https://doc.rust-lang.org/reference/keywords.html#r-lex.keywords.reserved
pub fn override_command(
    config: &Config,
    channel: &UserChannel,
    local_manifest: &mut Manifest,
) -> anyhow::Result<()> {
    todo!()
}
