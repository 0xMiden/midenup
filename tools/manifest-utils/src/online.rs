//! Checks that the sources a manifest pins exist upstream, which no offline rule can tell.

use std::{path::Path, time::Duration};

use anyhow::{Context, bail};
use midenup::{
    manifest::{
        Component, ComponentKind, InstallationMethod, Manifest, PackageInstallationMethod,
        validate::container_references,
    },
    version::{Authority, GitTarget},
};

/// Media types a registry may store an image or compose artifact as.
const OCI_ACCEPT: &str = "Accept: application/vnd.oci.image.index.v1+json, \
                          application/vnd.oci.image.manifest.v1+json, \
                          application/vnd.docker.distribution.manifest.list.v2+json, \
                          application/vnd.docker.distribution.manifest.v2+json";

/// Where crate metadata and crate files are fetched from.
pub struct Registry {
    /// Root of a sparse index, as served at `https://index.crates.io`.
    pub index: String,
    /// Root that `.crate` files are served under, as `<name>/<name>-<version>.crate`.
    pub crates: String,
}

impl Registry {
    pub fn crates_io() -> Self {
        Self {
            index: "https://index.crates.io".to_string(),
            crates: "https://static.crates.io/crates".to_string(),
        }
    }
}

/// Checks every component of `manifest` that `previous` does not publish identically.
pub fn check(
    manifest: &Manifest,
    previous: Option<&Manifest>,
    registry: &Registry,
) -> Vec<anyhow::Error> {
    let mut errors = Vec::new();
    for channel in manifest.get_channels() {
        let published = previous.and_then(|p| p.get_channel_by_name(&channel.name));
        for component in channel.components.iter() {
            if published.and_then(|c| c.get_component(&component.name)) == Some(component) {
                continue;
            }
            let at = || format!("channel {}: component '{}'", channel.name, component.name);
            if let Err(err) = check_component(component, registry) {
                errors.push(err.context(at()));
            }
            for reference in container_references(&channel.name, component) {
                if let Err(err) = check_container(&reference) {
                    errors.push(err.context(at()));
                }
            }
        }
    }
    errors
}

fn check_component(component: &Component, registry: &Registry) -> anyhow::Result<()> {
    // Only these name a crate. Elsewhere the version labels prebuilt artifacts, and the component
    // name is not a crate.
    let (crate_name, built) = match &component.kind {
        ComponentKind::Executable { installation_method, .. }
        | ComponentKind::CargoExtension { installation_method, .. } => match installation_method {
            InstallationMethod::Cargo { crate_name, .. }
            | InstallationMethod::PrebuiltWithCargoFallback { crate_name, .. } => {
                (crate_name, true)
            },
            InstallationMethod::Prebuilt => return Ok(()),
        },
        ComponentKind::LegacyPackage {
            installation_method: PackageInstallationMethod::Cargo { crate_name, .. },
            ..
        } => (crate_name, false),
        _ => return Ok(()),
    };

    match &component.version {
        Authority::Registry { version } => {
            check_published(registry, crate_name, version)?;
            if built {
                check_crate_lockfile(registry, crate_name, version)?;
            }
        },
        Authority::Git {
            repository_url,
            subpath,
            target: GitTarget::Revision { hash },
        } => check_revision(repository_url, hash, subpath.as_deref(), built)?,
        // Refused offline.
        Authority::Git { .. } | Authority::Path { .. } => {},
    }
    Ok(())
}

fn check_published(
    registry: &Registry,
    crate_name: &str,
    version: &semver::Version,
) -> anyhow::Result<()> {
    let uri = format!("{}/{}", registry.index, index_path(crate_name));
    let index = fetch(&uri)
        .with_context(|| format!("failed to read the index entry of crate '{crate_name}'"))?;
    let index = String::from_utf8(index).with_context(|| format!("'{uri}' is not text"))?;
    match find_release(&index, version)? {
        None => bail!("crate '{crate_name}' has no published version {version}"),
        Some(true) => bail!("crate '{crate_name}' {version} is yanked, so it cannot be installed"),
        Some(false) => Ok(()),
    }
}

