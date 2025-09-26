# Miden CLI

The miden CLI serves as the interface to all the toolchains installed via `midenup`.
It's worth mentioning that in actuality, `midenup` and `miden` are the same executable. The difference in behavior is determined at runtime depending on the value of `argv[0]`.

## Component interaction
It is via the `miden` utility that users can use the installed `miden` components.

## Aliases
The `miden` CLI is aware of a number of aliases that make interacting with the various miden components easier. The list of currently avaialble aliases can be found with `miden help toolchain`.
Aliases are channel specific, so different channels can have different number of aliases all together. A typical usage of these aliases can be found on the [tutorial](./getting-started/tutorial.md).




