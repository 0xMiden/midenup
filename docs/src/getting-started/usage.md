# Usage

## Installing a toolchain

In order to get started with `midenup`, a toolchain should be installed. A toolchain is simply a collection of miden programs (e.g. the vm, the client, the compiler, etc).
Toolchains are installed via "Channels", which are a specific release of a toolchain with instructions on how to obtain it.

Most users will want the toolchain that is deployed to a particular network, and can name the network rather than a version:

| Network   | Also accepted as | What it is                        |
|-----------|------------------|-----------------------------------|
| `mainnet` | `stable`         | The toolchain deployed to mainnet |
| `testnet` | `beta`           | The toolchain deployed to testnet |
| `devnet`  | `nightly`        | The toolchain deployed to devnet  |

```shell title=">_ Terminal"
midenup install mainnet
```

This command will install the toolchain mainnet currently runs, using the [official midenup channel](https://0xmiden.github.io/midenup/channel-manifest.json). Which toolchain that is comes from the manifest, so when a network is promoted to a newer toolchain, `midenup update mainnet` follows it.

Omit the channel to install the active toolchain selected by `MIDENUP_TOOLCHAIN`, the project's
`miden-toolchain.toml`, the system default, or finally `mainnet`. When a project file selects the
toolchain, its profile, components and patches apply. `--profile` overrides that profile, and
`--component` adds to the project's components. As with an explicit-channel install, the resulting
selection replaces the previously recorded selection for that toolchain.

To use an alternative upstream manifest, set `MIDENUP_MANIFEST_URI`:

```shell title=">_ Terminal"
MIDENUP_MANIFEST_URI=file://<path/to/custom/manifest.json> midenup install <toolchain>
```

:::warning
This functionality is still in early stages of development. Currently, this requires writing the channel manifest manually.
:::

### Specific releases

If required, a specific toolchain version can also be installed with the `midenup install <toolchain-version>` syntax, like so:

```shell title=">_ Terminal"
midenup install 0.15.0
```

To list all the currently installed toolchains in the system, run:

```shell title=">_ Terminal"
midenup show list
```

Each toolchain is listed with the networks that run it, and a network that has since moved to a different toolchain is listed with the command that catches you up: `0.14.0 (mainnet is now 0.15.0 upstream -- run 'midenup update mainnet')`.

## Using a toolchain

The `miden help toolchain` can be run to display a quick summary of what the currently active toolchain offers.

It should display a message similar to the following:

```shell title=">_ Terminal"
The Miden toolchain porcelain

Usage: miden <ALIAS|COMPONENT>

Available aliases:
  account
  build
  call
  deploy
  faucet
  new
  send
  simulate

Available components:
  vm
  client (requires init: miden client init )
  midenc
  cargo-miden
```

This displays the following information:

- A list of available aliases: These are a shortform versions of commonly used miden commands. The following [table](https://0xmiden.github.io/midenup/channel-manifest.json) showcases said mappings.
- A list of available components: Each of these represents a different miden executable. If the component requires initialization, like it is the case with the client, the corresponding initialization command will be displayed.

## Activating a toolchain

`midenup`, and by extension `miden`, have a notion of an 'active toolchain'. This value represents the toolchain that is going to be used in the current working directory. Unless configured otherwise, `midenup` will always default to the toolchain deployed to `mainnet`.

To check what the active toolchain is, the following command can be run:

```shell title=">_ Terminal"
midenup show active-toolchain
```

The active toolchain is selected in this order (highest priority first):

1. Explicit `+<toolchain>` selector.
2. `MIDENUP_TOOLCHAIN` environment override.
3. Directory local toolchain.
4. System default.
5. Fallback: If none of the above are detected, `midenup` will fallback to the `mainnet` toolchain as default.

### System wide active toolchain

The `midenup override <toolchain>` command will set the passed toolchain as the system's default. For instance, the following command will set toolchain version 0.15.0 as the system's default:

```shell title=">_ Terminal"
midenup override 0.15.0
```

A network name works here too: `midenup override mainnet` follows mainnet as it moves, whereas naming a version pins the default to that release.

To check this, use `midenup show active-toolchain`.

### Local toolchains

The `midenup set <toolchain>` command has the ability to set a toolchain to be the default in specific directory. For example, to set toolchain version 0.17.0 as the default run:

```shell title=">_ Terminal"
midenup set 0.17.0
```

This will create a `miden-toolchain.toml` file in the present working directory (similar to`rustup`'s `rust-toolchain.toml` file).
With this file now in place, toolchain version 0.17.0 will be the active toolchain in that directory and in all of if sub-directories.

### Named toolchains

Add `name = "project-dev"` to `[toolchain]` to derive an independent installation from an upstream
version or network. The `channel` remains its upstream source; `name` identifies the local
installation. Names use lowercase ASCII letters, digits, `_`, `-` and interior dots, with no leading dot or hyphen.
A named installation is complete and uses its own publication and mutable data.

Run `midenup install` in the declaring project to install it. Later activation reuses the recorded
source and patches and adds missing requested components. If another project uses the same name
with a different source or patches, activation fails with a conflict. Run `midenup install` from the
project whose definition you want to keep. Switching between network identities, or between a
network source and a pinned version, requires a new name so existing data keeps its meaning.

Select an existing installation elsewhere with any of these forms:

```shell title=">_ Terminal"
miden +custom:project-dev vm --help
MIDENUP_TOOLCHAIN=custom:project-dev miden vm --help
midenup override custom:project-dev
midenup set custom:project-dev
```

A selecting toolchain file uses only `channel = "custom:project-dev"`; `name`, `profile`,
`components`, and `[patches]` belong in the declaring file. Selection reuses the saved definition,
including patches. A declaration with `name` must derive from an
upstream version or network; named installations cannot derive from one another.

### Patching components

Patches require a named toolchain. A `[patches]` table in `miden-toolchain.toml` builds individual components from a git repository, a local path or another registry version instead of the channel's published release:

```toml title="miden-toolchain.toml"
[toolchain]
name = "project-dev"
channel = "0.17.0"
profile = "empty"
components = ["vm"]

[patches.vm]
crate_name = "miden-vm"
features = ["executable"]
version = { kind = "git", repository_url = "https://github.com/0xMiden/miden-vm.git", revision = "8160d8a22bc5342b01946ae00a6dc4c1f224fc35" }
```

`version` accepts `kind = "git"` (with `tag`, `branch` or `revision`), `kind = "path"` (relative to the `miden-toolchain.toml` file) and `kind = "registry"`. A patched executable is always built with `cargo install`; `crate_name` is required when the channel only publishes it pre-built, and `features` replaces the cargo features it is built with (`miden-vm` needs `executable`). A package that the channel extracts from a Rust crate, as older channels do, is extracted from the patched crate instead, with `crate_name` and `features` overriding the channel's. Components that are only distributed as pre-built files (pre-built packages, assets and commands) cannot be patched. A patched component must be part of the project's toolchain, through `profile`, `components` or a dependency of one of them; patching anything else is an error.

Local path builds keep their Cargo outputs in `$MIDENUP_HOME/cache/cargo`, partitioned by the canonical source path. This prevents different checkouts of the same crate from sharing stale build outputs, including when Cargo is configured with a shared target or build directory. Rebuilding the same source reuses its cache; registry and Git downloads still use your normal Cargo cache. The build cache is disposable. `midenup gc` removes caches no installed toolchain uses; caches shared by multiple variants remain until none uses that source.

The shared `0.17.0` installation and other named variants are unaffected. Selecting the named
installation outside this project preserves its patches. Updating it also preserves its source and
patches, and path patches follow the existing `--path-update` policy.

## Updating a toolchain

Toolchains can periodically require updates, which can be in one of the following forms:

### Updating a specific toolchain

When updating a specific toolchain, only updates which are known to work with that version of the toolchain will be installed/updated. These can occur when a component gets a new minor release, or it gets rolled back. The `midenup update <toolchain>` command will trigger these types of updates can be used.

If no `<toolchain>` is passed, like so:

```shell title=">_ Terminal"
midenup update
```

then `midenup` will look for updates on every installed toolchain, then ensure the active toolchain
and its requested components are installed. This also installs the active toolchain on a fresh
machine, which requires access to the upstream manifest or a cached copy.

The update pass reconciles canonical installations by version and named installations by their
saved source. When the active network's installed toolchain already satisfies the project's request, a bare update keeps that local
network selection. To follow an upstream promotion explicitly, name the network as below.

### Updating a named toolchain

```shell title=">_ Terminal"
midenup update custom:project-dev
```

This re-resolves the recorded components against the saved upstream source, then applies the saved
patches. A version source stays pinned. A network source follows upstream promotions independently
of `midenup update mainnet` and does not move the shared network link. Local path patches are rebuilt
according to `--path-update={off,interactive,all}`.

Mutable data stays under `var/custom/project-dev/pinned` for a version source or
`var/custom/project-dev/networks/<network>` for a network source. It survives updates independently
of the canonical channel and other named installations. Reusing a removed name for another source
therefore does not reuse a different network's data.

### Updating a network

When a network is promoted to a different toolchain, an installation that tracks that network is brought to it with:

```shell title=">_ Terminal"
midenup update mainnet
```

This follows the network's pointer wherever it has moved. Your component selection transfers verbatim and is re-resolved against the toolchain now being tracked. Network-associated data stays put: it is stored under the network you selected (`var/mainnet`), not under the toolchain version, so it follows the network without anything having to move. A network that has been moved *back* to an older toolchain is followed too, with a heads-up.

If the network's pointer has not moved, this still picks up any changes to the components of the toolchain it names.

:::note
A network and a pinned version are separate selections, so they get separate client databases. Work
under `mainnet` is stored in `var/mainnet` and work under a pinned `0.15.0` in `var/0.15.0`, even
during the periods when `mainnet` names 0.15.0 — the accounts you created while tracking the network
are not the ones a project pinned to the version sees, and vice versa. Pick one and stay with it for
a given project, and you will always be looking at the same data.
:::

## Uninstalling a toolchain

A toolchain can be uninstalled via the `midenup uninstall <TOOLCHAIN>` command.
For example, to uninstall toolchain version `0.16.0`, run:

```shell title=">_ Terminal"
midenup uninstall 0.16.0
```

This keeps the toolchain's mutable data and tells you where it left it. Removing a toolchain is not a request to delete your data. To remove that too:

```shell title=">_ Terminal"
midenup uninstall 0.16.0 --purge
```

The same commands accept named selectors: `midenup uninstall custom:project-dev` removes only that
installation and keeps `var/custom/project-dev`; add `--purge` to remove its data too.

## Reclaiming disk space

Installing or updating a toolchain publishes a fresh copy of it and leaves the previous copy in
place, because another shell may still be running a component out of it. Once you are done with
those, reclaim them:

```shell title=">_ Terminal"
midenup gc
```

This removes unreferenced publications and Cargo path-build caches no installed toolchain uses.
Uninstalling a toolchain, with or without `--purge`, leaves its build cache for the next `gc`.
Shared caches remain while another installation uses their source. Garbage collection preserves
installed toolchains, in-flight operations, source trees, and runtime data.

## Upgrading from an older midenup

Local state version 2 records named installation identities. Version 1 state remains readable as
canonical installations, and the next state write uses version 2. Older binaries reject version 2.
Legacy patches recorded on canonical installations remain readable; an ordinary canonical install
or update restores the upstream components.

For installations predating `state.json`, the first run converts the older record in
`$MIDENUP_HOME` into the current format. It carries over which channels you had installed and which components you had
in each — everything else is re-derived from the published manifest, which is authoritative for it.
Your toolchains are reinstalled the next time you use them, so that `midenup` knows exactly which
files it owns. `var/` is untouched throughout.

`midenup show list` marks a toolchain in that state as needing reinstallation until it happens.

:::warning
The conversion is one-way. Afterwards an older `midenup` will not see your installation and will
report it as absent. If you need to go back, reinstall your toolchains with the older version.
:::

## Working offline

Running a component never needs the network. `miden <cmd>` answers from what is recorded locally and
from the installed toolchain, so an unreachable manifest cannot stop you working.

The upstream manifest is fetched only when something actually needs to know what exists upstream —
installing, updating, or `midenup list`. Each successful fetch is cached, and if a later fetch
fails, `midenup` proceeds against that cached copy and tells you it is doing so.
