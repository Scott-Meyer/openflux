#!/usr/bin/env bash
# End-to-end dry run of index publication (stage-releases.sh, build-index.sh,
# write-site-manifest.sh) against a local HTTP stand-in for GitHub Pages and
# GitHub Releases. Every key is generated for the run; nothing is published.
#
# Verifies the promises the release pipeline makes:
#   1. First publish: staged keyboard packages (firmware, bridge) are signed,
#      placed on Pages under packages/<component>/<version>/, and accepted by
#      a host built with the root and initial release keys.
#   2. Subsequent publish (display + nothing-new refresh): previously published
#      Pages packages and compliance sources are preserved and sha256-verified
#      across deploys.
#   3. Rollback protection: refuses to publish over an index older than the
#      last recorded sequence, or a missing index after one was recorded.
#   4. Key rotation (A -> B): root-signed trust statement, re-signing every
#      published artifact (both Pages-hosted and Release-hosted), and revoking A.
#      A host with only root + A accepts B and rejects an index signed by A.
#
# Run from the repository root: .github/scripts/test-publish-index.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "${here}/../.." && pwd)"
die() { echo "test-publish-index: FAIL: $*" >&2; exit 1; }
step() { echo; echo "== $*"; }

cargo build --quiet --locked --release --manifest-path "${repo}/crates/keyglass-update/Cargo.toml" --bin keyglass-release
tool="$(cargo metadata --format-version 1 --no-deps --manifest-path "${repo}/crates/keyglass-update/Cargo.toml" | jq -r .target_directory)/release/keyglass-release"

work="$(mktemp -d)"
server_pid=""
cleanup() {
    if [ -n "$server_pid" ]; then kill "$server_pid" 2> /dev/null || true; fi
    rm -rf "$work"
}
trap cleanup EXIT

export FAKE_GH_ROOT="${work}/web" GITHUB_REPOSITORY=example/keyglass GH_TOKEN=unused
export PATH="${here}/testing:${PATH}"
mkdir -p "${FAKE_GH_ROOT}/pages"
port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$FAKE_GH_ROOT" > "${work}/http.log" 2>&1 &
server_pid=$!
export GITHUB_SERVER_URL="http://127.0.0.1:${port}"
pages="${GITHUB_SERVER_URL}/pages"
for _ in $(seq 1 50); do curl --silent --output /dev/null "$GITHUB_SERVER_URL/" && break; sleep 0.1; done

# Keys: root, initial release key A (built into hosts), and rotated key B.
"$tool" keygen --out-dir "${work}/keys" --name root 2> /dev/null
"$tool" keygen --out-dir "${work}/keys" --name a 2> /dev/null
"$tool" keygen --out-dir "${work}/keys" --name b 2> /dev/null
key() { find "${work}/keys" -name "$1-*.$2"; }
root_pub="$(key root pub)" a_pub="$(key a pub)" b_pub="$(key b pub)"
ROOT_KEY="$(cat "$(key root key)")" A_KEY="$(cat "$(key a key)")" B_KEY="$(cat "$(key b key)")"
export ROOT_KEY
a_id="$(basename "$a_pub" .pub)" a_id="${a_id#a-}"

pipeline_keys="${work}/pipeline-keys"
mkdir -p "${pipeline_keys}/statements"
cp "$a_pub" "${pipeline_keys}/active-release.pub"

compat_100="${work}/compat-1.0.0.toml"
sed -E 's/^version = "[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?"/version = "1.0.0"/' "${repo}/compat.toml" > "$compat_100"

# stage_release <staged-root> <component> <version> <artifact>...
stage_release() {
    local root="$1" comp="$2" ver="$3"
    shift 3
    local tag="${comp}-v${ver}"
    local dir="${root}/staged-${comp}"
    rm -rf "$dir"
    mkdir -p "${dir}/artifacts" "${dir}/source"
    cp "$compat_100" "${dir}/compat.toml"
    for name in "$@"; do
        printf 'contents of %s %s\n' "$tag" "$name" > "${dir}/artifacts/${name}"
    done
    printf 'license/source for %s\n' "$tag" > "${dir}/source/${comp}-source.txt"
    jq -n \
        --arg component "$comp" \
        --arg version "$ver" \
        --arg channel stable \
        --arg tag "$tag" \
        --arg source_date_epoch 1700000000 \
        --arg notes_url "${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/releases/tag/${tag}" \
        '{component: $component, version: $version, channel: $channel, tag: $tag, source_date_epoch: $source_date_epoch, notes_url: $notes_url}' \
        > "${dir}/meta.json"
}

