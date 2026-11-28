#!/usr/bin/env bash
# Writes the Tauri configuration overlay used for release builds of
# Keyglass Studio. Pass the result to `tauri build --config <file>`; Tauri
# merges it over host/studio/src-tauri/tauri.conf.json (JSON merge patch:
# objects merge, arrays and values replace).
#
# The overlay owns everything that is specific to a packaged release:
#   - bundle targets per OS (macOS app and disk image, Windows NSIS setup
#     program, Linux AppImage and .deb) and the bundled sidecar executables
#   - the keyboard software bundled for offline install and restore
#   - the FFmpeg tools' license and build information ($RESOURCE/licenses/)
#   - service registration files (LaunchAgent, systemd user unit, udev rules,
#     Windows Run key hooks) and macOS entitlements
#   - the updater public key, the stable update endpoint, and
#     requireSignedVersion: the updater accepts an update only when its
#     signature names the version the feed announces (tauri-feed.py signs
#     with --app-version)
#
# The base tauri.conf.json therefore leaves bundle.externalBin unset, and
# declares bundle.resources, if any, in map form so this script can add to it.
#
# Usage:
#   tauri-release-config.sh --os <macos|windows|linux> --version <semver>
#       --sidecars <dir> --keyboard-bundle <dir> --pages-url <url>
#       [--updater-pubkey <file>] -o <file>
#
# --sidecars holds <name>-<target triple>[.exe] files and licenses/
# (build-sidecars.sh).
# --updater-pubkey defaults to host/packaging/keys/tauri-updater.pub.
# --keyboard-bundle holds the verified keyboard artifacts and the signed index
# (keyglass-release bundle); it is installed as $RESOURCE/keyboard/.

set -euo pipefail

die() { echo "tauri-release-config: $*" >&2; exit 1; }

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
packaging="$(cd "${here}/.." && pwd)"
repo="$(cd "${packaging}/../.." && pwd)"
base_config="${repo}/host/studio/src-tauri/tauri.conf.json"
updater_pubkey="${packaging}/keys/tauri-updater.pub"

os="" version="" sidecars="" keyboard="" pages_url="" out=""
while [ $# -gt 0 ]; do
    case "$1" in
        --os) os="$2"; shift 2 ;;
        --version) version="$2"; shift 2 ;;
        --sidecars) sidecars="$2"; shift 2 ;;
        --keyboard-bundle) keyboard="$2"; shift 2 ;;
        --pages-url) pages_url="${2%/}"; shift 2 ;;
        --updater-pubkey) updater_pubkey="$2"; shift 2 ;;
        -o) out="$2"; shift 2 ;;
        *) die "unknown argument: $1" ;;
    esac
done
for v in os version sidecars keyboard pages_url out; do
    [ -n "${!v}" ] || die "--${v//_/-} is required"
done
[ -d "$sidecars" ] || die "sidecar directory not found: ${sidecars}"
[ -f "${keyboard}/index.json" ] || die "keyboard bundle has no index.json: ${keyboard}"
[ -f "${keyboard}/index.json.sig" ] || die "keyboard bundle has no index.json.sig: ${keyboard}"
[ -f "$updater_pubkey" ] || die "updater public key not found: ${updater_pubkey} (generate the updater key as described in docs/releasing.md and commit its public half there)"
[ -d "${sidecars}/licenses" ] || die "sidecar directory has no licenses/: ${sidecars}"
[ -f "$base_config" ] || die "base Tauri config not found: ${base_config}"
command -v jq >/dev/null 2>&1 || die "jq is required"

if jq -e '.bundle.externalBin != null' "$base_config" >/dev/null; then
    die "${base_config} sets bundle.externalBin; the release overlay owns that list"
fi
if jq -e '.bundle.resources != null and (.bundle.resources | type) != "object"' "$base_config" >/dev/null; then
    die "${base_config} must declare bundle.resources as a map so release resources can be added"
fi

# Absolute paths in the form the native Tauri CLI reads; under Git Bash or
# MSYS2 on Windows that is C:/... rather than /c/...
native() {
    if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s\n' "$1"; fi
}
abs() { native "$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"; }
sidecars="$(native "$(cd "$sidecars" && pwd)")"
keyboard="$(native "$(cd "$keyboard" && pwd)")"

