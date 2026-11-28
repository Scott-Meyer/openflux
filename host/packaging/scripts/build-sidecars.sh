#!/usr/bin/env bash
# Builds the executables Keyglass Studio bundles next to its main binary and
# names them the way Tauri's bundle.externalBin expects:
# <name>-<target triple>[.exe].
#
#   keyglassd          daemon (Rust, package keyglassd); on Windows it also
#                      provides the tray icon, and on Linux it shows a zenity
#                      notification icon when zenity is installed
#   keyglass-tray      macOS menu bar helper (Swift, host/macos-helpers)
#   keyglass-sensors   macOS system sensors helper (Swift, host/macos-helpers)
#   keyglass-ffmpeg    media import tool (build-ffmpeg.sh)
#   keyglass-ffprobe   media duration probe, from the same FFmpeg build
#
# It also writes <out>/licenses/, installed with the app:
#   keyglass-ffmpeg-*                 the FFmpeg tools' license and build
#                                     information
#   keyglassd-rust-licenses.txt       license texts of the Rust crates in
#   keyglass-studio-rust-licenses.txt keyglassd and Keyglass Studio, for the
#                                     target (scripts/licenses/rust-licenses.sh)
#   keyglass-studio-ui-npm-licenses.txt
#                                     license texts of the npm packages in
#                                     Studio's interface (npm-licenses.mjs)
# The macOS helpers are Swift programs using only system frameworks.
#
# Usage:
#   build-sidecars.sh --target <triple> --ffmpeg-dir <dir> --out <dir> [--development-keys]
#
# --target is a Rust target triple, or universal-apple-darwin for a macOS
# build containing arm64 and x86_64. --ffmpeg-dir holds <triple>/ from
# build-ffmpeg.sh (keyglass-ffmpeg[.exe], keyglass-ffprobe[.exe], the
# license and build information) for each architecture being built.
#
# keyglassd is built with the keyglass-update `production-keys` feature,
# which refuses to build if the embedded trust anchors are development keys.
# --development-keys leaves the feature off, for packaging tests only.
#
# Needs cargo-about (scripts/licenses/install-cargo-about.sh), Node.js and
# host/studio/node_modules (npm ci). Run from anywhere inside the repository.
# Cargo builds use --locked.

set -euo pipefail

die() { echo "build-sidecars: $*" >&2; exit 1; }

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
target="" ffmpeg_dir="" out="" features=(--features keyglass-update/production-keys)
while [ $# -gt 0 ]; do
    case "$1" in
        --target) target="$2"; shift 2 ;;
        --ffmpeg-dir) ffmpeg_dir="$2"; shift 2 ;;
        --out) out="$2"; shift 2 ;;
        --development-keys) features=(); shift ;;
        *) die "unknown argument: $1" ;;
    esac
done
[ -n "$target" ] || die "--target is required"
[ -n "$ffmpeg_dir" ] || die "--ffmpeg-dir is required"
[ -n "$out" ] || die "--out is required"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
ffmpeg_dir="$(cd "$ffmpeg_dir" && pwd)"
target_dir="${CARGO_TARGET_DIR:-${repo}/target}"

# package:binary pairs built with cargo.
rust_bins=(keyglassd:keyglassd fluxctl:fluxctl)

cargo_build() {
    local triple="$1"; shift
    local args=()
    for pair in "$@"; do
        args+=(-p "${pair%%:*}" --bin "${pair##*:}")
    done
    (cd "$repo" && cargo build --release --locked --target "$triple" "${features[@]}" "${args[@]}")
}

# copy_media_tool <keyglass-ffmpeg|keyglass-ffprobe> <triple> <dest>
copy_media_tool() {
    local name="$1" triple="$2" dest="$3" src
    src="${ffmpeg_dir}/${triple}/${name}"
    [ -f "$src" ] || src="${src}.exe"
    [ -f "$src" ] || die "no ${name} for ${triple} in ${ffmpeg_dir}/${triple}"
    cp "$src" "$dest"
    chmod 0755 "$dest"
}

