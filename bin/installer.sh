#!/usr/bin/env bash

set -eu

# Set pipefail if it works in a subshell, disregard if unsupported
# shellcheck disable=SC3040
(set -o pipefail 2> /dev/null) && set -o pipefail

usage() {
cat << EOF
Usage: install-from-release [OPTIONS]

Installs and initializes midenup on the current machine.

Options:

  -h, --help                   Displays this usage information
  -v, --version <VERSION>      Install a specific midenup release, if available
  --install-path <DIR>         Installs the midenup binary to DIR.
                               NOTE: Ensure that DIR is added to your shell's PATH
  --no-verify                  Do not verify the SHA256SUMs of the downloaded release
  --ignore-attestation         Do not verify the GitHub attestation of the downloaded release
  --no-cargo-fallback          Require a prebuilt release artifact to proceed with installation
  --no-init                    Skip automatic initialization after midenup is installed

Environment Variables:

These should be set in your shell if you want them to be persistent.

  MIDENUP_HOME       Customize where midenup will store its state and installed toolchains
EOF
}

CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"

version=
install_path="${CARGO_HOME}/bin"
skip_sha256sum=0
skip_gh_attestation=0
skip_init=0
cargo_fallback=1

while [[ "$#" -gt 0 ]];
do
    arg="$1"
    case "$arg" in
        -h|--help)
            usage
            exit 0
            ;;
        -v|--version)
            shift
            version="$1"
            ;;
        --install-path)
            shift
            install_path="$1"
            ;;
        --no-verify)
            skip_sha256sum=1
            ;;
        --ignore-attestation)
            skip_gh_attestation=1
            ;;
        --no-cargo-fallback)
            cargo_fallback=0
            ;;
        --no-init)
            skip_init=1
            ;;
        --)
            shift
            break
            ;;
    esac
    shift
done

do_curl() {
    curl --retry 10 -A "Mozilla/5.0 (X11; Linux x86_64; rv:60.0) Gecko/20100101 Firefox/81.0" -L --proto '=https' --tlsv1.2 -sSf "$@"
}

do_verify_sha256sums() {
    echo "Verifying SHA256SUMS.."
    sha256sum --check --ignore-missing "$@"
}

do_verify_attestation() {
    echo "Verifying GitHub attestation.."
    gh attestation verify "$@" --repo 0xMiden/midenup --signer-workflow 0xMiden/midenup/.github/workflows/release.yml
}

install_via_cargo() {
    case "${0:-}" in
        "")  cargo install --force --locked --bin midenup --no-track midenup ;;
        *)  cargo install --force --locked --bin midenup --no-track --version "$0" midenup;;
    esac
}

if [ ! -d "${install_path}" ]; then
    echo "Expected ${install_path} to be a directory"
    exit 2
fi

case "${version:-}" in
    "") ;; # unset
    v*) version_tag="$version" ;; # already includes the `v`
    *) version_tag="v${version}" ;; # Add a leading `v`
esac

# Download in a temporary directory
cd "$(mktemp -d)"

# If a version is specified, download from the latest release, otherwise download from the given
# release version.
#
# NOTE: The location of /download in the URLs are intentionally different
if [ -z "${version_tag:-}" ]; then
    base_url="https://github.com/0xMiden/midenup/releases/latest/download"
else
    base_url="https://github.com/0xMiden/midenup/releases/download/${version_tag}"
fi

if ! command -v rustc >/dev/null 2>&1; then
    echo "Expected Rust to be installed, and available in your PATH"
    exit 2
fi

arch="$(rustc --print host-tuple)"
file="midenup-${arch}.tar.gz"
url="${base_url}/${file}"
midenup_src=midenup
case "$arch" in
    aarch64-apple-darwin|x86_64-unknown-linux-gnu)
        do_curl -O "$url"
        tar -xzvf "${file}"
        # Verify sha256 hash of downloaded artifact, unless requested otherwise
        if [[ "${skip_sha256sum:-0}" -eq 0 ]] && command -v sha256sum >/dev/null 2>&1; then
            do_curl -O "${base_url}/SHA256SUMS"
            do_verify_sha256sums SHA256SUMS
            rm SHA256SUMS
        fi

        # Verify GitHub attestation, unless requested otherwise
        if [[ "${skip_gh_attestation:-0}" -eq 0 ]] && command -v gh >/dev/null 2>&1; then
            do_verify_attestation "$file"
        fi

        # Clean up downloaded archive
        rm "${file}"
        ;;
    *)
        if [[ "${cargo_fallback:-1}" -eq 0 ]]; then
            echo "Unsupported architecture, and --no-cargo-fallback was set: '${arch}'"
            exit 1
        fi
        install_via_cargo "$version"
        midenup_src="${CARGO_HOME}/bin/midenup"
        ;;
esac

# Install midenup to installation path
mv -f "${midenup_src}" "${install_path}/"

# Initialize midenup, unless requested otherwise
if [[ "${skip_init}" -eq 0 ]]; then
    "${install_path}/midenup" init
fi

# Finalize
case ":$PATH:" in
    *":${install_path}:"*) ;; # Cargo home is already in path
    *) needs_install_path=1 ;;
esac

if [ -n "${needs_install_path:-}" ]; then
    if [ -n "${CI:-}" ] && [ -n "${GITHUB_PATH:-}" ]; then
        echo "${install_path}" >> "$GITHUB_PATH"
    else
        echo
        printf "\033[0;31mYour path is missing %s, you might want to add it.\033[0m\n" "${install_path}"
        echo
    fi
fi
