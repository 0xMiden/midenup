cargo() {
    if [[ $@ == "+nightly" ]]; then
        command /home/runner/.rustup/toolchains/nightly-x86_64-unknown-linux-gnu/bin/cargo
    elif [[ $@ == "+nightly-2025-03-20" ]]; then
        command /home/runner/.rustup/toolchains/nightly-2025-03-20-x86_64-unknown-linux-gnu/bin/cargo
    else
        command /home/runner/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo
    fi
}
