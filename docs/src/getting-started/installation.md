# Installation

## Installing midenup

1. Install the Miden toolchain installer (midenup) using cargo:

```shell title=">_ Terminal"
cargo install midenup
```

:::important
Until this crate has been published to crates.io, it is only possible to install midenup by cloning the repository and then running `cargo install --path .` or `cargo install --git https://github.com/0xMiden/midenup `.
:::

2. Initialize the midenup environment:

```shell title=">_ Terminal"
midenup init
```

The `midenup init` command initializes the `$MIDENUP_HOME` directory and creates a `miden`
symlink in `$CARGO_HOME/bin` (default `~/.cargo/bin`). The `miden` command then routes to the
active toolchain components for you.

## Configure PATH Environment Variable

**This is a critical step!** You must ensure the directory containing the `miden` symlink is in
your shell `$PATH`. In a standard Rust install that is `$CARGO_HOME/bin`, which defaults to
`~/.cargo/bin`. `midenup` will automatically display the required commands if it detects that
`miden` is not accessible from the `$PATH`.

If you use a custom `CARGO_HOME`, replace the paths below accordingly.

### For Zsh (macOS default)

Add the following to your `~/.zprofile` file. If the file does not exist yet, create it first:

```bash title=">_ Terminal"
touch ~/.zprofile
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.zprofile
```

If you prefer to edit the file manually, add:

```bash title=">_ Terminal"
export PATH="$HOME/.cargo/bin:$PATH"
```

Then reload your shell configuration:

```bash title=">_ Terminal"
source ~/.zprofile
```

### For Bash

Add the following to your `~/.bash_profile` file:

```bash title=">_ Terminal"
export PATH="$HOME/.cargo/bin:$PATH"
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
midenup install stable
```

This installs the latest stable versions of all Miden components that work together.

## Verify Installation

1. Check that midenup is working:

```bash title=">_ Terminal"
midenup show active-toolchain
```

<details>
<summary>Expected output</summary>

```text
stable
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
