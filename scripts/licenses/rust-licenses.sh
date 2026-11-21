#!/usr/bin/env bash
# Writes the license texts of the third-party Rust crates compiled into one
# OpenFlux binary: every crate its package depends on for the given targets
# (normal and build dependencies, not dev-dependencies), with the license
# chosen under about.toml and the texts the crates ship. One plain text file.
#
# Fails when a crate's license can't be determined or isn't accepted:
# about.toml accepts exactly the licenses deny.toml allows, plus the
# per-crate clarifications it documents.
#
# Usage:
#   rust-licenses.sh --manifest-path <Cargo.toml> --target <triple>
#       [--target <triple>]... -o <file>
#
# Needs cargo-about (install-cargo-about.sh) and network access, or a Cargo
# registry that already holds every crate in Cargo.lock. Run from anywhere
# inside the repository.

set -euo pipefail

die() { echo "rust-licenses: $*" >&2; exit 1; }

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "${here}/../.." && pwd)"
manifest="" out="" targets=()
while [ $# -gt 0 ]; do
    case "$1" in
        --manifest-path) manifest="$2"; shift 2 ;;
        --target) targets+=(--target "$2"); shift 2 ;;
        -o) out="$2"; shift 2 ;;
        *) die "unknown argument: $1" ;;
    esac
done
[ -n "$manifest" ] || die "--manifest-path is required"
[ "${#targets[@]}" -gt 0 ] || die "--target is required"
[ -n "$out" ] || die "-o is required"
[ -f "$manifest" ] || die "no ${manifest}"
command -v cargo-about > /dev/null 2>&1 ||
    die "cargo-about is not installed; run ${repo}/scripts/licenses/install-cargo-about.sh"

mkdir -p "$(dirname "$out")"
cargo about generate --locked --fail \
    --config "${repo}/about.toml" \
    --manifest-path "$manifest" \
    "${targets[@]}" \
    --output-file "$out" \
    "${here}/rust-licenses.hbs"
[ -s "$out" ] || die "cargo-about wrote no licenses to ${out}"
echo "rust-licenses: wrote ${out} ($(grep -c '^Used by:$' "$out") license texts)"
