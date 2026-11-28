#!/usr/bin/env bash
# Fast local build + replace-install of Keyglass v2 (Studio.app + keyglassd + fluxctl) on macOS.
# Removes any installed v1 app, installs fresh v2 into ~/Applications/Keyglass Studio.app and
# ~/.local/bin/fluxctl, and launches the app ready for dogfooding.

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DEST="$HOME/Applications/Keyglass Studio.app"
CLI_DEST="$HOME/.local/bin/fluxctl"
NO_LAUNCH=0

for arg in "$@"; do
  case "$arg" in
    --no-launch) NO_LAUNCH=1 ;;
    *) echo "Unknown arg: $arg" >&2; exit 1 ;;
  esac
done

echo "==> [1/6] Building Keyglass Studio v2 frontend..."
(cd "$REPO/host/studio" && npm run build)

echo "==> [2/6] Building release Rust binaries (keyglassd, keyglass-studio, fluxctl)..."
(cd "$REPO" && cargo build --release -p keyglassd -p keyglass-studio -p fluxctl)

# Ensure macOS Swift helpers exist in target/release
if [ ! -x "$REPO/target/release/keyglass-tray" ] || [ ! -x "$REPO/target/release/keyglass-sensors" ]; then
  echo "==> Building macOS Swift helpers..."
  make -C "$REPO/host/macos-helpers"
  cp "$REPO/host/macos-helpers/build/keyglass-tray" "$REPO/target/release/keyglass-tray"
  cp "$REPO/host/macos-helpers/build/keyglass-sensors" "$REPO/target/release/keyglass-sensors"
fi

echo "==> [3/6] Staging Keyglass Studio v2.app bundle..."
STAGING="$(mktemp -d /tmp/keyglass-v2-app-XXXXXX)"
trap 'rm -rf "$STAGING"' EXIT
APP_STAGE="$STAGING/Keyglass Studio.app"
mkdir -p "$APP_STAGE/Contents/MacOS" "$APP_STAGE/Contents/Resources"

cat > "$APP_STAGE/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>English</string>
	<key>CFBundleExecutable</key>
	<string>keyglass-studio</string>
	<key>CFBundleIdentifier</key>
	<string>dev.keyglass.studio</string>
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
	<key>NSLocalNetworkUsageDescription</key>
	<string>Keyglass Studio connects to local-network devices (such as MQTT smart-home brokers, 3D printers, and cameras) configured by your modules.</string>
</dict>
</plist>
PLIST

cp "$REPO/target/release/keyglass-studio" "$APP_STAGE/Contents/MacOS/keyglass-studio"
cp "$REPO/target/release/keyglassd"       "$APP_STAGE/Contents/MacOS/keyglassd"
cp "$REPO/target/release/keyglass-tray"   "$APP_STAGE/Contents/MacOS/keyglass-tray"
cp "$REPO/target/release/keyglass-sensors" "$APP_STAGE/Contents/MacOS/keyglass-sensors"

# Carry over ffmpeg/ffprobe sidecars from target/release or existing app bundle
for tool in keyglass-ffmpeg keyglass-ffprobe; do
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

echo "==> [4/6] Stopping running Keyglass v1/v2 processes and removing old app..."
pkill -f "Keyglass Studio.app/Contents/MacOS/keyglass-studio" 2>/dev/null || true
pkill -f "Keyglass Studio.app/Contents/MacOS/keyglass-tray" 2>/dev/null || true
pkill -f "Keyglass Studio.app/Contents/MacOS/keyglassd" 2>/dev/null || true
pkill -f "Keyglass Studio.app/Contents/MacOS/keyglass-sensors" 2>/dev/null || true
sleep 1
# Ensure nothing lingers
pkill -9 -f "Keyglass Studio.app/Contents/MacOS/keyglass" 2>/dev/null || true
sleep 0.3

mkdir -p "$HOME/Applications" "$HOME/.local/bin"
rm -rf "$APP_DEST"
mv "$APP_STAGE" "$APP_DEST"

echo "==> [5/6] Installing fluxctl -> $CLI_DEST ..."
cp "$REPO/target/release/fluxctl" "$CLI_DEST"
chmod 0755 "$CLI_DEST"

if [ "$NO_LAUNCH" -eq 0 ]; then
  echo "==> [6/6] Launching Keyglass Studio v2..."
  open "$APP_DEST"
  for _ in $(seq 1 30); do
    sleep 0.4
    if "$CLI_DEST" status 2>/dev/null | grep -q "CONNECTED to live keyglassd"; then
      echo ""
      "$CLI_DEST" status
      exit 0
    fi
  done
  echo "Launched Keyglass Studio.app (daemon still initializing; run 'fluxctl status' in a moment)."
else
  echo "==> [6/6] Installed without launching (--no-launch)."
fi
