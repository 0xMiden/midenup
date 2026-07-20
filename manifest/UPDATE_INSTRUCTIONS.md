# Update instructions

This guide is meant to describe how to update midenup's manifest.

Each toolchain has an associated version which corresponds to the version of the Miden protocol that the toolchain is oriented towards.

You must ensure that all components in a toolchain are compatible with the given protocol version. The VM component in particular must be the same version as the protocol itself was built aggainst. You can look on crates.io to see what versions of each crate a component depends on.

## Prerequisites

Make sure you have built the `bin/update-manifest` CLI tool with:

```
make update-manifest
```

The following steps will use this tool to perform modifications to the channel manifest.

## New toolchain version

When a new release[^1] is made, a new channel entry needs to be added to the manifest's `channels` array. The channel name should match the protocol version it is linked to, without the patch version set to `0` (e.g. `0.15.0`). The simplest way to do this is to clone the latest stable release and give it the new version:

```
bin/update-manifest --manifest-path manifest/channel-manifest.json \
    clone-toolchain --from stable --to 0.15.0
```

Next, you will need to update each component in the cloned toolchain, as appropriate. See the section on updating an existing toolchain for details.

## Updating an existing toolchain (minor/patch)

In typical cases, this is just a matter of bumping the version of each affected component - for more complex changes, see the output of `bin/update-manifest help update-component`, or modify the manifest by hand. Bumping the component version is as simple as:

```
bin/update-manifest --manifest-path manifest/channel-manifest.json \
    update-component $COMPONENT \
    --channel $CHANNEL
    --authority=$COMPONENT_VERSION
```

For newly added components, see the `add-component` subcommand.

For removed components, see the `remove-component` subcommand.
