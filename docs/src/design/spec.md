---
sidebar_position: 3
title: Specification
---

# Specification

This document specifies `midenup`'s manifest schema (version 3.0), local installation state, resolution model, and installation executor based .

---

## 1. Purpose and scope

`midenup` is a multi-call executable installed under two names:

- **`midenup`** - toolchain management: install, update, uninstall, select toolchains.
- **`miden`** - an all-in-one CLI entry point that dispatches subcommands to toolchain components.

This specification covers:

- the **upstream channel manifest** wire format (schema v3);
- the **local installation state** document;
- the **component model**: kinds, installation methods, artifacts, destinations, runtime metadata;
- **selection and resolution**: profiles, project toolchain files, dependency closure;
- the **installation executor**: acquisition, staging, publication, recovery, uninstall;
- **runtime dispatch** semantics for `miden`;
- **migration** from manifest v1.0.1;
- **validation** rules and the **error taxonomy**;
- the **manifest authoring tool** (`update-manifest`).

Out of scope, with rationale in §16.

### 1.1 Design goals

1. **Schema stability.** Adding a component kind, an installation method, or an artifact attribute must not require a breaking schema change or break older `midenup` binaries for unrelated channels.
2. **No silent data loss.** Every failure either leaves prior state byte-for-byte intact or completes to a consistent new state. There is no third outcome.
3. **Correct multi-project behavior.** Two projects using the same channel with different component sets must not fight over each other's components.
4. **Fast, offline dispatch.** `miden <cmd>` must not touch the network.
5. **Minimal assumptions about components.** The system must not assume a component is executable, is a file, is downloaded, or exists at all in a given channel.

---

## 2. Glossary

Each term has exactly one meaning throughout this document.

| Term | Meaning |
|---|---|
| **Channel** | A named, versioned set of components (e.g. `0.15.0`). Identified by a semver version. |
| **Network** | A moving name for a channel (`mainnet`, `testnet`, `devnet`): the toolchain that network currently runs. Declared upstream, never derived. |
| **Component** | A named, installable (or purely virtual) unit within a channel. |
| **Artifact** | A single named file that a component installs, plus the rules for locating it per target. |
| **Artifact ID** | The exact filename the artifact is installed as. Also its key within a component. |
| **Profile** | A named grouping of components: `empty`, `minimal`, `complete`. |
| **Root** | A component explicitly selected for installation, before dependency closure. |
| **Intent** | The persisted description of what the user wants installed for a channel: profiles plus roots. |
| **Resolution** | Expanding intent into an ordered, deduplicated component set via dependency closure. |
| **Installation plan** | A fully resolved, target-specific description of every acquisition and file placement. |
| **Plan key** | A canonical digest over the material inputs of an installation plan. Diagnostic only. |
| **Publication** | An immutable directory tree containing one installed channel, plus its receipt. |
| **Publication ID** | An opaque unique identifier for a publication. Not derived from content. |
| **Receipt** | An immutable record inside a publication describing exactly what it owns. |
| **Active view** | The subset of an installed channel that the current project has requested. |
| **Sysroot** | The publication directory of the active toolchain, exposed as `$MIDEN_SYSROOT`. |

---

## 3. On-disk layout

```
$MIDENUP_HOME/
├── state.json                          # local installation state (sole logical authority)
├── .lock                               # advisory lock; guards mutating operations only
├── channel-manifest.json               # cached copy of the last successfully fetched upstream manifest
├── journal/
│   └── <operation-id>.json             # at most one; present only during a physical operation
├── publications/
│   └── <channel>-<publication-id>/     # immutable
│       ├── receipt.json
│       ├── bin/
│       ├── lib/
│       ├── etc/<component>/
│       └── opt/
├── var/
│   └── <selector>/                     # MUTABLE USER DATA - never deleted by install/update
│                                       # keyed by what the user selected: a network, or a version
├── toolchains/
│   ├── <channel-version>  -> ../publications/<channel>-<publication-id>
│   ├── <network>          -> <channel-version>       # one per network; derived from upstream
│   └── default            -> <channel-version> | <network>   # set by `midenup override`
└── opt -> toolchains/<active-channel>/opt
```

### 3.1 Publication immutability

A publication directory is written once, verified, published, and thereafter never modified. Any change to the installed set produces a **new** publication; the `toolchains/<channel>` symlink is repointed atomically; the old publication becomes unreferenced and is reclaimed by `midenup gc` (§11.6).

