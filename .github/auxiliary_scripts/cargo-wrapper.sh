
cargo() {
    raw_cargo="cargo"
    # TODO: Grab raw cargo executable PATH ad-hoc
    toolchain="/home/runner/.rustup/toolchains/"
    toolchain_is_specified=0
    arguments=()

    for arg in "$@"; do
        if [[ "$arg" == "+nightly" ]]; then
            toolchain="${toolchain}nightly-x86_64-unknown-linux-gnu/bin/"
            toolchain_is_specified=1
        elif [[ "$arg" == "+nightly-2025-03-20" ]]; then
            toolchain="${toolchain}nightly-2025-03-20-x86_64-unknown-linux-gnupple-darwin/bin/"
            toolchain_is_specified=1
        elif [[ "$arg" == "+stable" ]]; then
            toolchain="${toolchain}stable-x86_64-unknown-linux-gnupple-darwin/bin/"
            toolchain_is_specified=1
        elif [[ "$arg" == "install" ]]; then
            arguments+="install"
            arguments+="--debug"
        else
            arguments+="${arg}"
        fi
    done

    if [[ toolchain_is_specified -eq 0 ]]; then
        toolchain="${toolchain}stable-aarch64-apple-darwin/bin/"
    fi

    full_command="${toolchain}${raw_cargo}"
    echo "Running: ${full_command}"
    ${full_command} ${arguments}
}
