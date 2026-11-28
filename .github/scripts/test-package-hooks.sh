#!/usr/bin/env bash
# Tests the installer and service hooks against a stand-in keyglassd that
# answers --request-restart / --request-stop with scripted exit statuses
# (testing/hooks/). The hooks fail closed:
#   0 (stopped or restarted) and 4 (not running) proceed,
#   3 (deferred) waits, bounded, with a message,
#   anything else stops with a message.
#
# Covered here: the .deb postinst and prerm, run with /bin/sh and with every
# system command they use replaced by the stand-ins, and the restart and stop
# settings of the LaunchAgent and the systemd unit. The NSIS hooks are
# compiled by test-nsis-hooks.sh.
#
# Run from the repository root: .github/scripts/test-package-hooks.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "${here}/../.." && pwd)"
deb="${repo}/host/packaging/linux/deb"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
export FAKE_HOOKS="${work}/state"
fails=0

fail() { echo "FAIL: $*" >&2; fails=$((fails + 1)); }

# run_hook <script> <argument> <status>...: runs a maintainer script with the
# daemon answering the given statuses in turn (the last one repeats).
# Sets $status, $output, $requests (number of daemon requests), $slept
# (seconds waited) and $calls (the system commands run).
run_hook() {
    local script="$1" arg="$2"
    shift 2
    rm -rf "$FAKE_HOOKS"
    mkdir -p "$FAKE_HOOKS"
    printf '%s\n' "$@" > "${FAKE_HOOKS}/codes"
    [ -z "${DISABLED:-}" ] || touch "${FAKE_HOOKS}/disabled"
    : > "${FAKE_HOOKS}/requests.log"
    : > "${FAKE_HOOKS}/calls.log"
    : > "${FAKE_HOOKS}/slept.log"
    status=0
    output="$(PATH="${here}/testing/hooks:${PATH}" sh "${deb}/${script}" "$arg" 2>&1)" || status=$?
    requests="$(wc -l < "${FAKE_HOOKS}/requests.log" | tr -d ' ')"
    slept="$(awk '{s += $1} END {print s + 0}' "${FAKE_HOOKS}/slept.log")"
    calls="$(cat "${FAKE_HOOKS}/calls.log")"
}

expect() {
    # expect <name> <exit status> <requests> <seconds waited> [<text in output>]
    local name="$1" want_status="$2" want_requests="$3" want_slept="$4" text="${5:-}"
    local ok=true
    [ "$status" = "$want_status" ] || { fail "${name}: exit status ${status}, expected ${want_status}"; ok=false; }
    [ "$requests" = "$want_requests" ] || { fail "${name}: ${requests} daemon requests, expected ${want_requests}"; ok=false; }
    [ "$slept" = "$want_slept" ] || { fail "${name}: waited ${slept}s, expected ${want_slept}s"; ok=false; }
    if [ -n "$text" ] && ! grep -qF -- "$text" <<<"$output"; then
        fail "${name}: output lacks \"${text}\": ${output}"
        ok=false
    fi
    [ "$ok" = true ] && echo "ok: ${name}"
    return 0
}

# postinst configure: the daemon moves to the installed binary.
run_hook postinst configure 0
expect "postinst: restarted (0) proceeds" 0 1 0
grep -q -- '--request-restart' "${FAKE_HOOKS}/requests.log" || fail "postinst did not ask for a restart"
grep -q 'HOME=/home/alice XDG_RUNTIME_DIR=/run/user/1000' "${FAKE_HOOKS}/requests.log" ||
    fail "postinst did not pass the user's HOME and XDG_RUNTIME_DIR: $(cat "${FAKE_HOOKS}/requests.log")"
grep -q 'systemctl --user -M 1000@ start' <<<"$calls" && fail "postinst started a daemon that restarts by itself"

run_hook postinst configure 4
expect "postinst: not running (4) starts the service" 0 1 0
grep -q 'systemctl --user -M 1000@ start keyglassd.service' <<<"$calls" || fail "postinst did not start the service: ${calls}"

run_hook postinst configure 3 3 0
expect "postinst: deferred (3) waits, then proceeds" 0 3 10 "finishing work on the keyboard for alice"

run_hook postinst configure 3
expect "postinst: deferred (3) waits at most 15 minutes" 0 181 900 "starts by itself when that work is done"

run_hook postinst configure 1
expect "postinst: error (1) stops with a message" 1 1 0 "--request-restart for alice failed (status 1)"

run_hook postinst configure 2
expect "postinst: unknown status (2) stops with a message" 1 1 0 "failed (status 2)"

run_hook postinst configure 127
expect "postinst: keyglassd missing (127) stops with a message" 1 1 0 "failed (status 127)"

DISABLED=1 run_hook postinst configure 1
expect "postinst: a user who disabled the service is left alone" 0 0 0

# prerm remove: the daemon stops before its files go away.
run_hook prerm remove 0
expect "prerm: stopped (0) proceeds" 0 1 0
grep -q -- '--request-stop' "${FAKE_HOOKS}/requests.log" || fail "prerm did not ask for a stop"
grep -q 'systemctl --global disable keyglassd.service' <<<"$calls" || fail "prerm did not disable the unit"

run_hook prerm remove 4
expect "prerm: not running (4) proceeds" 0 1 0

run_hook prerm remove 3 3 3 4
expect "prerm: deferred (3) waits, then proceeds" 0 4 15 "finishing work on the keyboard for alice"

run_hook prerm remove 3
expect "prerm: deferred (3) longer than 15 minutes stops the removal" 1 181 900 "remove the package again when it has finished"
grep -q 'disable' <<<"$calls" && fail "prerm disabled the unit although the removal stopped"

run_hook prerm remove 1
expect "prerm: error (1) stops the removal" 1 1 0 "nothing was removed"

run_hook prerm remove 75
expect "prerm: unknown status (75) stops the removal" 1 1 0 "failed (status 75)"

run_hook prerm upgrade 1
expect "prerm: an upgrade leaves the daemon running" 0 0 0

# Service definitions: exit 75 relaunches, exit 0 stays stopped, and a stop
# waits for keyboard work.
plist="${repo}/host/packaging/macos/dev.keyglass.keyglassd.plist"
before=$fails
python3 - "$plist" <<'EOF' || fail "LaunchAgent settings"
import plistlib, sys
p = plistlib.load(open(sys.argv[1], "rb"))
assert p["KeepAlive"] == {"SuccessfulExit": False}, p["KeepAlive"]
assert p["ExitTimeOut"] >= 900, p.get("ExitTimeOut")
assert p["RunAtLoad"] is True
EOF
[ "$fails" = "$before" ] && echo "ok: LaunchAgent relaunches after a non-zero exit only, ExitTimeOut 900"
unit="${repo}/host/packaging/linux/keyglassd.service"
before=$fails
grep -qx 'Restart=on-failure' "$unit" || fail "keyglassd.service: Restart=on-failure missing"
grep -qx 'TimeoutStopSec=15min' "$unit" || fail "keyglassd.service: TimeoutStopSec=15min missing"
grep -q '^RestartPreventExitStatus\|^SuccessExitStatus' "$unit" && fail "keyglassd.service must not change which statuses restart"
[ "$fails" = "$before" ] && echo "ok: systemd unit restarts on failure (including 75), not after 0, TimeoutStopSec 15min"

if [ "$fails" -gt 0 ]; then
    echo "test-package-hooks: ${fails} failures" >&2
    exit 1
fi
echo "test-package-hooks: ok"
