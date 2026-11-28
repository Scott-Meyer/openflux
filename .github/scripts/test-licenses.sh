#!/usr/bin/env bash
# Checks the third-party license tooling every release uses
# (scripts/licenses/):
#   - about.toml accepts exactly the licenses deny.toml allows
#   - the keyboard build images pin the same cargo-about as
#     install-cargo-about.sh
#   - rust-licenses.sh can write the license file of every shipped Rust
#     binary for every release target (no crate with an unknown or
#     unaccepted license), and npm-licenses.mjs that of Studio's interface
#   - npm-licenses.mjs accepts and refuses license expressions the way
#     deny.toml's allow list says, and refuses a package without a license
#     file
#
# Needs cargo-about (scripts/licenses/install-cargo-about.sh), Node.js,
# host/studio/node_modules (npm ci) and network access for the Cargo
# registry. Run from the repository root:
#   .github/scripts/test-licenses.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "${here}/../.." && pwd)"
licenses="${repo}/scripts/licenses"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
fails=0
fail() { echo "FAIL: $*" >&2; fails=$((fails + 1)); }

# The quoted strings of a TOML array <key> = [...] in the [licenses] table of
# deny.toml, or at the top level of about.toml, one per line, sorted.
toml_list() {
    local file="$1" table="$2" key="$3"
    awk -v table="$table" -v key="$key" '
        BEGIN { in_table = (table == "") }
        /^\[/ { in_table = ($0 == table) }
        in_table && $0 ~ "^" key "[ ]*=[ ]*\\[" { collecting = 1 }
        collecting { print; if ($0 ~ /\]/) exit }
    ' "$file" | grep -o '"[^"]*"' | tr -d '"' | LC_ALL=C sort
}
toml_list "${repo}/deny.toml" "[licenses]" allow > "${work}/deny"
# about.toml's top-level keys come before its first table.
awk '/^\[/ { exit } { print }' "${repo}/about.toml" > "${work}/about-top.toml"
toml_list "${work}/about-top.toml" "" accepted > "${work}/about"
if [ ! -s "${work}/deny" ]; then
    fail "deny.toml has no [licenses] allow list"
elif diff -u "${work}/deny" "${work}/about" > "${work}/diff"; then
    echo "ok: about.toml accepts the $(wc -l < "${work}/deny" | tr -d ' ') licenses deny.toml allows"
else
    fail "about.toml accepted differs from deny.toml allow:
$(cat "${work}/diff")"
fi

# The cargo-about pin.
version="$(sed -n 's/^version=//p' "${licenses}/install-cargo-about.sh")"
sha_aarch64="$(sed -n 's/^ *Linux\/aarch64.* sha=\([0-9a-f]*\).*/\1/p' "${licenses}/install-cargo-about.sh")"
sha_x86_64="$(sed -n 's/^ *Linux\/x86_64.* sha=\([0-9a-f]*\).*/\1/p' "${licenses}/install-cargo-about.sh")"
[ -n "$version" ] && [ -n "$sha_aarch64" ] && [ -n "$sha_x86_64" ] || fail "cannot read the pin in install-cargo-about.sh"
for dockerfile in keyboard/bridge/Dockerfile.build keyboard/display/Dockerfile.build; do
    f="${repo}/${dockerfile}"
    if grep -qx "ARG CARGO_ABOUT_VERSION=${version}" "$f" &&
        grep -qx "ARG CARGO_ABOUT_SHA256_AARCH64=${sha_aarch64}" "$f" &&
        grep -qx "ARG CARGO_ABOUT_SHA256_X86_64=${sha_x86_64}" "$f"; then
        echo "ok: ${dockerfile} installs cargo-about ${version}"
    else
        fail "${dockerfile} does not pin cargo-about ${version} with the checksums of install-cargo-about.sh"
    fi
done

# Every shipped Rust binary, for all the targets it is released for.
host_targets=(aarch64-apple-darwin x86_64-apple-darwin x86_64-pc-windows-msvc x86_64-unknown-linux-gnu
    aarch64-unknown-linux-gnu)
