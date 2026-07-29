---
sidebar_position: 2
title: Manifest
---

# Manifest

## Upstream manifest, local state

`midenup` reads and writes two JSON documents. They are structurally distinct and are never parsed by the same code path.

| Document | Written by | Describes |
|---|---|---|
| `channel-manifest.json` | the Miden project | what exists and is installable |
| `$MIDENUP_HOME/state.json` | `midenup` | what this machine has installed |

They share no top-level key — the first declares `manifest_version`, the second `state_version` — so neither can be mistaken for the other. Handing one to the parser that expects the other is an error naming both, rather than a pile of missing-field complaints.

The upstream manifest is the source of truth for *everything about a component*: where to fetch it, what it installs, what it is called. Local state records only what this machine chose and what it got. Nothing is duplicated between them, because two copies of one fact drift.

Most users will use the manifest published and maintained by the Miden team [here](https://0xmiden.github.io/midenup/channel-manifest.json). `midenup` also supports custom manifests via `MIDENUP_MANIFEST_URI`; see [Custom manifests](#custom-manifests).

## Versioning and forward compatibility

`manifest_version` is a semantic version, and compatibility is decided on the **major** component alone:

| Condition | Behaviour |
|---|---|
| major above what this build supports | rejected: the manifest needs a newer `midenup` |
| major equal, minor or patch newer | **accepted** |
| major below | rejected, except for the supported upgrade path from 1.0.1 |

A newer minor version is accepted because a manifest is allowed to grow, and three rules make that safe:

- **Unknown fields are preserved.** Every schema type captures keys it does not recognize and emits them again unchanged, so an older `midenup` reading and rewriting a newer manifest loses nothing.
- **Unknown component kinds are opaque, not fatal.** A component whose `kind` this build has never heard of parses, round-trips, and belongs to no profile. It fails only if something explicitly selects it.
- **The version is read before the document is.** A minimal header parse decides compatibility first, so a manifest from the future is diagnosed rather than half-parsed.

## Channels

A channel is a set of [components](#components) under one version, meant to be used together.

```json
{
  "name": "0.15.0",
  "alias": "stable",
  "migrates_from": "0.14.0",
  "components": [ /* ... */ ]
}
```

- `name` — semantic version; the channel's identity.
- `alias` — `stable`, `nightly`, `nightly-<tag>`, or an ad-hoc tag.
- `migrates_from` — this channel supersedes the named one. An installation of that channel is carried here on the next update: its selection transfers verbatim, and its `var/` directory is renamed so client data follows the toolchain.

### The stable channel

A channel is *the* stable channel if it carries `alias: "stable"`; otherwise the highest non-prerelease version wins. This is computed from the upstream manifest only. Local state records channel versions and never an alias, so a stale local copy cannot disagree with upstream about what `stable` means.

## Components

A component is one installable thing: an executable, a package, an asset, a Cargo extension, or a `command` that is nothing but a way to invoke something else.

Each declares where it comes from (`version`: a registry version, a git revision, or a local path), what it installs, which profiles it belongs to, and what it requires. Profiles are `empty`, `minimal`, and `complete`; `minimal` is the default, and `complete` means every component in the channel regardless of its own `profiles` field.

### Artifacts

An artifact is one named file plus the rules for finding it per target:

```json
"artifacts": {
  "miden-vm": {
    "uri": "https://github.com/0xMiden/miden-vm/releases/download/v%version/%basename-%target",
    "digest": "sha256:9f86d081…",
    "targets": {
      "aarch64-apple-darwin":     { "basename": "miden-vm" },
      "x86_64-unknown-linux-gnu": { "basename": "miden-vm" }
    }
  }
}
```

The map key is both the artifact's identity **and the exact filename it is installed as**. It must be a single safe path segment: non-empty, no separators or NUL, not `.` or `..`, and not starting with `-`. Two artifacts resolving to the same destination — within a component or across two of them — is a validation error naming both owners.

Substitutions available in a URI are `%target`, `%version` (which requires a registry version), `%basename` (defaulting to the component name), and `%extension`. Per-target values override component-level ones.

`digest` is **recorded and never verified**. It exists so that a future release can start checking it without a schema change; nothing today draws any conclusion from it, and nothing should claim otherwise.

## Validation

Validation is an authoring gate, not a parsing gate: `update-manifest check` runs the full structural validator, and plan construction re-checks what depends on the current target. Parsing deliberately does *not* validate — the published manifest has historically contained channels with dangling requirements, and refusing to parse would break every command for every user over a channel they were not using.

The structural rules are platform-neutral: duplicate names, dangling or cyclic `requires`, unsafe artifact ids, destination collisions, alias conflicts, and malformed digests. Target availability is checked only when an installation is actually planned.

## Custom manifests

:::warning
This functionality is still in early stages of development. Currently, this requires writing the channel manifest manually.
:::

`midenup` supports custom, user-authored manifests for installing and managing custom toolchains. These can contain components installed from the local filesystem, from a specific git revision, or from a registry. Use `update-manifest` to author them: it shares the schema types and the validator with `midenup` itself rather than reimplementing either, and it validates every mutation before writing, leaving the original byte-for-byte intact if the result would not be valid.
