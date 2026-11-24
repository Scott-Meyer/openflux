#!/usr/bin/env bash
# Fast local build + replace-install of OpenFlux v2 (Studio.app + openfluxd + fluxctl) on macOS.
# Removes any installed v1 app, installs fresh v2 into ~/Applications/OpenFlux Studio.app and
# ~/.local/bin/fluxctl, and launches the app ready for dogfooding.

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DEST="$HOME/Applications/OpenFlux Studio.app"
CLI_DEST="$HOME/.local/bin/fluxctl"
NO_LAUNCH=0

for arg in "$@"; do
  case "$arg" in
    --no-launch) NO_LAUNCH=1 ;;
    *) echo "Unknown arg: $arg" >&2; exit 1 ;;
  esac
done

echo "==> [1/6] Building OpenFlux Studio v2 frontend..."
(cd "$REPO/host/studio" && npm run build)

echo "==> [2/6] Building release Rust binaries (openfluxd, openflux-studio, fluxctl)..."
(cd "$REPO" && cargo build --release -p openfluxd -p openflux-studio -p fluxctl)

# Ensure macOS Swift helpers exist in target/release
if [ ! -x "$REPO/target/release/openflux-tray" ] || [ ! -x "$REPO/target/release/openflux-sensors" ]; then
  echo "==> Building macOS Swift helpers..."
  make -C "$REPO/host/macos-helpers"
  cp "$REPO/host/macos-helpers/build/openflux-tray" "$REPO/target/release/openflux-tray"
  cp "$REPO/host/macos-helpers/build/openflux-sensors" "$REPO/target/release/openflux-sensors"
fi

echo "==> [3/6] Staging OpenFlux Studio v2.app bundle..."
STAGING="$(mktemp -d /tmp/openflux-v2-app-XXXXXX)"
trap 'rm -rf "$STAGING"' EXIT
APP_STAGE="$STAGING/OpenFlux Studio.app"
mkdir -p "$APP_STAGE/Contents/MacOS" "$APP_STAGE/Contents/Resources"

cat > "$APP_STAGE/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>English</string>
	<key>CFBundleDisplayName</key>
	<string>OpenFlux Studio</string>
	<key>CFBundleExecutable</key>
	<string>openflux-studio</string>
	<key>CFBundleIdentifier</key>
	<string>org.openflux.studio</string>
	<key>CFBundleInfoDictionaryVersion</key>
	<string>6.0</string>
	<key>CFBundleDisplayName</key>
	<string>Keyglass Studio</string>
	<key>CFBundleName</key>
	<string>Keyglass Studio</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>2.0.0-dev</string>
	<key>CFBundleVersion</key>
	<string>2.0.0-dev</string>
	<key>CSResourcesFileMapped</key>
	<true/>
	<key>LSMinimumSystemVersion</key>
	<string>13.0</string>
	<key>CFBundleIconFile</key>
	<string>icon.icns</string>
	<key>LSRequiresCarbon</key>
	<true/>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
PLIST

cp "$REPO/target/release/openflux-studio" "$APP_STAGE/Contents/MacOS/openflux-studio"
cp "$REPO/target/release/openfluxd"       "$APP_STAGE/Contents/MacOS/openfluxd"
cp "$REPO/target/release/openflux-tray"   "$APP_STAGE/Contents/MacOS/openflux-tray"
cp "$REPO/target/release/openflux-sensors" "$APP_STAGE/Contents/MacOS/openflux-sensors"

# Carry over ffmpeg/ffprobe sidecars from target/release or existing app bundle
for tool in openflux-ffmpeg openflux-ffprobe; do
  if [ -x "$REPO/target/release/$tool" ]; then
    cp "$REPO/target/release/$tool" "$APP_STAGE/Contents/MacOS/$tool"
  elif [ -x "$APP_DEST/Contents/MacOS/$tool" ]; then
    cp "$APP_DEST/Contents/MacOS/$tool" "$APP_STAGE/Contents/MacOS/$tool"
  fi
done
chmod 0755 "$APP_STAGE/Contents/MacOS/"*

cp "$REPO/host/studio/src-tauri/icons/icon.icns" "$APP_STAGE/Contents/Resources/icon.icns"
cp -R "$REPO/assets" "$APP_STAGE/Contents/Resources/assets"
cp -R "$REPO/modules" "$APP_STAGE/Contents/Resources/modules"

codesign --force --deep --sign - "$APP_STAGE" >/dev/null 2>&1 || true

echo "==> [4/6] Stopping running OpenFlux v1/v2 processes and removing old app..."
pkill -f "OpenFlux Studio.app/Contents/MacOS/openflux-studio" 2>/dev/null || true
pkill -f "OpenFlux Studio.app/Contents/MacOS/openflux-tray" 2>/dev/null || true
pkill -f "OpenFlux Studio.app/Contents/MacOS/openfluxd" 2>/dev/null || true
pkill -f "OpenFlux Studio.app/Contents/MacOS/openflux-sensors" 2>/dev/null || true
sleep 1
# Ensure nothing lingers
pkill -9 -f "OpenFlux Studio.app/Contents/MacOS/openflux" 2>/dev/null || true
sleep 0.3

mkdir -p "$HOME/Applications" "$HOME/.local/bin"
rm -rf "$APP_DEST"
mv "$APP_STAGE" "$APP_DEST"

echo "==> [5/6] Installing fluxctl -> $CLI_DEST ..."
cp "$REPO/target/release/fluxctl" "$CLI_DEST"
chmod 0755 "$CLI_DEST"

if [ "$NO_LAUNCH" -eq 0 ]; then
  echo "==> [6/6] Launching OpenFlux Studio v2..."
  open "$APP_DEST"
  for _ in $(seq 1 30); do
    sleep 0.4
    if "$CLI_DEST" status 2>/dev/null | grep -q "CONNECTED to live openfluxd"; then
      echo ""
      "$CLI_DEST" status
      exit 0
    fi
  done
  echo "Launched OpenFlux Studio.app (daemon still initializing; run 'fluxctl status' in a moment)."
else
  echo "==> [6/6] Installed without launching (--no-launch)."
fi
