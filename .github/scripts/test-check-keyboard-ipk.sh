#!/usr/bin/env bash
# Tests check-keyboard-ipk.sh on packages built from small AArch64 test
# programs: a glibc package may carry a fully static helper (the display's
# openflux-ffmpeg), a static package may not carry a dynamic executable,
# libraries outside the keyboard OS's set and too-new glibc symbols are
# refused, and required files and the presence of executables are checked.
#
# Needs an AArch64 C compiler for Linux (CC_AARCH64, default
# aarch64-linux-gnu-gcc from gcc-aarch64-linux-gnu, with static glibc from
# libc6-dev-arm64-cross), readelf and python3. Run from the repository root:
#   .github/scripts/test-check-keyboard-ipk.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "${here}/../.." && pwd)"
check="${here}/check-keyboard-ipk.sh"
cc="${CC_AARCH64:-aarch64-linux-gnu-gcc}"
command -v "${cc%% *}" > /dev/null || { echo "test-check-keyboard-ipk: ${cc} is required" >&2; exit 1; }
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
fails=0
fail() { echo "FAIL: $*" >&2; fails=$((fails + 1)); }

cd "$work"
printf 'int main(void) { return 0; }\n' > main.c
printf 'int foo(void) { return 1; }\n' > foo.c
printf 'int foo(void);\nint main(void) { return foo(); }\n' > usefoo.c
$cc -O1 -o dynamic main.c
$cc -O1 -static -o static main.c
$cc -O1 -shared -fPIC -o libfoo.so foo.c
$cc -O1 -o usesfoo usefoo.c -L. -lfoo

# package <name> <package> <version> <file>:<path in package>...
package() {
    local name="$1" pkg="$2" version="$3"
    shift 3
    rm -rf "stage-${name}"
    mkdir -p "stage-${name}/control" "stage-${name}/data"
    printf 'Package: %s\nVersion: %s\nArchitecture: aarch64\nMaintainer: test\nDescription: test\n' \
        "$pkg" "$version" > "stage-${name}/control/control"
    for entry in "$@"; do
        local src="${entry%%:*}" dest="stage-${name}/data${entry#*:}"
        mkdir -p "$(dirname "$dest")"
        cp "$src" "$dest"
    done
    mkdir -p "stage-${name}/data/home/root/.openflux"
    SOURCE_DATE_EPOCH=1700000000 python3 "${repo}/scripts/build_ipk.py" \
        --control-dir "stage-${name}/control" --data-dir "stage-${name}/data" -o "${name}.ipk" > /dev/null
}

# expect <pass|fail> <description> <check-keyboard-ipk.sh arguments>...
expect() {
    local want="$1" what="$2" out status=0
    shift 2
    out="$(KEYBOARD_GLIBC_MAX="${GLIBC_MAX:-2.36}" "$check" "$@" 2>&1)" || status=$?
    if [ "$want" = pass ] && [ "$status" -ne 0 ]; then fail "${what}: refused: ${out}"; return; fi
    if [ "$want" = fail ] && [ "$status" -eq 0 ]; then fail "${what}: accepted: ${out}"; return; fi
    echo "ok: ${what} ($(tail -n 1 <<<"$out"))"
}

printf 'LGPL\n' > license.txt
D=/usr/lib/openflux/display
package display openflux-display 1.0.0 dynamic:$D/openflux-display static:$D/openflux-ffmpeg license.txt:$D/licenses/FFMPEG-LGPL-2.1.txt
expect pass "glibc package with a fully static helper" display.ipk openflux-display 1.0.0 glibc \
    --require $D/openflux-ffmpeg --require $D/licenses/FFMPEG-LGPL-2.1.txt
expect fail "a required file is missing" display.ipk openflux-display 1.0.0 glibc --require $D/licenses/FFMPEG-NOTICE.txt
expect fail "the control version differs" display.ipk openflux-display 1.0.1 glibc
expect fail "the control package name differs" display.ipk openflux-bridge 1.0.0 glibc
GLIBC_MAX=2.16 expect fail "a glibc symbol newer than the keyboard's" display.ipk openflux-display 1.0.0 glibc

package bridge openflux-bridge 1.0.0 static:/usr/lib/openflux/bridge/openflux-bridge
expect pass "static package with a static executable" bridge.ipk openflux-bridge 1.0.0 static
package bridge-dynamic openflux-bridge 1.0.0 dynamic:/usr/lib/openflux/bridge/openflux-bridge
expect fail "static package with a dynamic executable" bridge-dynamic.ipk openflux-bridge 1.0.0 static

package foreign openflux-display 1.0.0 usesfoo:$D/openflux-display libfoo.so:$D/libfoo.so
expect fail "a library the keyboard OS does not provide" foreign.ipk openflux-display 1.0.0 glibc

package restore openflux-factory-restore 1.0.0
expect pass "a package without executables, with --no-executables" restore.ipk openflux-factory-restore 1.0.0 static --no-executables
expect fail "a package without executables" restore.ipk openflux-factory-restore 1.0.0 static
expect fail "executables where none are expected" bridge.ipk openflux-bridge 1.0.0 static --no-executables

if [ "$fails" -gt 0 ]; then
    echo "test-check-keyboard-ipk: ${fails} failures" >&2
    exit 1
fi
echo "test-check-keyboard-ipk: ok"
