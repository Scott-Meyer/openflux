#!/usr/bin/env bash
# Checks that a host installer contains what every installation needs:
# OpenFlux Studio, openfluxd, openflux-ffmpeg and openflux-ffprobe, the
# third-party license texts (licenses/: the FFmpeg tools' license, and the
# licenses of the Rust crates in openfluxd and Studio and of the npm packages
# in Studio's interface; each must be non-empty), and the bundled keyboard
# software with its signed index. On macOS it also checks the helpers and the LaunchAgent, and that
# every bundled executable is universal (arm64 and x86_64).
#
# Usage: check-host-installer.sh <installer>
#
#   *.dmg            macOS disk image (needs hdiutil, lipo)
#   *.app.tar.gz     macOS update archive
#   *-setup.exe      Windows NSIS setup program (needs 7z)
#   *.deb            Debian package (needs dpkg-deb)
#   *.AppImage       AppImage (extracted with --appimage-extract)
#
# Where the platform allows it, the bundled FFmpeg tools are also run once
# with -version.

set -euo pipefail

die() { echo "check-host-installer: $*" >&2; exit 1; }

installer="${1:?usage: check-host-installer.sh <installer>}"
[ -f "$installer" ] || die "${installer}: not found"
installer="$(cd "$(dirname "$installer")" && pwd)/$(basename "$installer")"
work="$(mktemp -d)"
mounted=""
cleanup() {
    if [ -n "$mounted" ]; then hdiutil detach -quiet "$mounted" || true; fi
    rm -rf "$work"
}
trap cleanup EXIT

missing=()
# require <listing file> <description> <extended regex matched against each path>
require() {
    if grep -Eq -- "$3" "$1"; then
        echo "  ${2}: present"
    else
        missing+=("$2")
    fi
}

# The files every installer's licenses/ directory holds (build-sidecars.sh).
license_files=(
    openflux-ffmpeg-COPYING.GPLv2.txt
    openfluxd-rust-licenses.txt
    openflux-studio-rust-licenses.txt
    openflux-studio-ui-npm-licenses.txt
)
# require_licenses <listing of non-empty files> <extended regex for the directory holding licenses/>
require_licenses() {
    local f
    for f in "${license_files[@]}"; do
        require "$1" "licenses/${f} (non-empty)" "${2}licenses/${f//./\\.}$"
    done
}
# nonempty_files <dir>: the non-empty files under <dir>, relative to it.
nonempty_files() {
    (cd "$1" && find . -type f -size +0 -print) | sed 's|^\./||'
}

check_macos_tree() {
    local root="$1" app
    app="$(find "$root" -maxdepth 2 -name '*.app' -type d | head -n 1)"
    [ -n "$app" ] || die "no .app bundle in ${installer}"
    (cd "$app" && find . -print) | sed 's|^\./||' > "${work}/list"
    nonempty_files "$app" > "${work}/nonempty"
    for bin in openfluxd openflux-tray openflux-sensors openflux-ffmpeg openflux-ffprobe; do
        require "${work}/list" "$bin" "^Contents/MacOS/${bin}$"
    done
    require "${work}/list" "LaunchAgent" '^Contents/Library/LaunchAgents/org\.openflux\.openfluxd\.plist$'
    require_licenses "${work}/nonempty" '^Contents/Resources/'
    require "${work}/list" "keyboard index" '^Contents/Resources/keyboard/index\.json$'
    require "${work}/list" "keyboard index signature" '^Contents/Resources/keyboard/index\.json\.sig$'
    [ "${#missing[@]}" -eq 0 ] || return 0
    if command -v lipo > /dev/null 2>&1; then
        for f in "$app"/Contents/MacOS/*; do
            archs="$(lipo -archs "$f")"
            if [[ " $archs " == *" arm64 "* && " $archs " == *" x86_64 "* ]]; then
                echo "  $(basename "$f"): ${archs}"
            else
                die "$(basename "$f") is not universal (${archs})"
            fi
        done
    fi
    "$app/Contents/MacOS/openflux-ffmpeg" -hide_banner -version | head -n 1
    "$app/Contents/MacOS/openflux-ffprobe" -hide_banner -version | head -n 1
}

check_linux_tree() {
    local root="$1"
    (cd "$root" && find . -print) | sed 's|^\./||' > "${work}/list"
    nonempty_files "$root" > "${work}/nonempty"
    for bin in openfluxd openflux-ffmpeg openflux-ffprobe; do
        require "${work}/list" "$bin" "^usr/bin/${bin}$"
    done
    require_licenses "${work}/nonempty" '/'
    require "${work}/list" "keyboard index" '/keyboard/index\.json$'
    require "${work}/list" "keyboard index signature" '/keyboard/index\.json\.sig$'
    if [ "${#missing[@]}" -eq 0 ] && [ "$(uname -s)" = Linux ]; then
        "$root/usr/bin/openflux-ffmpeg" -hide_banner -version | head -n 1
        "$root/usr/bin/openflux-ffprobe" -hide_banner -version | head -n 1
    fi
}

echo "check-host-installer: $(basename "$installer")"
case "$installer" in
    *.dmg)
        mounted="${work}/mnt"
        mkdir -p "$mounted"
        hdiutil attach -quiet -nobrowse -readonly -mountpoint "$mounted" "$installer"
        check_macos_tree "$mounted"
        ;;
    *.app.tar.gz)
        mkdir -p "${work}/app"
        tar -xzf "$installer" -C "${work}/app"
        check_macos_tree "${work}/app"
        ;;
    *-setup.exe)
        7z l -slt "$installer" > "${work}/7z"
        sed -n 's/^Path = //p' "${work}/7z" | tr '\134' '/' > "${work}/list"
        # Paths of the entries with a non-zero Size.
        awk '/^Path = / { path = substr($0, 8) } /^Size = / { if (substr($0, 8) + 0 > 0) print path }' "${work}/7z" |
            tr '\134' '/' > "${work}/nonempty"
        for bin in openflux-studio openfluxd openflux-ffmpeg openflux-ffprobe; do
            require "${work}/list" "${bin}.exe" "(^|/)${bin}\\.exe$"
        done
        require_licenses "${work}/nonempty" '(^|/)'
        require "${work}/list" "keyboard index" '(^|/)keyboard/index\.json$'
        require "${work}/list" "keyboard index signature" '(^|/)keyboard/index\.json\.sig$'
        ;;
    *.deb)
        mkdir -p "${work}/deb"
        dpkg-deb -x "$installer" "${work}/deb"
        check_linux_tree "${work}/deb"
        require "${work}/list" "udev rules" '^usr/lib/udev/rules\.d/70-openflux\.rules$'
        require "${work}/list" "systemd user unit" '^usr/lib/systemd/user/openfluxd\.service$'
        dpkg-deb -e "$installer" "${work}/control"
        for script in postinst prerm postrm; do
            if [ -f "${work}/control/${script}" ]; then echo "  ${script}: present"; else missing+=("$script"); fi
        done
        ;;
    *.AppImage)
        chmod +x "$installer"
        (cd "$work" && "$installer" --appimage-extract > /dev/null)
        check_linux_tree "${work}/squashfs-root"
        ;;
    *)
        die "unknown installer type: ${installer}"
        ;;
esac

if [ "${#missing[@]}" -gt 0 ]; then
    die "$(basename "$installer") is missing: ${missing[*]}"
fi
echo "check-host-installer: $(basename "$installer") ok"
