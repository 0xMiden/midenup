//! Manifest publication steps for publish-manifest.yml and verify-manifest.yml.
//!
//! Decisions are written to `GITHUB_OUTPUT` and explained in `GITHUB_STEP_SUMMARY`, or to stdout
//! and stderr outside of GitHub Actions.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, bail};
use clap::Subcommand;
use midenup::manifest::{Manifest, VersionedManifest};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;

/// The published files, as paths in the repository and on the site.
const FILES: [(&str, &str); 2] = [
    ("manifest/v3/manifest.json", "v3/manifest.json"),
    ("manifest/channel-manifest.json", "channel-manifest.json"),
];

/// A commit whose site carries a receipt has this file.
const RECEIPT_MARKER: &str = "tools/manifest-utils/src/publish.rs";

/// The branch whose tip a publication must match.
const TIP: &str = "origin/main";

const DEFAULT_BASE_URL: &str = "https://0xmiden.github.io/midenup";

#[derive(Debug, Subcommand)]
pub enum Step {
    /// Stage the site for a commit and decide what to do with it: deploy it, confirm an unconfirmed
    /// deployment of the same files, or nothing because it is already served
    Decide {
        /// The commit being published
        #[arg(long)]
        subject: String,
        /// The commit of the latest successful github-pages deployment; looked up if omitted
        #[arg(long)]
        deployed: Option<String>,
        /// The tag of the latest published midenup release, empty if there is none; looked up if
        /// omitted
        #[arg(long)]
        released: Option<String>,
        /// The GitHub repository to look them up in, authenticated by `GH_TOKEN` when set
        #[arg(long, env = "GITHUB_REPOSITORY", default_value = "0xMiden/midenup")]
        repository: String,
        /// Where to stage the site
        #[arg(long)]
        site: PathBuf,
        #[arg(long, default_value = DEFAULT_BASE_URL)]
        base_url: String,
    },
    /// Poll the served files until they match a receipt
    Confirm {
        /// The expected receipt, as JSON
        #[arg(long)]
        receipt: String,
        #[arg(long, default_value_t = 40)]
        attempts: u32,
        /// Seconds between attempts
        #[arg(long, default_value_t = 30)]
        interval: u64,
        #[arg(long, default_value = DEFAULT_BASE_URL)]
        base_url: String,
    },
    /// Check the served files against the served receipt, and that every artifact of the served
    /// manifest can be downloaded
    Verify {
        #[arg(long, default_value = DEFAULT_BASE_URL)]
        base_url: String,
    },
}

pub fn run(step: &Step) -> anyhow::Result<()> {
    match step {
        Step::Decide {
            subject,
            deployed,
            released,
            repository,
            site,
            base_url,
        } => {
            let deployed = match deployed {
                Some(deployed) => deployed.clone(),
                None => deployed_commit(repository)?,
            };
            let released = match released.as_deref() {
                Some("") => None,
                Some(tag) => Some(parse_tag(tag)?),
                None => latest_release(repository)?,
            };
            output("deployed", &deployed);
            let decision = decide_for(subject, &deployed, released.as_ref(), site, base_url);
            match decision {
                Ok(Decision { action, summary: message }) => {
                    summary(&message);
                    output("action", action.name());
                    if let Action::Deploy(receipt) | Action::Confirm(receipt) = &action {
                        output("receipt", &serde_json::to_string(receipt)?);
                    }
                    Ok(())
                },
                Err(Blocked(message)) => {
                    summary(&message);
                    bail!("{message}")
                },
            }
        },
        Step::Confirm { receipt, attempts, interval, base_url } => {
            let expected: Receipt = serde_json::from_str(receipt).context("invalid receipt")?;
            confirm(&expected, *attempts, Duration::from_secs(*interval), base_url)
        },
        Step::Verify { base_url } => verify(base_url),
    }
}

/// What a site records about itself: the commit it was staged from and each file's SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub source_commit: String,
    pub files: BTreeMap<String, String>,
}