recorded=0
# publish <staged-dir> [build-index.sh args...]: simulates sign + deploy to Pages.
publish() {
    local staged="$1"
    shift
    rm -rf "${work}/releases" "${work}/site" || return 1
    mkdir -p "${work}/releases" "${work}/site" || return 1
    "${here}/stage-releases.sh" \
        --tool "$tool" \
        --staged "$staged" \
        --pages-url "$pages" \
        --active-pubkey "${pipeline_keys}/active-release.pub" \
        --releases-out "${work}/releases" \
        --site-out "${work}/site" || return 1
    "${here}/build-index.sh" \
        --tool "$tool" \
        --pages-url "$pages" \
        --releases "${work}/releases" \
        --keys "$pipeline_keys" \
        --factory "${repo}/factory-reference.toml" \
        --recorded-sequence "$recorded" \
        --root-pubkey "$root_pub" \
        --initial-pubkey "$a_pub" \
        --expires-days 365 "$@" \
        -o "${work}/site" > "${work}/build-index.out" || return 1
    "${here}/write-site-manifest.sh" "${work}/site" > /dev/null || return 1
    # Replace the entire served Pages directory just like actions/deploy-pages does.
    rm -rf "${FAKE_GH_ROOT}/pages" && mkdir -p "${FAKE_GH_ROOT}/pages" || return 1
    cp -R "${work}/site"/. "${FAKE_GH_ROOT}/pages/" || return 1
    recorded="$(sed -n 's/^sequence=//p' "${work}/build-index.out")"
    cp "${work}/site/index.json" "${work}/index-${recorded}.json" || return 1
    cp "${work}/site/index.json.sig" "${work}/index-${recorded}.json.sig" || return 1
    echo "published sequence ${recorded}"
}

index() { jq -r "$1" "${FAKE_GH_ROOT}/pages/index.json"; }
host_accepts() {
    local idx="$1"
    shift
    rm -rf "${work}/host"
    "$tool" bundle --index "$idx" "$@" --channel beta --component firmware --component bridge --component display --allow-missing -o "${work}/host" > "${work}/host.log" 2>&1
}

step "1. first publish (firmware + bridge hosted on Pages)"
export KEYGLASS_RELEASE_KEY="$A_KEY"
rm -rf "${work}/staged" && mkdir -p "${work}/staged"
stage_release "${work}/staged" firmware 1.0.0 keyglass-firmware-1.0.0.uf2
stage_release "${work}/staged" bridge 1.0.0 keyglass-bridge_1.0.0_aarch64.ipk keyglass-factory-restore_1.0.0_aarch64.ipk
publish "${work}/staged"

[ "$(index .sequence)" = 1 ] || die "expected sequence 1, got $(index .sequence)"
[ "$(index '[.releases[][]] | length')" = 2 ] || die "expected 2 releases"
[ "$(index '.releases[][] | select(.component == "firmware") | .artifacts[0].url')" = "${pages}/packages/firmware/1.0.0/keyglass-firmware-1.0.0.uf2" ] ||
    die "firmware artifact URL does not point to Pages"
[ -f "${FAKE_GH_ROOT}/pages/packages/firmware/1.0.0/keyglass-firmware-1.0.0.uf2" ] || die "firmware file missing from Pages"
[ -f "${FAKE_GH_ROOT}/pages/source/firmware-source.txt" ] || die "firmware source file missing from Pages"
host_accepts "${pages}/index.json" --pubkey "$a_pub" --root-pubkey "$root_pub" ||
    die "host refuses the first live index: $(cat "${work}/host.log")"
echo "ok: sequence 1, keyboard packages hosted on Pages and verified by host"

step "2. subsequent publish preserves prior Pages packages and sources"
rm -rf "${work}/staged" && mkdir -p "${work}/staged"
stage_release "${work}/staged" display 1.0.0 keyglass-display_1.0.0_aarch64.ipk
publish "${work}/staged"

[ "$(index .sequence)" = 2 ] || die "expected sequence 2"
[ "$(index '[.releases[][]] | length')" = 3 ] || die "expected 3 releases"
[ -f "${FAKE_GH_ROOT}/pages/packages/firmware/1.0.0/keyglass-firmware-1.0.0.uf2" ] || die "prior firmware package lost across deploy"
[ -f "${FAKE_GH_ROOT}/pages/packages/bridge/1.0.0/keyglass-bridge_1.0.0_aarch64.ipk" ] || die "prior bridge package lost across deploy"
[ -f "${FAKE_GH_ROOT}/pages/packages/display/1.0.0/keyglass-display_1.0.0_aarch64.ipk" ] || die "new display package missing"
[ -f "${FAKE_GH_ROOT}/pages/source/firmware-source.txt" ] || die "prior source file lost across deploy"
host_accepts "${pages}/index.json" --pubkey "$a_pub" --root-pubkey "$root_pub" ||
    die "host refuses sequence 2: $(cat "${work}/host.log")"

