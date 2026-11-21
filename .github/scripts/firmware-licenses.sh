#!/usr/bin/env bash
# Writes openflux-firmware-licenses.txt: the license texts of the littlefs,
# Raspberry Pi Pico SDK and TinyUSB code the firmware image links, read from
# the exact sources in the firmware build image (keyboard/firmware/
# Dockerfile.build). Fails if any of them is missing or empty.
#
# Usage: firmware-licenses.sh <build image> <output file>

set -euo pipefail

die() { echo "firmware-licenses: $*" >&2; exit 1; }

image="${1:?usage: firmware-licenses.sh <build image> <output file>}"
out="${2:?usage: firmware-licenses.sh <build image> <output file>}"
mkdir -p "$(dirname "$out")"
text="$(mktemp)"
trap 'rm -f "$text"' EXIT
: > "$out"
for entry in "littlefs:/opt/littlefs/LICENSE.md" \
    "Raspberry Pi Pico SDK:/opt/pico-sdk/LICENSE.TXT" \
    "TinyUSB:/opt/pico-sdk/lib/tinyusb/LICENSE"; do
    name="${entry%%:*}" path="${entry#*:}"
    docker run --rm --network none "$image" cat "$path" > "$text" || die "${name}: cannot read ${path} in ${image}"
    [ -s "$text" ] || die "${name}: ${path} is empty in ${image}"
    { printf '%s\n\n' "$name"; cat "$text"; printf '\n\n'; } >> "$out"
done
echo "firmware-licenses: wrote ${out}"
