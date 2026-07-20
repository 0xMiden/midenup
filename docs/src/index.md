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

- The [Miden VM](https://docs.miden.xyz/reference/miden-vm/).
- The [Miden compiler](https://docs.miden.xyz/reference/compiler/usage/midenc) and its cargo extension [Cargo Miden](https://docs.miden.xyz/reference/compiler/usage/cargo-miden).
- The [Miden client](https://docs.miden.xyz/builder/tools/clients/rust-client/).
- The [Miden node](https://docs.miden.xyz/reference/node/).
- The [Miden standard library](https://docs.miden.xyz/reference/miden-vm/user_docs/core_lib/).
- The [Miden transaction kernel library](https://docs.miden.xyz/reference/protocol/transaction/).

In the future, more components will be added.
