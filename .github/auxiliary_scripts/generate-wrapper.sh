#!/bin/bash

toolchains=$(grep rustup_channel manifest/channel-manifest.json | awk '{ print  $2 }' | tr -d '\"' | uniq)

echo "
cargo() {
    if [[ \$@ == \"+nightly\" ]]; then
        command /home/runner/.rustup/toolchains/nightly-x86_64-unknown-linux-gnu/bin/cargo
" > $1

while read toolchain ; do
    echo "
    elif [[ \$@ == \"+$toolchain\" ]]; then
        command /home/runner/.rustup/toolchains/${toolchain}-x86_64-unknown-linux-gnu/bin/cargo
" >> $1
done  <<< "$toolchains"

echo "
    else
        command /home/runner/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo
    fi
}" >> $1
