#!/usr/bin/env bash
# Signs newly built releases staged by the build workflows and prepares their
# release manifests and GitHub Pages files for `build-index.sh`.
#
# Each staged release lives in its own subdirectory under --staged containing:
#   meta.json      {component, version, channel, tag, source_date_epoch, notes_url}
#   compat.toml    compat.toml from the release tag
#   artifacts/     release artifacts listed in index.json
#   source/        optional compliance/source files copied to <site>/source/
#   updater/       optional Tauri updater archive (handled by tauri-feed.py)
#
# Keyboard artifacts (firmware, bridge, display) are copied into
# <site>/packages/<component>/<version>/ and indexed at that Pages URL.
# Host installers live on the GitHub release (<server>/<repo>/releases/download/<tag>).
#
# Usage:
#   stage-releases.sh --tool <openflux-release> --staged <dir>
#       --pages-url <url> --active-pubkey <file>
#       --releases-out <dir> --site-out <dir>
#
# Environment: OPENFLUX_RELEASE_KEY (and OPENFLUX_RELEASE_KEY_PASSWORD when set),
# GITHUB_REPOSITORY, and optional GITHUB_SERVER_URL.

set -euo pipefail

die() { echo "stage-releases: $*" >&2; exit 1; }

tool="" staged="" pages_url="" active_pub="" releases_out="" site_out=""
while [ $# -gt 0 ]; do
    case "$1" in
        --tool) tool="$2"; shift 2 ;;
        --staged) staged="$2"; shift 2 ;;
        --pages-url) pages_url="${2%/}"; shift 2 ;;
        --active-pubkey) active_pub="$2"; shift 2 ;;
        --releases-out) releases_out="$2"; shift 2 ;;
        --site-out) site_out="$2"; shift 2 ;;
        *) die "unknown argument: $1" ;;
    esac
done
for v in tool staged pages_url active_pub releases_out site_out; do
    [ -n "${!v}" ] || die "--${v//_/-} is required"
done
[ -f "$active_pub" ] || die "${active_pub}: not found"
mkdir -p "$releases_out" "$site_out"

server="${GITHUB_SERVER_URL:-https://github.com}"
repo="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
sign_args=(--key-env OPENFLUX_RELEASE_KEY)
[ -n "${OPENFLUX_RELEASE_KEY_PASSWORD:-}" ] && sign_args+=(--password-env OPENFLUX_RELEASE_KEY_PASSWORD)

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

count=0
if [ -d "$staged" ]; then
    while IFS= read -r meta; do
        dir="$(dirname "$meta")"
        component="$(jq -er '.component' "$meta")"
        version="$(jq -er '.version' "$meta")"
        channel="$(jq -er '.channel' "$meta")"
        tag="$(jq -er '.tag' "$meta")"
        epoch="$(jq -er '.source_date_epoch' "$meta")"
        notes_url="$(jq -er '.notes_url' "$meta")"
        compat="${dir}/compat.toml"
        [ -f "$compat" ] || die "${tag}: missing compat.toml"
        [ -d "${dir}/artifacts" ] || die "${tag}: missing artifacts/"

        echo "${tag}: signing"
        sign_dir="${work}/${tag}"
        mkdir -p "$sign_dir"
        args=()
        while IFS= read -r f; do
            name="$(basename "$f")"
            cp "$f" "${sign_dir}/${name}"
            "$tool" sign "${sign_args[@]}" "${sign_dir}/${name}"
            "$tool" verify --pubkey "$active_pub" "${sign_dir}/${name}" > /dev/null
            args+=(--artifact "${sign_dir}/${name}")
            if [ "$component" != host ]; then
                mkdir -p "${site_out}/packages/${component}/${version}"
                cp "$f" "${site_out}/packages/${component}/${version}/${name}"
            fi
        done < <(find "${dir}/artifacts" -maxdepth 1 -type f | LC_ALL=C sort)
        [ "${#args[@]}" -gt 0 ] || die "${tag}: no artifacts found"

        if [ "$component" = host ]; then
            base_url="${server}/${repo}/releases/download/${tag}"
        else
            base_url="${pages_url}/packages/${component}/${version}"
        fi

        SOURCE_DATE_EPOCH="$epoch" \
            "$tool" manifest --component "$component" --version "$version" --channel "$channel" \
            --compat "$compat" "${args[@]}" \
            --base-url "$base_url" --notes-url "$notes_url" \
            -o "${releases_out}/${tag}.json"

        if [ -d "${dir}/source" ]; then
            while IFS= read -r sf; do
                mkdir -p "${site_out}/source"
                cp "$sf" "${site_out}/source/$(basename "$sf")"
            done < <(find "${dir}/source" -maxdepth 1 -type f | LC_ALL=C sort)
        fi
        count=$((count + 1))
    done < <(find "$staged" -name meta.json | LC_ALL=C sort)
fi

echo "stage-releases: staged ${count} release(s)"
