#!/usr/bin/env bash
# Installs the pinned cargo-about into <dir> (default ~/.cargo/bin), so
# `cargo about` runs from PATH. It generates the license files of the Rust
# crates compiled into each OpenFlux binary (rust-licenses.sh).
#
# Uses the project's prebuilt release for this machine, checked against the
# SHA-256 recorded here; on a machine without one (an Intel Mac) it builds
# the same version from crates.io with `cargo install --locked`.
#
# The keyboard build images (keyboard/*/Dockerfile.build) install the same
# version; .github/scripts/test-licenses.sh checks that they agree with this
# file.
#
# Usage: install-cargo-about.sh [<dir>]

set -euo pipefail

version=0.9.3
die() { echo "install-cargo-about: $*" >&2; exit 1; }

dir="${1:-${CARGO_HOME:-${HOME}/.cargo}/bin}"
# Git Bash on Windows: CARGO_HOME is a Windows path.
if command -v cygpath > /dev/null 2>&1; then dir="$(cygpath -u "$dir")"; fi
mkdir -p "$dir"

if command -v cargo-about > /dev/null 2>&1 && [ "$(cargo-about --version 2>/dev/null)" = "cargo-about ${version}" ]; then
    echo "install-cargo-about: cargo-about ${version} is already installed ($(command -v cargo-about))"
    exit 0
fi

exe=""
case "$(uname -s)/$(uname -m)" in
    Darwin/arm64) triple=aarch64-apple-darwin sha=45520df8680978a6e7c4023ace0e3f311c73efc208cd68e5a7f4ed4822fa0349 ;;
    Linux/x86_64) triple=x86_64-unknown-linux-musl sha=3ad1e94ea1e63d69d1961f4fb15ccbbe9575d906111f96d2cb4dce6df48f71b4 ;;
    Linux/aarch64|Linux/arm64) triple=aarch64-unknown-linux-musl sha=eb4d9ff6ebd2518152a8308729431a04b3836e71d2bd5ead7aea69f91a71162e ;;
    MINGW*/x86_64|MSYS*/x86_64|CYGWIN*/x86_64) triple=x86_64-pc-windows-msvc sha=9e2942533ef1c592c4a70bdb384cae1550a3f5fc9ae40379b6662e3d2f16fe02 exe=.exe ;;
    *) triple="" ;;
esac

if [ -z "$triple" ]; then
    echo "install-cargo-about: no prebuilt cargo-about for $(uname -s)/$(uname -m); building it"
    cargo install --locked --features cli --root "$(dirname "$dir")" "cargo-about@${version}"
    exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
name="cargo-about-${version}-${triple}"
curl -fsSL --retry 3 -o "${work}/${name}.tar.gz" \
    "https://github.com/EmbarkStudios/cargo-about/releases/download/${version}/${name}.tar.gz"
if command -v sha256sum > /dev/null 2>&1; then
    got="$(sha256sum "${work}/${name}.tar.gz" | cut -d' ' -f1)"
else
    got="$(shasum -a 256 "${work}/${name}.tar.gz" | cut -d' ' -f1)"
fi
[ "$got" = "$sha" ] || die "${name}.tar.gz has SHA-256 ${got}, expected ${sha}"
tar -xzf "${work}/${name}.tar.gz" -C "$work"
cp "${work}/${name}/cargo-about${exe}" "${dir}/cargo-about${exe}"
chmod 0755 "${dir}/cargo-about${exe}"
echo "install-cargo-about: installed $("${dir}/cargo-about${exe}" --version) in ${dir}"