/// What `decide` resolved to.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Deploy the staged site, then confirm this receipt.
    Deploy(Receipt),
    /// The deployed commit publishes the same files; only confirm this receipt.
    Confirm(Receipt),
    /// Already served.
    Done,
}

impl Action {
    fn name(&self) -> &'static str {
        match self {
            Self::Deploy(_) => "deploy",
            Self::Confirm(_) => "confirm",
            Self::Done => "done",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Decision {
    pub action: Action,
    pub summary: String,
}

/// Publication cannot go ahead; the message says what has to happen first.
#[derive(Debug, PartialEq, Eq)]
pub struct Blocked(pub String);

/// What the served site looks like.
#[derive(Debug, Default)]
pub struct Served {
    /// The served receipt, when it could be downloaded.
    pub receipt: Option<Receipt>,
    /// Every file was downloaded and matches the served receipt.
    pub consistent: bool,
}

/// Everything `decide` needs, gathered from git, the site and the latest release.
#[derive(Debug)]
pub struct Inputs<'a> {
    pub subject: &'a str,
    pub candidate: &'a Receipt,
    /// The receipt of main's tip.
    pub tip: &'a Receipt,
    pub deployed: &'a str,
    /// The deployed commit is in this repository's history.
    pub deployed_known: bool,
    /// The receipt the deployed commit's site carries, if it carries one.
    pub recorded: Option<&'a Receipt>,
    pub served: &'a Served,
    pub minimum: &'a semver::Version,
    pub released: Option<&'a semver::Version>,
}

/// Decides what to do with the candidate site.
pub fn decide(inputs: &Inputs<'_>) -> Result<Decision, Blocked> {
    let Inputs { subject, candidate, deployed, served, .. } = *inputs;
    if candidate.files != inputs.tip.files {
        return Err(Blocked(format!(
            "Not published: main's tip {} changes the manifest after {subject}. Dispatch the \
             workflow again to publish main's tip.",
            inputs.tip.source_commit
        )));
    }

    if !inputs.deployed_known {
        return Err(Blocked(format!(
            "Blocked: the latest github-pages deployment is of {deployed}, which is not in this \
             repository's history. Establish what is being served before publishing again."
        )));
    }

    match &served.receipt {
        Some(receipt) if served.consistent => {
            if receipt.files == candidate.files {
                return Ok(Decision {
                    action: Action::Done,
                    summary: format!(
                        "Already served: the files of {subject} match the served receipt (source \
                         commit {}). Nothing to deploy.",
                        receipt.source_commit
                    ),
                });
            }
            if receipt.source_commit != deployed {
                return unconfirmed(inputs);
            }
        },
        // A receipt is or should be served, but what is served is not what it records.
        Some(_) => return unconfirmed(inputs),
        None if inputs.recorded.is_some() => return unconfirmed(inputs),
        // The served site and the deployment both predate receipts: an ordinary predecessor.
        None => (),
    }

    if !released_satisfies(inputs.released, inputs.minimum) {
        let released = inputs.released.map_or("none".to_string(), |version| format!("v{version}"));
        return Err(Blocked(format!(
            "Not published: the manifest requires midenup >= {} as min_client_version, and the \
             latest release is {released}. Release that midenup first, then dispatch the workflow \
             again.",
            inputs.minimum
        )));
    }

    Ok(Decision {
        action: Action::Deploy(candidate.clone()),
        summary: format!(
            "Deploying the manifest of {subject} over the one deployed from {deployed}."
        ),
    })
}

/// Handles a deployment not confirmed as served: confirms it when it publishes the candidate's
/// files, and blocks publication otherwise.
fn unconfirmed(inputs: &Inputs<'_>) -> Result<Decision, Blocked> {
    let Inputs { subject, deployed, .. } = *inputs;
    if let Some(recorded) = inputs.recorded
        && recorded.files == inputs.candidate.files
    {
        return Ok(Decision {
            action: Action::Confirm(recorded.clone()),
            summary: format!(
                "Deployed, unconfirmed: {deployed} publishes the same files as {subject}. \
                 Confirming what is served instead of deploying again."
            ),
        });
    }
    Err(Blocked(format!(
        "Blocked: the latest github-pages deployment, of {deployed}, is not confirmed as served, \
         and {subject} publishes different files. Reconcile it before publishing again; see \
         release-process.md."
    )))
}

/// Whether a stable `released` version satisfies `minimum`. A prerelease minimum is satisfied
/// once a release reaches its major.minor.patch.
pub fn released_satisfies(released: Option<&semver::Version>, minimum: &semver::Version) -> bool {
    let core = semver::Version::new(minimum.major, minimum.minor, minimum.patch);
    released.is_some_and(|released| *released >= core)
}

/// Gathers the inputs of `decide` for `subject`, staging its site into `site`.
fn decide_for(
    subject: &str,
    deployed: &str,
    released: Option<&semver::Version>,
    site: &Path,
    base_url: &str,
) -> Result<Decision, Blocked> {
    let gathered = || -> anyhow::Result<Result<Decision, Blocked>> {
        let candidate = stage(subject, site)?;
        let tip = receipt_of(&git(&["rev-parse", TIP])?)?;
        let deployed_known = git(&["cat-file", "-e", &format!("{deployed}^{{commit}}")]).is_ok();
        let recorded = if deployed_known && has_receipt(deployed) {
            Some(receipt_of(deployed)?)
        } else {
            None
        };
        let served = fetch_served(base_url);
        let manifest = std::fs::read_to_string(site.join("v3/manifest.json"))?;
        let minimum =
            midenup::manifest::version::read_version_header(&manifest, "min_client_version")?
                .version;
        Ok(decide(&Inputs {
            subject,
            candidate: &candidate,
            tip: &tip,
            deployed,
            deployed_known,
            recorded: recorded.as_ref(),
            served: &served,
            minimum: &minimum,
            released,
        }))
    };
    gathered().unwrap_or_else(|err| Err(Blocked(format!("{err:#}"))))
}

/// The commit of the newest github-pages deployment whose latest status is `success`. A newer
/// successful deployment marks the older ones inactive, so this is what Pages serves.
fn deployed_commit(repository: &str) -> anyhow::Result<String> {
    let deployments = github(repository, "deployments?environment=github-pages&per_page=30")?
        .context("GitHub has no deployments for this repository")?;
    first_successful(&deployments, |id| {
        let statuses = github(repository, &format!("deployments/{id}/statuses?per_page=1"))?
            .with_context(|| format!("GitHub has no deployment {id}"))?;
        Ok(statuses[0]["state"].as_str().map(str::to_string))
    })?
    .context("no successful github-pages deployment among the latest 30")
}

/// The commit of the first of `deployments` whose state, as `state_of` reports it, is `success`.
pub fn first_successful(
    deployments: &serde_json::Value,
    mut state_of: impl FnMut(u64) -> anyhow::Result<Option<String>>,
) -> anyhow::Result<Option<String>> {
    for deployment in deployments.as_array().context("expected a list of deployments")? {
        let id = deployment["id"].as_u64().context("a deployment has no id")?;
        if state_of(id)?.as_deref() == Some("success") {
            let commit = deployment["sha"].as_str().context("a deployment has no commit")?;
            return Ok(Some(commit.to_string()));
        }
    }
    Ok(None)
}

/// The version of the latest published midenup release, or `None` if there is none.
fn latest_release(repository: &str) -> anyhow::Result<Option<semver::Version>> {
    match github(repository, "releases/latest")? {
        Some(release) => {
            let tag = release["tag_name"].as_str().context("the latest release has no tag")?;
            parse_tag(tag).map(Some)
        },
        None => Ok(None),
    }
}

fn parse_tag(tag: &str) -> anyhow::Result<semver::Version> {
    semver::Version::parse(tag.trim_start_matches('v'))
        .with_context(|| format!("invalid release tag '{tag}'"))
}

/// Reads `path` from the GitHub API for `repository`, or `None` when GitHub answers 404.
fn github(repository: &str, path: &str) -> anyhow::Result<Option<serde_json::Value>> {
    let mut headers = vec![
        "Accept: application/vnd.github+json".to_string(),
        // GitHub refuses requests without one.
        "User-Agent: manifest-utils".to_string(),
    ];
    if let Ok(token) = std::env::var("GH_TOKEN")
        && !token.is_empty()
    {
        headers.push(format!("Authorization: Bearer {token}"));
    }
    let uri = format!("https://api.github.com/repos/{repository}/{path}");
    let response = crate::online::transfer(&uri, &headers, false)?;
    match response.status {
        404 => Ok(None),
        200..=299 => serde_json::from_slice(&response.body)
            .map(Some)
            .with_context(|| format!("'{uri}' did not answer JSON")),
        status => bail!("'{uri}' answered HTTP {status}"),
    }
}

/// Whether the site deployed from `commit` included a receipt.
fn has_receipt(commit: &str) -> bool {
    git(&["cat-file", "-e", &format!("{commit}:{RECEIPT_MARKER}")]).is_ok()
}

/// Writes the site for `commit` into `dir`: the published files byte-for-byte and receipt.json.
pub fn stage(commit: &str, dir: &Path) -> anyhow::Result<Receipt> {
    let mut files = BTreeMap::new();
    for (repository, site) in FILES {
        let bytes = git_bytes(&["show", &format!("{commit}:{repository}")])?;
        let path = dir.join(site);
        std::fs::create_dir_all(path.parent().expect("site paths have a parent"))?;
        std::fs::write(&path, &bytes)?;
        files.insert(site.to_string(), sha256(&bytes));
    }
    let receipt = Receipt { source_commit: commit.to_string(), files };
    std::fs::write(dir.join("receipt.json"), serde_json::to_vec_pretty(&receipt)?)?;
    Ok(receipt)
}

/// The receipt the site staged from `commit` would carry.
fn receipt_of(commit: &str) -> anyhow::Result<Receipt> {
    let mut files = BTreeMap::new();
    for (repository, site) in FILES {
        let bytes = git_bytes(&["show", &format!("{commit}:{repository}")])?;
        files.insert(site.to_string(), sha256(&bytes));
    }
    Ok(Receipt { source_commit: commit.to_string(), files })
}

/// Downloads the served receipt and files, and checks them against each other.
fn fetch_served(base_url: &str) -> Served {
    let receipt = match fetch(base_url, "receipt.json") {
        Ok((200, body)) => serde_json::from_slice::<Receipt>(&body).ok(),
        Ok((status, _)) => {
            eprintln!("{base_url}/receipt.json returned HTTP {status}");
            None
        },
        Err(err) => {
            eprintln!("{err:#}");
            None
        },
    };
    let consistent = receipt.as_ref().is_some_and(|receipt| matches_receipt(receipt, base_url));
    Served { receipt, consistent }
}

/// Whether every file `receipt` lists is served with the hash it records.
fn matches_receipt(receipt: &Receipt, base_url: &str) -> bool {
    receipt.files.iter().all(|(path, expected)| match fetch(base_url, path) {
        Ok((200, body)) => sha256(&body) == *expected,
        Ok((status, _)) => {
            eprintln!("{base_url}/{path} returned HTTP {status}");
            false
        },
        Err(err) => {
            eprintln!("{err:#}");
            false
        },
    })
}

/// Polls the served files until they match `expected`.
fn confirm(
    expected: &Receipt,
    attempts: u32,
    interval: Duration,
    base_url: &str,
) -> anyhow::Result<()> {
    for attempt in 1..=attempts {
        let served = fetch_served(base_url);
        if served.consistent && served.receipt.as_ref() == Some(expected) {
            summary(&format!(
                "Confirmed: {base_url} serves the files of {}.",
                expected.source_commit
            ));
            return Ok(());
        }
        eprintln!("attempt {attempt}/{attempts}: served files do not match the receipt yet");
        if attempt < attempts {
            std::thread::sleep(interval);
        }
    }
    let message = format!(
        "Deployed, unconfirmed: {base_url} does not serve the files of {} after {attempts} \
         attempts. Publication is blocked until this is reconciled; see release-process.md.",
        expected.source_commit
    );
    summary(&message);
    bail!("{message}")
}

/// Checks the served files against the served receipt, then probes every artifact of the served
/// manifest.
fn verify(base_url: &str) -> anyhow::Result<()> {
    let mut failed = false;
    match fetch(base_url, "receipt.json")? {
        (404, _) => {
            for (_, path) in FILES {
                let (status, _) = fetch(base_url, path)?;
                if status != 200 {
                    summary(&format!("{base_url}/{path} returned HTTP {status}."));
                    failed = true;
                }
            }
            if !failed {
                summary("No receipt served yet, so the served files are not compared with one.");
            }
        },
        _ => {
            let served = fetch_served(base_url);
            match served.receipt {
                Some(receipt) if served.consistent => summary(&format!(
                    "The served files match the receipt of {}.",
                    receipt.source_commit
                )),
                Some(receipt) => {
                    summary(&format!(
                        "The served files do not match the served receipt (source commit {}).",
                        receipt.source_commit
                    ));
                    failed = true;
                },
                None => {
                    summary("The served receipt cannot be read; see the log.");
                    failed = true;
                },
            }
        },
    }

    let (status, body) = fetch(base_url, "v3/manifest.json")?;
    if status != 200 {
        bail!("{base_url}/v3/manifest.json returned HTTP {status}");
    }
    let manifest = VersionedManifest::parse_str(&String::from_utf8_lossy(&body))
        .context("the served manifest cannot be parsed")?;
    let uris = artifact_uris(&manifest)?;
    let unavailable: Vec<_> = uris.iter().filter(|uri| !available(uri)).collect();
    for uri in &unavailable {
        summary(&format!("- unavailable: {uri}"));
    }
    summary(&format!(
        "\n{} of {} artifact URIs are available.",
        uris.len() - unavailable.len(),
        uris.len()
    ));

    if failed || !unavailable.is_empty() {
        bail!("the served manifest failed verification; see the summary");
    }
    Ok(())
}

/// The URI of every artifact `manifest` declares, for every target.
pub fn artifact_uris(manifest: &Manifest) -> anyhow::Result<BTreeSet<String>> {
    let mut uris = BTreeSet::new();
    for channel in manifest.get_channels() {
        for component in channel.components.iter() {
            for (id, artifact) in component.artifacts.iter() {
                let resolved = artifact.get_uris_for(id, component).with_context(|| {
                    format!("toolchain {} has an invalid artifact", channel.name)
                })?;
                uris.extend(resolved.into_iter().map(|uri| uri.to_string()));
            }
        }
    }
    Ok(uris)
}

/// Whether the first byte of `uri` can be downloaded, retrying transient failures.
fn available(uri: &str) -> bool {
    (0..3).any(|_| {
        crate::online::transfer(uri, &["Range: bytes=0-0".to_string()], false)
            .is_ok_and(|response| (200..300).contains(&response.status))
    })
}

/// Downloads `path` from the site, past caches as far as requests can ask, and returns the HTTP
/// status with the body. A `file://` site answers 404 for a missing file.
fn fetch(base_url: &str, path: &str) -> anyhow::Result<(u32, Vec<u8>)> {
    if let Some(dir) = base_url.strip_prefix("file://") {
        return Ok(match std::fs::read(Path::new(dir).join(path)) {
            Ok(body) => (200, body),
            Err(_) => (404, Vec::new()),
        });
    }
    let nocache = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let headers = ["Cache-Control: no-cache".to_string(), "Pragma: no-cache".to_string()];
    let uri = format!("{base_url}/{path}?nocache={nocache}");
    let mut attempt = 1;
    let response = loop {
        match crate::online::transfer(&uri, &headers, false) {
            Err(_) if attempt < 3 => attempt += 1,
            response => break response?,
        }
    };
    Ok((response.status, response.body))
}

fn sha256(bytes: &[u8]) -> String {
    sha2::Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn git(args: &[&str]) -> anyhow::Result<String> {
    Ok(String::from_utf8(git_bytes(args)?)?.trim().to_string())
}

fn git_bytes(args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let output = std::process::Command::new("git")
        .args(args)
        .output()
        .context("failed to run git, is it installed?")?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

/// Appends `line` to the job summary, or prints it to stderr outside of GitHub Actions.
fn summary(line: &str) {
    append_or("GITHUB_STEP_SUMMARY", line, |line| eprintln!("{line}"));
}

/// Sets a step output, or prints it to stdout outside of GitHub Actions.
fn output(name: &str, value: &str) {
    append_or("GITHUB_OUTPUT", &format!("{name}={value}"), |line| println!("{line}"));
}

fn append_or(variable: &str, line: &str, fallback: impl FnOnce(&str)) {
    let appended = std::env::var_os(variable).is_some_and(|path| {
        std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(path)
            .and_then(|mut file| writeln!(file, "{line}"))
            .is_ok()
    });
    if !appended {
        fallback(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(commit: &str, manifest: &str) -> Receipt {
        Receipt {
            source_commit: commit.to_string(),
            files: BTreeMap::from([
                ("v3/manifest.json".to_string(), manifest.to_string()),
                ("channel-manifest.json".to_string(), "legacy".to_string()),
            ]),
        }
    }

    fn version(text: &str) -> semver::Version {
        semver::Version::parse(text).unwrap()
    }

    /// Decides for a candidate `new` at main's tip, deployed from `deployed`, with 1.3.0 released
    /// and required.
    fn decide_with(served: Served, recorded: Option<&Receipt>) -> Result<Decision, Blocked> {
        let candidate = receipt("subject", "new");
        let minimum = version("1.3.0");
        let released = version("1.3.0");
        decide(&Inputs {
            subject: "subject",
            candidate: &candidate,
            tip: &candidate,
            deployed: "deployed",
            deployed_known: true,
            recorded,
            served: &served,
            minimum: &minimum,
            released: Some(&released),
        })
    }

    fn served(receipt: Receipt, consistent: bool) -> Served {
        Served { receipt: Some(receipt), consistent }
    }

    #[test]
    fn a_predecessor_without_a_receipt_is_deployed_over() {
        let decision = decide_with(Served::default(), None).unwrap();
        assert_eq!(decision.action, Action::Deploy(receipt("subject", "new")));
    }

    #[test]
    fn a_confirmed_predecessor_is_deployed_over() {
        let old = receipt("deployed", "old");
        let decision = decide_with(served(old.clone(), true), Some(&old)).unwrap();
        assert_eq!(decision.action, Action::Deploy(receipt("subject", "new")));
    }

    #[test]
    fn files_already_served_are_done() {
        let decision = decide_with(served(receipt("earlier", "new"), true), None).unwrap();
        assert_eq!(decision.action, Action::Done);
    }

    #[test]
    fn a_tip_with_different_files_refuses_the_candidate() {
        let candidate = receipt("subject", "new");
        let tip = receipt("tip", "newer");
        let version = version("1.3.0");
        let decision = decide(&Inputs {
            subject: "subject",
            candidate: &candidate,
            tip: &tip,
            deployed: "deployed",
            deployed_known: true,
            recorded: None,
            served: &Served::default(),
            minimum: &version,
            released: Some(&version),
        });
        let Err(Blocked(message)) = decision else {
            panic!("must not publish: {decision:?}")
        };
        assert!(message.contains("Dispatch the workflow again"), "{message}");
    }

    #[test]
    fn an_unreleased_minimum_refuses_publication() {
        let candidate = receipt("subject", "new");
        let minimum = version("1.4.0");
        let released = version("1.3.0");
        let decision = decide(&Inputs {
            subject: "subject",
            candidate: &candidate,
            tip: &candidate,
            deployed: "deployed",
            deployed_known: true,
            recorded: None,
            served: &Served::default(),
            minimum: &minimum,
            released: Some(&released),
        });
        let Err(Blocked(message)) = decision else {
            panic!("must not publish: {decision:?}")
        };
        assert!(message.contains("midenup >= 1.4.0"), "{message}");
    }

    #[test]
    fn an_unconfirmed_deployment_of_the_same_files_is_confirmed() {
        let recorded = receipt("deployed", "new");
        let decision =
            decide_with(served(receipt("older", "old"), false), Some(&recorded)).unwrap();
        assert_eq!(decision.action, Action::Confirm(recorded));
    }

    #[test]
    fn an_unconfirmed_deployment_of_other_files_blocks_publication() {
        let recorded = receipt("deployed", "other");
        assert!(decide_with(served(receipt("deployed", "other"), false), Some(&recorded)).is_err());
        // Served and consistent, but from a commit other than the deployed one.
        assert!(decide_with(served(receipt("older", "old"), true), Some(&recorded)).is_err());
        // A deployment that should serve a receipt but serves none.
        assert!(decide_with(Served::default(), Some(&recorded)).is_err());
    }

    #[test]
    fn a_deployment_outside_the_history_blocks_publication() {
        let candidate = receipt("subject", "new");
        let version = version("1.3.0");
        let decision = decide(&Inputs {
            subject: "subject",
            candidate: &candidate,
            tip: &candidate,
            deployed: "unknown",
            deployed_known: false,
            recorded: None,
            served: &Served::default(),
            minimum: &version,
            released: Some(&version),
        });
        assert!(decision.is_err());
    }

    #[test]
    fn the_deployed_commit_is_the_newest_successful_deployment() {
        let deployments = serde_json::json!([
            {"id": 3, "sha": "failed"},
            {"id": 2, "sha": "served"},
            {"id": 1, "sha": "older"}
        ]);
        let states = BTreeMap::from([(3, "failure"), (2, "success"), (1, "success")]);
        let state_of = |id| Ok(states.get(&id).map(|state| state.to_string()));
        assert_eq!(first_successful(&deployments, state_of).unwrap().as_deref(), Some("served"));
        assert_eq!(first_successful(&serde_json::json!([]), state_of).unwrap(), None);
    }

    #[test]
    fn a_release_satisfies_a_minimum_it_reaches() {
        assert!(released_satisfies(Some(&version("1.3.0")), &version("1.3.0")));
        assert!(released_satisfies(Some(&version("1.4.0")), &version("1.3.0")));
        assert!(!released_satisfies(Some(&version("1.2.1")), &version("1.3.0")));
        assert!(released_satisfies(Some(&version("1.3.0")), &version("1.3.0-rc.1")));
        assert!(!released_satisfies(None, &version("1.3.0")));
    }

    /// A staged site is served from `file://` and confirmed, then verified with its artifacts.
    #[test]
    fn a_staged_site_is_confirmed_once_served() {
        let site = tempfile::tempdir().unwrap();
        let receipt = stage("HEAD", site.path()).unwrap();
        let base_url = format!("file://{}", site.path().display());

        let served = fetch_served(&base_url);
        assert!(served.consistent);
        assert_eq!(served.receipt.as_ref(), Some(&receipt));
        confirm(&receipt, 1, Duration::ZERO, &base_url).unwrap();

        std::fs::write(site.path().join("channel-manifest.json"), b"changed").unwrap();
        assert!(!fetch_served(&base_url).consistent);
        assert!(confirm(&receipt, 1, Duration::ZERO, &base_url).is_err());
    }
}
