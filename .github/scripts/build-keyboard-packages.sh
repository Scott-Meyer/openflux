#!/usr/bin/env bash
# Builds a keyboard component's release packages twice and checks that the
# two builds are identical.
#
# Each round runs the component's Makefile in its build image
# (keyglass-<component>-build, from keyboard/<component>/Dockerfile.build)
# with its own freshly created build, Cargo target and output directories.
# They are mounted at the same path inside the container, so paths compiled
# into the binaries match. Each round must compile the component; the
# files the two rounds write must be identical.
#
#   bridge:  make ipk, make factory-restore-ipk
#   display: make ipk (also writes the FFmpeg source archive and build
#            information)
#
# Usage: build-keyboard-packages.sh <bridge|display> <version> <out dir>
#
# Writes the .ipk files to <out dir>/artifacts and every other file the
# Makefile wrote to <out dir>/extra. Honors SOURCE_DATE_EPOCH. Run from the
# repository root.

set -euo pipefail

die() { echo "build-keyboard-packages: $*" >&2; exit 1; }

component="${1:?usage: build-keyboard-packages.sh <bridge|display> <version> <out dir>}"
version="${2:?usage}"
out="${3:?usage}"
case "$component" in
    bridge) targets=(ipk factory-restore-ipk) ;;
    display) targets=(ipk) ;;
    *) die "builds bridge and display, not ${component}" ;;
esac
: "${SOURCE_DATE_EPOCH:?SOURCE_DATE_EPOCH is required}"
export SOURCE_DATE_EPOCH
rounds="$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/keyboard-build.XXXXXX")"
# The build container may write files as root that this user can't remove;
# those are left for the runner's cleanup of RUNNER_TEMP, without failing
# the build.
trap 'rm -rf "$rounds" 2>/dev/null || true' EXIT

for n in 1 2; do
    round="${rounds}/round-${n}"
    mkdir -p "${round}/build" "${round}/target" "${round}/out"
    for t in "${targets[@]}"; do
        docker run --rm -e SOURCE_DATE_EPOCH \
            -v "${PWD}:/work" -v "${round}:/round" -w /work "keyglass-${component}-build" \
            make -C "keyboard/${component}" "$t" VERSION="$version" CARGO="cargo --locked" \
            BUILD_DIR=/round/build TARGET_DIR=/round/target OUT_DIR=/round/out 2>&1 | tee -a "${round}/build.log"
    done
    grep -q "Compiling keyglass-${component} v${version}" "${round}/build.log" ||
        die "round ${n} did not compile keyglass-${component} ${version}"
    [ -f "${round}/out/keyglass-${component}_${version}_aarch64.ipk" ] ||
        die "round ${n} did not produce keyglass-${component}_${version}_aarch64.ipk: $(ls "${round}/out")"
    (cd "${round}/out" && find . -maxdepth 1 -type f | sed 's|^\./||' | LC_ALL=C sort | xargs sha256sum) > "${round}/SHA256SUMS"
    echo "round ${n}:"
    cat "${round}/SHA256SUMS"
done
diff -u "${rounds}/round-1/SHA256SUMS" "${rounds}/round-2/SHA256SUMS" ||
    die "the ${component} packages differ between the two builds"

mkdir -p "${out}/artifacts" "${out}/extra"
for f in "${rounds}"/round-1/out/*; do
    case "$f" in
        *.ipk) cp "$f" "${out}/artifacts/" ;;
        *) cp "$f" "${out}/extra/" ;;
    esac
done
echo "build-keyboard-packages: ${component} ${version} is reproducible"
ls -l "${out}/artifacts" "${out}/extra"
