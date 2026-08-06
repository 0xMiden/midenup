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

Make sure you have built the `bin/update-manifest` CLI tool with:

```
make update-manifest
```

The following steps will use this tool to perform modifications to the channel manifest.

## New toolchain version

When a new release[^1] is made, a new channel entry needs to be added to the manifest's `channels` array. The channel name should match the protocol version it is linked to, without the patch version set to `0` (e.g. `0.15.0`). The simplest way to do this is to clone the toolchain a network already runs and give it the new version:

```
bin/update-manifest --manifest-path manifest/channel-manifest.json \
    clone-toolchain --from mainnet --to 0.15.0
```

`--from` accepts either a network name or a toolchain version; `--to` must be a version, since a
toolchain is named by its version and never by a network.

Note that the clone deliberately does **not** carry any network across: the new toolchain is a
draft, and no user is tracking it. It reaches users only once it is promoted (see below).

Next, you will need to update each component in the cloned toolchain, as appropriate. See the section on updating an existing toolchain for details.

## Updating an existing toolchain (minor/patch)

In typical cases, this is just a matter of bumping the version of each affected component - for more complex changes, see the output of `bin/update-manifest help update-component`, or modify the manifest by hand. Bumping the component version is as simple as:

```
bin/update-manifest --manifest-path manifest/channel-manifest.json \
    update-component $COMPONENT \
    --channel $CHANNEL \
    --authority=$COMPONENT_VERSION
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
bin/update-manifest --manifest-path manifest/channel-manifest.json \
    promote testnet 0.15.0
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
bin/update-manifest --manifest-path manifest/channel-manifest.json \
    promote mainnet 0.15.0
```

The tool prints what it did — `created network 'testnet' at 0.15.0`, or `moved 'mainnet' from 0.14.0
to 0.15.0` — and prints it only after the write has committed, so the line is safe to check the
change against in review. A promotion that changes nothing says so and writes nothing.

## Checking the result

```
make check-manifest
```

Validation covers the networks map as well as the channels: every network must name a toolchain that
exists in the same document, no network may be named like a toolchain or after one of the synonyms,
and `mainnet` must be declared, since it is the toolchain `midenup` uses when nothing else selects
one.

There is deliberately no ordering rule between networks: a mainnet hotfix can legitimately put
mainnet ahead of testnet.

## Publishing

Everything above edits your working copy and nothing else. `promote` deploys nothing, `make
check-manifest` deploys nothing, and no user has seen any of it yet.

What users fetch is the copy of `manifest/` deployed to GitHub Pages, and
`.github/workflows/publish-manifest.yml` runs that deployment on any push to `main` touching
`manifest/**`. So the release lands the way every other change does:

```
git add manifest/channel-manifest.json
git commit
```

then open a pull request for review. Merging to `main` triggers the Pages deployment, and *that* is
the point at which the new toolchain and the promotion become reachable. Until the merge,
`midenup install mainnet` on any machine still resolves to whatever the deployed manifest says —
a promotion that only exists locally has shipped to nobody.

[^1]: A release refers to a tagged version of the Miden protocol. That version tag serves as the
reference point for assembling a compatible toolchain, as all related components (VM, client, node,
faucet, etc.) are expected to align with it.
