#!/usr/bin/env bash
# Checks the Tauri release overlay (host/packaging/scripts/tauri-release-config.sh)
# for each OS: the bundle targets (Windows: the NSIS setup program only), the
# sidecars (keyglassd, keyglass-ffmpeg and keyglass-ffprobe everywhere, plus
# the macOS helpers), the bundled keyboard software and FFmpeg licenses, the
# service files, and the updater settings (requireSignedVersion, the
# committed updater public key, the stable feed).
#
# Run from the repository root: .github/scripts/test-tauri-release-config.sh

set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
config="${repo}/host/packaging/scripts/tauri-release-config.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
fails=0
fail() { echo "FAIL: $*" >&2; fails=$((fails + 1)); }
# expect <overlay> <description> <jq expression that must be true>
expect() {
    if jq -e "$3" "$1" > /dev/null; then echo "ok: $2"; else fail "$2: $(jq -c . "$1")"; fi
}

mkdir -p "${work}/keyboard" "${work}/sidecars/licenses"
touch "${work}/keyboard/index.json" "${work}/keyboard/index.json.sig" "${work}/sidecars/licenses/keyglass-ffmpeg-COPYING.GPLv2.txt"
for b in keyglassd fluxctl keyglass-tray keyglass-sensors keyglass-ffmpeg keyglass-ffprobe; do
    touch "${work}/sidecars/${b}-universal-apple-darwin" "${work}/sidecars/${b}-x86_64-pc-windows-msvc.exe" \
        "${work}/sidecars/${b}-x86_64-unknown-linux-gnu"
done
pubkey="$(tr -d '\r\n' < "${repo}/host/packaging/keys/tauri-updater.pub")"

for os in macos windows linux; do
    out="${work}/${os}.json"
    "$config" --os "$os" --version 1.2.0-beta.1 --sidecars "${work}/sidecars" --keyboard-bundle "${work}/keyboard" \
        --pages-url https://example.invalid/keyglass/ -o "$out" > /dev/null
    expect "$out" "${os}: version" '.version == "1.2.0-beta.1"'
    # shellcheck disable=SC2016 # jq variables
    expect "$out" "${os}: keyglassd, fluxctl, keyglass-ffmpeg and keyglass-ffprobe are sidecars" \
        '[.bundle.externalBin[] | split("/") | last] as $b | ["keyglassd", "fluxctl", "keyglass-ffmpeg", "keyglass-ffprobe"] - $b == []'
    expect "$out" "${os}: keyboard software, licenses, core modules, shortcuts and preview assets are resources" \
        '[.bundle.resources[]] | sort == ["assets/bundled_fonts/", "assets/default_backgrounds/", "assets/shortcuts/", "assets/weather_icons/", "keyboard/", "licenses/", "modules/"]'
    expect "$out" "${os}: updater requires the signed version" '.plugins.updater.requireSignedVersion == true'
    if jq -e --arg k "$pubkey" '.plugins.updater.pubkey == $k and
        .plugins.updater.endpoints == ["https://example.invalid/keyglass/updates/stable/latest.json"]' "$out" > /dev/null; then
        echo "ok: ${os}: updater key and stable feed"
    else
        fail "${os}: updater key or endpoint"
    fi
    expect "$out" "${os}: updater artifacts are signed at publication, not at build" '.bundle.createUpdaterArtifacts == false'
done
expect "${repo}/host/studio/src-tauri/tauri.conf.json" "existing installer identity and branded window" \
    '.identifier == "dev.keyglass.studio" and .productName == "Keyglass Studio" and .app.windows[0].title == "Keyglass Studio"'
expect "${work}/macos.json" "macos: app and disk image" '.bundle.targets == ["app", "dmg"]'
expect "${work}/macos.json" "macos: display name override" \
    '.bundle.macOS.infoPlist | endswith("studio-display-name.plist")'
expect "${work}/macos.json" "macos: helpers are sidecars" \
    '[.bundle.externalBin[] | split("/") | last] | index("keyglass-tray") != null and index("keyglass-sensors") != null'
expect "${work}/macos.json" "macos: LaunchAgent in Contents/Library/LaunchAgents" \
    '.bundle.macOS.files | has("Library/LaunchAgents/dev.keyglass.keyglassd.plist")'
expect "${work}/windows.json" "windows: NSIS only" '.bundle.targets == ["nsis"] and (.bundle.windows | has("wix") | not)'
expect "${work}/windows.json" "windows: installer hooks" '.bundle.windows.nsis.installerHooks | endswith("installer-hooks.nsh")'
expect "${work}/linux.json" "linux: AppImage and .deb" '.bundle.targets == ["appimage", "deb"]'
expect "${work}/linux.json" "linux: .deb installs the udev rules and the user unit" \
    '.bundle.linux.deb.files | has("/usr/lib/udev/rules.d/70-keyglass.rules") and has("/usr/lib/systemd/user/keyglassd.service")'
expect "${work}/linux.json" "linux: desktop display name override" \
    '.bundle.linux.deb.desktopTemplate | endswith("studio.desktop")'
expect "${work}/linux.json" "linux: .deb maintainer scripts" \
    '.bundle.linux.deb | (.postInstallScript | endswith("postinst")) and (.preRemoveScript | endswith("prerm")) and (.postRemoveScript | endswith("postrm"))'

rm "${work}/sidecars/keyglass-ffprobe-x86_64-pc-windows-msvc.exe"
if "$config" --os windows --version 1.0.0 --sidecars "${work}/sidecars" --keyboard-bundle "${work}/keyboard" \
    --pages-url https://example.invalid -o "${work}/missing.json" 2> "${work}/missing.log"; then
    fail "a missing keyglass-ffprobe sidecar was accepted"
else
    echo "ok: refused: $(cat "${work}/missing.log")"
fi
if "$config" --os linux --version 1.0.0 --sidecars "${work}/sidecars" --keyboard-bundle "${work}/keyboard" \
    --pages-url https://example.invalid --updater-pubkey "${work}/none.pub" -o "${work}/nokey.json" 2> "${work}/nokey.log"; then
    fail "a missing updater public key was accepted"
else
    echo "ok: refused: $(cat "${work}/nokey.log")"
fi

if [ "$fails" -gt 0 ]; then
    echo "test-tauri-release-config: ${fails} failures" >&2
    exit 1
fi
echo "test-tauri-release-config: ok"