# The license and build information of the FFmpeg tools, from the first
# architecture built.
copy_media_licenses() {
    local triple="$1" dir="${out}/licenses" f
    mkdir -p "$dir"
    f="${ffmpeg_dir}/${triple}/keyglass-ffmpeg-COPYING.GPLv2.txt"
    [ -f "$f" ] || die "no keyglass-ffmpeg-COPYING.GPLv2.txt for ${triple} in ${ffmpeg_dir}/${triple}"
    cp "$f" "$dir/"
    for f in "${ffmpeg_dir}"/*/keyglass-ffmpeg-*.buildinfo.txt; do
        [ -f "$f" ] && cp "$f" "$dir/"
    done
    compgen -G "${dir}/keyglass-ffmpeg-*.buildinfo.txt" > /dev/null || die "no keyglass-ffmpeg build information in ${ffmpeg_dir}"
}

# The license texts of the third-party code in the programs built for the
# given target triples.
write_dependency_licenses() {
    local dir="${out}/licenses" triples=() t
    for t in "$@"; do triples+=(--target "$t"); done
    mkdir -p "$dir"
    "${repo}/scripts/licenses/rust-licenses.sh" --manifest-path "${repo}/host/daemon/Cargo.toml" \
        "${triples[@]}" -o "${dir}/keyglassd-rust-licenses.txt"
    "${repo}/scripts/licenses/rust-licenses.sh" --manifest-path "${repo}/host/studio/src-tauri/Cargo.toml" \
        "${triples[@]}" -o "${dir}/keyglass-studio-rust-licenses.txt"
    # vite: the production build includes its module preload polyfill.
    node "${repo}/scripts/licenses/npm-licenses.mjs" --package-dir "${repo}/host/studio" --bundled vite \
        -o "${dir}/keyglass-studio-ui-npm-licenses.txt"
}

case "$target" in
    universal-apple-darwin|aarch64-apple-darwin|x86_64-apple-darwin)
        if [ "$target" = universal-apple-darwin ]; then
            arches=(aarch64 x86_64)
        else
            arches=("${target%%-*}")
        fi
        helpers_build="${repo}/target/macos-helpers"
        for arch in "${arches[@]}"; do
            triple="${arch}-apple-darwin"
            cargo_build "$triple" "${rust_bins[@]}"
            swift_arch="${arch/aarch64/arm64}"
            make -C "${repo}/host/macos-helpers" \
                BUILD_DIR="${helpers_build}/${triple}" \
                SWIFTC="swiftc -target ${swift_arch}-apple-macos13.0"
        done
        for name in keyglassd fluxctl keyglass-tray keyglass-sensors keyglass-ffmpeg keyglass-ffprobe; do
            inputs=()
            for arch in "${arches[@]}"; do
                triple="${arch}-apple-darwin"
                case "$name" in
                    keyglassd|fluxctl) inputs+=("${target_dir}/${triple}/release/${name}") ;;
                    keyglass-tray|keyglass-sensors) inputs+=("${helpers_build}/${triple}/${name}") ;;
                    keyglass-ffmpeg|keyglass-ffprobe)
                        copy_media_tool "$name" "$triple" "${out}/.${name}-${triple}"
                        inputs+=("${out}/.${name}-${triple}") ;;
                esac
            done
            lipo -create -output "${out}/${name}-${target}" "${inputs[@]}"
            chmod 0755 "${out}/${name}-${target}"
            if [ "$target" = universal-apple-darwin ]; then
                # A universal Tauri build compiles each architecture on its own, and each
                # pass looks for the sidecars under its own triple.
                for i in "${!arches[@]}"; do
                    cp "${inputs[$i]}" "${out}/${name}-${arches[$i]}-apple-darwin"
                    chmod 0755 "${out}/${name}-${arches[$i]}-apple-darwin"
                done
            fi
        done
        rm -f "${out}"/.keyglass-ff*
        copy_media_licenses "${arches[0]}-apple-darwin"
        triples=()
        for arch in "${arches[@]}"; do triples+=("${arch}-apple-darwin"); done
        write_dependency_licenses "${triples[@]}"
        ;;
    *-pc-windows-msvc)
        cargo_build "$target" "${rust_bins[@]}"
        cp "${target_dir}/${target}/release/keyglassd.exe" "${out}/keyglassd-${target}.exe"
        cp "${target_dir}/${target}/release/fluxctl.exe" "${out}/fluxctl-${target}.exe"
        copy_media_tool keyglass-ffmpeg "$target" "${out}/keyglass-ffmpeg-${target}.exe"
        copy_media_tool keyglass-ffprobe "$target" "${out}/keyglass-ffprobe-${target}.exe"
        copy_media_licenses "$target"
        write_dependency_licenses "$target"
        ;;
    *-unknown-linux-gnu)
        cargo_build "$target" "${rust_bins[@]}"
        cp "${target_dir}/${target}/release/keyglassd" "${out}/keyglassd-${target}"
        cp "${target_dir}/${target}/release/fluxctl" "${out}/fluxctl-${target}"
        copy_media_tool keyglass-ffmpeg "$target" "${out}/keyglass-ffmpeg-${target}"
        copy_media_tool keyglass-ffprobe "$target" "${out}/keyglass-ffprobe-${target}"
        copy_media_licenses "$target"
        write_dependency_licenses "$target"
        ;;
    *)
        die "unsupported target: ${target}"
        ;;
esac

ls -lR "$out"
