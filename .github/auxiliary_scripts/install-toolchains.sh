#!/bin/bash

echo "Installing stable"
rustup install stable

toolchains=$(grep rustup_channel manifest/channel-manifest.json | awk '{ print  $2 }' | tr -d '\"' | uniq)

while read toolchain; do
    echo "Installing ${toolchain}"
    rustup install ${toolchain}
done <<< "$toolchains"

echo "Installing nightly"
rustup install nightly
