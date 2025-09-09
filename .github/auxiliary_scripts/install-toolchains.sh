#!/bin/bash

echo "Installing stable"
rustup install stable

toolchains=$(find . -path "*/target" -prune -o -name "*.json" -exec grep rustup_channel \{\} + | awk '{print $3}' | tr -d '\",' | uniq)


while read toolchain; do
    echo "Installing ${toolchain}"
    rustup install ${toolchain}
done <<< "$toolchains"

echo "Installing nightly"
rustup install nightly
