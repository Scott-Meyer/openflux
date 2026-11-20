#!/usr/bin/env bash
# Checks that `openflux-release bundle`, which collects the keyboard software
# for the host installers, refuses indexes it must not build from:
#   - an index with no releases
#   - an index whose keyboard releases are all betas
# In both cases it has to fail and say that no stable keyboard release is
# published, rather than produce an empty or partial bundle.
#
# Uses a throwaway key pair and test indexes in a temporary directory; nothing
# is published. Run from the repository root:
#   .github/scripts/check-bundle-states.sh

set -euo pipefail

die() { echo "check-bundle-states: $*" >&2; exit 1; }

cargo build --locked --release --manifest-path crates/openflux-update/Cargo.toml --bin openflux-release
tool="$(cargo metadata --format-version 1 --no-deps --manifest-path crates/openflux-update/Cargo.toml | jq -r .target_directory)/release/openflux-release"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

"$tool" keygen --out-dir "${work}/keys" --name test 2> /dev/null
pub="$(find "${work}/keys" -name 'test-*.pub')"
TEST_RELEASE_KEY="$(cat "$(find "${work}/keys" -name 'test-*.key')")"
export TEST_RELEASE_KEY

expect_refusal() {
    local name="$1" index="$2" log="${work}/${1}.log"
    "$tool" sign --key-env TEST_RELEASE_KEY "$index" 2> /dev/null
    if "$tool" bundle --index "$index" --pubkey "$pub" --channel stable \
        --component firmware --component bridge --component display -o "${work}/${name}-bundle" > "$log" 2>&1; then
        die "${name}: bundle succeeded; it must refuse an index without stable keyboard releases"
    fi
    grep -qi 'stable' "$log" || die "${name}: bundle failed without explaining that no stable release exists: $(cat "$log")"
    if [ -d "${work}/${name}-bundle" ] && [ -n "$(ls -A "${work}/${name}-bundle")" ]; then
        die "${name}: bundle left files in its output directory"
    fi
    echo "${name}: refused ($(head -n 1 "$log"))"
}

# No releases at all.
mkdir -p "${work}/empty"
"$tool" index --releases "${work}/empty" --previous none --pubkey "$pub" --expires-days 30 \
    -o "${work}/empty-index.json" 2> /dev/null
expect_refusal empty "${work}/empty-index.json"

# Only beta keyboard releases.
mkdir -p "${work}/beta" "${work}/beta-files"
for c in firmware bridge display; do
    sed "/^\[components\.${c}\]/,/^\[/ s/^version = .*/version = \"1.1.0-beta.1\"/" compat.toml > "${work}/compat-${c}.toml"
    printf 'test artifact for %s\n' "$c" > "${work}/beta-files/openflux-${c}-1.1.0-beta.1.bin"
    "$tool" sign --key-env TEST_RELEASE_KEY "${work}/beta-files/openflux-${c}-1.1.0-beta.1.bin" 2> /dev/null
    "$tool" manifest --component "$c" --version 1.1.0-beta.1 --channel beta --compat "${work}/compat-${c}.toml" \
        --artifact "${work}/beta-files/openflux-${c}-1.1.0-beta.1.bin" \
        --base-url "file://${work}/beta-files" --notes-url https://example.invalid/notes \
        -o "${work}/beta/${c}.json" 2> /dev/null
done
"$tool" index --releases "${work}/beta" --previous none --pubkey "$pub" --expires-days 30 \
    -o "${work}/beta-index.json" 2> /dev/null
expect_refusal beta-only "${work}/beta-index.json"

echo "check-bundle-states: ok"
