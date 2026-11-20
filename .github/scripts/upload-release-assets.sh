#!/usr/bin/env bash
# Uploads files to a GitHub release without ever replacing a published file.
#
# Release assets are immutable. For each file:
#   - no asset of that name on the release: it is uploaded
#   - an asset with the same bytes: it is skipped
#   - an asset with different bytes: the upload fails with
#     "published artifacts are immutable; cut a new version"
# Every file is checked before anything is uploaded, so a conflict leaves the
# release unchanged. Files are uploaded in the order given; callers list the
# file that marks a finished upload (artifacts.txt, release.json) last.
#
# Usage: upload-release-assets.sh <tag> <file>...
#
# Environment: GH_TOKEN, and GITHUB_REPOSITORY (owner/name).

set -euo pipefail

die() { echo "upload-release-assets: $*" >&2; exit 1; }

tag="${1:?usage: upload-release-assets.sh <tag> <file>...}"
shift
[ $# -gt 0 ] || die "no files given"
repo="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

existing="$(gh release view "$tag" --repo "$repo" --json assets --jq '.assets[].name')" ||
    die "${tag}: release not found"

sha256_of() { sha256sum "$1" | cut -d' ' -f1; }

to_upload=()
conflicts=()
for f in "$@"; do
    [ -f "$f" ] || die "${f}: not a file"
    name="$(basename "$f")"
    if grep -qxF "$name" <<<"$existing"; then
        mkdir -p "${work}/published"
        rm -f "${work}/published/${name}"
        gh release download "$tag" --repo "$repo" --pattern "$name" --dir "${work}/published" ||
            die "${tag}: could not download the published ${name} to compare it"
        if [ "$(sha256_of "$f")" = "$(sha256_of "${work}/published/${name}")" ]; then
            echo "${tag}: ${name} is already published with the same contents; skipped"
        else
            conflicts+=("$name")
        fi
    else
        to_upload+=("$f")
    fi
done

if [ "${#conflicts[@]}" -gt 0 ]; then
    for name in "${conflicts[@]}"; do
        echo "${tag}: ${name} is already published with different contents" >&2
    done
    die "${tag}: published artifacts are immutable; cut a new version"
fi

for f in "${to_upload[@]}"; do
    gh release upload "$tag" --repo "$repo" "$f"
    echo "${tag}: uploaded $(basename "$f")"
done
