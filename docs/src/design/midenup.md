# Midenup

The midenup tool is mainly in charge of "toolchain management" which consists of toolchain installation, updating, uninstallation and setting.

## Midenup directory

Midenup stores all of its files and directories in a single directory called `midenup`. By default, this directory will reside in `XDG_DATA_HOME/midneup` (for more information regarding `XDG_DATA_HOME` see [here](https://specifications.freedesktop.org/basedir-spec/latest/#variables)).

Most notably, this directory stores:
- The local manifest file, `manifest.json`.
- The `toolchains` directory which stores all the installed toolchains as subdirectories.

## Installation
To install a toolchain, midenup begins by creating a directory inside the `toolchains` with the same name as channel's.

After creating the directory, `midenup` constructs a rust script named `install.rs` that will be used to install the components. This script takes the form of a [Cargo script](https://rust-lang.github.io/rfcs/3424-cargo-script.html), which broadly speaking, is a single file rust script with an embedded manifest at the top. Internally, the script makes calls to `cargo install`, passing the corresponding flags and options dependenign on the component.
Once generated, midenup proceeds to execute it.
