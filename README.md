# midenup

The Miden toolchain installer.

The `midenup` executable facilitates two primary tasks:

1. Toolchain management, i.e. bootstrapping the environment, and installing, updating, and configuring installed toolchain components.
2. Using toolchains for working on Miden projects

> [!NOTE]
> The notion of a _toolchain_ here refers to the various components of the Miden
> project which are required in order to develop, test, run, and interact with
> Miden programs, both locally and on the network.
>
> Currently, the set of such components consists of:
>
> * Miden VM
> * The [Miden compiler](https://github.com/0xMiden/compiler), `midenc`, and its Miden Assembly and Rust-specific tooling, i.e. `miden-format`, `cargo-miden`
> * The [Miden debugger](https://github.com/0xMiden/miden-debug)
> * The Miden client
> * The Miden core library package
> * The Miden protocol library packages (transaction kernel, protocol and standards library, and various standard components).
> * Aliases for running a local instance of the [Miden node](https://github.com/0xMiden/node)
>
> Run `midenup component list` to see a list of all available components for the active toolchain.

## Prerequisites

- [Rust](https://rustup.rs), latest stable.
- [Docker](https://docs.docker.com/get-docker/) with Docker Compose v2.34.0 or later, only if you use the
  `node` component (`miden node`).

## Usage

To get started, you must first install `midenup`, and then initialize its
environment:

We provide pre-built executables with build attestations for some platforms (Linux and macOS). You can use our installer script to install
`midenup` easily, while also verifying against those attestations (if you have
the `gh` executable installed):

```
curl -L --proto '=https' --tlsv1.2 -sSf 'https://github.com/0xMiden/midenup/releases/latest/download/installer.sh | bash
```

If prebuilt artifacts are not available for your machine, the script will install from crates.io using Cargo. You can disable this fallback by running
the script with arguments, like so:

```
curl -L --proto '=https' --tlsv1.2 -sSf -O 'https://github.com/0xMiden/midenup/releases/latest/download/installer.sh
./installer.sh --no-cargo-fallback
```

You can also install a specific `midenup` release using the `--version` flag. For more usage information, download the script and pass `--help`.

The installer also runs `midenup init` for you, so after the script exits, `midenup` is ready to use.

### Install via Cargo

Alternatively, you can install yourself from crates.io like so:

```
cargo install --locked midenup && midenup init
```

NOTE: This presumes that `$CARGO_HOME/bin` is in your shell's `PATH` environment variable.

The `midenup init` command initializes the `$MIDENUP_HOME` directory, and creates a `miden` symlink in `$CARGO_HOME/bin` (default `~/.cargo/bin`) pointing to the `midenup` executable. Since Rust users typically already have `$CARGO_HOME/bin` in their PATH, the `miden` command should be available immediately.

> [!WARNING]
> If `miden` is not found after running `midenup init`, ensure `$CARGO_HOME/bin`
> is in your PATH. On macOS with zsh, add `export PATH="$HOME/.cargo/bin:$PATH"`
> to `~/.zprofile` and create that file first if it does not exist.

You are now ready to install your first toolchain!

### Use in GitHub Actions

The repository doubles as a composite action that installs `midenup` and a toolchain in one step:

```yaml
- uses: 0xMiden/midenup@v1.1.0  # We use immutable releases, and only exact tags
  with:
    toolchain: testnet          # network or version, default `mainnet`
    profile: minimal            # `minimal` or `complete`, default `minimal`
    components: client,vm       # optional extra components
```

The requested toolchain becomes the system default, so `miden` uses it in later steps. Set `midenup-version` to pin a specific `midenup` release instead of the latest.

> [!IMPORTANT]
> If your repository contains a `miden-toolchain.toml`, it will take precedence
> over the system default selected by the action. If you intend to run commands
> with a different toolchain than your repository pins, then you must specify
> that when invoking those commands, e.g. `miden +0.16.0 build` or by setting
> the `MIDENUP_TOOLCHAIN` environment variable, e.g. `MIDENUP_TOOLCHAIN=0.16.0`.

### Installing a toolchain

After initializing `midenup`, the first thing you will want to do is actually
install a toolchain so you can work with the various Miden components. There
are two ways to do this:

1. Installing a release network, e.g. `mainnet`, which installs the toolchain currently deployed to
that network. When a network is promoted to a newer toolchain, `midenup update mainnet` follows it,
carrying your component selection and any toolchain-managed data across.
2. Installing a specific toolchain version, e.g. `0.15.0`, which pins you to that toolchain regardless of what the networks do.

In both cases, you simply run `midenup install <TOOLCHAIN>`.

The networks are:

| Network   | Also accepted as | What it is                        |
|-----------|------------------|-----------------------------------|
| `mainnet` | `stable`         | The toolchain deployed to mainnet |
| `testnet` | `beta`           | The toolchain deployed to testnet |
| `devnet`  | `nightly`        | The toolchain deployed to devnet  |

When getting started, it is recommended that you install the `mainnet` toolchain, like so:
```
midenup install mainnet
```

`midenup` also assumes `mainnet` to be the default toolchain if not overridden in
the current working directory or by the user's default toolchain (for more
information on how to configure the active toolchain, see [Configuring the active
toolchain](#configuring-the-active-toolchain)).

### Updating a toolchain

To update a given toolchain, you can use the `midenup update <TOOLCHAIN>`
command. This command's behavior differs slightly depending on how it is called.

#### Updating a network

To bring a network up to the toolchain it now runs, run:
```
midenup update mainnet
```

This follows the network's pointer wherever it has moved, carrying your component selection and toolchain-owned data across. If the pointer has not moved, it still picks up any changes to the components of the toolchain it names.

#### Updating a specific toolchain

When updating a versioned toolchain, only updates which are known
to work with that version of the toolchain will be installed/updated.

For example, if you'd like to update toolchain version `0.16.0`, run:
```
midenup update 0.16.0
```


### Using a toolchain

Interacting with Miden toolchain components is done via the `miden` command,
which handles delegating commands to the underlying components using
subprocesses. For example, `miden new` calls out to `cargo miden new` to create
a new Rust-based Miden project.

By default, the `miden` command uses the currently active toolchain, which you
can view using `midenup show active-toolchain`. To see how to configure the
active toolchain, see [Configuring the active toolchain](#configuring-the-active-toolchain) section.

#### Aliases

To facilitate development, the `miden` command is also aware of a number of
aliases. These aliases exist to facilitate the execution of common miden task.

Here's a table with all the currently available aliases:

| Alias                  | Action                                  | Corresponds to                        |
|------------------------|-----------------------------------------|---------------------------------------|
| miden new              | Create new project                      | cargo miden new                       |
| miden build            | Build project                           | midenc                                |
| miden format           | Format Miden Assembly                   | miden-format                          |
| miden registry         | Run a local package registry            | miden-registry                        |
| miden mint             | Fund account via faucet                 | miden-faucet-client mint              |
| miden account          | View and manage accounts                | miden-client account                  |
| miden new-wallet       | Create a wallet                         | miden-client new-wallet               |
| miden sync             | Sync client state with the network      | miden-client sync                     |
| miden consumable-notes | List notes that can be consumed         | miden-client notes --list consumable  |
| miden consume-notes    | Consume notes                           | miden-client consume-notes            |
| miden transfer         | Create a pay-to-id transaction          | miden-client transfer                 |
| miden call             | Call a procedure on an account          | miden-client call                     |
| miden exec             | Execute a program against an account    | miden-client exec                     |


### Uninstalling a toolchain

You can easily uninstall a Miden toolchain with the `midenup uninstall <TOOLCHAIN>` command.
For example, to uninstall toolchain version `0.16.0`, run:
```
midenup uninstall 0.16.0
```

This keeps the toolchain's mutable data and tells you where it left it. To remove that too, pass `--purge`:
```
midenup uninstall 0.16.0 --purge
```

> [!WARNING]
> It is **strongly discouraged** to delete the toolchain directories manually,
> since this will most likely generate an invalid environment and `midenup` will
> probably *not* work as intended.

### Reclaiming disk space

Installing or updating a toolchain publishes a fresh copy of it and leaves the previous copy in
place, because another shell may still be running a component out of it. Once you are done with
those, reclaim them with:
```
midenup gc
```

This removes unreferenced publications and Cargo path-build caches no installed toolchain uses.
Caches shared by several variants remain while any variant uses their source. Installed toolchains,
in-flight operations, source trees, and runtime data are preserved.

### Upgrading from an older `midenup`

Local state version 2 supports named installations and reads version 1 records as canonical
installations. Subsequent writes use version 2, which older binaries reject.

For installations predating `state.json`, the first run converts the older record in
`$MIDENUP_HOME` into the current format. It carries over which channels you had installed and which
components you had in each, and nothing else - everything else is re-derived from the published
manifest, which is authoritative for it. Your toolchains are reinstalled the next time you use
them, so that `midenup` knows exactly which files it owns; `var/` is untouched throughout.

**This is one-way.** After the conversion, an older `midenup` will not see your installation and
will report it as absent. If you need to go back, reinstall your toolchains with the older version.

### Uninstalling `midenup`

You can easily uninstall `midenup` itself by deleting the `$MIDENUP_HOME` directory.
The location of the `$MIDENUP_HOME` directory can be obtained by running:
```
midenup show home
```

### Configuring the active toolchain

`miden` and `midenup` select the active toolchain in this order:

1. An explicit `+<toolchain>` selector.
2. The `MIDENUP_TOOLCHAIN` environment override.
3. A `miden-toolchain.toml` file, searched for from the working directory upward.
4. The system default.
5. `mainnet`.

#### Setting a project specific toolchain

To configure a toolchain to be active in the present working directory, you can use the `midenup set <TOOLCHAIN>` command.
For example, to set `0.16.0` run:
```
midenup set 0.16.0
```

This procedure will generate a `miden-toolchain.toml` file in the directory where `midenup set` was invoked:

```toml
[toolchain]
channel = "0.16.0"
components = []
```

The `channel` entry may also name a network, e.g. `channel = "mainnet"`, in which case the project
follows that network as it moves. A file written before the networks were named, saying `channel =
"stable"`, still works and means `mainnet`: `stable`, `beta` and `nightly` are accepted as synonyms
for `mainnet`, `testnet` and `devnet`.

Now, whenever `miden` is called in this directory (or any of its subdirectories), it will use the specified toolchain.

The `profile` entry selects a baseline set of components, and `components` names extras on top of
it. An omitted `profile` means `minimal`, so an empty `components` list installs the minimal
profile's members -- not everything. To install every component in the channel, ask for the
`complete` profile:

```toml
[toolchain]
channel = "mainnet"
profile = "complete"
components = []
```

Listing components adds them to the profile's members. With this file:

```toml
[toolchain]
channel = "mainnet"
components = ["vm", "midenc", "client"]
```

the `minimal` profile is installed, plus `vm`, `midenc` and `client` if they are not already part
of it.

Activating an unnamed, unpatched project's toolchain only ever *adds* to what is installed for a
channel. Two projects sharing a channel cannot remove each other's components: if one asks for less, the other's
components stay. Use `midenup install <channel> --profile <profile>` to deliberately reduce what is
installed.

#### Patching components

Give a project toolchain a `name` to install an independent variant of an upstream channel.
A name is required when using `[patches]` to build components from another source:

```toml
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

`version` takes the same forms as in the channel manifest (`git`, `path` or `registry`). Patched
executables are built with `cargo install`, and legacy packages are extracted from the patched
Rust crate. Run `midenup install` in the project to install this variant; the shared `0.17.0`
installation and other named variants remain independent.

Select it elsewhere with `miden +custom:project-dev`, `MIDENUP_TOOLCHAIN=custom:project-dev`, or
`channel = "custom:project-dev"` in another toolchain file. These selectors reuse its recorded
source and patches. `midenup update custom:project-dev` preserves that definition; a network source
tracks its network independently, while a version source stays pinned. Mutable data lives beneath
`var/custom/project-dev`, separated by source network or pin, and survives uninstall unless
`--purge` is given.

Projects sharing a name must agree on its source and patches. After changing the definition, run
`midenup install` from the declaring project to reconcile it explicitly. Changing its network
identity requires a new name.
See [Patching components](docs/src/getting-started/usage.md#patching-components) for the full rules.


#### Setting a global default toolchain

The first toolchain you install becomes your system's default. You can change it with `midenup override <TOOLCHAIN>`. For example, to set `0.16.0` as the default toolchain, run:
```
midenup override 0.16.0
```

You can even set toolchains that are not currently installed in the
system. `midenup` (via `miden`) will handle installation as soon as you use any
component from the newly selected toolchain.

> [!NOTE]
> If a network such as `mainnet` is set as the active toolchain, `midenup` follows that network as
> it moves. To pin a specific release instead, name its version.

## Development

Internally, `midenup` relies on a _channel manifest_, which describes the available toolchain channels, their names and versions, and their components. Currently, the canonical version of our channel manifest lives in this repo as `channel-manifest.json`, and is published to Github Pages here: https://0xmiden.github.io/midenup/channel-manifest.json .

Locally, you can override the channel manifest URI, for testing or development purposes, by setting the `MIDENUP_MANIFEST_URI` environment variable. The URI must begin with either `file://` or `https://` at this time, but we could in theory support other URIs in the future if found useful.

The manifest format is described by the `Manifest` struct in `src/manifest/v3/mod.rs`, and supports a variety of features that we haven't currently fully implemented, but which are intended to allow for handy functionality such as defining toolchains that pull components from the local filesystem, or from a Git repository.

For now, a simple `make build` and `make test` is all you need to work on `midenup` itself, though there is not yet much in the way of tests.

To work with the `midenup` executable after running `make build`, you'll need to invoke it as `target/debug/midenup`.
