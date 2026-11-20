#!/usr/bin/env bash
# Dry run of the index publication (finalize-releases.sh, build-index.sh and
# upload-release-assets.sh) against a local stand-in for GitHub: releases
# live in a directory served over HTTP, the GitHub CLI is testing/gh, and
# "deploying" copies the built site into the served Pages directory. Every
# key is generated for the run; nothing is published.
#
# The run walks through:
#   1. the first index: two keyboard releases are built (artifacts,
#      SHA256SUMS, artifacts.txt), signed and published at sequence 1, with
#      the factory reference embedded
#   2. a later publication with nothing new (sequence 2), reusing the
#      published release.json files
#   3. refusal to publish when the site serves an older index than the last
#      recorded one, or no index at all after one was recorded
#   4. immutability: a rebuilt artifact with different bytes is refused
#   5. finishing a release whose .sig files were uploaded by an earlier run
#      that stopped before release.json
#   6. a release key rotation from A to B: the root-signed statement trusting
#      B, every published artifact signed again by B (each then carries both
#      signatures), and the revocation of A; at every step a host built with
#      only the root key and A accepts the index, and after the revocation an
#      index signed by A is refused
#
# Run from the repository root: .github/scripts/test-publish-index.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "${here}/../.." && pwd)"
die() { echo "test-publish-index: FAIL: $*" >&2; exit 1; }
step() { echo; echo "== $*"; }

cargo build --quiet --locked --release --manifest-path "${repo}/crates/openflux-update/Cargo.toml" --bin openflux-release
tool="$(cargo metadata --format-version 1 --no-deps --manifest-path "${repo}/crates/openflux-update/Cargo.toml" | jq -r .target_directory)/release/openflux-release"

work="$(mktemp -d)"
server_pid=""
cleanup() {
    if [ -n "$server_pid" ]; then kill "$server_pid" 2> /dev/null || true; fi
    rm -rf "$work"
}
trap cleanup EXIT

# The stand-in GitHub: releases at /<owner>/<repo>/releases/download/<tag>/,
# Pages at /pages/.
export FAKE_GH_ROOT="${work}/web" GITHUB_REPOSITORY=example/openflux GH_TOKEN=unused
export PATH="${here}/testing:${PATH}"
mkdir -p "${FAKE_GH_ROOT}/pages"
port="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$FAKE_GH_ROOT" > "${work}/http.log" 2>&1 &
server_pid=$!
export GITHUB_SERVER_URL="http://127.0.0.1:${port}"
pages="${GITHUB_SERVER_URL}/pages"
for _ in $(seq 1 50); do curl --silent --output /dev/null "$GITHUB_SERVER_URL/" && break; sleep 0.1; done
releases_dir="${FAKE_GH_ROOT}/example/openflux/releases/download"

# Keys: the root key, the initial release key A (built into hosts) and B.
"$tool" keygen --out-dir "${work}/keys" --name root 2> /dev/null
"$tool" keygen --out-dir "${work}/keys" --name a 2> /dev/null
"$tool" keygen --out-dir "${work}/keys" --name b 2> /dev/null
key() { find "${work}/keys" -name "$1-*.$2"; }
root_pub="$(key root pub)" a_pub="$(key a pub)" b_pub="$(key b pub)"
ROOT_KEY="$(cat "$(key root key)")" A_KEY="$(cat "$(key a key)")" B_KEY="$(cat "$(key b key)")"
export ROOT_KEY
a_id="$(basename "$a_pub" .pub)" a_id="${a_id#a-}"

# The pipeline's key directory (host/packaging/keys in the repository).
pipeline_keys="${work}/pipeline-keys"
mkdir -p "${pipeline_keys}/statements"
cp "$a_pub" "${pipeline_keys}/active-release.pub"

# A repository with release tags, for compat.toml and commit times. Git runs
# without the user's or the system's configuration (no signing, no editor).
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_TERMINAL_PROMPT=0 GIT_PAGER=cat GIT_EDITOR=false
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid
src="${work}/src"
mkdir -p "$src"
cp "${repo}/compat.toml" "${src}/compat.toml"
tgit() {
    timeout 60 git -C "$src" -c core.hooksPath=/dev/null -c commit.gpgSign=false -c tag.gpgSign=false "$@" < /dev/null
}
tgit init --quiet
tgit add compat.toml
tgit commit --quiet -m "feat: first release"
for t in firmware-v1.0.0 bridge-v1.0.0 display-v1.0.0; do tgit tag "$t"; done