# Sidecars per OS. Tauri appends the target triple to each entry.
case "$os" in
    macos) bins=(keyglassd fluxctl keyglass-tray keyglass-sensors keyglass-ffmpeg keyglass-ffprobe) triple='*-apple-darwin' ;;
    windows) bins=(keyglassd fluxctl keyglass-ffmpeg keyglass-ffprobe) triple='*-pc-windows-msvc.exe' ;;
    linux) bins=(keyglassd fluxctl keyglass-ffmpeg keyglass-ffprobe) triple='*-unknown-linux-gnu' ;;
    *) die "unsupported --os: ${os}" ;;
esac
for b in "${bins[@]}"; do
    compgen -G "${sidecars}/${b}-${triple}" >/dev/null || die "missing sidecar ${b}-${triple} in ${sidecars}"
done
external_bin="$(printf '%s\n' "${bins[@]}" | jq -R --arg dir "$sidecars" '$dir + "/" + .' | jq -s .)"

jq -n \
    --arg os "$os" \
    --arg version "$version" \
    --argjson externalBin "$external_bin" \
    --argjson baseResources "$(jq '.bundle.resources // {}' "$base_config")" \
    --arg keyboard "${keyboard}/" \
    --arg licenses "${sidecars}/licenses/" \
    --arg bundledFonts "$(abs "${repo}/assets/bundled_fonts")/" \
    --arg defaultBgs "$(abs "${repo}/assets/default_backgrounds")/" \
    --arg weatherIcons "$(abs "${repo}/assets/weather_icons")/" \
    --arg shortcuts "$(abs "${repo}/assets/shortcuts")/" \
    --arg coreModules "$(abs "${repo}/modules")/" \
    --arg pubkey "$(tr -d '\r\n' < "$updater_pubkey")" \
    --arg endpoint "${pages_url}/updates/stable/latest.json" \
    --arg entitlements "$(abs "${packaging}/macos/entitlements.plist")" \
    --arg launchAgent "$(abs "${packaging}/macos/dev.keyglass.keyglassd.plist")" \
    --arg infoPlist "$(abs "${packaging}/macos/studio-display-name.plist")" \
    --arg desktopTemplate "$(abs "${packaging}/linux/studio.desktop")" \
    --arg nsisHooks "$(abs "${packaging}/windows/installer-hooks.nsh")" \
    --arg udevRules "$(abs "${packaging}/linux/70-keyglass.rules")" \
    --arg userUnit "$(abs "${packaging}/linux/keyglassd.service")" \
    --arg postinst "$(abs "${packaging}/linux/deb/postinst")" \
    --arg prerm "$(abs "${packaging}/linux/deb/prerm")" \
    --arg postrm "$(abs "${packaging}/linux/deb/postrm")" \
'{
  version: $version,
  bundle: {
    active: true,
    createUpdaterArtifacts: false,
    externalBin: $externalBin,
    resources: ($baseResources + {
      ($keyboard): "keyboard/",
      ($licenses): "licenses/",
      ($bundledFonts): "assets/bundled_fonts/",
      ($defaultBgs): "assets/default_backgrounds/",
      ($weatherIcons): "assets/weather_icons/",
      ($shortcuts): "assets/shortcuts/",
      ($coreModules): "modules/"
    })
  },
  plugins: {
    updater: { pubkey: $pubkey, endpoints: [ $endpoint ], requireSignedVersion: true }
  }
}
* (if $os == "macos" then
  { bundle: { targets: ["app", "dmg"], macOS: {
      minimumSystemVersion: "13.0",
      hardenedRuntime: true,
      entitlements: $entitlements,
      infoPlist: $infoPlist,
      files: { "Library/LaunchAgents/dev.keyglass.keyglassd.plist": $launchAgent }
  } } }
elif $os == "windows" then
  { bundle: { targets: ["nsis"], windows: {
      nsis: { installMode: "currentUser", installerHooks: $nsisHooks }
  } } }
else
  { bundle: { targets: ["appimage", "deb"], linux: {
      deb: {
        desktopTemplate: $desktopTemplate,
        files: {
          "/usr/lib/udev/rules.d/70-keyglass.rules": $udevRules,
          "/usr/lib/systemd/user/keyglassd.service": $userUnit
        },
        postInstallScript: $postinst,
        preRemoveScript: $prerm,
        postRemoveScript: $postrm
      },
      appimage: {
        files: { "/usr/share/keyglass/70-keyglass.rules": $udevRules }
      }
  } } }
end)' > "$out"

echo "tauri-release-config: wrote ${out}"
