#!/usr/bin/env bash
# Builds and signs the next update index from the published one.
#
#   1. Fetch the published index.json and index.json.sig from the site,
#      bypassing caches, and verify the signature against the release keys
#      the pipeline knows (the active key, the key it replaces during a
#      rotation, and the initial key built into every host).
#   2. Refuse to continue if its sequence is lower than the last sequence
#      recorded as published (--recorded-sequence): the site is serving an
#      older index than one that was already deployed, and building on it
#      would drop what came after it. With no published index, the recorded
#      sequence must be 0 (no index was ever published).
#   3. With --resign, sign every artifact of every published release again
#      with the release key in the environment, for a key rotation.
#   4. `openflux-release index`: merge the published index with the release
#      manifests in --releases, record the active key, the factory reference
#      and the key statements, apply withdrawals, and increment the sequence.
#   5. Sign the index with the release key, verify it with the active key,
#      and check it the way a host does: with only the root and initial
#      release keys built into the host, the new index must be accepted.
#
# Usage:
#   build-index.sh --tool <openflux-release> --pages-url <url> --releases <dir>
#       --keys <dir> --factory <factory-reference.toml> --recorded-sequence <n>
#       [--expires-days <n>] [--withdraw "<component>@<version> ..."] [--resign]
#       [--root-pubkey <file>] [--initial-pubkey <file>] -o <site dir>
#
# --keys holds active-release.pub, optionally previous-release.pub, and
# statements/*.json (root-signed key statements to carry in the index).
# --root-pubkey and --initial-pubkey default to the keys built into hosts
# (crates/openflux-update/keys/root.pub and release.pub).
#
# Writes <site dir>/index.json and index.json.sig, and prints sequence=<n>
# and sha256=<hex> (also to $GITHUB_OUTPUT when set).
#
# Environment: OPENFLUX_RELEASE_KEY, and OPENFLUX_RELEASE_KEY_PASSWORD when
# the key has one. Requires curl, jq and sha256sum.

set -euo pipefail

die() { echo "build-index: $*" >&2; exit 1; }

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tool="" pages_url="" releases="" keys="" factory="" recorded="" out=""
expires_days=90 withdraw="" resign=false
root_pub="${repo}/crates/openflux-update/keys/root.pub"
initial_pub="${repo}/crates/openflux-update/keys/release.pub"
while [ $# -gt 0 ]; do
    case "$1" in
        --tool) tool="$2"; shift 2 ;;
        --pages-url) pages_url="${2%/}"; shift 2 ;;
        --releases) releases="$2"; shift 2 ;;
        --keys) keys="$2"; shift 2 ;;
        --factory) factory="$2"; shift 2 ;;
        --recorded-sequence) recorded="$2"; shift 2 ;;
        --expires-days) expires_days="$2"; shift 2 ;;
        --withdraw) withdraw="$2"; shift 2 ;;
        --resign) resign=true; shift ;;
        --root-pubkey) root_pub="$2"; shift 2 ;;
        --initial-pubkey) initial_pub="$2"; shift 2 ;;
        -o) out="$2"; shift 2 ;;
        *) die "unknown argument: $1" ;;
    esac
done
for v in tool pages_url releases keys factory recorded out; do
    [ -n "${!v}" ] || die "--${v//_/-} is required"
done
[[ "$recorded" =~ ^[0-9]+$ ]] || die "--recorded-sequence must be a number, not ${recorded}"
active_pub="${keys}/active-release.pub"
[ -f "$active_pub" ] || die "${active_pub}: not found"
[ -d "$releases" ] || die "${releases}: not a directory"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$out"

sign_args=(--key-env OPENFLUX_RELEASE_KEY)
[ -n "${OPENFLUX_RELEASE_KEY_PASSWORD:-}" ] && sign_args+=(--password-env OPENFLUX_RELEASE_KEY_PASSWORD)

# 1. The published index.
known_keys=(--pubkey "$active_pub" --pubkey "$initial_pub")
[ -f "${keys}/previous-release.pub" ] && known_keys+=(--pubkey "${keys}/previous-release.pub")
fresh="fresh=$(date +%s)-$$"
status="$(curl --silent --show-error --location --output "${work}/index.json" --write-out '%{http_code}' \
    -H 'Cache-Control: no-cache' "${pages_url}/index.json?${fresh}")" || status=000
