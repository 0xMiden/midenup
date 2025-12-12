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

> [!IMPORTANT]
> Until this crate has been published to crates.io, it is only possible to
> install using `cargo install --path .` or `cargo install --git <repo_uri>`.

The `midenup init` command initializes the `$MIDENUP_HOME` directory, and symlinks `midenup` to `$MIDENUP_HOME/bin/miden` so that all of the executable Miden components can be accessed using the `miden` command.

You must also ensure `$MIDENUP_HOME/bin` is added to your shell `$PATH`. You can obtain the current value of `$MIDENUP_HOME` using `midenup show home` if you don't set it explicitly. For example, you might have something like this in your shell profile (assuming a `sh`-like shell):

```
export MIDENUP_HOME=$XDG_DATA_DIR/midenup
export PATH=${MIDENUP_HOME}/bin:$PATH
```

> [!WARNING]
> If you forget to do the step above, some functionality will not work as
> expected!

You are now ready to install your first toolchain!

### Installing a toolchain

After initializing `midenup`, the first thing you will want to do is actually
install a toolchain so you can work with the various Miden components. There
are two ways to do this:

1. Installing a release channel, e.g. `stable`, which will install the latest
stable versions of all components that work together. When updating a release
channel, breaking changes can occur if there were breaking changes between
stable releases of any of the toolchain components. The upside is that you
stay up to date with changes upstream, without having to think about version
management.
2. Installing a specific toolchain version, e.g. `0.15.0`, which will install
the latest versions of all components which are compatible with that version of
the toolchain.

In both cases, you simply run `midenup install <TOOLCHAIN>`.

When getting started, it is recommended that you install the `stable` toolchain, like so:
```
midenup install stable
```

`midenup` also assumes stable to be the default toolchain if not overridden in
the current working directory or by the user's default toolchain (for more
information on how to configure the active toolchain, see [Configuring the active
toolchain](#configuring-the-active-toolchain)).

### Updating a toolchain

To update a given toolchain, you can use the `midenup update <TOOLCHAIN>`
command. This command's behavior differs slightly depending on how it is called.

#### Updating stable

To update stable to the latest release, run:
```
midenup update stable
```

This will check if there's a newer stable release and will update the toolchain accordingly.

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
| miden call       | Call view function (read-only)    | miden-client account --show                                          |
| miden send       | Send transaction (state-changing) | miden-client send                                                    |
| miden simulate   | Simulate transaction (no commit)  | miden-client exec                                                    |

Aliases are defined at the channel level in `manifest/channel-manifest.json`. Each alias is a pipeline of steps, and each step must specify which component's executable to run. Commands use the existing `CliCommand` tokens (`executable`, `lib_path`, `var_path`, or verbatim strings).

Pipeline steps execute in order. User-provided CLI arguments are appended to the first step only.


### Uninstalling a toolchain

You can easily uninstall a Miden toolchain with the `midenup uninstall <TOOLCHAIN>` command.
For example, to uninstall toolchain version `0.16.0`, run:
```
midenup uninstall 0.16.0
```

> [!WARNING]
> It is **strongly discouraged** to delete the toolchain directories manually,
> since this will most likely generate an invalid environment and `midenup` will
> probably *not* work as intended.

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

If none of the previous conditions are met, then `stable` will be used.

#### Setting a project specific toolchain

To configure a toolchain to be active in the present working directory, you can use the `midenup set <TOOLCHAIN>` command.
For example, to set `0.16.0` run:
```
midenup set 0.16.0
```

This procedure will generate a `miden-toolchain.toml` file in the directory where `midenup set` was invoked:

```toml
[toolchain]
channel = "stable"
components = []
```

Now, whenever `miden` is called in this directory (or any of its subdirectories), it will use the specified toolchain.
If the `components` entry is left blank, all the available components for the selected channel will be installed. However, if the list is not empty, only the listed components will be installed.
For example, with the following `miden-toolchain.toml` file:
```toml
[toolchain]
channel = "stable"
components = ["vm", "midenc", "client"]
```
Only the `vm`, `midenc`, `client` will be installed after `miden` gets executed.


#### Setting a global default toolchain

You can customize your system's default toolchain with `midenup override <TOOLCHAIN>`. For example, to set `0.16.0` as the default toolchain, run:
```
midenup override 0.16.0
```

You can even set toolchains that are not currently installed in the
system. `midenup` (via `miden`) will handle installation as soon as you use any
component from the newly selected toolchain.

> [!NOTE]
> If `stable` is set as the active toolchain, `midenup` will use the latest
> available `stable` toolchain.
> If you desire to pinpoint a specific release as the default, then use the
> version name explicitly.

## Development

Internally, `midenup` relies on a _channel manifest_, which describes the available toolchain channels, their names and versions, and their components. Currently, the canonical version of our channel manifest lives in this repo as `channel-manifest.json`, and is published to Github Pages here: https://0xmiden.github.io/midenup/channel-manifest.json .

Locally, you can override the channel manifest URI, for testing or development purposes, by setting the `MIDENUP_MANIFEST_URI` environment variable. The URI must begin with either `file://` or `https://` at this time, but we could in theory support other URIs in the future if found useful.

The manifest format is described by the `Manifest` struct in `src/manifest.rs`, and supports a variety of features that we haven't currently fully implemented, but which are intended to allow for handy functionality such as defining toolchains that pull components from the local filesystem, or from a Git repository.

For now, a simple `make build` and `make test` is all you need to work on `midenup` itself, though there is not yet much in the way of tests.

To work with the `midenup` executable after running `make build`, you'll need to invoke it as `target/debug/midenup`.
