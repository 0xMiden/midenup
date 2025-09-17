# Installing midenup
Firstly, midenup can be installed with the following command:

```shell
cargo install midenup
```

> [!IMPORTANT]
> Until this crate has been published to crates.io, it is only possible to
> install midenup by cloning the repository and then running `cargo install --path .`
> or `cargo install --git <repo_uri>`.

Once installed, midenup needs to be initialized in order to work properly. This can be achieved like so:

``` shell
midenup init
```

The first time it is run, the following message will probably be displayed:

``` shell
Could not find `miden` executable in the system's PATH. To enable it, add midenup's bin directory to your system's PATH. The following lines can be added to the system's shell profile:

export MIDENUP_HOME='<some/path>/midenup'
export PATH=${MIDENUP_HOME}/bin:$PATH
```

> [!IMPORTANT]
> If this message is *not* displayed, yet this is the first time that midneup is
> being installed in the system, then this is probably due to an executable
> called "miden" already being present in the PATH; this can be the case of
> older VM or client releases. If that is the case, consider uninstalling those
> executables via `cargo uninstall miden` in order for midenup to work
> correctly. Nowadays, those executables are named miden-vm and miden-client
> respectively.

After adding those `export`s to the shell's profile, the shell session must be restarted in order for these variables to take effect. This will enable `midenup`'s `miden` command, which is used to interact with the different miden components.

# Getting started
In order to get started with `midenup`, a toolchain should be installed. A toolchain is simmply a collection of miden programs (e.g. the vm, the client, the compiler, etc).
Toolchains are installed via "Channels", which are a specific release of a toolchain with instructions on how to obtain it.

Most users will want to install the latest stable toolchain from the official midenup channel, like so:

``` shell
midenup install stable
```

This command will install the stable toolchain using the [official midenup channel](https://0xmiden.github.io/midenup/channel-manifest.json).
However, midenup also supports "custom channels", where one can create a customized version of a toolchain. In order to use a custom channel, `midenup` must called with the`MIDENUP_MANIFEST_URI` environment variable, like so:
```
MIDENUP_MANIFEST_URI=file://<path/to/custom/manifest.json> midenup install <toolchain>
```

> [!WARNING]
> This functionality is still in early stages of development. Currently, this
> requires writing the channel manifest manually.

## Specific releases
If required, a specific toolchain version can also be installed with the `midenup install <toolchain-version>` syntax, like so:

``` shell
midneup install 0.15.0
```

To list all the currently installed toolchains in the system, run:

``` shell
midenup show list
```

# Using a toolchain
The `miden help toolchain` can be run to display a quick summary of what the currently active toolchain offers.

It should display a message similar to the following:

``` shell
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

``` shell
midenup show active-toolchain
```

There are currently 2 main mechanisms to alter the active toolchain: setting a system wide default or setting a directory local default. Each method has an associated priority according to the following chart (from highest to lowest):

1. Directory local toolchains.
2. System default.
3. Fallback: If none of the above are detected, `midenup` will fallback to the `stable` toolchain as default.

### System wide active toolchain
The `midenup override <toolchain>` command will set the passed toolchain as the system's default. For instance, the following command will set toolchain version 0.15.0 as the system's default:

``` shell
midenup override 0.15.0
```

To check this, use `midenup show active-toolchain`.

### Local toolchains
The `midenup set <toolchain>` command has the ability to set a toolchain to be the default in specific directory. For example, to set toolchain version 0.17.0 as the default run:

``` shell
midenup set 0.17.0
```

This will create a `miden-toolchain.toml` file in the present working directory (similar to`rustup`'s `rust-toolchain.toml` file).
With this file now in place, toolchain version 0.17.0 will be the active toolchain in that directory and in all of if sub-directories.

## Updating a toolchain
Toolchains can periodically require updates, which can be in one of the following forms:

### Updating a specific toolchain
When updating a specific toolchain, only updates which are known to work with that version of the toolchain will be installed/updated. These can occur when a component gets a new minor release, or it gets rolled back. The `midenup update <toolchain>` command will trigger these types of updates can be used.

If no `<toolchain>` is passed, like so:

``` shell
midenup update
```

then `midenup` will look for updates on every installed toolchain.

### Updating stable
If the latest installed "stable" toolchain in the system is older than the latest available version present upstream, the system can be brought up to date with the following command:

``` shell
midenup update stable
```

## Uninstalling a toolchain
A toolchain can be uninstalled via the `midenup uninstall <TOOLCHAIN>` command.
For example, to uninstall toolchain version `0.16.0`, run:
```
midenup uninstall 0.16.0
```

# Example usage
A typical usage of midenup and miden might look like the following:

1. midenup is downloaded
2. The latest stable toolchain can then be installed:
   ```sh
   midenup install stable
   ```
3. With the toolchain now installed, the installed components can be inspected with the following command:
   ``` sh
   miden help toolchain
   ```
4. On this list, components that require initialization will display their corresponding commmand. One such component is the miden client, which can be initialized like so:
   ```sh
   miden client init --network devnet
   ```
   (`devnet` is used as an example).
5. With the client now initialized, an account can be created and deployed using code from a custom miden project. To start, create a new miden project:
   ```sh
   miden new miden_project && cd miden_project
   ```
6. If said project requires a specific toolchain version, for instance 0.17.0, then it can be set with the following command:
   ```sh
   midenup set 0.17.0
   ```
   Note that if the toolchain is not already installed, midenup/miden will automatically install it as soon as it detects that it is required.
7. With the project now generated and the required toolchain established, the `src/lib.rs` can be modified with any desired additions. Afterwards, a build can be issued:
   ```sh
   miden build
   ```
   Once compilation finishes, a message displaying the location of the generated Miden Package will be shown.
8. With the generated Miden Package, an account can be created and deployed with the following command:
   ```sh
   miden client new-account --account-type regular-account-updatable-code -p /path/to/package.masp
   ```
