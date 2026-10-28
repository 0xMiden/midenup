# Update instructions

This guide is meant to describe how to update midenup's manifest.

Each toolchain has an associated version which corresponds to the version of the Miden protocol that the toolchain is oriented towards.

You must ensure that all components in a toolchain are compatible with the given protocol version. The VM component in particular must be the same version as the protocol itself was built aggainst. You can look on crates.io to see what versions of each crate a component depends on.

Authoring a toolchain and publishing it are two separate steps. A toolchain in the manifest reaches
nobody until a *network* is pointed at it: `mainnet`, `testnet` and `devnet` each name the toolchain
that network runs, and `midenup install mainnet` installs whatever `mainnet` names today. Moving
that pointer is the `promote` step below, and it is what makes a release *reachable* — but only once
the edited manifest itself is published, which is a separate step again (see [Publishing](#publishing)
at the end).

## Prerequisites

Make sure you have built the `bin/manifest-utils` CLI tool with:

```
make manifest-utils
```

The following steps will use this tool to perform modifications to the channel manifest.

## New toolchain version

When a new release[^1] is made, a new channel entry needs to be added to the manifest's `channels` array. The channel name should match the protocol version it is linked to, without the patch version set to `0` (e.g. `0.15.0`). The simplest way to do this is to clone the toolchain a network already runs and give it the new version:

```
bin/manifest-utils clone-toolchain --manifest-path manifest/v3/manifest.json \
    --from mainnet --to 0.15.0
```

`--from` accepts either a network name or a toolchain version; `--to` must be a version, since a
toolchain is named by its version and never by a network.

Note that the clone deliberately does **not** carry any network across: the new toolchain is a
draft, and no user is tracking it. It reaches users only once it is promoted (see below). Nor does
it carry the source's `migrates_from`. A toolchain is never removed from the manifest, since users
may still have it installed. If the new toolchain supersedes one that is already absent, say so with
`--migrates-from <VERSION>`: `midenup update` then carries installations of the absent toolchain to
the new one.

Next, you will need to update each component in the cloned toolchain, as appropriate. See the section on updating an existing toolchain for details.

## Updating an existing toolchain (minor/patch)

In typical cases, this is just a matter of bumping the version of each affected component - for more complex changes, see the output of `bin/manifest-utils help update-component`, or modify the manifest by hand. Bumping the component version is as simple as:

```
bin/manifest-utils update-component --manifest-path manifest/v3/manifest.json \
    $COMPONENT \
    --channel $CHANNEL \
    --authority="{\"kind\":\"registry\",\"version\":\"$COMPONENT_VERSION\"}"
```

`--channel` accepts a network name as well as a version, but prefer naming the version explicitly:
which toolchain a network runs changes over time, so a command naming a network does not mean the
same thing when it is run again later.

For newly added components, see the `add-component` subcommand.

For removed components, see the `remove-component` subcommand.

## Pointing a network at a toolchain

A toolchain in the manifest is installable by version, but nobody tracking a network sees it until
that network names it:

```
bin/manifest-utils promote --manifest-path manifest/v3/manifest.json \
    testnet 0.15.0
```

This deploys nothing. It records that `midenup install testnet` resolves to `0.15.0` from now on,
and that `midenup update testnet` should carry existing installations there - along with their
component selection and their data.

`promote` is also how a network is created: if the named network is not in the manifest yet, it is
added.

It refuses, before writing anything:

- a network named like a version, which would be ambiguous with a toolchain of the same name;
- `stable`, `beta` or `nightly` — these are synonyms `midenup` rewrites to `mainnet`, `testnet` and
  `devnet` as it reads them, so a network declared under one of them could never be reached. Promote
  the network itself;
- a toolchain that is not in the manifest;
- a toolchain that is not installable — it is resolved for the `complete` profile first, so that a
  network never names a toolchain whose users would discover it is broken at install time;
- a move to an *older* toolchain than the network names now, unless `--allow-downgrade` is passed.
  Following that pointer hands every user tracking the network a toolchain older than the one their
  data was written by, so it has to be deliberate.

When a toolchain that has been running on testnet is deployed to mainnet, promote mainnet to it as
well. Several networks naming one toolchain is the normal state, and is expected:

```
bin/manifest-utils promote --manifest-path manifest/v3/manifest.json \
    mainnet 0.15.0
```

The tool prints what it did — `created network 'testnet' at 0.15.0`, or `moved 'mainnet' from 0.14.0
to 0.15.0` — and prints it only after the write has committed, so the line is safe to check the
change against in review. A promotion that changes nothing says so and writes nothing.

## Checking the result

```
cargo make check-manifest
```

Validation covers the networks map as well as the channels: every network must name a toolchain that
exists in the same document, no network may be named like a toolchain or after one of the synonyms,
and `mainnet` must be declared, since it is the toolchain `midenup` uses when nothing else selects
one. No two toolchains may declare `migrates_from` the same one, since `midenup update` must find
exactly one successor, and a toolchain may only migrate from an older one. A component must not be
fetched from a local path, a git branch, a git tag or a git revision that is not a full commit ID,
whose contents change without the manifest changing, nor from a `file://` artifact or git repository,
which only exists on the machine that wrote the manifest, and the timestamp must not be in the future.
Every component must install on each target midenup is released for (`aarch64-apple-darwin` and
`x86_64-unknown-linux-gnu`): a prebuilt component missing an artifact for one must be
`prebuilt-with-cargo-fallback`, and any other reason the installer would refuse it, such as an
artifact not named after the installed executable, is reported for that target. A field the schema does not define is refused: `midenup` preserves such
fields when reading, so a misspelled one would be published and silently never read. The exception
is a field whose value is empty (`[]`, `{}` or `null`), which is indistinguishable from a known
field the schema omits when empty; such a typo is harmless, since an empty field reads the same as
an absent one.

There is deliberately no ordering rule between networks: a mainnet hotfix can legitimately put
mainnet ahead of testnet.

To also judge the change as a replacement for the manifest it supersedes, compare against that one.
`--against` takes any manifest URI; the one on the branch you are targeting is the usual choice:

```
git show origin/next:manifest/v3/manifest.json > /tmp/previous-manifest.json
cargo make check-manifest --against file:///tmp/previous-manifest.json
```

This refuses a timestamp that did not advance, a removed network, a toolchain dropping or replacing
the `migrates_from` it was published with, a lower minimum client or manifest version, a network
moving to an older toolchain, a component moving to an older version, a removed toolchain, a
component removed from a toolchain, and a component changing its installation method. CI runs this
comparison on every pull request against the base branch as it was when the pull request was last
updated, on every push to `next` against the previous tip, and before deploying a push to `main`.

Every component built from source (`cargo` or `prebuilt-with-cargo-fallback`) must set
`rustup_channel` to an exact release such as `1.90.0` or a dated nightly such as
`nightly-2025-01-01`; `1.90` or `stable` can name a different compiler tomorrow. A `legacy-package`
built from a crate is refused, since its build is neither locked nor on a pinned toolchain. An
`oci://` reference in a command must be pinned by digest, such as
`oci://ghcr.io/0xmiden/miden-local-network:v%{version}@sha256:...`, since Docker fetches it and a tag
can be pushed again.

Add `--online` to also check upstream that each crate version a component builds or extracts from is
published on crates.io and not yanked, that each pinned git revision exists in its repository, and
that every source build has a `Cargo.lock` to build with: packaged in the crate, or committed at the
revision, and that the tag of each pinned `oci://` reference resolves to the digest it is pinned to.
With `--against`, only new or changed components are checked. CI passes `--online` to the
comparison above.

To see what a change does, list it against the same manifest:

```
bin/manifest-utils diff --manifest-path manifest/v3/manifest.json \
    file:///tmp/previous-manifest.json
```

It prints one Markdown line per change: the schema and minimum client versions, networks added,
removed or moved, toolchains added (with their components) or removed, `migrates_from` changes,
unknown fields, and components added, removed or changed, with each changed field's previous and
new value. An empty field counts as absent. Downgrades are marked.

The pre-release test installs real toolchains and runs their components on Linux x86-64 and macOS
ARM64, the targets midenup is released for. It runs on pull requests into
`main`, on pull requests labelled `check:install`, and on every push to `main`. On a pull request
that changes nothing outside `manifest/`, it installs only the toolchains whose contents changed
since the base branch, including ones no network names; otherwise it installs every network's
toolchain. A tested toolchain that a network names is installed through that network. A toolchain
with a `prebuilt-with-cargo-fallback` component is installed a second time with those components
built by `cargo`, so the fallback is known to work before a download ever fails. To scope a local
run the same way:

```
PRERELEASE_AGAINST=file:///tmp/previous-manifest.json cargo make prerelease-test
```

Acceptance scenarios in `acceptance/scenarios.toml` check that the components of a toolchain work
together. Each names the toolchains it covers (`toolchain`, a semver range that must have an upper
bound) and the components it exercises (`requires`), and is either a shell `script`, which passes
when it exits successfully, or a `lit` suite in a directory next to the file, run with `litcheck`.
Every component of a toolchain needs at least one scenario: a change to a toolchain with an uncovered
component is rejected, so a component is only carried into a new toolchain line once its team widens
its scenarios' ranges (see `acceptance/README.md`).
CI runs the scenarios that require a component a pull request changes, and every scenario before
deploying. Each runs against a scratch midenup home with the toolchain installed, `miden` on `PATH`,
and the versions in `MIDEN_TOOLCHAIN_VERSION` and `MIDEN_COMPONENT_<NAME>_VERSION` (and as lit
features such as `vm=0.17.2`). A scenario that needs Docker declares `needs = ["docker"]` and is
skipped where Docker is missing. To run them locally:

```
cargo build -p midenup -p manifest-utils
./target/debug/manifest-utils acceptance --manifest-path manifest/v3/manifest.json \
    --against file:///tmp/previous-manifest.json --midenup target/debug/midenup
```

## Publishing

Everything above edits your working copy and nothing else. `promote` deploys nothing, `make
check-manifest` deploys nothing, and no user has seen any of it yet.

What users fetch is the copy of `manifest/` deployed to GitHub Pages, and
`.github/workflows/publish-manifest.yml` runs that deployment on any push to `main` touching
`manifest/**`. So the release lands the way every other change does:

```
git add manifest/v3/manifest.json
git commit
```

then open a pull request for review. Merging to `main` triggers the Pages deployment, and *that* is
the point at which the new toolchain and the promotion become reachable. Until the merge,
`midenup install mainnet` on any machine still resolves to whatever the deployed manifest says —
a promotion that only exists locally has shipped to nobody.

[^1]: A release refers to a tagged version of the Miden protocol. That version tag serves as the
reference point for assembling a compatible toolchain, as all related components (VM, client, node,
faucet, etc.) are expected to align with it.
