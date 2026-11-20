#!/usr/bin/env bash
# Collects the release.json of every published component release into a
# directory for `openflux-release index`, finishing releases whose build has
# uploaded its files but which are not signed yet.
#
# A release build uploads its artifacts, SHA256SUMS and, last, artifacts.txt
# (the file names the index lists). Finishing a release here means: download
# the listed artifacts, check them against SHA256SUMS, sign each with the
# release key (writing <artifact>.sig) and verify the signature against the
# active release key, write release.json, which includes those signatures and
# the interfaces from compat.toml at the release tag, and upload the .sig
# files and then release.json to the release. Releases without artifacts.txt
# are being built, or their build failed, and are skipped.
#
# Uploads never replace a published file (upload-release-assets.sh). If an
# earlier run uploaded the .sig files but not release.json, those signatures
# are reused, so the release.json written here matches them.
#
# Usage: finalize-releases.sh <openflux-release> <releases-dir> <active-release.pub>
#
# Environment: GH_TOKEN, GITHUB_REPOSITORY, GITHUB_SERVER_URL, and the release
# key in OPENFLUX_RELEASE_KEY (with OPENFLUX_RELEASE_KEY_PASSWORD when the
# key has one). Run from a full clone (tags fetched).

set -euo pipefail

die() { echo "finalize-releases: $*" >&2; exit 1; }

usage="usage: finalize-releases.sh <openflux-release> <releases-dir> <active-release.pub>"
tool="${1:?$usage}"
out="${2:?$usage}"
active_pub="${3:?$usage}"
[ -f "$active_pub" ] || die "${active_pub}: not found"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mkdir -p "$out"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
server="${GITHUB_SERVER_URL:-https://github.com}"
sign_args=(--key-env OPENFLUX_RELEASE_KEY)
[ -n "${OPENFLUX_RELEASE_KEY_PASSWORD:-}" ] && sign_args+=(--password-env OPENFLUX_RELEASE_KEY_PASSWORD)

tags="$(gh release list --repo "$GITHUB_REPOSITORY" --exclude-drafts --limit 1000 --json tagName --jq '.[].tagName')"
for tag in $tags; do
    [[ "$tag" =~ ^(host|firmware|bridge|display)-v([0-9]+\.[0-9]+\.[0-9]+(-beta\.[0-9]+)?)$ ]] || continue
    component="${BASH_REMATCH[1]}" version="${BASH_REMATCH[2]}"
    channel=stable
    [ -n "${BASH_REMATCH[3]}" ] && channel=beta
    assets="$(gh release view "$tag" --repo "$GITHUB_REPOSITORY" --json assets --jq '.assets[].name')"

    if grep -qx 'release.json' <<<"$assets"; then
        rm -f "${out}/${tag}.json"
        gh release download "$tag" --repo "$GITHUB_REPOSITORY" --pattern release.json --output "${out}/${tag}.json"
        echo "${tag}: published"
        continue
    fi
    if ! grep -qx 'artifacts.txt' <<<"$assets"; then
        echo "${tag}: no artifacts.txt (build not finished); skipped"
        continue
    fi

    echo "${tag}: signing"
    dir="${work}/${tag}"
    mkdir -p "$dir"
    gh release download "$tag" --repo "$GITHUB_REPOSITORY" --pattern artifacts.txt --pattern SHA256SUMS --dir "$dir"
    mapfile -t names < "${dir}/artifacts.txt"
    [ "${#names[@]}" -gt 0 ] || die "${tag}: artifacts.txt is empty"
    patterns=()
    for n in "${names[@]}"; do patterns+=(--pattern "$n"); done
    gh release download "$tag" --repo "$GITHUB_REPOSITORY" "${patterns[@]}" --dir "$dir"
    for n in "${names[@]}"; do
        line="$(awk -v n="$n" '$2 == n || $2 == "*" n' "${dir}/SHA256SUMS")"
        [ -n "$line" ] || die "${tag}: ${n} is not in SHA256SUMS"
        printf '%s\n' "$line"
    done > "${dir}/artifacts.sha256"
    (cd "$dir" && sha256sum --check --strict artifacts.sha256)

    args=()
    sigs=()
    for n in "${names[@]}"; do
        if grep -qxF "${n}.sig" <<<"$assets"; then
            gh release download "$tag" --repo "$GITHUB_REPOSITORY" --pattern "${n}.sig" --dir "$dir"
            echo "${tag}: ${n}.sig is already published; reusing it"
        else
            "$tool" sign "${sign_args[@]}" "${dir}/${n}"
        fi
        "$tool" verify --pubkey "$active_pub" "${dir}/${n}"
        args+=(--artifact "${dir}/${n}")
        sigs+=("${dir}/${n}.sig")
    done
    git show "${tag}:compat.toml" > "${dir}/compat.toml"
    SOURCE_DATE_EPOCH="$(git log -1 --format=%ct "${tag}^{commit}")" \
        "$tool" manifest --component "$component" --version "$version" --channel "$channel" \
        --compat "${dir}/compat.toml" "${args[@]}" \
        --base-url "${server}/${GITHUB_REPOSITORY}/releases/download/${tag}" \
        --notes-url "${server}/${GITHUB_REPOSITORY}/releases/tag/${tag}" \
        -o "${dir}/release.json"
    # Signatures first, so a release.json on a release always has its .sig files.
    "${here}/upload-release-assets.sh" "$tag" "${sigs[@]}" "${dir}/release.json"
    cp "${dir}/release.json" "${out}/${tag}.json"
done

count="$(find "$out" -name '*.json' | wc -l)"
echo "finalize-releases: ${count} releases"