# Also verify a refresh with nothing staged preserves everything.
rm -rf "${work}/staged" && mkdir -p "${work}/staged"
publish "${work}/staged"
[ "$(index .sequence)" = 3 ] || die "expected sequence 3"
[ -f "${FAKE_GH_ROOT}/pages/packages/display/1.0.0/keyglass-display_1.0.0_aarch64.ipk" ] || die "display package lost on empty refresh"
echo "ok: sequence 3, all Pages packages and sources preserved across deploys"

step "3. rollback protection refuses older or missing index"
cp "${work}/index-1.json" "${FAKE_GH_ROOT}/pages/index.json"
cp "${work}/index-1.json.sig" "${FAKE_GH_ROOT}/pages/index.json.sig"
if publish "${work}/staged" > "${work}/refused.log" 2>&1; then die "published over an older index"; fi
grep -q 'refusing to publish over an older index' "${work}/refused.log" || die "unexpected refusal: $(cat "${work}/refused.log")"
rm "${FAKE_GH_ROOT}/pages/index.json" "${FAKE_GH_ROOT}/pages/index.json.sig"
if publish "${work}/staged" > "${work}/refused.log" 2>&1; then die "started a new index after one was recorded"; fi
grep -q 'refusing to start a new index' "${work}/refused.log" || die "unexpected refusal: $(cat "${work}/refused.log")"
cp "${work}/index-3.json" "${FAKE_GH_ROOT}/pages/index.json"
cp "${work}/index-3.json.sig" "${FAKE_GH_ROOT}/pages/index.json.sig"
echo "ok: rollback refused"

step "4. host release references GitHub Release URLs and rotates cleanly with key B"
gh release create host-v1.0.0
rm -rf "${work}/staged" && mkdir -p "${work}/staged"
stage_release "${work}/staged" host 1.0.0 keyglass-studio_1.0.0_universal.dmg keyglass-studio_1.0.0_x64-setup.exe
"${here}/upload-release-assets.sh" host-v1.0.0 \
    "${work}/staged/staged-host/artifacts/keyglass-studio_1.0.0_universal.dmg" \
    "${work}/staged/staged-host/artifacts/keyglass-studio_1.0.0_x64-setup.exe" > /dev/null
publish "${work}/staged"
[ "$(index '.releases[][] | select(.component == "host") | .artifacts[0].url')" = "${GITHUB_SERVER_URL}/${GITHUB_REPOSITORY}/releases/download/host-v1.0.0/keyglass-studio_1.0.0_universal.dmg" ] ||
    die "host artifact URL does not point to GitHub release"

"$tool" statement --root-key-env ROOT_KEY --trust "$b_pub" -o "${pipeline_keys}/statements/trust-b.json" 2> /dev/null
cp "$a_pub" "${pipeline_keys}/previous-release.pub"
cp "$b_pub" "${pipeline_keys}/active-release.pub"
export KEYGLASS_RELEASE_KEY="$B_KEY"
rm -rf "${work}/staged" && mkdir -p "${work}/staged"
publish "${work}/staged" --resign
[ "$(index '[.releases[][] | .artifacts[] | .signatures | length] | unique')" = "$(printf '[\n  2\n]')" ] ||
    die "after re-signing every artifact should carry two signatures"
host_accepts "${pages}/index.json" --pubkey "$a_pub" --root-pubkey "$root_pub" ||
    die "host built with A refuses rotated index signed by B: $(cat "${work}/host.log")"

"$tool" statement --root-key-env ROOT_KEY --revoke "$a_id" --reason "rotated" -o "${pipeline_keys}/statements/revoke-a.json" 2> /dev/null
rm "${pipeline_keys}/previous-release.pub"
publish "${work}/staged"
host_accepts "${pages}/index.json" --pubkey "$a_pub" --root-pubkey "$root_pub" ||
    die "host refuses index after A is revoked: $(cat "${work}/host.log")"
cp "${FAKE_GH_ROOT}/pages/index.json" "${work}/forged.json"
KEYGLASS_RELEASE_KEY="$A_KEY" "$tool" sign --key-env KEYGLASS_RELEASE_KEY "${work}/forged.json" 2> /dev/null
if host_accepts "${work}/forged.json" --pubkey "$a_pub" --root-pubkey "$root_pub"; then
    die "host accepts an index signed by revoked key A"
fi
echo "ok: key rotation verified (sequence ${recorded})"

echo
echo "test-publish-index: ok (last sequence ${recorded})"
