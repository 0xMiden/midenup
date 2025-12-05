---
sidebar_position: 1
title: Overview
---

# Introduction

`midenup` is a tool created to facilitate the usage of various components that make up the miden ecosystem. It comes with two major utilities:
- `midenup`: used to install, update and manage toolchains.
- `miden`: used to interact with the various components that make a toolchain up.

:::warning
This tool is still a work in progress.
:::

## Toolchains
The notion of a _toolchain_ here refers to a group of components from the Miden project. These are required to develop, test, run, and interact with Miden programs, both locally and on the network.

Currently, the set of components consists of:

- The [Miden VM](https://0xmiden.github.io/miden-vm/).
- The [Miden compiler](https://0xmiden.github.io/compiler/usage/midenc.html) and its cargo extension [Cargo Miden](https://0xmiden.github.io/compiler/usage/cargo-miden.html).
- The [Miden client](https://0xmiden.github.io/miden-client/).
- The [Miden node](https://0xmiden.github.io/miden-node/).
- The [Miden standard library](https://github.com/0xMiden/miden-vm?tab=readme-ov-file#project-structure).
- The [Miden transaction kernel library](https://github.com/0xMiden/miden-base?tab=readme-ov-file#project-structure).

In the future, more components will be added.