case "$status" in
    200)
        curl --fail --silent --show-error --location --output "${work}/index.json.sig" \
            -H 'Cache-Control: no-cache' "${pages_url}/index.json.sig?${fresh}"
        "$tool" verify "${known_keys[@]}" --signature "${work}/index.json.sig" "${work}/index.json" >&2 ||
            die "the published index is not signed by a known release key; not building on it"
        published="$(jq -er '.sequence' "${work}/index.json")" || die "the published index has no sequence"
        echo "build-index: published index has sequence ${published}; last recorded ${recorded}" >&2
        # 2. Never build on an index older than one already deployed.
        if [ "$published" -lt "$recorded" ]; then
            die "the site serves index sequence ${published}, but sequence ${recorded} was already published; refusing to publish over an older index (see docs/releasing.md, 'The published index is older than the last one')"
        fi
        previous=(--previous "${work}/index.json")
        ;;
    404)
        if [ "$recorded" -ne 0 ]; then
            die "the site has no index.json, but sequence ${recorded} was already published; refusing to start a new index (see docs/releasing.md, 'The published index is older than the last one')"
        fi
        echo "build-index: no index is published yet; this is the first index" >&2
        previous=(--previous none)
        ;;
    *)
        die "fetching ${pages_url}/index.json returned HTTP ${status}; not publishing over an index that could not be read"
        ;;
esac

# 3. Signatures by the active key for every published artifact.
resign_args=()
if [ "$resign" = true ]; then
    [ "${previous[1]}" != none ] || die "--resign needs a published index"
    mkdir -p "${work}/resign"
    while IFS=$'\t' read -r name url; do
        echo "build-index: signing ${name} again" >&2
        curl --fail --silent --show-error --location --output "${work}/resign/${name}" "$url"
        "$tool" sign "${sign_args[@]}" "${work}/resign/${name}" 2>&1 | sed 's/^/  /' >&2
        "$tool" verify --pubkey "$active_pub" "${work}/resign/${name}" > /dev/null
        resign_args+=(--signature "${work}/resign/${name}.sig")
    done < <(jq -r '.releases[][] | .artifacts[] | "\(.name)\t\(.url)"' "${work}/index.json")
    echo "build-index: $(( ${#resign_args[@]} / 2 )) artifacts signed again" >&2
fi

# 4. The next index.
statements=()
for s in "${keys}"/statements/*.json; do
    [ -f "$s" ] && statements+=(--statement "$s")
done
withdraw_args=()
for w in $withdraw; do withdraw_args+=(--withdraw "$w"); done
"$tool" index --releases "$releases" "${previous[@]}" --expires-days "$expires_days" \
    --pubkey "$active_pub" --factory "$factory" \
    "${statements[@]}" "${resign_args[@]}" "${withdraw_args[@]}" -o "${out}/index.json"
count="$(jq '[.releases[][]] | length' "${out}/index.json")"
[ "$count" -gt 0 ] || die "the index has no releases; nothing is published"
jq -r '.releases | to_entries[] | .key as $c | .value[] | "  \($c)\t\(.component) \(.version)\t\(.artifacts | length) artifacts\(if .withdrawn then " (withdrawn)" else "" end)"' "${out}/index.json" >&2

# 5. Sign, verify, and check it as a host would.
"$tool" sign "${sign_args[@]}" "${out}/index.json"
"$tool" verify --pubkey "$active_pub" "${out}/index.json" >&2 ||
    die "the index signature does not verify with ${active_pub}: the release key secret and the active public key differ"
host_check="${work}/host-check"
"$tool" bundle --index "${out}/index.json" --pubkey "$initial_pub" --root-pubkey "$root_pub" \
    --channel beta --component firmware --allow-missing -o "$host_check" >&2 ||
    die "a host built with ${initial_pub} and ${root_pub} would refuse this index (is a root-signed statement for the active key missing?)"

sequence="$(jq -r .sequence "${out}/index.json")"
digest="$(sha256sum "${out}/index.json" | cut -d' ' -f1)"
{
    echo "sequence=${sequence}"
    echo "sha256=${digest}"
} | tee -a "${GITHUB_OUTPUT:-/dev/null}"
