#!/usr/bin/env bash
# Writes site-manifest.json into a prepared GitHub Pages site directory,
# recording every static file (packages/, source/, updates/) with its sha256
# so the next publication mirrors and verifies them before deploying.
#
# Usage: write-site-manifest.sh <site-dir>

set -euo pipefail

site="${1:?usage: write-site-manifest.sh <site-dir>}"
[ -d "$site" ] || { echo "write-site-manifest: ${site}: not a directory" >&2; exit 1; }

python3 - "$site" <<'PY'
import hashlib
import json
import sys
from pathlib import Path

site = Path(sys.argv[1])
skip = {"index.json", "index.json.sig", "site-manifest.json", ".nojekyll"}
files = {}
for path in sorted(site.rglob("*")):
    if not path.is_file():
        continue
    rel = path.relative_to(site).as_posix()
    if rel in skip:
        continue
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    files[rel] = h.hexdigest()

out = site / "site-manifest.json"
out.write_text(json.dumps({"schema": 1, "files": files}, indent=2, sort_keys=True) + "\n")
print(f"write-site-manifest: recorded {len(files)} file(s)")
PY
