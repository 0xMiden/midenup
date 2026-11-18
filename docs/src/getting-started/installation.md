# Installation

## Prerequisites

- [Rust](https://rustup.rs), latest stable, only if you install `midenup` with Cargo, patch
  components, or use a toolchain with components built from source.
- [Docker](https://docs.docker.com/get-docker/) with Docker Compose v2.34.0 or later, only if you use the
  `node` component (`miden node`).

## Installing midenup

Install `midenup` with the installer script. It downloads the prebuilt release for your platform
(Linux x86_64 or macOS arm64) to `~/.local/bin` and runs `midenup init`. It warns if another
`midenup` earlier in your `PATH` would run instead:

```shell title=">_ Terminal"
curl -L --proto '=https' --tlsv1.2 -sSf 'https://github.com/0xMiden/midenup/releases/latest/download/installer.sh' | bash
```

`midenup init` initializes the `$MIDENUP_HOME` directory and creates a `miden` symlink next to
the `midenup` executable. The `miden` command then routes to the active toolchain components for
you.

Alternatively, if you have Rust installed, build `midenup` with Cargo and initialize it yourself.
This places both executables in `$CARGO_HOME/bin` (default `~/.cargo/bin`):

```shell title=">_ Terminal"
cargo install --locked midenup && midenup init
```

## Configure PATH Environment Variable

**This is a critical step!** You must ensure the directory containing the `miden` symlink is in
your shell `$PATH`: `~/.local/bin` if you used the installer script, or `~/.cargo/bin` if you
installed with Cargo. `midenup` will automatically display the required commands if it detects
that `miden` is not accessible from the `$PATH`.

The examples below use `~/.local/bin`; replace it with your install directory if it differs.

### For Zsh (macOS default)

Add the following to your `~/.zprofile` file. If the file does not exist yet, create it first:

```bash title=">_ Terminal"
touch ~/.zprofile
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zprofile
```

If you prefer to edit the file manually, add:

```bash title=">_ Terminal"
export PATH="$HOME/.local/bin:$PATH"
```

Then reload your shell configuration:

```bash title=">_ Terminal"
source ~/.zprofile
```

### For Bash

Add the following to your `~/.bash_profile` file (use `~/.bashrc` on Linux). If the file does
not exist yet, create it first:

```bash title=">_ Terminal"
touch ~/.bash_profile
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bash_profile
```

If you prefer to edit the file manually, add:

```bash title=">_ Terminal"
export PATH="$HOME/.local/bin:$PATH"
```

Then reload your shell configuration:

```bash title=">_ Terminal"
source ~/.bash_profile
```

:::warning Critical Step
If you forget to do the step above, some functionality will not work as expected!
:::

### For PowerShell (Windows)

:::note todo
Add instructions here
:::

## Install the Miden Toolchain

After initializing `midenup`, install the Miden toolchain:

```bash title=">_ Terminal"
midenup install mainnet
```

This installs the toolchain currently deployed to mainnet: the versions of all Miden components that work together on that network.

## Verify Installation

1. Check that midenup is working:

```bash title=">_ Terminal"
midenup show active-toolchain
```

<details>
<summary>Expected output</summary>

```text
mainnet
```

</details>


2. Verify that the `miden` command is available:

```bash title=">_ Terminal"
miden help
```

<details>
<summary>Expected output</summary>

```text
The Miden toolchain porcelain

Help:
  help                   Print this help message
  help toolchain         Print help about the currently available aliases and components *
  help <COMPONENT>       Print a specific <COMPONENTS>'s help message *

*: These commands will install the currently present toolchain if not installed.
```

</details>
