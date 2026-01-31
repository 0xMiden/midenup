# Update instructions
This guide is meant to describe how to update midenup's manifest.

Each toolchain has an associated version which corresponds to the included Miden Virtual Machine's.

Every component present in a toolchain is indented to be compatible with the same underlying VM version; which can be checked by looking at the component's Cargo.toml file.

## New toolchain version
When a new release[^1] is made, a new channel entry needs to be added to the manifest's `channels` array. The channel name should match the VM version (e.g., `0.20.3`).

1. Create a new channel object at the end of the `channels` array:
   ```json
   {
     "name": "<VM_VERSION>",
     "components": []
   }
   ```

2. Populate the `components` array by looking up each component's compatible version. For each component, check its `Cargo.toml` to verify it depends on the correct VM version. Note that most repositories depend on `miden-base` rather than `miden-vm` directly, so ensure the `miden-base` version points to the target VM version.

## Updating an existing toolchain (minor/patch)
When a minor or patch version is released for a component within an existing toolchain, update the component entry in place rather than creating a new channel.

1. Identify the target channel in the `channels` array and locate the component that received the update.

2. Update the version field. If the patch affects the VM itself, update the channel `name` to reflect the new version (e.g., `0.20.2` → `0.20.3`).

[^1]: A release refers to a tagged version of the Miden Virtual Machine. This version tag serves as the reference point for assembling a compatible toolchain, as all related components (client, node, faucet, etc.) are expected to align with the same VM version.
