#!/bin/bash
set -euo pipefail

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$DIR/.."

expected_packages=(midenup)
release=false
case "${1:-}" in
    '') ;;
    --release) release=true ;;
    *) echo 'Usage: check-release.sh [--release]' >&2; exit 2 ;;
esac
[[ $# -le 1 ]] || { echo 'Usage: check-release.sh [--release]' >&2; exit 2; }

fail() {
    echo "ERROR: $*" >&2
    exit 1
}

crates_io_get() {
    curl -sS -L --retry 3 --retry-max-time 60 --connect-timeout 10 --max-time 60 \
        -A "${expected_packages[0]}-release" -o "$2" -w '%{http_code}' \
        "https://crates.io/api/v1/$1"
}

if grep -Eq "^[[:space:]]*\[[[:space:]]*['\"]?(patch|replace)['\"]?([[:space:]]*\.|[[:space:]]*\])" Cargo.toml; then
    fail 'Remove Cargo patches and replacements before publication.'
fi

metadata_json="$(cargo metadata --locked --no-deps --format-version 1)"
members="$(jq -c '
    . as $m | [.packages[] | select(.id as $id | $m.workspace_members | index($id))]
' <<< "$metadata_json")"
public="$(jq -r '.[] | select(.publish != []) | .name' <<< "$members" | sort)"
expected="$(printf '%s\n' "${expected_packages[@]}" | sort)"
[[ "$public" == "$expected" ]] || fail "Publishable crates must be: $expected. Found: $public."

version="$(awk '
    /^\[workspace.package\]/ { in_workspace_package = 1; next }
    /^\[/ { in_workspace_package = 0 }
    in_workspace_package && /^version[[:space:]]*=/ {
        gsub(/"/, "", $3)
        print $3
        exit
    }
' Cargo.toml)"
[[ -n "$version" ]] || fail 'Could not read workspace.package.version.'
jq -e --arg version "$version" '
    all(.[] | select(.publish != []); .version == $version)
' <<< "$members" >/dev/null || fail 'Publishable crate versions must match the workspace version.'

subject="$(git rev-parse HEAD)"
tag="v$version"
if [[ "$release" == true ]] && git show-ref --verify --quiet "refs/tags/$tag"; then
    tagged="$(git rev-parse "$tag^{commit}")"
    [[ "$tagged" == "$subject" ]] || fail "$tag already points to $tagged; cut a new version."
fi

release_tmpdir="$(mktemp -d)"
trap 'rm -rf "$release_tmpdir"' EXIT
excludes=''
while IFS= read -r package; do
    excludes="${excludes:+$excludes }--exclude $package"
done < <(jq -r '.[] | select(.publish == []) | .name' <<< "$members")

pending=false
while IFS= read -r package; do
    status="$(crates_io_get "crates/$package/$version" "$release_tmpdir/version.json")"
    case "$status" in
        200) ;;
        404) pending=true; continue ;;
        *) fail "crates.io returned HTTP $status for $package@$version." ;;
    esac
    jq -e '.version | type == "object"' "$release_tmpdir/version.json" >/dev/null ||
        fail "Invalid registry metadata for $package@$version."
    [[ "$release" == true ]] || continue
    jq -e '.version.yanked == false' "$release_tmpdir/version.json" >/dev/null ||
        fail "$package@$version is yanked or has invalid registry metadata; cut a new version."
    status="$(crates_io_get "crates/$package/$version/download" "$release_tmpdir/package.crate")"
    [[ "$status" == 200 ]] || fail "Could not download $package@$version: HTTP $status."
    checksum="$(jq -er '.version.checksum' "$release_tmpdir/version.json")"
    actual_checksum="$(sha256sum "$release_tmpdir/package.crate")"
    [[ "${actual_checksum%% *}" == "$checksum" ]] || fail "Checksum mismatch for $package@$version."
    if ! tar -xOzf "$release_tmpdir/package.crate" "$package-$version/.cargo_vcs_info.json" |
        jq -e --arg subject "$subject" '.git.sha1 == $subject and (.git.dirty // false) == false' >/dev/null; then
        fail "$package@$version was published from different or unverifiable source; cut a new version."
    fi
    excludes="${excludes:+$excludes }--exclude $package"
done <<< "$public"

prerelease=false
[[ "${version%%+*}" != *-* ]] || prerelease=true
result="$(jq -n --arg version "$version" --arg tag "$tag" \
    --arg prerelease "$prerelease" --arg pending "$pending" --arg excludes "$excludes" '$ARGS.named')"
printf '%s\n' "$result"
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    jq -r 'to_entries[] | "\(.key)=\(.value)"' <<< "$result" >> "$GITHUB_OUTPUT"
fi