:::note
This previously said the old publication is *removed* as a post-commit cleanup step, but it cannot be removed: another process may be executing a component out of it at that moment - `miden vm ...` in one terminal while the other installs - and removing the directory under a running program is fatal (macOS `SIGKILL`s it; on Linux a script's interpreter fails to open it).

This was confirmed with a concurrent-activation test on macOS that failed roughly one in five runs with `Killed: 9`, and zero failures once we switched to the current specification behavior. Once the symlink is repointed, nothing can start using the old publication, and leaving it unreferenced will cause it to be picked up by `gc` the next time it is run. Note that an explicit `uninstall` _does_ removes its publication, since we're honoring a direct user request in that case.
:::

### 3.2 `var/` is outside the publication, and keyed by the selector

`var/` holds mutable component-owned state - most importantly the Miden client's local database, referenced from the manifest as `%var(data)`. It is outside the publication because a publication is replaced wholesale on every change and this must survive that.

It is keyed by the **toolchain selector the user chose** - `var/mainnet`, `var/testnet`, `var/0.15.0` - and not by the channel that selector resolves to. Two consequences, both intended:

- **Networks are logically distinct even when they share a toolchain.** Several networks routinely name one channel, so a channel key would pool a user's mainnet accounts and their testnet notes into one database. The selector is the identity the user is working under, and that is what their data belongs to.
- **A selector does not move when a pointer moves.** `mainnet` advancing to a new channel leaves its store exactly where it was, so nothing is ever carried between keys and no operation has to order a pointer move against a data move.

- Install, update, republication, and pointer moves **never** read, write, move, or delete `var/`.
- Exactly two operations move it, and this is the whole list. Channel migration (§11.4) **renames** `var/<old>` to `var/<new>`. Both are pinned-version selectors, and migration is what retires one: the channel the user pinned ceases to exist. A network selector is neither source nor destination.
- The other is the one-time conversion of a pre-network home (§12.5), which **renames** `var/<version>` to `var/mainnet`. Such a home kept a single store under the channel it tracked, and that store is the default network's - unless the home records a pin on that version, in which case the version is already the selector and nothing moves.
- `midenup uninstall <selector>` removes the publication and the state record. It removes `var/<selector>` - the selector exactly as given - **only** when `--purge` is passed; otherwise it is retained and the user is told it was kept and where it lives. Uninstalling a channel therefore never removes a network's store, which is correct: the network outlives any channel it names.
- `%var` resolves to `$MIDENUP_HOME/var/<selector>`, created on demand at dispatch time.

Previously, `var/` lived inside the publication, and every toolchain update would destroy the user's client data (if it was in the toolchain `var` directory).

### 3.3 `opt/` and the clap display shim

`opt/` contains symlinks named `miden <component>` pointing at `../bin/<installed-executable>`. They exist so that `clap`, which derives program name from `argv[0]`, renders help text as `miden vm ...` rather than `miden-vm ...`. `$MIDENUP_HOME/opt` is a symlink to the active publication's `opt/`, and is prepended to `PATH` for spawned components.

### 3.4 Network links

`toolchains/<network>` is one symlink per network (§5.1), naming the channel that network runs. Several of them may name the same channel.

- It is written only for a channel that is installed here, so it never dangles. Repointing a network at a channel this machine does not have would produce a broken link, and `midenup update <network>` is what advances it (§11.7).
- It records the last answer upstream gave *that this machine acted on*. That is what lets dispatch name the active channel without the network (§13.1): resolving `mainnet` against the upstream manifest on every `miden` invocation is exactly the round trip §13.1 forbids. There is deliberately no fallback - "the highest installed version" is a plausible wrong answer for `mainnet` - so an unresolvable network name sends the caller upstream instead.
- `midenup override <network>` points `default` at this link rather than at a channel directory, so that the default keeps following the network as it moves.

---

## 4. Manifests

There are two manifest documents. They are structurally distinct and are never parsed by the same code path.

### 4.1 Upstream channel manifest

Describes what exists and is installable. Published by the Miden project; never written by `midenup` except to cache it locally verbatim.

```json
{
  "manifest_version": "3.0.0",
  "date": 1735689600,
  "networks": { "mainnet": "0.15.0", "testnet": "0.16.0", "devnet": "0.16.0" },
  "channels": [ /* Channel */ ]
}
```

### 4.2 Local installation state

Describes what this machine has installed. Written only by `midenup`; never published or fetched.

```json
{
  "state_version": "1.0.0",
  "installations": [ /* Installation */ ]
}
```

The top-level discriminating key differs (`manifest_version` vs `state_version`), so the two can never be confused, and neither needs a `role` field. Loading one where the other is expected is a hard error naming both the expected and actual document type.

### 4.3 Version compatibility

`manifest_version` and `state_version` are SemVer version strings. Compatibility is evaluated on the **major** component only:

| Condition | Behavior |
|---|---|
| major > supported | Reject: `manifest requires a newer midenup (found 4.x, supported 3.x)` |
| major < supported | Reject, except for the specific supported migration path (§12) |
| major equal, minor/patch newer | **Accept.** Unknown fields are preserved (§4.4). |
| major equal, minor/patch older | Accept. |

The version is read by a two-stage parse: a minimal header struct containing only the version field is deserialized first, the version is checked, and only then is the full document parsed with the matching schema. The version is never supplied by a serde default and never silently overwritten on serialize.

### 4.4 Forward compatibility

Three mechanisms, all mandatory:

**Unknown fields are preserved.** Every schema type carries an `#[serde(flatten)] extra: Map<String, Value>` field to capture unknown keys. Unknown keys round-trip byte-equivalently through parsing/deserialization and serialization. This makes additive schema evolution free.

**Unknown enum variants are opaque, not fatal.** `kind` and `installation-method` deserialize into a passthrough variant when the tag is unrecognized:

```rust
enum ComponentKind {
    Executable { .. },
    CargoExtension { .. },
    Command { .. },
    Package,
    LegacyPackage { .. },
    Asset,
    /// Any `kind` this build does not recognize.
    Unsupported { tag: String, body: serde_json::Value },
}
```

An `Unsupported` component:

- parses without error and round-trips losslessly;
- is visible to `midenup show`, marked as unsupported;
- is **never** selected implicitly - it belongs to no profile, regardless of what its `profiles` field says, because this build cannot know how to install it;
- **fails plan construction** with a precise diagnostic if named as an explicit root or reached through a `requires` edge from a selected component.

The consequence is that a channel introducing a new component kind stays installable on older `midenup` for every profile that does not include the new component, and produces an actionable error otherwise. Refusing the whole manifest - the current behavior - would brick every older `midenup` for every channel on the first new kind.

**Reserved-but-inert fields.** Fields specified now and honored later must round-trip and be recorded, never silently dropped. Currently this applies to `digest` (§6.4) and `initialization` (§7.5).

---

## 5. Channels

```json
{
  "name": "0.15.0",
  "migrates_from": "0.14.0",
  "components": [ /* Component */ ]
}
```

| Field | Required | Meaning |
|---|---|---|
| `name` | yes | semver; the channel's identity |
| `migrates_from` | no | this channel supersedes the named channel; see §11.4 |
| `components` | yes | the component set |

**Removed from v1** the `tags` array. `Tags::Partial` was a local-state concern and is replaced by derivation (§8.6). `Tags::Migration { NameChange }` becomes the explicit `migrates_from` field on the upstream channel. Local state never carries channel tags.

**Removed in v3** the per-channel `alias` field. It could name at most one release train per channel, which cannot express the ordinary state of a testnet toolchain having been promoted to mainnet. It is replaced by the top-level `networks` map (§5.1).

### 5.1 Networks

A network is a **moving name** for a channel. `mainnet` names whichever toolchain is deployed to mainnet today; `midenup install mainnet` installs that one.

```json
"networks": {
  "devnet":  "0.16.0",
  "mainnet": "0.15.0",
  "testnet": "0.16.0"
}
```

Several networks may name one channel, which is the normal state once a testnet toolchain is promoted to mainnet.

**Declared, never derived.** Which toolchain a network runs is a deployment fact, not a function of version ordering: mainnet may lag testnet by several releases, and a hotfix may put it ahead. No ordering over version numbers can express that, so the map is authored - by `update-manifest promote` (§15) - and read literally.

**Synonyms.** `stable`, `beta` and `nightly` are accepted as input for `mainnet`, `testnet` and `devnet`, and are rewritten as they are read. Everything downstream - output, symlinks, local state, diagnostics - sees only the network name, so a `miden-toolchain.toml` written before networks existed keeps working and means `mainnet`. The mapping is fixed in `midenup` rather than declared in the manifest: it is user vocabulary, not deployment, and letting a manifest author redefine `stable` is not a capability worth having.

**`mainnet` is the default channel**, used when nothing else selects one (§13.2).

**Names are manifest data, not a fixed set.** A name that is not a version parses as a network name and fails at *lookup*, so a new network needs no release of `midenup`. The cost is that a typo is diagnosed late; the diagnostic therefore lists the networks the manifest actually declares.

Validation (§14.2) requires that every network name a channel in the same document, that no network is named like a channel or after one of the synonyms, and that `mainnet` is declared. There is deliberately **no ordering invariant** between networks: a mainnet hotfix legitimately puts mainnet ahead of testnet, and a validator that has to be overridden during an incident is worse than none.

Local state records channel versions only and never a network name, so a stale local copy cannot disagree with upstream about what `mainnet` means. `toolchains/<network>` (§3.4) is derived, rebuilt after every successful operation that installs the channel it names.

---

## 6. Artifacts

An artifact is one named file plus the rules for locating it per target.

```json
"artifacts": {
  "miden-vm": {
    "uri": "https://github.com/0xMiden/miden-vm/releases/download/v%version/%basename-%target",
    "digest": "sha256:9f86d0…",
    "targets": {
      "aarch64-apple-darwin":     { "basename": "miden-vm" },
      "x86_64-unknown-linux-gnu": { "basename": "miden-vm" }
    }
  }
}
```

### 6.1 Artifact ID is the installed filename

The map key is both the artifact's identity and the **exact filename it is installed as**. It must be a single safe path segment:

- non-empty;
- contains no `/`, `\`, or NUL;
- is not `.` or `..`;
- does not begin with `-`.

Two artifacts resolving to the same destination path - within one component or across components in the same plan - is a validation error, reported with both owners.

### 6.2 Target-specific vs target-agnostic

Target-specific artifacts declare a `targets` map; the URI must contain `%target`. Target-agnostic artifacts declare a bare `uri` and no `targets`.

Substitutions: `%target`, `%version` (requires a registry authority), `%basename` (defaults to the component name), `%extension`. Per-target substitutions override component-level ones.

### 6.3 Target support is required, not optional

If a component is selected and any of its declared artifacts has no entry for the current target, plan construction **fails**. The single exception is an executable with `prebuilt-with-cargo-fallback`, where missing target support selects the Cargo path instead.

### 6.4 Digests are reserved, not verified

`digest` is optional, of the form `<algorithm>:<hex>`. It is validated for shape at parse time, recorded verbatim in the installation receipt when present, and round-trips losslessly. **No verification is performed currently.** Enabling verification later is a behavior change, not a schema change.

For an archived artifact (§6.5) the digest describes the archive as fetched, not the file installed out of it: it belongs to the bytes at the URI.

### 6.5 Archived artifacts

An artifact may be published inside an archive. `archive` names the format:

```json
"artifacts": {
  "miden-vm": {
    "uri": "https://github.com/0xMiden/miden-vm/releases/download/v%version/%basename-%target.tar.gz",
    "archive": "tar.gz",
    "targets": {
      "aarch64-apple-darwin":     { "basename": "miden-vm" },
      "x86_64-unknown-linux-gnu": { "basename": "miden-vm" }
    }
  }
}
```

- **Format:** `tar.gz`. Others are additive: a format is a variant plus a reader, and a manifest declaring one this build does not know would still parse (§4.4) and be rejected only when an installation is planned for it. The plan carries a format narrowed to the set this build reads (`SupportedFormat`), not the declared one, so the executor has no unsupported case to handle.
- **The archive must hold exactly one file**, which is the artifact. Zero, or more than one, is an error; nothing is inferred from member names, and there is no way to select among several. Directory entries are skipped, so a file nested under one is found.
- **The decompressed archive may be at most 2 GiB**, including format metadata and padding. Anything larger is rejected while it is being unpacked.
- The object form `{ "format": "tar.gz" }` is also accepted, so a newer schema can add fields beside the format without an older `midenup` losing them (§4.4).

An archive changes only how the bytes travel: the artifact id is still the installed filename, the destination and mode still come from §8, and the receipt records `prebuilt` like any other artifact. The compressed source is held in memory while the decompressed archive is streamed. Its sole file is written to temporary storage and published only after the complete archive has been validated; the archive container itself is never written.

---

## 7. Components

```json
{
  "name": "vm",
  "version": { "kind": "registry", "version": "0.15.0" },
  "kind": "executable",
  "installation-method": { "kind": "prebuilt" },
  "installed-executable": "miden-vm",
  "profiles": ["minimal"],
  "requires": ["core"],
  "artifacts": { "miden-vm": { /* … */ } }
}
```

### 7.1 Authority

`version` names the versioning authority, unchanged from v1:

- `registry` - a crates.io version;
- `git` - repository URL plus a `revision`, `tag`, or `branch` target;
- `path` - a local filesystem path.

A `branch` target is resolved to a concrete commit at install time and that commit is what gets installed and recorded (§9.2). A `path` authority records the tree's modification state.

### 7.2 Kinds

| Kind | Physical output | Callable via `miden` |
|---|---|---|
| `executable` | one binary in `bin/` | yes, unless `hide` |
| `cargo-extension` | one binary in `bin/` | via `cargo <name>`; `miden` only through aliases |
| `command` | zero or more files in `etc/<component>/` | yes - purely virtual dispatch |
| `package` | one or more files in `lib/` | no |
| `legacy-package` | one file in `lib/` | no |
| `asset` | one or more files in `etc/<component>/` | no |
| `unsupported` | none | no |

### 7.3 Installation methods

`installation-method` applies **only** to `executable` and `cargo-extension`:

- `prebuilt` - artifact required for the current target;
- `prebuilt-with-cargo-fallback { crate-name, rustup-channel?, features? }` - use the artifact when
  the current target is supported and the transfer succeeds; otherwise build with Cargo;
- `cargo { crate-name, rustup-channel?, features? }` - always build with Cargo.

**Packages are never built with Cargo in v3.** `package` components now require prebuilt artifacts. The old v1 behavior is maintained via `legacy-package` (§7.4) as the sole exception.

### 7.4 `legacy-package` is closed

`legacy-package` denotes a Miden package that must be extracted from a Rust crate at install time by compiling an expression against that crate:

```json
{
  "name": "protocol",
  "kind": "legacy-package",
  "crate-name": "miden-protocol",
  "features": ["std"],
  "extractor": "miden_protocol::CoreLibrary::default().package()",
  "installed-package": "protocol.masp"
}
```

It exists because channels up to 0.15.0 did not ship packages as assets in their releases, and the only way to obtain them was using this methodology. It is **closed to new channels**: `update-manifest` refuses to author a `legacy-package` into any channel, and validation reports it as deprecated. When the affected channels are removed, this kind and the generated Cargo script (§9.3) should be removed together.

`installed-package` is the exact output filename. It carries forward from v1's
`installed_library.library_name`, which was previously inferred.

### 7.5 Executable metadata

Shared by `executable` and `cargo-extension`:

| Field | Meaning |
|---|---|
| `installed-executable` | exact binary filename installed into `bin/`. Required. |
| `symlink-name` | name of the `opt/` shim. Defaults to `miden <component-name>`. |
| `call-format` | argv template for direct invocation. Defaults to `["%installed-executable"]`. |
| `aliases` | map of `miden` alias -> argv template |
| `initialization` | argv template for first-run setup. **Recorded, currently never executed.** |
| `hide` | disables direct `miden <name>` invocation; requires at least one alias |

`initialization` is currently preserved through parsing, serialization, migration, and update. No code path executes it at thist ime. It is excluded from the plan key for this reason. It is retained because not only would removing it would be a breaking schema change, we expect that this feature will likely be needed in the future.

`hide` governs **direct `miden <name>` invocation, not shim creation.** `opt/` serves two distinct purposes (§3.3): the clap `argv[0]` display trick, and PATH discoverability - `opt/` is the only toolchain directory placed on `PATH`, so a binary invoked by an external tool resolves only if it has a shim there.

The shim rule is therefore:

| `symlink-name` | `hide` | Shim created |
|---|---|---|
| set | either | `opt/<symlink-name>` |
| absent | `false` | `opt/miden <component-name>` |
| absent | `true` | none |

The `symlink-name`-set-and-hidden row _is_ a valid case: `cargo-miden` is `hide: true` with `symlink-name: "cargo-miden"`, and that shim is how `cargo miden` - which currently backs the `miden new` alias - is found via `PATH`. Suppressing shims for hidden components would break that functionality.

### 7.6 `command` components

A `command` is a virtual component: it defines `miden` subcommands whose implementation is external software or other installed components. For example, the `node` component, the original motivating case, is a set of `docker compose` invocations over YAML assets.

```json
{
  "name": "node",
  "kind": "command",
  "command-name": "node",
  "format": ["docker", "compose",
             "-f", "%etc(node/docker-compose.yml)",
             "-f", "%etc(node/telemetry.yml)"],
  "subcommands": {
    "up":     ["up", "-d"],
    "down":   ["down", "--remove-orphans"],
    "logs":   ["logs", "-f"]
  },
  "artifacts": {
    "docker-compose.yml": { "uri": "https://…/docker-compose.yml" },
    "telemetry.yml":      { "uri": "https://…/telemetry.yml" }
  }
}
```

A `command` may declare zero artifacts, in which case it installs no files at all. It still counts as an installed component, participates in `requires` edges, and appears in `midenup show`.

### 7.7 The component/artifact matrix

Enforced during plan construction. Violations are errors, not warnings.

| Kind | Method | Artifact cardinality | Artifact ID constraint |
|---|---|---|---|
| `executable`, `cargo-extension` | `prebuilt` | exactly 1 | must equal `installed-executable` |
| `executable`, `cargo-extension` | `prebuilt-with-cargo-fallback` | 0 or 1 | must equal `installed-executable` |
| `executable`, `cargo-extension` | `cargo` | 0 | - |
| `package` | n/a | ≥ 1 | any valid ID |
| `legacy-package` | n/a | 0 | - |
| `asset` | n/a | ≥ 1 | any valid ID |
| `command` | n/a | ≥ 0 | any valid ID |
| `unsupported` | n/a | unconstrained | not installable |

### 7.8 Destinations and file modes

Destinations are computed, never declared. There is exactly one rule per kind:

| Kind | Destination | Mode |
|---|---|---|
| `executable`, `cargo-extension` | `bin/<installed-executable>` | `0755` |
| `package` | `lib/<artifact-id>` | `0644` |
| `legacy-package` | `lib/<installed-package>` | `0644` |
| `asset`, `command` | `etc/<component-name>/<artifact-id>` | `0644` |

The v1 implementation applied `0755` to every downloaded file, including packages and YAML assets - the current schema corrects this.

---

## 8. Selection and resolution

### 8.1 Ownership model: one global superset per channel

- A channel has **one** installed publication, holding the union of everything requested.
- A project's `miden-toolchain.toml` declares that project's requirements. Activating it **adds** missing components to the global installation; it **never** removes components merely because a different project asked for less.
- An explicit `midenup install <channel> --profile <p> [--component …]` **replaces** the intent and may remove components outside the newly resolved set.

Two projects on the same channel with disjoint component sets therefore converge on a superset containing both, and neither can break the other. The superset grows monotonically over a channel's lifetime; §11.6 provides the reclamation path.

### 8.2 Intent

Persisted per installation:

```json
"intent": {
  "profiles": ["minimal"],
  "roots": ["client", "debug"]
}
```

`profiles` is the set of profiles observed across all activations and direct installs. `roots` is the set of explicitly named components. There is no third kind of intent.

- **Activation** (project toolchain file): unions the project's profile and components into `intent`, then installs whatever is missing.
- **Direct install**: replaces `intent` outright, even if the resolved physical set is unchanged.
- **Update**: re-resolves the existing `intent` against the new upstream channel.

`complete` dominates: if it is present in `profiles`, the resolved set is every component in the channel regardless of the other entries.

### 8.3 Profiles

Current supported profiles are: 

* `empty` - install no components as the base, only explicitly listed components
* `minimal` - install all of the base developer tools (e.g. the compiler, VM, client)
* `complete` - install all components. 

The default profile (i.e. `Profile::default()`) is `minimal`.

An omitted `profile` key in `miden-toolchain.toml` means `minimal`. An empty `components` list means only the profile's members* - it does **not** mean "install everything."

### 8.4 The resolver

One function, used by every code path that needs a component set. The v1 `create_subset` function and the separate profile-filtering pass are both removed.

```
resolve(channel, intent) -> Result<Vec<&Component>, ResolutionError>

  roots := { c ∈ channel | c.profiles ∩ intent.profiles ≠ ∅ }
         ∪ { c ∈ channel | c.name ∈ intent.roots }
         (or all of channel, if "complete" ∈ intent.profiles)

  closure := transitive closure of roots under `requires`
  return topological_sort(closure), dependencies before dependents
```

Errors, all fatal before any filesystem mutation:

- `intent.roots` names a component not in the channel;
- a `requires` edge names a component not in the channel;
- the `requires` graph contains a cycle;
- a selected component is `unsupported`.

Closure is **fully transitive**. The current implementation expands one level only, so `A -> B -> C` silently omits `C`.

### 8.5 Active view

The active view is the resolution of *this project's* request - profile plus components from its toolchain file - against the installed channel. It is transient and never persisted.

- `miden --help` and command discovery list only the active view.
- Alias resolution and preferred-name resolution use the active view.
- A component that is installed globally but outside the active view **remains executable** when named explicitly, with a warning identifying it as outside the project's declared toolchain.

The active view is a scoping and discovery mechanism, not a security boundary.

**Alias conflicts are scoped to the view.** In v1, `Channel::get_aliases` would hard-error on any duplicate alias across the whole channel. Under a superset that accretes components from multiple projects, two components that no project ever activates together could collide and break *every* command. Therefore: a conflict *within the active view* is an error; a conflict that exists only in
the superset is a warning, and the component in the active view wins.

### 8.6 Partial status is derived

Local state does not record a "partial" flag. When the upstream manifest is available, an installation is displayed as partial if upstream resolves its recorded intent to components it does not hold — that is, the channel has grown within what the user asked for, not merely grown at all. An intent upstream can no longer resolve makes no claim, and when upstream is unavailable, partial status is not displayed.

Components with no physical output (`command` with zero artifacts) count as installed for membership purposes and are exempt from physical verification (§9.6).

The same rule governs the network annotation in `midenup show list`. Each installed channel is
listed with the networks that name it - `0.15.0 (mainnet, testnet)` - as a list, since several
networks naming one channel is normal. Which networks name a channel is upstream's answer, so when
upstream is unavailable the annotation is omitted entirely rather than derived locally: a local
guess would be exactly the derivation networks exist to eliminate, and a stale one would tell a user
they are on mainnet when they are not. The other markers are unchanged: `(needs reinstallation)` is
derived from local state alone - a migrated record with no publication - and is always shown, while
`(partially installed)` and `(unavailable upstream)` need upstream and are omitted with it.

---

## 9. Installation

### 9.1 The installation plan

Resolution produces a `Channel` subset. Plan construction turns that into a target-specific, fully-explicit description with no remaining decisions:

```rust
struct InstallationPlan {
    target: String,
    channel: semver::Version,
    steps: Vec<PlanStep>,          // dependency order
    symlinks: Vec<SymlinkSpec>,    // opt/<name> -> ../bin/<binary>
    key: PlanKey,
}

enum PlanStep {
    Download   { uri: ArtifactUri, dest: PathBuf, mode: u32, owner: ComponentName,
                 digest: Option<Digest>, archive: Option<SupportedFormat> },
    CopyLocal  { src: PathBuf, dest: PathBuf, mode: u32, owner: ComponentName,
                 archive: Option<SupportedFormat> },
    CargoBuild { crate_name: String, authority: ResolvedAuthority, features: Vec<String>,
                 rustup_channel: Option<String>, expect_binary: String,
                 dest: PathBuf, owner: ComponentName },
    ExtractPackage { crate_name: String, authority: ResolvedAuthority, features: Vec<String>,
                     extractor: String, dest: PathBuf, owner: ComponentName },
}
```

:::note
`dest` is always the exact final absolute path.
:::

Plan construction performs all target-availability and matrix validation. **Execution makes no decisions and performs no filtering.**

### 9.2 Resolving mutable authorities

Before the plan key is computed, every authority is pinned:

- `git` + `branch` -> resolve to a commit SHA; install that revision; record the SHA.
- `path` -> canonicalize; snapshot the tree's latest modification time. After the build completes, re-check; if it changed during staging, abort with a diagnostic advising a retry. A silently mismatched path build is worse than a failed one.
- `git` + `tag` / `revision`, `registry` -> already immutable.

### 9.3 Acquisition

**Downloads** are performed natively by `midenup`. For each `Download` step:

1. Transfer to a unique temporary sibling of `dest` (`<dest>.<random>.part`), following at most 10 redirects.
2. Check the HTTP status **after** the transfer completes and reject any non-2xx terminal response. Previously the install script read the response code early as 0, so 404/500 responses would be written to disk as if they succeeded.
3. Reject an empty body.
4. When the step carries an `archive` (§6.5), read the one file out of the transferred bytes and continue with that; the container itself is never written.
5. Apply the mode from the plan.
6. `rename` into place.
7. On any failure, remove the temporary file. If the owning component declares `prebuilt-with-cargo-fallback`, convert the step to `CargoBuild` and retry once; a successful fallback clears the failure.

For `https` sources the destination filename comes from the **plan**, never from the URL.

**Cargo builds** invoke `cargo install` directly as a subprocess - no generated script.

:::note
`CARGO_HOME` is **not** overridden. The two location settings serve different purposes: `--root` decides where the built binary and Cargo's install bookkeeping go, and that *is* isolated per installation; while `CARGO_HOME` holds the registry index, crate cache, git checkouts and credentials, which are shared caches, and isolating them would mean re-downloading the crates.io index for every `MIDENUP_HOME` - slow, and ultimately pointless, since a cache entry is identical regardless of who fetched it and the credentials are the user's. Concurrency is handled by the advisory lock (§9.9), not by cache isolation.
:::

`Config::cargo_home` therefore governs only where the `miden` symlink is placed, which is its actual purpose.

```
cargo [+<rustup-channel>] install --locked --profile <dev|release> [--quiet]
      --bin <installed-executable>
      <authority args> [--features a,b]
      --root <staging-dir>
```

Optional arguments are omitted entirely when empty.

`--bin <installed-executable>` is always passed, so a multi-binary crate cannot deposit unexpected executables. After the build, the expected binary must exist at `<staging>/bin/<installed-executable>` and no unexpected binaries may have appeared; both are checked.

**Package extraction** is the only remaining use of a generated Cargo script. When a plan contains `ExtractPackage` steps, `midenup` generates one script declaring the required crates as dependencies, runs it under `cargo +nightly -Zscript`, and each extractor expression writes its package to its exact `dest`. Plans with no `ExtractPackage` steps generate no script.

### 9.4 Cargo ownership

`cargo install --root` maintains `.crates.toml` and `.crates2.json` in the staging root. These are installer bookkeeping, not toolchain content.

- A **Cargo installation unit** is keyed by `(resolved authority, crate name)`. A plan containing two components claiming the same unit is rejected at planning time.
- Uninstall and replacement operate on the **exact binary path recorded in the receipt**, never via package-scoped `cargo uninstall`, which would remove a sibling component's binary.
- `.crates.toml` and `.crates2.json` are deleted from the staging tree before publication. The receipt is the ownership record.
- For `prebuilt-with-cargo-fallback`, the receipt records which path was actually taken, so uninstall matches the realized method.

### 9.5 Publication protocol

Multi-object publication cannot be made atomic by a single filesystem operation. It is made recoverable* by a journal with one defined commit point.

```
1. PREPARE   write journal/<op-id>.json:
             { op: install|uninstall|migrate, channel, old_publication?,
               new_publication?, target_intent, plan_key }
2. STAGE     build publications/<channel>-<new-pub-id>/
             seeding from the old publication via its receipt's owned paths only,
             omitting anything not in the new plan
3. VERIFY    structural check (§9.6); write receipt.json
4. COMMIT    atomically repoint toolchains/<channel>  <- THE COMMIT POINT
5. RECORD    atomically commit state.json
6. DERIVE    repoint every toolchains/<network> naming this channel, and
             $MIDENUP_HOME/opt
7. CLEAN     release the old publication (see 3.1); delete the journal
```

Uninstall replaces step 4 with an atomic replacement of the symlink by a **tombstone**, so recovery can distinguish a committed removal from external damage. Channel migration journals both publications and removes the old one only after the new state record is committed.

### 9.6 Structural verification

Before publication, for every plan step: the destination exists, is of the expected type, is owned by the expected component, has the expected mode, and collides with nothing else. Components with no physical output are skipped.

Verification is **structural only**. No byte or digest verification is performed (§6.4). A matching plan key is not evidence of matching bytes and never authorizes content reuse.

### 9.7 Recovery

Recovery runs at startup, after migration (§12) and before any upstream fetch.

| Journal state | Action |
|---|---|
| absent | nothing to do |
| present, symlink not yet repointed | discard the staged publication, delete the journal, retain prior state |
| present, symlink repointed | roll **forward**: complete steps 5–7 from the journal's target |
| present, tombstoned symlink | complete the uninstall: commit state removal, clean up |

The journal is the authority after commit; `state.json` is the authority before it. A missing or corrupt `state.json` with no journal is a deterministic divergence error (§14) - never a guess.

### 9.8 Logical-only changes

Changes that touch selection or runtime metadata but no installed file - new aliases, changed call formats, changed `initialization`, an activation that adds no components - are committed as a single atomic `state.json` write.

### 9.9 Concurrency

Mutating operations take an exclusive advisory lock (`flock`) on `$MIDENUP_HOME/.lock`. Read-only operations take no lock.

This is required, not optional: `miden <cmd>` can trigger an install via `ensure_current_is_installed`, so two `miden` invocations in two project directories are two concurrent writers against one `MIDENUP_HOME`.

`miden` dispatch is therefore lock-free *until* it determines an install is needed, at which point it acquires the lock for the duration of that install and releases it before exec'ing the component. Startup recovery (§9.7) and migration (§12) also take the lock, since both mutate.

A blocked writer prints `waiting for another midenup operation to finish…` after one second and blocks up to ten minutes before failing. On acquiring the lock it **re-reads** `state.json` and re-plans, because the prior holder may have changed the installed set.

The lock is process-crash safe: `flock` is released by the kernel on process exit. It does not protect against a shared `MIDENUP_HOME` on a network filesystem, which is unsupported.

### 9.10 Durability scope

The protocol targets **process-crash consistency**: `fsync` on file contents before rename, and atomic `rename` for every commit point. It does not claim sudden-power-loss durability, which would require directory-fsync guarantees and platform-specific testing.

---

## 10. The plan key

```
plan_key = "pk1:" || hex(sha256(canonical_encoding(inputs)))
```

**Included:** target triple; each selected component's name, resolved authority (with branches pinned to commits and paths to canonical path + mtime), kind, installation method, artifact IDs and resolved URIs, the archive format of any archived artifact, exact destinations, file modes, Cargo crate name / features / rustup channel, and the complete symlink layout.

The archive contribution is emitted only for an artifact that declares one, so an unarchived artifact encodes identically either way. That is the general rule for extending the encoding: a contribution emitted only for an input no manifest could express without it keeps every existing key byte-for-byte stable, so no installed component is reclassified as changed and the prefix stands; a contribution that alters an already-expressible input requires the prefix to change.

**Excluded:** intent and profiles; aliases; call formats; `subcommands`; `initialization`; which networks name the channel; anything that does not change a byte on disk.

Aliases are excluded because they are resolved at dispatch time from `state.json` and are never materialized as files. `opt/` symlinks are included because they are.

**Canonicalization:** fields are encoded in a fixed declared order with explicit length prefixes; absent and empty are encoded distinctly; collections are sorted by a declared key. The `pk1:` prefix versions the algorithm - when destination policy changes, the prefix changes, and old keys are treated as *unknown* (reinstall) rather than *changed*.

**The key is diagnostic and cache-input metadata.** It names nothing on disk: publication directories are named by an opaque, randomly generated publication ID. Equal keys do not imply equal bytes and never authorize skipping work or reusing another publication's content.

---

## 11. Update, activation, and migration

All of the following route through the same resolver and the same executor. The special-cased name-intersection path for `update stable` and the "partial channels suppress all new components" rule are both removed.

| Operation | Intent effect | Physical effect |
|---|---|---|
| `midenup install <channel-or-network> [--profile] [--component]` | replaces | may add and remove |
| toolchain-file activation | unions | adds only |
| `midenup update <ch>` (same version) | unchanged | re-resolve, replace changed |
| `midenup update <network>` (pointer moved) | carried to the channel it now names | full install into that channel |
| channel migration (`migrates_from`) | carried to new channel | install new, remove old |

### 11.1 Change classification

`Component::is_up_to_date` - a hand-written field-by-field comparison that ignores artifacts, requirements, and profiles - is replaced by explicit classification:

- **Installation-impacting** - authority or pinned revision, kind, installation method, artifacts or their targets, destination, file mode, Cargo features or rustup channel, symlink layout. Requires replacing the component's files.
- **Graph-only** - `requires`, `profiles`. Changes *selection* without necessarily reinstalling an otherwise unchanged component.
- **Runtime-metadata-only** - aliases, `call-format`, `subcommands`, `initialization`. Updates `state.json` only.

Equivalently: a component needs reinstallation iff its contribution to the plan key changed.

### 11.2 Path and git update policy

`--path-update={off,interactive,all}` continues to govern whether `path`-authority components are rebuilt. A component held back by this policy still receives graph-only and runtime-metadata-only updates.

### 11.3 Update resolution

Update re-resolves the **persisted intent** against the new upstream channel:

- `profiles` are re-resolved, so a `minimal` installation gains components newly tagged `minimal`.
- An installation whose intent is roots-only gains new transitive *dependencies* of those roots, but not unrelated new profile members.
- An explicit root that no longer exists upstream **blocks** the update and preserves the existing installation. The schema has no component-rename declaration, so guessing is not available.

### 11.4 Channel migration

When an upstream channel declares `migrates_from: <old>` and `<old>` is installed, the installation is carried to the new channel: intent transfers verbatim, is resolved against the new channel, the new publication is installed, and the old one is removed after the new state record commits. A root missing in the new channel blocks the migration.

`var/<old-channel>` is **renamed** to `var/<new-channel>` as part of the migration. Both are pinned-version selectors, and migration is the one operation that retires one, so the data would otherwise be stranded under a key nothing can select. A network's store is unaffected.

### 11.5 Uninstall

Uninstall consults the receipt for the exact owned paths, tolerates hidden executables with no symlink, and uses the tombstoned unpublish sequence (§9.5). It removes the publication, the state record, and the derived symlinks. `var/<selector>` - keyed by the selector as given - is retained unless `--purge` is given.

Every `toolchains/<network>` link naming the channel is removed, before the commit point: they are derived, so a discarded operation costs nothing and the next install or update recomputes them. They are found by *scanning* the toolchains directory rather than by asking upstream which networks name this channel, because uninstall must work offline, and a network may have moved upstream since this machine last acted on it - in which case upstream would not name the link that is actually here. `default` is removed after the commit point instead, and only if it has been left dangling: it is the user's `midenup override` choice rather than a derived link, so nothing would recompute it.

### 11.6 Reclamation

`midenup gc` removes publication directories not referenced by any `state.json` record and not named by an active journal. It is idempotent and never removes a referenced or in-flight publication.

This is the **only** thing that reclaims a replaced publication (§3.1), so it is not optional housekeeping: without it, every update leaves its predecessor on disk. It is deliberately explicit and user-initiated, because "unreferenced" does not mean "unused" - a process that was already running when the publication was replaced is still executing out of it.

Because the superset only grows, `midenup install <channel> --profile <p>` is the documented way to shrink a channel back to a known set; it replaces intent and removes everything outside the new resolution.

### 11.7 Following a network

`midenup update <network>` reconciles **the pointer**, not the channel. It reads what `networks[<network>]` names upstream and compares it with the channel `toolchains/<network>` names here:

- **Unmoved** - fall through to the ordinary same-version update (§11.1, §11.3) of that channel. A network standing still does not mean its channel has.
- **Moved** - the installation is carried to the channel the network now names: intent transfers verbatim and is re-resolved against it, so it gains components that did not exist there before, and `toolchains/<network>` is repointed. `var/<network>` is **not** touched: it is keyed by the network, so it is already where the newly named channel will look for it. Nothing has to be ordered against the pointer move, and an interrupted run has nothing outstanding to finish beyond the move itself.

This is deliberately **not** `migrates_from` lineage (§11.4). That describes a relationship between two channels and is what someone tracking a pinned version follows. A user tracking `mainnet` asked for `mainnet`, and their data belongs to the network rather than to a version.

The comparison is **inequality, not "is newer"**. The pointer is authoritative in both directions: `update-manifest promote` refuses to author a backwards move without an explicit flag (§15), but once one is published, following it is what tracking a network means. A backwards move is announced, because a network naming an older channel than it did is worth saying out loud; nothing is carried, so there is nothing else to say about it.

Repointing the link is done by this command rather than left to the DERIVE step of the install it performs, because an update whose target is already installed can legitimately have nothing to install - and moving the pointer is the thing this command exists to do.

A link that disagrees with upstream is reported by `midenup show list` on the channel it names, with this command as the remedy - `0.14.0 (mainnet is now 0.15.0 upstream -- run 'midenup update mainnet')` - so a user learns that a network has moved.

---

## 12. Migration from manifest v1.0.1

### 12.1 Contract

The supported migration floor is **1.0.1**. A local document older than that is rejected without modification.

Migration carries forward the **selection only**: for each installed channel, its version and the names of its installed components. Everything else - installed filenames, aliases, call formats, artifact destinations, Cargo bookkeeping - is discarded and re-derived from the upstream manifest, because upstream is authoritative for all of it.

The migrated installation is expressed as ordinary native intent:

```json
{ "channel": "0.15.0", "intent": { "profiles": [], "roots": ["vm", "client", "core"] } }
```

This is why there is no `Frozen` intent variant. Roots-only intent already provides exactly the semantics a frozen migration needs - new dependencies of the roots are picked up, unrelated new profile members are not - using the same resolver as everything else.

**One-time root relaxation.** §11.3 blocks an update when an explicit root has disappeared upstream, because the user chose that root deliberately. Migrated roots were not chosen in the current schema's terms - they are inferred from a v1 record - so on the *first* install after migration, roots absent from the channel are **dropped with a warning** listing each one, and the intent is rewritten without them. Blocking here would strand every v1 user whose channel dropped a component. From the second operation onward the normal blocking rule applies.

### 12.2 Sequence

Migration is the **first** local operation, before recovery and before any upstream fetch. A missing or unreachable upstream manifest must not prevent it.

```
1. Read $MIDENUP_HOME/manifest.json. Absent -> nothing to migrate.
2. Parse the version header only.
     ≥ 2.0.0  -> not a v1 document; stop.
     < 1.0.1  -> error UnsupportedVersion; leave the file untouched.
     = 1.0.1  -> continue.
3. Extract (channel version, component names) per channel. Fallibly - no `expect`.
4. Serialize state.json to a unique temporary sibling; flush, fsync, close.
5. Re-open and parse the temporary file as a state document. Validate it.   ← GATE
6. Atomically rename it to state.json.                                       ← COMMIT
7. Delete manifest.json.
8. Re-open state.json for normal operation.
```

Every failure before step 6 leaves the original bytes untouched. After step 6, `state.json` is always valid. The post-commit re-open in step 8 is not a substitute for the step-5 gate: validating only after replacement is too late to preserve the original.

### 12.3 Physical state after migration

The migrated record describes an installation whose on-disk layout predates publications. It is marked `needs-reinstall`. Such a record is never executed against: the pre-publication tree is not described by any receipt, so `midenup` cannot know what it owns.

The next operation touching that channel resolves this:

- `midenup install` / `midenup update` - performs a full install from the migrated intent.
- `miden <cmd>` - triggers the same install automatically, exactly as it does for a toolchain that was never installed (§13.2 step 2), taking the lock for the duration.
- If that install cannot proceed - upstream unreachable, or a migrated root no longer exists in the channel - the command fails with `NeedsReinstall`, naming the exact recovery command. It never falls back to executing against the unmanaged tree.

`midenup show` displays the channel as needing reinstallation until this completes.
`var/<selector>` is preserved across the reinstall, since it lives outside publications.

If a migrated channel no longer exists in the upstream manifest at all, the record is retained and reported by `midenup show` as unavailable. It is not deleted - the user may still want `var/` and an explicit uninstall.

### 12.4 Downgrade

Migration is one-way. After it commits, `midenup` 0.3.x reading `$MIDENUP_HOME` finds no `manifest.json` and treats the installation as absent. This must be stated in the release notes and covered by a test asserting the message the older binary produces.

### 12.5 Migration to the network layout

An installation made before channels were named after networks has a single `toolchains/stable` link where it now needs one link per network (§3.4). Nothing else about it is wrong: **`state_version` stays at 1.0.0**, because local state records channel versions and never a release-train name, so the state document needs no change at all. Only what is derived on disk does.

```
1. toolchains/stable exists and toolchains/mainnet does not:
   a. var/<the version stable names> -> rename it to var/mainnet, unless
      toolchains/default names that version rather than the stable link.
   b. rename toolchains/stable to toolchains/mainnet.
2. toolchains/default names the old link                    -> repoint it at mainnet.
3. If (1) converted a home, drop the cached upstream manifest when this build cannot read it.
```

- Step 1a runs before 1b because the legacy link is what names the version the store sits under, and 1b is what retires that link. An existing `var/mainnet` means this is not a home to convert, and it is left as it is.
- The exception in 1a exists because the legacy link was written for whichever channel was the latest stable, regardless of what the user typed - so it is present for someone who pinned the newest release just as it is for someone who asked for `stable`, and moving that user's store would hide it. `toolchains/default` naming the version rather than the link is that user. A pin living in a project's `miden-toolchain.toml` is not visible from the home at all, so the message reporting the move says how to reverse it.
- Step 2 is not conditional on step 1: a run interrupted between them would otherwise leave `default` dangling forever.
- Step 3 runs only for a home actually being converted. Only an installation from that era can be holding a cache this build cannot read, and checking on every startup would mean parsing the cached manifest twice per command on the dispatch path for an answer that is almost always "nothing to do". A **v1** cache is not stale - it is run through the v1 converter - so it is kept, which is what preserves the offline capability of §13.1.
- It runs **without** the home lock, alongside §12.2, because it is on the `miden` dispatch path and that path must not wait on the lock. Every step is therefore idempotent and tolerates another process having done it first.
- A home that already has a `mainnet` link is not from that era, and is left alone: its link is the authority.

---

## 13. Runtime dispatch (`miden`)

### 13.1 No network

`miden <cmd>` reads `state.json` and the active publication. It does not fetch the upstream manifest. Previous versions of `midenup` fetched unconditionally on both entry points, so `miden vm run` performed a network round trip before doing anything.

The upstream manifest is fetched only when either: 

* an explicit `midenup` operation requires it
* the active toolchain is not installed and must be

On fetch failure with a cached `channel-manifest.json` present, the cache is used and staleness is reported.

### 13.2 Resolution order

```
1. Determine the active channel:  miden-toolchain.toml (searched upward from CWD)
                                -> toolchains/default
                                -> mainnet
2. If not installed -> fetch upstream, resolve, install, then continue.
3. Compute the active view (§8.5).
4. Resolve argv[1] against, in order:
     a. callable component names in the active view
     b. `command-name` values in the active view
     c. aliases in the active view
     d. callable names outside the active view  -> execute with a warning
5. Compose argv (§13.3) and exec.
```

When the active channel is a network name - from the toolchain file, from `default`, or by falling through to `mainnet` - it is resolved to a version through `toolchains/<network>` (§3.4), never by consulting upstream. A name with no such link is not guessed at: step 2 goes upstream, which install and update consult anyway.

### 13.3 Command composition

For a component with a non-empty `subcommands` map:

```
resolve(format) ++ resolve(subcommands[argv[1]]) ++ argv[2..]
```

`argv[1]` must name a declared subcommand; otherwise the error lists the valid ones. For an empty `subcommands` map:

```
resolve(format) ++ argv[1..]
```

For `executable` and `cargo-extension`, `format` is `call-format`, defaulting to
`["%installed-executable"]`.

### 13.4 Substitutions

Resolved against the active publication at dispatch time:

| Expression | Resolves to |
|---|---|
| `%installed-executable` | `<sysroot>/opt/<shim>` of the owning component, or `<sysroot>/bin/<installed-executable>` when it has no shim |
| `%lib` | `<sysroot>/lib` |
| `%lib(<name>)` | `<sysroot>/lib/<name>` |
| `%etc(<path>)` | `<sysroot>/etc/<path>` |
| `%var` | `$MIDENUP_HOME/var/<selector>` |
| `%var(<name>)` | `$MIDENUP_HOME/var/<selector>/<name>` |

`%lib` and `%etc` resolve into the **immutable publication**; `%var` resolves **outside** it (§3.2). A `%etc` or `%lib` path that does not exist in the active publication is an error naming the component that declared it - not a silently passed argument.

`<sysroot>` is the publication reached through `toolchains/<channel>`, resolved once per invocation.

:::note
If we resolved `%installed-executable` to `<sysroot>/bin/<installed-executable>`, then this would work against what §3.3 says `opt/` exists for: namely `clap` derives its program name from `argv[0]`, so executing `bin/miden-vm` makes its help read `miden-vm ...` rather than `miden vm ...`. 

This is why we resolve to the shim when the component has one.
:::

### 13.5 Environment

Spawned components receive `MIDENUP_HOME`, `MIDENUP_TOOLCHAIN`, `MIDEN_SYSROOT`, and a `PATH` prefixed with the active `opt/`.

---

## 14. Validation and diagnostics

### 14.1 When validation runs

| Point | Validator |
|---|---|
| loading an upstream manifest | **none** - parsing is permissive (see below) |
| after parsing local state | structural + referential |
| after every `update-manifest` mutation, before writing | structural, platform-neutral |
| `update-manifest check` | structural, platform-neutral, reports every error |
| during plan construction | structural for the selected channel, plus target-specific installability |

Structural validation is **platform-neutral**: a manifest is not invalid because the current machine cannot install one of its components. Target availability is checked only when building a plan.

**Loading an upstream manifest does not validate it.** Validation is an authoring gate and an install-time gate, never a precondition for reading the document.

This is deliberate: it is the same rule as §4.4's treatment of unknown component kinds: a defect scoped to one part of a manifest must not take down the rest. A defective channel becomes uninstallable; it does not make the tool unusable.

This was motivated by realizing that the manifest published at the time this spec was written, had dangling requirements in channel 0.13.3 - `midenc` required `base` and `std`, which were renamed to `core` and `protocol`. Validating at parse time would have made **every** `midenup` and `miden` invocation fail for **every** user, including those on well-formed channels, because one stale channel in the same file is broken. Obviously a validation feature that bricks the tool is not helpful.

### 14.2 Structural rules

The following are call disallowed and caught by validation:

* Duplicate channel names 
* Duplicate component names within a channel
* `requires` referencing an unknown component
* cycles in `requires` 
* invalid semver 
* non-positive or absent `date` 
* empty or unsafe `installed-executable` 
* artifact IDs violating §6.1 
* destination collisions within and across components
* alias colliding with a direct command name 
* alias declared by two components
* `hide: true` with no aliases 
* a `command` reachable by none of `format`, `subcommands`, or `aliases` 
* the §7.7 matrix
*  a `%target`-less URI in a target-specific artifact
* malformed `digest`
* an `archive` format this build cannot read
* an artifact whose resolved URI ends in a known archive extension without declaring `archive`, which would install the container as the artifact - unless the artifact id carries that extension too, which asks for exactly that
* `legacy-package` in a newly authored channel
* a network naming a channel that is not in the document
* a network with an empty name
* a network named like a channel, or after one of the synonyms in §5.1
* a manifest that declares no `mainnet` network.

The `command` rule is of particular note. A command is reachable through **any** of three routes, not just `format`. The shipped `node` component declares no `format` and no `aliases` - only `subcommands`, each carrying its full `docker compose …` invocation - so requiring `format` would reject a valid component.

A cycle check is only meaningful once every `requires` edge resolves. When a dangling requirement is found, cycle detection is skipped for that channel: the graph is incomplete, so any cycle it reports is an artifact of the missing nodes rather than a real one.

### 14.3 Error taxonomy

Each variant carries the file path, the offending identifier, and a remediation line.

| Variant | Condition |
|---|---|
| `UnsupportedVersion { found, floor }` | document older than the migration floor |
| `RequiresNewerMidenup { found, supported }` | major version above what this build supports |
| `WrongDocumentType { expected, found }` | state document where a manifest was expected, or vice versa |
| `UnsupportedComponentKind { component, tag }` | an `unsupported` component was selected |
| `TargetUnsupported { component, artifact, target }` | no artifact entry for the current target |
| `UnknownRoot { component, channel }` | intent names a component absent from the channel |
| `UnknownRequirement { component, requires }` | dangling `requires` edge |
| `RequirementCycle { path }` | cycle, with the full path |
| `DestinationCollision { path, owners }` | two artifacts target one path |
| `CargoUnitConflict { unit, components }` | two components claim one Cargo installation unit |
| `TransferFailed { uri, status }` | non-2xx or empty download with no fallback |
| `StagingVerificationFailed { path, reason }` | pre-publication structural check failed |
| `RecoveredOperation { op, channel }` | informational; a journal was rolled forward |
| `DivergentState { channel, detail }` | state and filesystem disagree with no journal |
| `NeedsReinstall { channel }` | a migrated installation has no publication yet |
| `RootRemovedUpstream { component, channel }` | update blocked; installation preserved |
| `LockTimeout { holder_pid }` | another operation held the lock too long |
| `DanglingNetwork { network, version }` | a network names a channel absent from the manifest |
| `InvalidNetworkName { name, reason }` | a network is named like a channel, or after a synonym |
| `MissingDefaultNetwork(network)` | the manifest declares no `mainnet` |

`DivergentState` and `NeedsReinstall` both name the exact recovery command.

### 14.4 Output and verbosity

**Streams.** Stdout carries a command's results; stderr carries its progress, warnings, and traces.

**Levels.** Four, ordered:

| Level | Flag | Emits |
|---|---|---|
| quiet | `-q`, `--quiet` | warnings and errors only |
| normal | *(default)* | one line per unit of work as it happens |
| verbose | `-v` | the above, and the output of spawned programs is no longer suppressed |
| debug | `-vv` | the above, and the individual actions taken: fetches, spawned commands, seeded files, symlink commits, link updates, record writes, deletions |

Warnings and interactive prompts survive `quiet`. `verbose` and `debug` are distinct axes -
spawned programs' output (the `--quiet` in the cargo argv, §9.3) versus `midenup`'s own actions -
ordered on one ladder because the former is the coarser.

**Where the level comes from.** The `-q`/`-v` flags, which are `midenup`'s alone; `-q` and `-v`
together are rejected. `miden` takes no flags of its own - everything after it belongs to the
component being dispatched to - so an install triggered by `miden` (§13) always runs at the
default level.

**What an install says.** Before the manifest fetch, that a sync is starting - the whole manifest
is synced, so no channel is named. Then the manifest's date, the channel being installed - a
network and the version it resolves to, or a version requested directly, named once - and the work
ahead by kind (`3 steps: 2 downloads, 1 source build`). Then one numbered line per component and
kind of work
(`[2/3] building component 'x' from source`); a fallback taken mid-run (§9.3) is announced
un-numbered, since the total was fixed before the run. After the commit point (§9.5), the channel
that was installed.

**Progress granularity.** The counter is a position in a list, never a whole-toolchain percentage:
a source build and a package download are not comparable units. On a terminal a transient redrawn
line shows bytes within a single transfer and elapsed time within a single build or extraction; in
a file or CI log the announcement lines are the whole report.

---

## 15. `update-manifest`

The authoring tool shares the schema types and validators; it does not reimplement them.

- Every mutation is validated before the file is written. A failed mutation leaves the original byte-for-byte intact, via the same validate-temp-then-rename writer used for `state.json`.
- `update-component --kind` applies its JSON merge patch in the correct direction. It currently applies it in reverse, so partial updates keep the old value.
- `check` runs the complete structural validator including cycle detection and dependency ordering. It currently accepts cyclic manifests.
- Removing a component still required by a remaining component is rejected, listing the dependents.
- `add-component` accepts `--profile`.
- Authoring a `legacy-package` is rejected (§7.4).
- The tool never writes `manifest_version` by hand; it is emitted by the schema type.

### 15.1 `promote`

```
update-manifest promote <NETWORK> <CHANNEL> [--allow-downgrade]
```

`promote` is the only way a network moves (§5.1). It deploys nothing; it records which toolchain `midenup install <NETWORK>` resolves to from now on. Creating the network if it does not exist is the same operation as moving one, so there is no separate "add network" command.

It refuses, before writing:

- a `<NETWORK>` that parses as a version, which would be ambiguous with a toolchain of the same name;
- a `<NETWORK>` that is one of the §5.1 synonyms, since it is rewritten before any lookup and so could never be reached;
- a `<CHANNEL>` that is not a toolchain in the manifest;
- a `<CHANNEL>` that is not **installable**: the channel is resolved for the `complete` profile first, because a network must never name a toolchain every user tracking it would discover to be broken at install time;
- a move to an older channel than the network names now, unless `--allow-downgrade` is passed - it hands every user tracking that network a toolchain older than the one their data was written by.

A promotion that changes nothing says so and writes nothing, and what happened is reported only *after* the write commits: the printed line is what a reviewer checks a promotion against, so it must never describe a change that failed.

`clone-toolchain` deliberately does not carry networks across. A cloned toolchain is a draft; it reaches users only when a network is promoted to it.

---

## 16. Non-goals

| Excluded | Rationale |
|---|---|
| Per-artifact byte/digest verification | Schema slot reserved (§6.4); enabling it later is a behavior change, not a schema break. |
| Per-project physical installations | The superset model handles the multi-project case at a fraction of the disk and complexity cost. |
| Rollback to a previous publication | Old publications are removed after commit. Recovery restores consistency, not history. |
| Archive/compressed artifacts | Deferred until a motivating example exists |
| Component rename declarations | Update blocks on a missing root rather than guessing. Add if the need becomes real. |
| Shared `MIDENUP_HOME` over a network filesystem | `flock` semantics are not dependable there. |
| Sudden-power-loss durability | Process-crash consistency only (§9.10). |
| Executing `initialization` | Recorded and preserved, never run (§7.5). |

---

## 17. Testing strategy

### Unit - pure, no filesystem

* version dispatch across all four compatibility cases;
* unknown-field and unknown-kind round-tripping;
* the resolver behavior over chains, diamonds, cycles, missing roots, and each profile;
* the §7.7 matrix;
* destination and mode computation per kind;
* artifact-ID validation;
* plan-key stability under input reordering and sensitivity to every material input;
* command composition with and without subcommands;
* substitution resolution including `%var` pointing outside the publication.

### Integration - real filesystem

The tests place `MIDENUP_HOME` in a tempdir; use local `file://` manifests and
artifacts. These assert against *reopened* `state.json` and *actual files*, not in-memory counts:

* empty / minimal / complete direct installs;
* two projects with different subsets: activation is additive, neither removes the other's components, each gets its own active view;
* direct install shrinks; the next activation re-adds;
* multi-file packages and multi-file command assets install **every** file;
* 404, 500, and empty-body downloads publish nothing; a declared Cargo fallback recovers;
* unsupported target fails plan construction without touching the filesystem;
* two Cargo-backed components install, update, and uninstall independently without disturbing each other's binaries;
* `var/<selector>` survives update, republication, and pointer moves; two networks on one channel keep separate stores; is removed only under `--purge`, and then only for the selector named;
* v1.0.1 migration with an unreachable upstream still commits `state.json`;
* v1.0.0 is rejected and the file is byte-for-byte unchanged;
* an `unsupported` component parses, shows, and is installable-around, but errors when selected;
* installing a channel that several networks name writes a link for each of them, and uninstalling it removes all of them while leaving other channels' links resolving;
* a synonym reaches the same channel as the network it names, and produces the network's link;
* `update <network>` follows a promotion the user does not have installed, and follows a rollback both to a channel they do have and to one they do not, leaving `var/<network>` in place in each case;
* `update <network>` leaves other networks alone, and still updates its own channel when the pointer has not moved;
* an unknown network name is answered with the networks the manifest declares.

### End to end

A version install (`0.15.0`) -> two project toolchains activating additively -> an in-place update of that same channel -> a bare `midenup update` following a `migrates_from` successor -> `gc` -> uninstall, asserting physical layout at every step.

### Fault injection

A test hook aborts the process at each labeled point in §9.5 (post-prepare,
post-stage, post-verify, post-commit, post-record, post-derive). For each, a subsequent startup must produce exactly one consistent state, and the assertion is against the reopened state document plus the filesystem.

### Concurrency 

Two processes installing different components of the same channel concurrently produce a superset containing both, with no lost update and no corrupt state document.
