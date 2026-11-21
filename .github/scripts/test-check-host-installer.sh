#!/usr/bin/env bash
# Tests check-host-installer.sh on small installers built from stand-in
# files: a macOS update archive, a .deb and an NSIS setup program, each
# complete, then without one license file and with an empty one. The
# stand-in FFmpeg tools are shell scripts, so the .deb is only fully checked
# on Linux and the macOS archive only where lipo is absent.
#
# Needs dpkg-deb, makensis and 7z (packages dpkg, nsis, p7zip-full). Run
# from the repository root:
#   .github/scripts/test-check-host-installer.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
check="${here}/check-host-installer.sh"
for tool in dpkg-deb makensis 7z; do
    command -v "$tool" > /dev/null || { echo "test-check-host-installer: ${tool} is required" >&2; exit 1; }
done
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
fails=0
fail() { echo "FAIL: $*" >&2; fails=$((fails + 1)); }

licenses=(openflux-ffmpeg-COPYING.GPLv2.txt openfluxd-rust-licenses.txt openflux-studio-rust-licenses.txt
    openflux-studio-ui-npm-licenses.txt)
tool_script='#!/bin/sh
echo "ffmpeg version test"'

# licenses_into <dir> <variant>: complete, missing (no Rust licenses of
# openfluxd) or empty (an empty npm license file).
licenses_into() {
    local dir="$1/licenses" f
    mkdir -p "$dir"
    for f in "${licenses[@]}"; do printf 'license text\n' > "${dir}/${f}"; done
    case "$2" in
        missing) rm "${dir}/openfluxd-rust-licenses.txt" ;;
        empty) : > "${dir}/openflux-studio-ui-npm-licenses.txt" ;;
    esac
}

app_archive() {
    local variant="$1" root="${work}/app-${1}" c b
    c="${root}/OpenFlux Studio.app/Contents"
    mkdir -p "${c}/MacOS" "${c}/Resources/keyboard" "${c}/Library/LaunchAgents"
    for b in openflux-studio openfluxd openflux-tray openflux-sensors openflux-ffmpeg openflux-ffprobe; do
        printf '%s\n' "$tool_script" > "${c}/MacOS/${b}"
        chmod 0755 "${c}/MacOS/${b}"
    done
    : > "${c}/Library/LaunchAgents/org.openflux.openfluxd.plist"
    printf '{}\n' > "${c}/Resources/keyboard/index.json"
    printf 'sig\n' > "${c}/Resources/keyboard/index.json.sig"
    licenses_into "${c}/Resources" "$variant"
    tar -czf "${work}/${variant}.app.tar.gz" -C "$root" "OpenFlux Studio.app"
}

deb() {
    local variant="$1" root="${work}/deb-${1}" b
    mkdir -p "${root}/DEBIAN" "${root}/usr/bin" "${root}/usr/lib/OpenFlux Studio/keyboard" \
        "${root}/usr/lib/udev/rules.d" "${root}/usr/lib/systemd/user"
    printf 'Package: openflux-studio\nVersion: 1.0.0\nArchitecture: amd64\nMaintainer: test\nDescription: test\n' \
        > "${root}/DEBIAN/control"
    for s in postinst prerm postrm; do printf '#!/bin/sh\n' > "${root}/DEBIAN/${s}"; chmod 0755 "${root}/DEBIAN/${s}"; done
    for b in openflux-studio openfluxd openflux-ffmpeg openflux-ffprobe; do
        printf '%s\n' "$tool_script" > "${root}/usr/bin/${b}"
        chmod 0755 "${root}/usr/bin/${b}"
    done
    printf '{}\n' > "${root}/usr/lib/OpenFlux Studio/keyboard/index.json"
    printf 'sig\n' > "${root}/usr/lib/OpenFlux Studio/keyboard/index.json.sig"
    : > "${root}/usr/lib/udev/rules.d/70-openflux.rules"
    : > "${root}/usr/lib/systemd/user/openfluxd.service"
    licenses_into "${root}/usr/lib/OpenFlux Studio" "$variant"
    dpkg-deb --root-owner-group --build "$root" "${work}/${variant}.deb" > /dev/null
}

nsis() {
    local variant="$1" root="${work}/nsis-${1}" b
    mkdir -p "${root}/keyboard"
    for b in openflux-studio openfluxd openflux-ffmpeg openflux-ffprobe; do printf 'MZ\n' > "${root}/${b}.exe"; done
    printf '{}\n' > "${root}/keyboard/index.json"
    printf 'sig\n' > "${root}/keyboard/index.json.sig"
    licenses_into "$root" "$variant"
    cat > "${work}/${variant}.nsi" <<EOF
Unicode true
OutFile "${work}/${variant}-setup.exe"
InstallDir "\$LOCALAPPDATA\\OpenFlux Studio"
RequestExecutionLevel user
Section
  SetOutPath "\$INSTDIR"
  File /r "${root}/*"
SectionEnd
EOF
    makensis -V1 "${work}/${variant}.nsi" > /dev/null
}

# expect <pass|fail> <description> <installer>
expect() {
    local want="$1" what="$2" out status=0
    out="$("$check" "$3" 2>&1)" || status=$?
    if [ "$want" = pass ] && [ "$status" -ne 0 ]; then fail "${what}: refused: ${out}"; return; fi
    if [ "$want" = fail ] && [ "$status" -eq 0 ]; then fail "${what}: accepted: ${out}"; return; fi
    echo "ok: ${what} ($(tail -n 1 <<<"$out"))"
}

for variant in complete missing empty; do
    app_archive "$variant"
    deb "$variant"
    nsis "$variant"
done
command -v lipo > /dev/null || expect pass "complete macOS update archive" "${work}/complete.app.tar.gz"
expect fail "macOS update archive without a license file" "${work}/missing.app.tar.gz"
expect fail "macOS update archive with an empty license file" "${work}/empty.app.tar.gz"
[ "$(uname -s)" != Linux ] || expect pass "complete .deb" "${work}/complete.deb"
expect fail ".deb without a license file" "${work}/missing.deb"
expect fail ".deb with an empty license file" "${work}/empty.deb"
expect pass "complete setup program" "${work}/complete-setup.exe"
expect fail "setup program without a license file" "${work}/missing-setup.exe"
expect fail "setup program with an empty license file" "${work}/empty-setup.exe"

if [ "$fails" -gt 0 ]; then
    echo "test-check-host-installer: ${fails} failures" >&2
    exit 1
fi
echo "test-check-host-installer: ok"
