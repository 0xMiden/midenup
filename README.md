# midenup

The Miden toolchain installer.

> [!WARNING]
> This tool is still a work in progress.

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
> * The Miden compiler, `midenc`, and its Rust tooling, i.e. `cargo-miden`
> * The Miden client
> * The Miden standard library
> * The Miden transaction kernel library
>
> In the future, more components will be added.

## Usage

To get started, you must first install `midenup`, and then initialize its
environment, like so:

```
cargo install midenup && midenup init
```

The `midenup init` command initializes the `$MIDENUP_HOME` directory, and creates a `miden` symlink in `$CARGO_HOME/bin` (default `~/.cargo/bin`) pointing to the `midenup` executable. Since Rust users typically already have `$CARGO_HOME/bin` in their PATH, the `miden` command should be available immediately.

> [!WARNING]
> If `miden` is not found after running `midenup init`, ensure `$CARGO_HOME/bin`
> is in your PATH. On macOS with zsh, add `export PATH="$HOME/.cargo/bin:$PATH"`
> to `~/.zprofile` and create that file first if it does not exist.

You are now ready to install your first toolchain!

### Installing a toolchain

After initializing `midenup`, the first thing you will want to do is actually
install a toolchain so you can work with the various Miden components. There
are two ways to do this:

1. Installing a release network, e.g. `mainnet`, which installs the toolchain currently deployed to
that network. When a network is promoted to a newer toolchain, `midenup update mainnet` follows it,
carrying your component selection and your client data across.
2. Installing a specific toolchain version, e.g. `0.15.0`, which pins you to that toolchain
regardless of what the networks do.

In both cases, you simply run `midenup install <TOOLCHAIN>`.

The networks are:

| Network   | Also accepted as | What it is                        |
|-----------|------------------|-----------------------------------|
| `mainnet` | `stable`         | The toolchain deployed to mainnet |
| `testnet` | `beta`           | The toolchain deployed to testnet |
| `devnet`  | `nightly`        | The newest published toolchain    |

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

This follows the network's pointer wherever it has moved, carrying your component selection and your
client data across. If the pointer has not moved, it still picks up any changes to the components of
the toolchain it names.

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

| Alias            | Action                            | Corresponds to                                                       |
|------------------|-----------------------------------|----------------------------------------------------------------------|
| miden account    | Create local account              | miden-client account                                                 |
| miden faucet     | Fund account via faucet           | miden-client mint                                                    |
| miden new        | Create new project                | cargo miden new                                                      |
| miden build      | Build project                     | cargo miden build                                                    |
| miden deploy     | Deploy a contract                 | miden-client -s public --account-type regular-account-immutable-code |
| miden new-wallet | Create a wallet                   | miden-client new-wallet --deploy                                     |
| miden call       | Call a procedure on an account    | miden-client call                                          |
| miden send       | Send transaction (state-changing) | miden-client send                                                    |
| miden simulate   | Simulate transaction (no commit)  | miden-client exec                                                    |


### Uninstalling a toolchain

You can easily uninstall a Miden toolchain with the `midenup uninstall <TOOLCHAIN>` command.
For example, to uninstall toolchain version `0.16.0`, run:
```
midenup uninstall 0.16.0
```

This keeps the toolchain's mutable data — the Miden client's database, for instance — and tells you
where it left it. To remove that too, pass `--purge`:
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

This only ever removes installations nothing refers to any more. It never touches an installed
toolchain, and it is safe to run at any time.

### Upgrading from an older `midenup`

The first time a newer `midenup` runs, it converts the record an older one left in
`$MIDENUP_HOME` into its own format. It carries over which channels you had installed and which
components you had in each, and nothing else — everything else is re-derived from the published
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

`miden` and `midenup` determine the current active toolchain according to the following rules:
1. If there's a `miden-toolchain.toml` file in the present working directory,
   then `miden` will use that to determine the current active toolchain.
2. If not, `miden` will check if a toolchain has been set as the system's
   default (more details in the [Configuring the active toolchain](#configuring-the-active-toolchain) section).

If none of the previous conditions are met, then `mainnet` will be used.

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

Activating a project's toolchain only ever *adds* to what is installed for a channel. Two projects
sharing a channel cannot remove each other's components: if one asks for less, the other's
components stay. Use `midenup install <channel> --profile <profile>` to deliberately reduce what is
installed.


#### Setting a global default toolchain

You can customize your system's default toolchain with `midenup override <TOOLCHAIN>`. For example, to set `0.16.0` as the default toolchain, run:
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
