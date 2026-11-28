#!/usr/bin/env bash
# Writes keyglass-firmware-licenses.txt: the license texts of the littlefs,
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
# Also include QMK / Vial GPL license text statically linked from third_party/vial-qmk
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
for qmk_lic in "${repo_root}/third_party/vial-qmk/LICENSE" "${repo_root}/third_party/vial-qmk/COPYING"; do
    if [ -s "$qmk_lic" ]; then
        { printf 'QMK / Vial Firmware (GPL-2.0-or-later / GPL-3.0-or-later)\n\n'; cat "$qmk_lic"; printf '\n\n'; } >> "$out"
        break
    fi
done
echo "firmware-licenses: wrote ${out}"
