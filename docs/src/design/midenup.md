# Midenup

The midenup tool is mainly in charge of "toolchain management" which consists of toolchain installation, updating, uninstallation and setting.

## Midenup directory

Midenup stores all of its files and directories in a single directory called `midenup`. By default, this directory will reside in `XDG_DATA_HOME/midneup` (for more information regarding `XDG_DATA_HOME` see [here](https://specifications.freedesktop.org/basedir-spec/latest/#variables)).

Most notably, this directory stores:
- The local manifest file, `manifest.json`.
- The `toolchains` directory which stores all the installed toolchains as subdirectories.

## Installation
To install a toolchain, midenup begins by creating a directory inside the `toolchains` with the same name as channel's.

After creating the directory, `midenup` constructs a rust script named `install.rs` that will be used to install the components. This script takes the form of a [Cargo script](https://rust-lang.github.io/rfcs/3424-cargo-script.html), which consist of a single file rust script with an embedded manifest at the top. Internally, the script makes calls to `cargo install`, with the corresponding flags and options depending on the component's requirements.

### Installation Idempotence
It is important to note that midenup's install scripts are intended to also serve as a "description" of what the toolchain looks like in the system. That is, the components described inside the `install.rs`file should also be the components that are actually installed in the system; making the `install.rs` file idempotent.

Adittionally, if installation fails mid way through, midenup uses a log file, `.installation-in-progress`, to determine where to resume the installation from.
- After each component is installed, a line is added to the log with the name of the component.
- After all the componenent are installed, `midenup` changes the name of the file to `installation-successful` to indicate that installation finalized. With this change now in place, if a re-install is issued, `midenup` will simply skip the install all-together.

## Update
Updates can be broken down into three major stages:
1. The first stage consists of determining which components from the desired channel require updating.
    - The term "update" is used broadly in this context. An "update" is understood as any type of difference in the locally installed channel when compared against its upstream equivalent. (these can because of a larger version, a smaller version, difference in the amount of components, etc).
2. The second stage simply consists of removing the outdated components to make space for the updated versions.
3. And on the third stage, we install the newer version of the channel.
    - This channel might differ *slightly* from the upstream channel. This can happen if a user decides to willingly skip/ignore a component update. An example of this can happen with path-managed components which are skipped during updates by default.
