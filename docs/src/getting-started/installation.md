# Installation
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
