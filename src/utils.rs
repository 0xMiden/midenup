use anyhow::Context;

#[cfg(unix)]
pub fn symlink(from: &std::path::Path, to: &std::path::Path) -> anyhow::Result<()> {
    std::os::unix::fs::symlink(to, from).context("could not create symlink")
}

#[cfg(windows)]
pub fn symlink(from: &std::path::Path, to: &std::path::Path) -> anyhow::Result<()> {
    std::os::windows::fs::symlink_file(to, from).context("could not create symlink")
}

pub fn find_latest_hash(repository_url: &str, branch_name: &str) -> anyhow::Result<String> {
    let check_revision_hash = std::process::Command::new("git")
        .arg("ls-remote")
        .arg(repository_url)
        .arg("--branch")
        .arg(branch_name)
        .stderr(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::piped())
        .output()
        .context(format!(
            "failed to fetch latest git rev-hash from branch {branch_name}, is git installed?.",
        ))?;

    let revision_hash: String = String::from_utf8(check_revision_hash.stdout)
        .context(format!(
            "failed to format latest git rev-hash from branch {branch_name}, does the branch exist?.",
        ))?
        .chars()
        .take_while(|&c| c != ' ' && c != '\t')
        .collect();

    Ok(revision_hash)
}