/// `cargo install --locked` builds with the lockfile packaged in the crate, if there is one.
fn check_crate_lockfile(
    registry: &Registry,
    crate_name: &str,
    version: &semver::Version,
) -> anyhow::Result<()> {
    let uri = format!("{}/{crate_name}/{crate_name}-{version}.crate", registry.crates);
    let file = fetch(&uri)?;
    if !packages_lockfile(&file, crate_name, version)
        .with_context(|| format!("'{uri}' is not a crate file"))?
    {
        bail!(
            "crate '{crate_name}' {version} is published without a Cargo.lock, so it cannot be \
             built locked"
        );
    }
    Ok(())
}

/// Requires the tag of a pinned `oci://<host>/<repository>:<tag>@sha256:<digest>` to resolve to
/// that digest, so a pin copied from the wrong build is caught when it is added.
fn check_container(reference: &str) -> anyhow::Result<()> {
    let Some((host, repository, tag, digest)) = parse_container(reference) else {
        // Refused offline.
        return Ok(());
    };
    let target = tag.unwrap_or(digest);
    let uri = format!("https://{host}/v2/{repository}/manifests/{target}");

    let mut response = transfer(&uri, &[OCI_ACCEPT.to_string()], true)?;
    if response.status == 401 {
        let token = registry_token(&response.headers)
            .with_context(|| format!("'{uri}' asked for credentials"))?;
        let auth = format!("Authorization: Bearer {token}");
        response = transfer(&uri, &[OCI_ACCEPT.to_string(), auth], true)?;
    }
    if !(200..300).contains(&response.status) {
        bail!("'{reference}' cannot be fetched: '{uri}' answered HTTP {}", response.status);
    }
    let resolved = header(&response.headers, "docker-content-digest")
        .with_context(|| format!("'{uri}' did not report a digest"))?;
    if resolved != digest {
        bail!("'{reference}' pins {digest}, but {target} resolves to {resolved}");
    }
    Ok(())
}

/// Splits a digest-pinned `oci://` reference into host, repository, optional tag and digest.
fn parse_container(reference: &str) -> Option<(&str, &str, Option<&str>, &str)> {
    let (name, digest) = reference.strip_prefix("oci://")?.split_once('@')?;
    if !digest.starts_with("sha256:") {
        return None;
    }
    let (host, path) = name.split_once('/')?;
    let (repository, tag) = match path.rsplit_once(':') {
        Some((repository, tag)) => (repository, Some(tag)),
        None => (path, None),
    };
    Some((host, repository, tag, digest))
}

/// Fetches an anonymous pull token from the `Bearer` challenge in `headers`.
fn registry_token(headers: &[String]) -> anyhow::Result<String> {
    let challenge = header(headers, "www-authenticate").context("no authentication challenge")?;
    let params = challenge.strip_prefix("Bearer ").context("not a Bearer challenge")?;
    let param = |key: &str| {
        params.split(',').find_map(|pair| {
            let (k, v) = pair.trim().split_once('=')?;
            (k == key).then(|| v.trim_matches('"'))
        })
    };
    let realm = param("realm").context("the challenge names no realm")?;
    let query: Vec<String> = ["service", "scope"]
        .into_iter()
        .filter_map(|key| param(key).map(|value| format!("{key}={value}")))
        .collect();
    let body = fetch(&format!("{realm}?{}", query.join("&")))?;

    #[derive(serde::Deserialize)]
    struct Token {
        token: String,
    }
    let token: Token = serde_json::from_slice(&body).context("unreadable registry token")?;
    Ok(token.token)
}

/// The value of response header `name`, matched without regard to case.
fn header<'a>(headers: &'a [String], name: &str) -> Option<&'a str> {
    headers.iter().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

