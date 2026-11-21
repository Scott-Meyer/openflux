#!/usr/bin/env bash
# Cross-builds OpenFlux v2 keyboard artifacts (firmware .uf2, bridge .ipk, display .ipk)
# into .work/v2-keyboard-builds/ so OpenFlux Studio's Updates -> Local Builds (or openflux_dev.py)
# can flash/sideload them straight to a connected Flux keyboard.
#
# Usage:
#   scripts/v2-build-keyboard.sh [all|firmware|bridge|display]

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$REPO/.work/v2-keyboard-builds"
TARGET="${1:-all}"
mkdir -p "$OUT"

build_firmware() {
  echo "==> Building RP2040 v2 firmware (.uf2) via Docker..."
  docker build -t openflux-fw-builder -f "$REPO/keyboard/firmware/Dockerfile.build" "$REPO/keyboard/firmware"
  docker run --rm -v "$REPO:/work" -w /work/keyboard/firmware openflux-fw-builder \
    bash -lc 'cmake -B build-pico -DPICO_SDK_PATH=/opt/pico-sdk && cmake --build build-pico -j"$(nproc)"'
  cp "$REPO/keyboard/firmware/build-pico/"*.uf2 "$OUT/"
  echo "  -> Staged firmware in $OUT"
}

build_bridge() {
  echo "==> Building ARM64 v2 bridge (.ipk) via Docker..."
  docker build -t openflux-bridge-builder -f "$REPO/keyboard/bridge/Dockerfile.build" "$REPO/keyboard/bridge"
  docker run --rm -v "$REPO:/work" -w /work openflux-bridge-builder \
    bash -lc 'cargo build --release --target aarch64-unknown-linux-musl -p openflux-bridge && python3 scripts/build_ipk.py bridge --bin target/aarch64-unknown-linux-musl/release/openflux-bridge --out .work/v2-keyboard-builds'
  echo "  -> Staged bridge in $OUT"
}

build_display() {
  echo "==> Building ARM64 v2 display (.ipk) via Docker..."
  docker build -t openflux-display-builder -f "$REPO/keyboard/display/Dockerfile.build" "$REPO/keyboard/display"
  docker run --rm -v "$REPO:/work" -w /work openflux-display-builder \
    bash -lc 'cargo build --release --target aarch64-unknown-linux-gnu -p openflux-display && python3 scripts/build_ipk.py display --bin target/aarch64-unknown-linux-gnu/release/openflux-display --out .work/v2-keyboard-builds'
  echo "  -> Staged display in $OUT"
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
