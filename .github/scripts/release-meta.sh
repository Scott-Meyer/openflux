#!/usr/bin/env bash
# Resolves the facts every release job needs from a release tag, checks that
# the sources at that tag carry the same version, and writes them to
# $GITHUB_OUTPUT (or stdout outside Actions).
#
# Usage: release-meta.sh <tag>      e.g. host-v1.2.0, bridge-v1.3.0-beta.2
#
# Outputs: component, version, channel (stable|beta), prerelease (true|false),
# source_date_epoch (commit time of the tag), pages_url, release_url,
# notes_url.
#
# Run from the repository root with the tag checked out.

set -euo pipefail

die() { echo "release-meta: $*" >&2; exit 1; }

tag="${1:?usage: release-meta.sh <tag>}"
if [[ ! "$tag" =~ ^(host|firmware|bridge|display)-v([0-9]+\.[0-9]+\.[0-9]+(-beta\.[0-9]+)?)$ ]]; then
    die "${tag} is not <host|firmware|bridge|display>-vX.Y.Z or -vX.Y.Z-beta.N"
fi
component="${BASH_REMATCH[1]}"
version="${BASH_REMATCH[2]}"
if [ -n "${BASH_REMATCH[3]}" ]; then channel=beta prerelease=true; else channel=stable prerelease=false; fi

toml_get() {
    python3 -c 'import sys, tomllib
d = tomllib.load(open(sys.argv[1], "rb"))
for k in sys.argv[2].split("."):
    d = d[k]
print(d)' "$1" "$2"
}

case "$component" in
    host) source_version="$(toml_get Cargo.toml workspace.package.version)" ;;
    bridge|display) source_version="$(toml_get "keyboard/${component}/Cargo.toml" package.version)" ;;
    firmware)
        source_version="$(sed -n 's/^set(OPENFLUX_FIRMWARE_VERSION "\([^"]*\)").*/\1/p' keyboard/firmware/CMakeLists.txt)"
        ;;
esac
[ "$source_version" = "$version" ] || die "${tag}: sources declare ${component} ${source_version:-<none>}"
compat_version="$(toml_get compat.toml "components.${component}.version")"
[ "$compat_version" = "$version" ] || die "${tag}: compat.toml declares ${component} ${compat_version}"

repo="${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
owner="${repo%%/*}"
pages_url="${OPENFLUX_PAGES_URL:-https://${owner,,}.github.io/${repo#*/}}"
server="${GITHUB_SERVER_URL:-https://github.com}"

{
    echo "component=${component}"
    echo "version=${version}"
    echo "channel=${channel}"
    echo "prerelease=${prerelease}"
    echo "source_date_epoch=$(git log -1 --format=%ct "${tag}^{commit}")"
    echo "pages_url=${pages_url%/}"
    echo "release_url=${server}/${repo}/releases/download/${tag}"
    echo "notes_url=${server}/${repo}/releases/tag/${tag}"
} | tee -a "${GITHUB_OUTPUT:-/dev/stdout}"
