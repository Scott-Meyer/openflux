#!/usr/bin/env bash
# Checks a keyboard .ipk before it is published:
#   - it is an ar archive with debian-binary, control.tar.gz and data.tar.gz
#   - the control file names the expected package, version and architecture
#   - every ELF file is AArch64
#   - in a "static" package every executable is fully static (no program
#     interpreter, no shared libraries)
#   - in a "glibc" package every executable is either fully static (such as
#     the bundled openflux-ffmpeg) or dynamic against glibc with no symbol
#     version newer than KEYBOARD_GLIBC_MAX and no library outside the
#     keyboard OS's base set
#   - every path given with --require is in the package (paths as installed,
#     for example /usr/lib/openflux/display/licenses/FFMPEG-LGPL-2.1.txt)
#   - the package has executables, or with --no-executables has none (a
#     package that only runs maintainer scripts)
#
# Usage: check-keyboard-ipk.sh <file.ipk> <package> <version> <static|glibc>
#            [--require <path>]... [--no-executables]
#
# Requires binutils (readelf).

set -euo pipefail

die() { echo "check-keyboard-ipk: $*" >&2; exit 1; }

ipk="${1:?usage}" package="${2:?usage}" version="${3:?usage}" linkage="${4:?usage}"
shift 4
required=() expect_executables=true
while [ $# -gt 0 ]; do
    case "$1" in
        --require) required+=("$2"); shift 2 ;;
        --no-executables) expect_executables=false; shift ;;
        *) die "unknown argument: $1" ;;
    esac
done
case "$linkage" in static|glibc) ;; *) die "linkage must be static or glibc, not ${linkage}" ;; esac
glibc_max="${KEYBOARD_GLIBC_MAX:-}"
[ "$linkage" = static ] || [ -n "$glibc_max" ] || die "KEYBOARD_GLIBC_MAX is required for a glibc package"
# Shared libraries the keyboard OS provides to the display.
allowed_libs="libc.so.6 libm.so.6 libdl.so.2 libpthread.so.0 librt.so.1 libgcc_s.so.1 ld-linux-aarch64.so.1 libwayland-client.so.0 libwayland-egl.so.1 libEGL.so.1 libGLESv2.so.2 librockchip_mpp.so.1"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
ipk="$(cd "$(dirname "$ipk")" && pwd)/$(basename "$ipk")"
(cd "$work" && ar x "$ipk")
for m in debian-binary control.tar.gz data.tar.gz; do
    [ -f "${work}/${m}" ] || die "${ipk}: missing ${m}"
done
[ "$(cat "${work}/debian-binary")" = "2.0" ] || die "${ipk}: debian-binary is not 2.0"
mkdir -p "${work}/control" "${work}/data"
tar -xzf "${work}/control.tar.gz" -C "${work}/control"
tar -xzf "${work}/data.tar.gz" -C "${work}/data"

field() { sed -n "s/^$1: *//p" "${work}/control/control"; }
[ "$(field Package)" = "$package" ] || die "control Package is $(field Package), expected ${package}"
[ "$(field Version)" = "$version" ] || die "control Version is $(field Version), expected ${version}"
[ "$(field Architecture)" = "aarch64" ] || die "control Architecture is $(field Architecture)"

for path in "${required[@]}"; do
    [ -f "${work}/data/${path#/}" ] || die "${ipk}: ${path} is missing"
    echo "${path}: present"
done

elf_count=0
while IFS= read -r -d '' f; do
    [ "$(head -c 4 "$f" | od -An -tx1 | tr -d ' \n')" = 7f454c46 ] || continue
    elf_count=$((elf_count + 1))
    rel="${f#"${work}/data"}"
    readelf -h "$f" | grep -q 'Machine:.*AArch64' || die "${rel}: not an AArch64 ELF file"
    needed="$(readelf -d "$f" 2>/dev/null | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')"
    interp="$(readelf -l "$f" 2>/dev/null | sed -n 's/.*Requesting program interpreter: \(.*\)\]/\1/p')"
    if [ -z "$needed" ] && [ -z "$interp" ]; then
        echo "${rel}: static"
        continue
    fi
    [ "$linkage" = glibc ] || die "${rel}: expected a static executable, found interpreter ${interp:-none} and libraries: ${needed}"
    for lib in $needed; do
        case " ${allowed_libs} " in
            *" ${lib} "*) ;;
            *) die "${rel}: links ${lib}, which the keyboard OS does not provide" ;;
        esac
    done
    newest="$({ readelf --version-info -W "$f" 2>/dev/null || true; } | { grep -o 'GLIBC_[0-9][0-9.]*' || true; } | sed 's/GLIBC_//' | sort -uV | tail -n 1)"
    if [ -n "$newest" ] && [ "$(printf '%s\n%s\n' "$newest" "$glibc_max" | sort -V | tail -n 1)" != "$glibc_max" ]; then
        die "${rel}: needs GLIBC_${newest}, newer than the keyboard's ${glibc_max}"
    fi
    echo "${rel}: ${needed//$'\n'/ } (newest glibc symbol ${newest:-none})"
done < <(find "${work}/data" -type f -print0)
if [ "$expect_executables" = true ]; then
    [ "$elf_count" -gt 0 ] || die "${ipk}: no executables in data.tar.gz"
else
    [ "$elf_count" -eq 0 ] || die "${ipk}: expected no executables, found ${elf_count}"
fi
echo "check-keyboard-ipk: ${ipk} ok (${elf_count} ELF files)"
