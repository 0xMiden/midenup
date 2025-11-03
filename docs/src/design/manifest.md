---
sidebar_position: 2
title: Manifest
---

# Manifest

## Manifest files

In midenup, a Manifest is a JSON file containing a list of [channels](#channel). There are two "types" of manifests:

- The "upstream" manifest, which contains the set of channels which `midenup` takes to be as the source of truth when determining which components to install, update or downgrade.
    - Most users will use the default manifest which can be found in the repository's [Github Page](https://0xmiden.github.io/midenup/channel-manifest.json). It contains the canonical set of Miden channels.
    - Midenup also supports the use of custom manifests, see [Custom Manifests](#custom-manifests) for more information.
- The "local" manifest, which is used to describe the locally installed channels, with some additional information.
### Custom manifests
:::warning
This functionality is still in early stages of development. Currently, this requires writing the channel manifest manually.
:::
Midenup does also support the use of custom, user-made, manifests to install and manage custom toolchains.

These can contain custom versions of components, which can be installed from the user's filesystem, specific git revisions of a repository, etc.

## Channels
Channels are a list of [Components](#component) under a common version/name which are meant to be used in conjunction.

### Stable channel
In midenup, the notion of a "stable channel" is defined to be the latest, non nightly, available channel in the "upstream" manifest.
To denote this, `midenup` tags the channel as `stable` in the Channel's `alias` field in the local manifest.


## Component
Components are the individual binaries/libraries used in Miden. Besides having a version, each Component present in a [channel](#channels) showcases additional metadata like from where to obtain the source code, wheter it has a pre-built binary, the file it installs, its dependencies, etc.