/// Fetches `revision` alone from `repository_url`, which fails unless the commit exists there.
fn check_revision(
    repository_url: &str,
    revision: &str,
    subpath: Option<&str>,
    built: bool,
) -> anyhow::Result<()> {
    let scratch = tempfile::Builder::new().prefix("manifest-utils-revision").tempdir()?;
    git(scratch.path(), &["init", "--quiet"])?;
    git(scratch.path(), &["fetch", "--quiet", "--depth=1", repository_url, revision])
        .with_context(|| format!("failed to fetch revision {revision} from '{repository_url}'"))?;

    // The lockfile of the crate's own workspace: beside the crate, or at the repository root.
    let lockfiles = [
        subpath.map(|s| format!("{}/Cargo.lock", s.trim_end_matches('/'))),
        Some("Cargo.lock".to_string()),
    ];
    if built
        && !lockfiles.into_iter().flatten().any(|path| {
            git(scratch.path(), &["cat-file", "-e", &format!("FETCH_HEAD:{path}")]).is_ok()
        })
    {
        bail!(
            "revision {revision} of '{repository_url}' has no Cargo.lock, so it cannot be built \
             locked"
        );
    }
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> anyhow::Result<()> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
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
    Ok(())
}

/// Where a crate's entry lives in a sparse index.
fn index_path(crate_name: &str) -> String {
    let name = crate_name.to_lowercase();
    match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!("3/{}/{name}", &name[..1]),
        _ => format!("{}/{}/{name}", &name[..2], &name[2..4]),
    }
}

/// Whether `version` is yanked, or `None` if the index entry does not list it.
fn find_release(index: &str, version: &semver::Version) -> anyhow::Result<Option<bool>> {
    #[derive(serde::Deserialize)]
    struct Release {
        vers: semver::Version,
        yanked: bool,
    }

    for line in index.lines().filter(|line| !line.trim().is_empty()) {
        let release: Release =
            serde_json::from_str(line).context("unreadable crate index entry")?;
        if release.vers == *version {
            return Ok(Some(release.yanked));
        }
    }
    Ok(None)
}