# build_release <tag> <artifact>...: what a release build uploads.
build_release() {
    local tag="$1" dir="${work}/build-${1}"
    shift
    rm -rf "$dir"
    mkdir -p "$dir"
    gh release create "$tag"
    for name in "$@"; do printf 'contents of %s\n' "$name" > "${dir}/${name}"; done
    printf 'source archive\n' > "${dir}/notes-source.tar.xz"
    (cd "$dir" && printf '%s\n' "$@" > artifacts.txt && sha256sum -- * > SHA256SUMS)
    local files=()
    for name in "$@" notes-source.tar.xz; do files+=("${dir}/${name}"); done
    "${here}/upload-release-assets.sh" "$tag" "${files[@]}" "${dir}/SHA256SUMS" "${dir}/artifacts.txt" > /dev/null
}

recorded=0
# publish [build-index.sh arguments]: one run of the sign job and the deploy job.
publish() {
    # Each command ends in "|| return 1" so a failure stops the run even
    # when publish is called as a condition.
    rm -rf "${work}/releases" "${work}/site" || return 1
    (cd "$src" && "${here}/finalize-releases.sh" "$tool" "${work}/releases" "${pipeline_keys}/active-release.pub") || return 1
    "${here}/build-index.sh" --tool "$tool" --pages-url "$pages" --releases "${work}/releases" \
        --keys "$pipeline_keys" --factory "${repo}/factory-reference.toml" --recorded-sequence "$recorded" \
        --root-pubkey "$root_pub" --initial-pubkey "$a_pub" --expires-days 30 "$@" -o "${work}/site" \
        > "${work}/build-index.out" || return 1
    cp "${work}/site/index.json" "${work}/site/index.json.sig" "${FAKE_GH_ROOT}/pages/" || return 1
    recorded="$(sed -n 's/^sequence=//p' "${work}/build-index.out")"
    cp "${work}/site/index.json" "${work}/index-${recorded}.json" || return 1
    cp "${work}/site/index.json.sig" "${work}/index-${recorded}.json.sig" || return 1
    echo "published sequence ${recorded}"
}
index() { jq -r "$1" "${FAKE_GH_ROOT}/pages/index.json"; }
# host_accepts <index.json> <host trust: --pubkey ...>: the check a host makes.
host_accepts() {
    local idx="$1"
    shift
    rm -rf "${work}/host"
    "$tool" bundle --index "$idx" "$@" --channel beta --component firmware -o "${work}/host" > "${work}/host.log" 2>&1
}

step "1. first index"
export OPENFLUX_RELEASE_KEY="$A_KEY"
build_release firmware-v1.0.0 openflux-firmware-1.0.0.uf2
build_release bridge-v1.0.0 openflux-bridge_1.0.0_aarch64.ipk openflux-factory-restore_1.0.0_aarch64.ipk
publish
[ "$(index .sequence)" = 1 ] || die "the first index has sequence $(index .sequence)"
[ "$(index '[.releases[][]] | length')" = 2 ] || die "the first index does not list both releases"
[ "$(index '.factory.label')" = "$(sed -n 's/^label = "\(.*\)"/\1/p' "${repo}/factory-reference.toml")" ] ||
    die "the factory reference is not in the index"
[ "$(index '.press_refinement_source.payload_sha256')" = "$(sed -n 's/^payload_sha256 = "\(.*\)"/\1/p' "${repo}/factory-reference.toml")" ] ||
    die "the press-refinement source is not in the index"
[ "$(index '[.releases[][] | .artifacts[] | .signatures | length] | unique')" = "$(printf '[\n  1\n]')" ] ||
    die "every artifact should carry one signature"
[ "$(index '.releases[][] | select(.component == "bridge") | [.artifacts[].name] | sort | join(" ")')" = \
    "openflux-bridge_1.0.0_aarch64.ipk openflux-factory-restore_1.0.0_aarch64.ipk" ] ||
    die "the bridge release should list the bridge and factory-restore packages only"
for f in release.json openflux-firmware-1.0.0.uf2.sig; do
    [ -f "${releases_dir}/firmware-v1.0.0/${f}" ] || die "${f} was not uploaded to firmware-v1.0.0"
done
host_accepts "${FAKE_GH_ROOT}/pages/index.json" --pubkey "$a_pub" --root-pubkey "$root_pub" ||
    die "a host refuses the first index: $(cat "${work}/host.log")"
echo "ok: sequence 1, factory reference recorded, artifacts signed, release.json uploaded"

step "2. a publication with nothing new"
cp "${releases_dir}/firmware-v1.0.0/release.json" "${work}/firmware-release.json"
publish
[ "$(index .sequence)" = 2 ] || die "expected sequence 2"
cmp -s "${work}/firmware-release.json" "${releases_dir}/firmware-v1.0.0/release.json" || die "release.json changed"
echo "ok: sequence 2, published release.json reused"

