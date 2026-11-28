#!/usr/bin/env bash
# Compiles the NSIS installer hooks (host/packaging/windows/installer-hooks.nsh)
# into a minimal installer and uninstaller with warnings treated as errors,
# so syntax errors, unknown labels and unreferenced functions fail here
# rather than in the Windows release build. The hooks run against the real
# daemon only on Windows; the decisions they implement are the same ones
# test-package-hooks.sh exercises for the .deb scripts.
#
# The post-install hook calls a plugin of the Tauri NSIS template and is not
# compiled here.
#
# Requires makensis. Run from the repository root:
#   .github/scripts/test-nsis-hooks.sh

set -euo pipefail

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
hooks="${repo}/host/packaging/windows/installer-hooks.nsh"
command -v makensis > /dev/null || { echo "test-nsis-hooks: makensis is required" >&2; exit 1; }
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

cat > "${work}/harness.nsi" <<EOF
Unicode true
!include LogicLib.nsh
Name "Keyglass hooks test"
OutFile "${work}/hooks-test.exe"
RequestExecutionLevel user
InstallDir "\$LOCALAPPDATA\\Keyglass Studio"
; Set by the Tauri template when the setup program runs as an update.
Var UpdateMode

!include "${hooks}"

; The wait for keyboard work is bounded at 15 minutes, like the systemd
; unit's TimeoutStopSec and the LaunchAgent's ExitTimeOut.
!if \${KEYGLASS_STOP_TIMEOUT_SECONDS} != 900
  !error "the wait for keyboard work must be bounded at 15 minutes"
!endif

Section "Install"
  StrCpy \$UpdateMode 0
  !insertmacro NSIS_HOOK_PREINSTALL
  WriteUninstaller "\$INSTDIR\\uninstall.exe"
SectionEnd

Section "Uninstall"
  !insertmacro NSIS_HOOK_PREUNINSTALL
SectionEnd
EOF

makensis -V2 -WX "${work}/harness.nsi"
echo "test-nsis-hooks: ok"
