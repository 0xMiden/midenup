use anyhow::Context;

#[cfg(unix)]
pub fn symlink(from: &std::path::Path, to: &std::path::Path) -> anyhow::Result<()> {
    std::os::unix::fs::symlink(to, from).context("could not create symlink")
}

#[cfg(windows)]
pub fn symlink(from: &std::path::Path, to: &std::path::Path) -> anyhow::Result<()> {
    std::os::windows::fs::symlink_file(to, from).context("could not create symlink")
}
