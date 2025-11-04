# Miden CLI

The miden CLI serves as the interface to all the toolchains installed via `midenup`.
It's worth mentioning that in actuality, `midenup` and `miden` are the same executable. The difference in behavior is determined at runtime depending on the value of `argv[0]`.

## Component interaction

Once installed, components can be called directly with `miden <component-name>`. This will pass all the additional arguments back to the underlying executable.

## Aliases

Since some tasks in Miden development come up frequently, the `miden` CLI is also aware of a number of aliases. This include things like compiling a project, creating an account, deploying a local node, etc.

The list of currently avaialble aliases can be found with `miden help toolchain`. Aliases are channel specific, so different channels may have different number of aliases all together. A typical usage of these aliases can be found on the [tutorial](../getting-started/tutorial.md)
