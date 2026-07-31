# Usage

## Installing a toolchain

In order to get started with `midenup`, a toolchain should be installed. A toolchain is simply a collection of miden programs (e.g. the vm, the client, the compiler, etc).
Toolchains are installed via "Channels", which are a specific release of a toolchain with instructions on how to obtain it.

Most users will want to install the latest stable toolchain from the official midenup channel, like so:

```shell title=">_ Terminal"
midenup install stable
```

This command will install the stable toolchain using the [official midenup channel](https://0xmiden.github.io/midenup/channel-manifest.json).
However, midenup also supports "custom channels", where one can create a customized version of a toolchain. In order to use a custom channel, `midenup` must called with the`MIDENUP_MANIFEST_URI` environment variable, like so:

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

### Installing the toolchain for a network

A channel can declare the network its toolchain targets, and that network's name selects it:

```shell title=">_ Terminal"
midenup install devnet
```

This is usually what you want when you are working against a specific network, including one that is still being developed against: it installs the whole toolchain that network is running, and it keeps pointing there as that toolchain is updated. The available networks are `devnet`, `testnet` and `mainnet`, though a channel only answers to a network if the manifest says it targets one.

Selecting a network never affects `stable`. Installing `devnet` does not change what `midenup install stable` gives you, and `stable` moves only when the Miden project promotes a channel.

:::warning
A channel on `devnet` is typically built from pre-release components, so it carries no stability guarantees. Prefer `stable` unless you need to work against that network specifically.
:::

To list all the currently installed toolchains in the system, run:

```shell title=">_ Terminal"
midenup show list
```

Toolchains are annotated with the alias they hold and the network they target, for example:

```
Installed toolchains:
0.15.0 (stable) [testnet]
0.16.0 [devnet]
```

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

`midenup`, and by extension `miden`, have a notion of an 'active toolchain'. This value represents the toolchain that is going to be used in the current working directory. Unless configured otherwise, `midenup` will always default to using the latest stable toolchain.

To check what the active toolchain is, the following command can be run:

```shell title=">_ Terminal"
midenup show active-toolchain
```

There are currently 2 main mechanisms to alter the active toolchain: setting a system wide default or setting a directory local default. Each method has an associated priority according to the following chart (from highest to lowest):

1. Directory local toolchains.
2. System default.
3. Fallback: If none of the above are detected, `midenup` will fallback to the `stable` toolchain as default.

### System wide active toolchain

The `midenup override <toolchain>` command will set the passed toolchain as the system's default. For instance, the following command will set toolchain version 0.15.0 as the system's default:

```shell title=">_ Terminal"
midenup override 0.15.0
```

To check this, use `midenup show active-toolchain`.

### Local toolchains

The `midenup set <toolchain>` command has the ability to set a toolchain to be the default in specific directory. For example, to set toolchain version 0.17.0 as the default run:

```shell title=">_ Terminal"
midenup set 0.17.0
```

This will create a `miden-toolchain.toml` file in the present working directory (similar to`rustup`'s `rust-toolchain.toml` file).
With this file now in place, toolchain version 0.17.0 will be the active toolchain in that directory and in all of if sub-directories.

## Updating a toolchain

Toolchains can periodically require updates, which can be in one of the following forms:

### Updating a specific toolchain

When updating a specific toolchain, only updates which are known to work with that version of the toolchain will be installed/updated. These can occur when a component gets a new minor release, or it gets rolled back. The `midenup update <toolchain>` command will trigger these types of updates can be used.

If no `<toolchain>` is passed, like so:

```shell title=">_ Terminal"
midenup update
```

then `midenup` will look for updates on every installed toolchain.

### Updating stable

If the latest installed "stable" toolchain in the system is older than the latest available version present upstream, the system can be brought up to date with the following command:

```shell title=">_ Terminal"
midenup update stable
```

## Uninstalling a toolchain

A toolchain can be uninstalled via the `midenup uninstall <TOOLCHAIN>` command.
For example, to uninstall toolchain version `0.16.0`, run:

```shell title=">_ Terminal"
midenup uninstall 0.16.0
```

This keeps the toolchain's mutable data — the Miden client's database, for instance — and tells you
where it left it. Removing a toolchain is not a request to delete your data. To remove that too:

```shell title=">_ Terminal"
midenup uninstall 0.16.0 --purge
```

## Reclaiming disk space

Installing or updating a toolchain publishes a fresh copy of it and leaves the previous copy in
place, because another shell may still be running a component out of it. Once you are done with
those, reclaim them:

```shell title=">_ Terminal"
midenup gc
```

This only ever removes installations that nothing refers to any more. It never touches an installed
toolchain, and it is safe to run at any time.

## Upgrading from an older midenup

The first time a newer `midenup` runs, it converts the record an older one left in `$MIDENUP_HOME`
into its own format. It carries over which channels you had installed and which components you had
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
