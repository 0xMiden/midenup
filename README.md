# midenup

The Miden toolchain installer.

> [!WARNING]
> This tool is still a work in progress.

The `midenup` executable facilitates two primary tasks:

1. Toolchain managment, i.e. bootstrapping the environment, and installing, updating, and configuring installed toolchain components.
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
the toolchain. When updating a versioned toolchain, only updates which are known
to work with that version of the toolchain will be installed/updated.

In both cases, you simply run `midenup install <TOOLCHAIN>`.

When getting started, it is recommended that you install the `stable` toolchain
first, which `midenup` also assumes is the default toolchain if not overridden
in the current working directory.

### Using a toolchain

Interacting with Miden toolchain components is done via the `miden` command, which handles delegating commands to the underlying components using subprocesses. For example, `miden new` calls out to `cargo miden new` to create a new Rust-based Miden project.

By default, the `miden` command uses the currently active toolchain, which you can view using `midenup show active-toolchain`. If you've installed a toolchain other than the default (i.e. `stable`) toolchain, you currently need to create a `miden-toolchain.toml` file in your working directory to make that toolchain your default. We'll be improving the ergonomics of this soon.

An example `miden-toolchain.toml` looks like so:

```toml
channel = "0.15.0"
components = ["std", "base", "vm", "midenc", "cargo-miden"]
```

### Uninstalling a toolchain

You can easily uninstall a Miden toolchain by deleting the corresponding directory under `$MIDENUP_HOME/toolchains`.

### Uninstalling `midenup`

You can easily uninstall `midenup` itself by deleting the `$MIDENUP_HOME` directory.

## Development

Internally, `midenup` relies on a _channel manifest_, which describes the available toolchain channels, their names and versions, and their components. Currently, the canonical version of our channel manifest lives in this repo as `channel-manifest.json`, and is published to GitHub Pages.

Locally, you can override the channel manifest URI, for testing or development purposes, by setting the `MIDENUP_MANIFEST_URI` environment variable. The URI must begin with either `file://` or `https://` at this time, but we could in theory support other URIs in the future if found useful.

The manifest format is described by the `Manifest` struct in `src/manifest.rs`, and supports a variety of features that we haven't currently fully implemented, but which are intended to allow for handy functionality such as defining toolchains that pull components from the local filesystem, or from a Git repository.

For now, a simple `cargo build` and `cargo test` is all you need to work on `midenup` itself, though there is not yet much in the way of tests.

To work with the `midenup` executable after running `cargo build`, you'll need to invoke it as `target/debug/midenup`.