# rust_licenses <manifest> <name> <target>...
rust_licenses() {
    local manifest="$1" name="$2" args=() t
    shift 2
    for t in "$@"; do args+=(--target "$t"); done
    if "${licenses}/rust-licenses.sh" --manifest-path "${repo}/${manifest}" "${args[@]}" \
        -o "${work}/${name}.txt" > "${work}/${name}.log" 2>&1 && [ -s "${work}/${name}.txt" ]; then
        echo "ok: $(tail -n 1 "${work}/${name}.log")"
    else
        fail "license file of ${name}: $(cat "${work}/${name}.log")"
    fi
}
rust_licenses host/daemon/Cargo.toml keyglassd "${host_targets[@]}"
rust_licenses host/studio/src-tauri/Cargo.toml keyglass-studio "${host_targets[@]}"
rust_licenses keyboard/bridge/Cargo.toml keyglass-bridge aarch64-unknown-linux-musl
rust_licenses keyboard/display/Cargo.toml keyglass-display aarch64-unknown-linux-gnu
grep -q 'GNU LESSER GENERAL PUBLIC LICENSE' "${work}/keyglassd.txt" ||
    fail "keyglassd's license file lacks the LGPL-2.1 text of the libusb it links"

# Studio's interface.
if node "${licenses}/npm-licenses.mjs" --package-dir "${repo}/host/studio" --bundled vite \
    -o "${work}/ui.txt" > "${work}/ui.log" 2>&1 && [ -s "${work}/ui.txt" ]; then
    echo "ok: $(tail -n 1 "${work}/ui.log")"
else
    fail "license file of Studio's interface: $(cat "${work}/ui.log")"
fi

# npm-licenses.mjs on a stand-in project with one production package.
# npm_case <pass|fail> <description> <license field or -> [<license file>]
npm_case() {
    local want="$1" what="$2" license="$3" file="${4:-}" dir="${work}/npm" status=0 out manifest
    rm -rf "$dir"
    mkdir -p "${dir}/node_modules/dep"
    if [ "$license" = - ]; then manifest='{"name":"dep","version":"1.0.0"}'
    else manifest="{\"name\":\"dep\",\"version\":\"1.0.0\",\"license\":\"${license}\"}"; fi
    printf '%s\n' "$manifest" > "${dir}/node_modules/dep/package.json"
    [ -z "$file" ] || printf 'license text\n' > "${dir}/node_modules/dep/${file}"
    printf '{"name":"t","version":"1.0.0","dependencies":{"dep":"1.0.0"}}\n' > "${dir}/package.json"
    printf '%s\n' '{"lockfileVersion":3,"packages":{"":{"name":"t"},"node_modules/dep":{"version":"1.0.0"},"node_modules/devtool":{"version":"1.0.0","dev":true}}}' \
        > "${dir}/package-lock.json"
    out="$(node "${licenses}/npm-licenses.mjs" --package-dir "$dir" -o "${dir}/out.txt" 2>&1)" || status=$?
    if [ "$want" = pass ] && [ "$status" -ne 0 ]; then fail "npm-licenses: ${what}: refused: ${out}"; return; fi
    if [ "$want" = fail ] && [ "$status" -eq 0 ]; then fail "npm-licenses: ${what}: accepted"; return; fi
    if [ "$want" = pass ] && ! grep -q '^license text$' "${dir}/out.txt"; then fail "npm-licenses: ${what}: no license text"; return; fi
    echo "ok: npm-licenses: ${what}"
}
npm_case pass "an allowed license" MIT LICENSE
npm_case pass "an OR with one allowed alternative" "(GPL-3.0-only OR MIT)" LICENSE.md
npm_case pass "an allowed WITH exception" "Apache-2.0 WITH LLVM-exception" LICENSE-APACHE
npm_case pass "an AND of allowed licenses" "MIT AND ISC" COPYING
npm_case fail "a license deny.toml does not allow" GPL-3.0-only LICENSE
npm_case fail "an AND with one license not allowed" "MIT AND GPL-3.0-only" LICENSE
npm_case fail "no declared license" - LICENSE
npm_case fail "no license file" MIT

if [ "$fails" -gt 0 ]; then
    echo "test-licenses: ${fails} failures" >&2
    exit 1
fi
echo "test-licenses: ok"