/// Whether the gzipped tarball `file` holds the lockfile of `crate_name` `version`.
fn packages_lockfile(
    file: &[u8],
    crate_name: &str,
    version: &semver::Version,
) -> anyhow::Result<bool> {
    let wanted = format!("{crate_name}-{version}/Cargo.lock");
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    for entry in archive.entries()? {
        if entry?.path()? == Path::new(&wanted) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Reads `uri`, either a `file://` path or an http(s) URL answering with a success status.
fn fetch(uri: &str) -> anyhow::Result<Vec<u8>> {
    if let Some(path) = uri.strip_prefix("file://") {
        return std::fs::read(path).with_context(|| format!("failed to read '{path}'"));
    }

    let response = transfer(uri, &[], false)?;
    if !(200..300).contains(&response.status) {
        bail!("'{uri}' answered HTTP {}", response.status);
    }
    Ok(response.body)
}

struct Response {
    status: u32,
    headers: Vec<String>,
    body: Vec<u8>,
}

/// Requests `uri` with `headers`, without a body when `head` is set; any status is returned.
fn transfer(uri: &str, headers: &[String], head: bool) -> anyhow::Result<Response> {
    let mut response = Response {
        status: 0,
        headers: Vec::new(),
        body: Vec::new(),
    };
    let mut handle = curl::easy::Easy::new();
    handle.url(uri)?;
    handle.follow_location(true)?;
    handle.nobody(head)?;
    // A stalled upstream must fail the check, not hang it.
    handle.timeout(Duration::from_secs(120))?;
    let mut list = curl::easy::List::new();
    for header in headers {
        list.append(header)?;
    }
    handle.http_headers(list)?;
    {
        let mut transfer = handle.transfer();
        transfer.header_function(|line| {
            response.headers.push(String::from_utf8_lossy(line).trim_end().to_string());
            true
        })?;
        transfer.write_function(|chunk| {
            response.body.extend_from_slice(chunk);
            Ok(chunk.len())
        })?;
        transfer.perform().with_context(|| format!("failed to fetch '{uri}'"))?;
    }
    response.status = handle.response_code()?;
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_references_split_into_their_parts() {
        assert_eq!(
            parse_container("oci://ghcr.io/0xmiden/miden-local-network:v0.17.6@sha256:6f77"),
            Some(("ghcr.io", "0xmiden/miden-local-network", Some("v0.17.6"), "sha256:6f77"))
        );
        assert_eq!(
            parse_container("oci://ghcr.io/org/image@sha256:6f77"),
            Some(("ghcr.io", "org/image", None, "sha256:6f77"))
        );
        assert_eq!(parse_container("oci://ghcr.io/org/image:v1"), None);
    }

    #[test]
    fn headers_are_matched_without_regard_to_case() {
        let headers = ["HTTP/2 200".to_string(), "Docker-Content-Digest: sha256:6f77".to_string()];
        assert_eq!(header(&headers, "docker-content-digest"), Some("sha256:6f77"));
        assert_eq!(header(&headers, "www-authenticate"), None);
    }

    #[test]
    fn index_paths_follow_the_sparse_layout() {
        assert_eq!(index_path("a"), "1/a");
        assert_eq!(index_path("ab"), "2/ab");
        assert_eq!(index_path("abc"), "3/a/abc");
        assert_eq!(index_path("Miden-VM"), "mi/de/miden-vm");
    }

    #[test]
    fn a_release_is_found_with_its_yanked_state() {
        let index = concat!(
            r#"{"name":"miden-vm","vers":"0.1.0","yanked":true,"deps":[]}"#,
            "\n",
            r#"{"name":"miden-vm","vers":"0.2.0","yanked":false,"deps":[]}"#,
            "\n",
        );
        let find = |v: &str| find_release(index, &semver::Version::parse(v).unwrap()).unwrap();
        assert_eq!(find("0.1.0"), Some(true));
        assert_eq!(find("0.2.0"), Some(false));
        assert_eq!(find("0.3.0"), None);
    }

    fn crate_file(entries: &[&str]) -> Vec<u8> {
        let mut builder =
            tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), Default::default()));
        for path in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_cksum();
            builder.append_data(&mut header, path, std::io::empty()).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn a_packaged_lockfile_is_found_only_at_the_crate_root() {
        let version = semver::Version::new(0, 2, 0);
        let with = crate_file(&["miden-vm-0.2.0/Cargo.toml", "miden-vm-0.2.0/Cargo.lock"]);
        assert!(packages_lockfile(&with, "miden-vm", &version).unwrap());
        let nested = crate_file(&["miden-vm-0.2.0/Cargo.toml", "miden-vm-0.2.0/sub/Cargo.lock"]);
        assert!(!packages_lockfile(&nested, "miden-vm", &version).unwrap());
    }

    fn manifest(component: serde_json::Value) -> Manifest {
        let src = serde_json::json!({
            "manifest_version": "3.0.0",
            "date": 1735689600,
            "min_client_version": "1.0.0",
            "networks": {"mainnet": "0.15.0"},
            "channels": [{"name": "0.15.0", "components": [component]}]
        });
        midenup::manifest::VersionedManifest::parse_str(&src.to_string()).unwrap()
    }

    fn built(version: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "name": "vm",
            "version": version,
            "kind": "executable",
            "installation_method": {"kind": "cargo", "crate_name": "miden-vm"},
            "installed-executable": "miden-vm"
        })
    }

    /// A registry laid out on disk, holding `miden-vm` 0.2.0 with a lockfile and 0.3.0 without.
    fn registry(dir: &Path) -> Registry {
        let index = dir.join("index/mi/de");
        std::fs::create_dir_all(&index).unwrap();
        std::fs::write(
            index.join("miden-vm"),
            concat!(
                r#"{"name":"miden-vm","vers":"0.2.0","yanked":false}"#,
                "\n",
                r#"{"name":"miden-vm","vers":"0.3.0","yanked":false}"#,
                "\n",
            ),
        )
        .unwrap();
        let crates = dir.join("crates/miden-vm");
        std::fs::create_dir_all(&crates).unwrap();
        std::fs::write(
            crates.join("miden-vm-0.2.0.crate"),
            crate_file(&["miden-vm-0.2.0/Cargo.lock"]),
        )
        .unwrap();
        std::fs::write(
            crates.join("miden-vm-0.3.0.crate"),
            crate_file(&["miden-vm-0.3.0/Cargo.toml"]),
        )
        .unwrap();
        Registry {
            index: format!("file://{}", dir.join("index").display()),
            crates: format!("file://{}", dir.join("crates").display()),
        }
    }

    fn registry_version(version: &str) -> serde_json::Value {
        serde_json::json!({"kind": "registry", "version": version})
    }

    #[test]
    fn a_registry_crate_must_be_published_with_a_lockfile() {
        let dir = tempfile::Builder::new().prefix("online-registry").tempdir().unwrap();
        let registry = registry(dir.path());

        let ok = manifest(built(registry_version("0.2.0")));
        assert!(check(&ok, None, &registry).is_empty());

        for (version, expected) in [("0.3.0", "without a Cargo.lock"), ("0.4.0", "no published")] {
            let m = manifest(built(registry_version(version)));
            let errors: Vec<String> =
                check(&m, None, &registry).iter().map(|e| format!("{e:#}")).collect();
            assert!(errors.len() == 1 && errors[0].contains(expected), "{errors:?}");
        }
    }

    /// Nothing is fetched for a component the previous manifest already published.
    #[test]
    fn an_unchanged_component_is_not_checked() {
        let unreachable = Registry {
            index: "file:///nonexistent".to_string(),
            crates: "file:///nonexistent".to_string(),
        };
        let m = manifest(built(registry_version("0.4.0")));
        assert!(check(&m, Some(&m), &unreachable).is_empty());
        assert_eq!(check(&m, None, &unreachable).len(), 1);
    }

    #[test]
    fn a_git_revision_must_exist_and_carry_a_lockfile() {
        let dir = tempfile::Builder::new().prefix("online-git").tempdir().unwrap();
        let repo = dir.path();
        let commit = |message: &str| {
            git(
                repo,
                &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", message],
            )
            .unwrap();
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(repo)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap();
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        git(repo, &["init", "-q"]).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "").unwrap();
        git(repo, &["add", "."]).unwrap();
        let without = commit("without");
        std::fs::write(repo.join("Cargo.lock"), "").unwrap();
        git(repo, &["add", "."]).unwrap();
        let with = commit("with");
        // Not a ref tip: a pin is usually an arbitrary commit.
        std::fs::write(repo.join("README"), "").unwrap();
        git(repo, &["add", "."]).unwrap();
        commit("tip");

        let at = |revision: &str| {
            manifest(built(serde_json::json!({
                "kind": "git",
                "repository_url": format!("file://{}", repo.display()),
                "revision": revision
            })))
        };
        let errors = |revision: &str| -> Vec<String> {
            check(&at(revision), None, &Registry::crates_io())
                .iter()
                .map(|e| format!("{e:#}"))
                .collect()
        };
        assert!(errors(&with).is_empty(), "{:?}", errors(&with));
        assert!(errors(&without)[0].contains("no Cargo.lock"), "{:?}", errors(&without));
        let missing = "0123456789012345678901234567890123456789";
        assert!(errors(missing)[0].contains("failed to fetch revision"), "{:?}", errors(missing));
    }
}
