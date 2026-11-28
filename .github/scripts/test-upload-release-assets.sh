#!/usr/bin/env bash
# Dry run of upload-release-assets.sh against a stand-in for the GitHub CLI
# (testing/gh): published release files are never replaced.
#   - new files are uploaded, in the order given
#   - a file identical to the published one is skipped
#   - a file whose bytes differ fails with "published artifacts are
#     immutable; cut a new version", and nothing of that call is uploaded
#   - --clobber is never used
#
# Run from the repository root: .github/scripts/test-upload-release-assets.sh

set -euo pipefail

die() { echo "test-upload-release-assets: FAIL: $*" >&2; exit 1; }

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
export FAKE_GH_ROOT="${work}/gh" GITHUB_REPOSITORY=example/keyglass GH_TOKEN=unused
export PATH="${here}/testing:${PATH}"
mkdir -p "$FAKE_GH_ROOT" "${work}/files"
upload="${here}/upload-release-assets.sh"
published="${FAKE_GH_ROOT}/example/keyglass/releases/download/bridge-v1.0.0"

gh release create bridge-v1.0.0
printf 'package\n' > "${work}/files/keyglass-bridge_1.0.0_aarch64.ipk"
printf 'sums\n' > "${work}/files/SHA256SUMS"
printf 'keyglass-bridge_1.0.0_aarch64.ipk\n' > "${work}/files/artifacts.txt"

# New files: uploaded in order.
"$upload" bridge-v1.0.0 "${work}/files/keyglass-bridge_1.0.0_aarch64.ipk" "${work}/files/SHA256SUMS" "${work}/files/artifacts.txt"
for f in keyglass-bridge_1.0.0_aarch64.ipk SHA256SUMS artifacts.txt; do
    cmp -s "${work}/files/${f}" "${published}/${f}" || die "${f} was not uploaded"
done
order="$(grep 'release upload' "${FAKE_GH_ROOT}/calls.log" | awk '{print $NF}' | xargs -n1 basename | tr '\n' ' ')"
[ "$order" = "keyglass-bridge_1.0.0_aarch64.ipk SHA256SUMS artifacts.txt " ] || die "upload order was: ${order}"
echo "ok: new files uploaded in order"

# The same files again: all skipped.
: > "${FAKE_GH_ROOT}/calls.log"
out="$("$upload" bridge-v1.0.0 "${work}/files/keyglass-bridge_1.0.0_aarch64.ipk" "${work}/files/artifacts.txt")"
grep -q 'release upload' "${FAKE_GH_ROOT}/calls.log" && die "identical files were uploaded again"
[ "$(grep -c 'same contents; skipped' <<<"$out")" -eq 2 ] || die "identical files were not reported as skipped: ${out}"
echo "ok: identical files skipped"

# A changed file next to a new one: fails, and nothing is uploaded.
: > "${FAKE_GH_ROOT}/calls.log"
printf 'rebuilt package\n' > "${work}/files/keyglass-bridge_1.0.0_aarch64.ipk"
printf 'notes\n' > "${work}/files/notes.txt"
if out="$("$upload" bridge-v1.0.0 "${work}/files/notes.txt" "${work}/files/keyglass-bridge_1.0.0_aarch64.ipk" 2>&1)"; then
    die "a changed file was accepted"
fi
grep -q 'published artifacts are immutable; cut a new version' <<<"$out" || die "unexpected message: ${out}"
grep -q 'release upload' "${FAKE_GH_ROOT}/calls.log" && die "files were uploaded despite the conflict"
[ "$(cat "${published}/keyglass-bridge_1.0.0_aarch64.ipk")" = package ] || die "the published file changed"
[ ! -e "${published}/notes.txt" ] || die "notes.txt was uploaded despite the conflict"
echo "ok: a changed file is refused: $(tail -n 1 <<<"$out")"

users="$(grep -rl -- '--clobber' "${here}"/*.sh "${here}"/*.py "${here}/../actions" "${here}/../workflows" | grep -v '/test-' || true)"
[ -z "$users" ] || die "--clobber is used by: ${users}"
echo "ok: no release script uses --clobber"

echo "test-upload-release-assets: ok"
