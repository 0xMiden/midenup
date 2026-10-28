# Acceptance scenarios

Checks that the components of a toolchain work together. Each team owns the scenarios for its
components. A change to a toolchain is rejected while any of its components has no scenario, so a
component is carried into a new toolchain line only once its scenarios cover it.

## Adding a scenario

Add an entry to `scenarios.toml`:

```toml
[[scenarios]]
title = "Can <do something> with <component>"
toolchain = "^0.17.0"          # semver range; must have an upper bound
requires = ["<component>"]     # components exercised; it runs when a change touches one
kind = "script"                # or "lit", with `suite = "<directory next to scenarios.toml>"`
needs = ["docker"]             # optional; skipped where Docker is missing
script = '''
set -euo pipefail
# `miden ...` commands that exercise the component
'''
```

- **`script`:** passes when the script exits successfully. It runs with `bash`.
- **`lit`:** a directory holding `lit.suite.toml` and its tests, run with `litcheck`. Use it when
  checking output matters. See `debug-midenc-produced/`.

`scenarios.toml` has a working example of each kind.

To cover a new toolchain line, widen `toolchain`, e.g. `>=0.17.0, <0.19.0`.

## What a scenario can rely on

- A scratch midenup home with the toolchain installed with the `complete` profile, and that
  toolchain selected.
- `miden` and `midenup` on `PATH`.
- An empty working directory of its own.
- The versions in `MIDEN_TOOLCHAIN_VERSION` and `MIDEN_COMPONENT_<NAME>_VERSION`, e.g.
  `MIDEN_COMPONENT_REMOTE_PROVER_VERSION`. Lit suites also get them as features, e.g.
  `toolchain=0.17.0` and `vm=0.17.2`, for `REQUIRES` and `UNSUPPORTED`.
- Network access. Start any service the scenario needs, e.g. with `miden node up`, and stop it
  before exiting.

## Running locally

```
cargo build -p midenup -p manifest-utils
./target/debug/manifest-utils acceptance --manifest-path manifest/v3/manifest.json --midenup target/debug/midenup
```

Without `--against`, every network's toolchain is verified. With
`--against file:///path/to/previous-manifest.json`, only the toolchains that changed are verified.
Lit scenarios need `litcheck`: `cargo install litcheck`.
