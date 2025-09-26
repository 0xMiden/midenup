# Manifest

## Manifest files

In midenup, a Manifest is JSON file which contains textual representations of channels. There are two "types" of manifests:

- The "upstream" manifest, which is used to install channels. Most users will use the default manifest which can be found [here](https://0xmiden.github.io/midenup/channel-manifest.json).
- The "local" manifest, which is used to describe the locally installed channels, with some additional information.
### Custom manifests
> [!WARNING]
> This functionality is still in early stages of development. Currently, this
> requires writing the channel manifest manually.
Midenup does also support the use of custom, user-made, manifests to install and manage custom toolchains. 

These can contain custom versions of components, which can be installed from the users filesystem, specific git revisions of a repository, etc.

## Channels
Channels are a data textual representation of channels. Most notably, they contain the list of components that are installed with the channel, along with the

Channels are used by midenup to install toolchains. They contain a list of components that make the toolchain up, along with additional information such as required cargo version, calling format, required aliases, etc.

### Stable channel
In midenup, the notion of a "stable channel" is defined to be the latest, non nightly, available channel in the "upstream" manifest.
To denote this, `midenup` tags the channel as `stable` in the Channel's `alias` field in the local manifest.

