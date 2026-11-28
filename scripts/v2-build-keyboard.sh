#!/usr/bin/env bash
# Cross-builds Keyglass v2 keyboard artifacts (firmware .uf2, bridge .ipk, display .ipk)
# natively in ARM64 Docker using the repo's exact Dockerfiles and Makefiles, and stages
# them in .work/v2-keyboard-builds/ for instant sideload/flash to a connected keyboard.
#
# Usage:
#   scripts/v2-build-keyboard.sh [all|firmware|bridge|display]

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$REPO/.work/v2-keyboard-builds"
TARGET="${1:-all}"
VERSION="${KEYGLASS_V2_VERSION:-2.1.0}"
mkdir -p "$OUT"

build_firmware() {
  echo "==> [firmware] Building RP2040 v2 firmware (.uf2) via Docker..."
  docker build -t keyglass-fw-builder -f "$REPO/keyboard/firmware/Dockerfile.build" "$REPO/keyboard/firmware"
  docker run --rm -v "$REPO:/work" -w /work/keyboard/firmware keyglass-fw-builder \
    bash -lc "cmake -B build-pico -DPICO_SDK_PATH=/opt/pico-sdk -DKEYGLASS_VERSION=${VERSION} && cmake --build build-pico -j\$(nproc)"
  cp "$REPO/keyboard/firmware/build-pico/"*.uf2 "$OUT/keyglass-firmware-${VERSION}.uf2"
  echo "  -> $OUT/keyglass-firmware-${VERSION}.uf2"
}

build_bridge() {
  echo "==> [bridge] Building ARM64 v2 bridge (.ipk) via Docker..."
  docker build -t keyglass-bridge-builder -f "$REPO/keyboard/bridge/Dockerfile.build" "$REPO/keyboard/bridge"
  docker run --rm -v "$REPO:/work" -w /work/keyboard/bridge keyglass-bridge-builder \
    make ipk VERSION="$VERSION" TARGET_DIR=/work/target-linux OUT_DIR=/work/.work/v2-keyboard-builds RUST_LICENSES="true"
  echo "  -> $OUT/keyglass-bridge_${VERSION}_aarch64.ipk"
}

build_display() {
  echo "==> [display] Building ARM64 v2 display (.ipk) via Docker..."
  docker build -t keyglass-display-builder -f "$REPO/keyboard/display/Dockerfile.build" "$REPO/keyboard/display"
  docker run --rm -v "$REPO:/work" -w /work/keyboard/display keyglass-display-builder \
    make ipk VERSION="$VERSION" TARGET_DIR=/work/target-linux OUT_DIR=/work/.work/v2-keyboard-builds RUST_LICENSES="true"
  echo "  -> $OUT/keyglass-display_${VERSION}_aarch64.ipk"
}

case "$TARGET" in
  firmware) build_firmware ;;
  bridge)   build_bridge ;;
  display)  build_display ;;
  all)
    build_firmware
    build_bridge
    build_display
    ;;
  *)
    echo "Usage: $0 [all|firmware|bridge|display]" >&2
    exit 1
    ;;
esac

echo "==> Local v2 keyboard builds ready in: $OUT"
ls -lh "$OUT"