step "3. the published index is older than the last one"
cp "${work}/index-1.json" "${FAKE_GH_ROOT}/pages/index.json"
cp "${work}/index-1.json.sig" "${FAKE_GH_ROOT}/pages/index.json.sig"
if publish > "${work}/refused.log" 2>&1; then die "published over an older index"; fi
grep -q 'refusing to publish over an older index' "${work}/refused.log" || die "unexpected refusal: $(cat "${work}/refused.log")"
echo "ok: refused: $(grep -o 'the site serves index sequence [0-9]*, but sequence [0-9]* was already published' "${work}/refused.log")"
rm "${FAKE_GH_ROOT}/pages/index.json" "${FAKE_GH_ROOT}/pages/index.json.sig"
if publish > "${work}/refused.log" 2>&1; then die "started a new index after one was recorded"; fi
grep -q 'refusing to start a new index' "${work}/refused.log" || die "unexpected refusal: $(cat "${work}/refused.log")"
echo "ok: refused: no index.json on the site after sequence ${recorded}"
cp "${work}/index-2.json" "${FAKE_GH_ROOT}/pages/index.json"
cp "${work}/index-2.json.sig" "${FAKE_GH_ROOT}/pages/index.json.sig"

step "4. published artifacts are immutable"
printf 'rebuilt\n' > "${work}/openflux-firmware-1.0.0.uf2"
if out="$("${here}/upload-release-assets.sh" firmware-v1.0.0 "${work}/openflux-firmware-1.0.0.uf2" 2>&1)"; then
    die "a rebuilt artifact replaced the published one"
fi
grep -q 'published artifacts are immutable; cut a new version' <<<"$out" || die "unexpected message: ${out}"
echo "ok: $(tail -n 1 <<<"$out")"

step "5. a release whose signatures were uploaded before release.json"
build_release display-v1.0.0 openflux-display_1.0.0_aarch64.ipk
cp "${releases_dir}/display-v1.0.0/openflux-display_1.0.0_aarch64.ipk" "${work}/display.ipk"
"$tool" sign --key-env OPENFLUX_RELEASE_KEY "${work}/display.ipk" 2> /dev/null
cp "${work}/display.ipk.sig" "${releases_dir}/display-v1.0.0/openflux-display_1.0.0_aarch64.ipk.sig"
publish
cmp -s "${work}/display.ipk.sig" "${releases_dir}/display-v1.0.0/openflux-display_1.0.0_aarch64.ipk.sig" ||
    die "the uploaded signature was replaced"
[ "$(index '.releases[][] | select(.component == "display") | .artifacts[0].signatures[0]')" = "$(cat "${work}/display.ipk.sig")" ] ||
    die "the index does not carry the uploaded signature"
echo "ok: the uploaded signature is reused; sequence ${recorded}"

step "6. rotating the release key from A to B"
"$tool" statement --root-key-env ROOT_KEY --trust "$b_pub" -o "${pipeline_keys}/statements/trust-b.json" 2> /dev/null
cp "$a_pub" "${pipeline_keys}/previous-release.pub"
cp "$b_pub" "${pipeline_keys}/active-release.pub"
export OPENFLUX_RELEASE_KEY="$B_KEY"
publish --resign
[ "$(index '[.releases[][] | .artifacts[] | .signatures | length] | unique')" = "$(printf '[\n  2\n]')" ] ||
    die "after re-signing every artifact should carry two signatures"
"$tool" verify --pubkey "$b_pub" "${FAKE_GH_ROOT}/pages/index.json" > /dev/null || die "the index is not signed by B"
host_accepts "${FAKE_GH_ROOT}/pages/index.json" --pubkey "$a_pub" --root-pubkey "$root_pub" ||
    die "a host that knows only A refuses the index signed by B: $(cat "${work}/host.log")"
echo "ok: index signed by B, trusted through the root statement; every artifact signed by A and B"

"$tool" statement --root-key-env ROOT_KEY --revoke "$a_id" --reason "rotated" -o "${pipeline_keys}/statements/revoke-a.json" 2> /dev/null
rm "${pipeline_keys}/previous-release.pub"
publish
host_accepts "${FAKE_GH_ROOT}/pages/index.json" --pubkey "$a_pub" --root-pubkey "$root_pub" ||
    die "a host refuses the index after A is revoked: $(cat "${work}/host.log")"
cp "${FAKE_GH_ROOT}/pages/index.json" "${work}/forged.json"
OPENFLUX_RELEASE_KEY="$A_KEY" "$tool" sign --key-env OPENFLUX_RELEASE_KEY "${work}/forged.json" 2> /dev/null
if host_accepts "${work}/forged.json" --pubkey "$a_pub" --root-pubkey "$root_pub"; then
    die "a host accepts an index signed by the revoked key A"
fi
echo "ok: A revoked; a host refuses an index signed by A ($(tail -n 1 "${work}/host.log"))"
[ "$(index .sequence)" = "$recorded" ] || die "the site does not serve the last sequence"

echo
echo "test-publish-index: ok (last sequence ${recorded})"
