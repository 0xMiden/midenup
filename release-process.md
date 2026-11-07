# Releases

Release all publishable crates at `workspace.package.version` in `Cargo.toml`.
The release tag is `v` followed by that version. Keep `update-manifest` unpublished
with `publish = false`.

Prepare a version bump on `next` and resolve `Cargo.lock`. Update internal
workspace dependency requirements when needed. Update the changelog in the same PR.
Run these checks before merging:

```sh
bash scripts/check-release.sh
cargo +stable publish --workspace --locked --dry-run --exclude update-manifest
```

The required `release / gate` check runs the version check for every PR.
Changes to release inputs also run Cargo's workspace dry-run.
Cargo verifies the packaged crates together before publication credentials
are available.

Merge the release candidate into `main` and dispatch `.github/workflows/release.yml`
from `main`. The workflow derives its version and package list from the checked-out
Cargo manifests. It refuses a tag pointing elsewhere or an existing crate version
published from different source.

Linux and macOS binaries are built and smoke-tested before approval. Their archives
are attested and uploaded to a draft release, along with the installer.
The workflow downloads the assets again and checks their hashes.

Approve the `release` environment after inspecting the draft and successful checks.
The upload job uses crates.io Trusted Publishing and `cargo publish --workspace`.
It skips compilation because the workspace dry-run already verified the packages.
A separate job rechecks the draft assets and release refs. It creates the tag if absent
and publishes the draft under the repository's release immutability settings.

For a partial publication, redispatch from the same commit. Already published crates
are skipped only when their recorded source commit matches. A version published
from different source or a tag pointing elsewhere requires a new release version.
Never move a published tag. Keep the trusted publishers configured for `release.yml`
and the `release` environment in this repository.
