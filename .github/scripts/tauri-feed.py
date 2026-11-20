#!/usr/bin/env python3
"""Writes the Tauri updater feed (latest.json) for one update channel.

The feed points OpenFlux Studio's built-in updater at the newest host release
of the channel that is not withdrawn: for "stable" the newest stable release,
for "beta" the newest stable or beta release. Releases are read from the
signed index.json, each update archive is downloaded and checked against the
index's sha256, and is then signed with the updater key
(TAURI_SIGNING_PRIVATE_KEY, TAURI_SIGNING_PRIVATE_KEY_PASSWORD) by the Tauri
CLI.

Each signature is bound to the release's version (`tauri signer sign
--app-version`): the version is written into the signature's trusted comment,
which the signature covers. The feed itself is not signed, so Studio enables
the updater's requireSignedVersion and refuses an update whose signature
names a different version than the feed announces. This script checks every
signature it writes for that.

Usage:
  tauri-feed.py --index index.json --feed <stable|beta> --work <dir> -o latest.json

Exits 0 without writing the feed when the channel has no host release.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import re
import subprocess
import sys
import urllib.request
from pathlib import Path

TAURI_CLI = "@tauri-apps/cli@2.12.6"

# Update archive name suffix -> Tauri updater platform keys.
PLATFORMS = {
    "_universal.app.tar.gz": ["darwin-aarch64", "darwin-x86_64"],
    "_x64-setup.exe": ["windows-x86_64"],
    "_x86_64.AppImage": ["linux-x86_64"],
    "_aarch64.AppImage": ["linux-aarch64"],
}

SEMVER = re.compile(r"^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$")


def version_key(v: str):
    m = SEMVER.match(v)
    if not m:
        raise SystemExit(f"tauri-feed: {v!r} is not a version")
    major, minor, patch, pre = m.groups()
    if pre is None:
        pre_key = (1,)
    else:
        parts = []
        for p in pre.split("."):
            parts.append((0, int(p), "") if p.isdigit() else (1, 0, p))
        pre_key = (0, tuple(parts))
    return (int(major), int(minor), int(patch), pre_key)


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def signed_version(signature_b64: str) -> str | None:
    """The version recorded in a Tauri updater signature's trusted comment."""
    text = base64.b64decode(signature_b64).decode("utf-8")
    for line in text.splitlines():
        if line.startswith("trusted comment: "):
            for field in line[len("trusted comment: ") :].split("\t"):
                if field.startswith("version:"):
                    return field[len("version:") :]
    return None


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--index", required=True, type=Path)
    p.add_argument("--feed", required=True, choices=["stable", "beta"])
    p.add_argument("--work", required=True, type=Path)
    p.add_argument("-o", "--output", required=True, type=Path)
    a = p.parse_args()

    index = json.loads(a.index.read_text())
    channels = ["stable"] if a.feed == "stable" else ["stable", "beta"]
    candidates = [
        r
        for ch in channels
        for r in index.get("releases", {}).get(ch, [])
        if r.get("component") == "host" and not r.get("withdrawn", False)
    ]
    if not candidates:
        print(f"tauri-feed: no {a.feed} host release; no feed written")
        return 0
    release = max(candidates, key=lambda r: version_key(r["version"]))

    a.work.mkdir(parents=True, exist_ok=True)
    platforms = {}
    for art in release["artifacts"]:
        keys = next((v for k, v in PLATFORMS.items() if art["name"].endswith(k)), None)
        if keys is None:
            continue
        dest = a.work / art["name"]
        if not dest.exists():
            urllib.request.urlretrieve(art["url"], dest)
        if sha256(dest) != art["sha256"]:
            raise SystemExit(f"tauri-feed: {art['name']} does not match the index")
        subprocess.run(
            ["npx", "--yes", TAURI_CLI, "signer", "sign", "--app-version", release["version"], str(dest)],
            check=True,
        )
        signature = Path(f"{dest}.sig").read_text().strip()
        bound = signed_version(signature)
        if bound != release["version"]:
            raise SystemExit(
                f"tauri-feed: the signature of {art['name']} is bound to version {bound!r}, not {release['version']}"
            )
        for key in keys:
            platforms[key] = {"signature": signature, "url": art["url"]}
    missing = {k for v in PLATFORMS.values() for k in v} - platforms.keys()
    if missing:
        raise SystemExit(f"tauri-feed: host {release['version']} has no update archive for {sorted(missing)}")

    feed = {
        "version": release["version"],
        "notes": release.get("notes_url", ""),
        "pub_date": release["released"],
        "platforms": dict(sorted(platforms.items())),
    }
    a.output.parent.mkdir(parents=True, exist_ok=True)
    a.output.write_text(json.dumps(feed, indent=2) + "\n")
    print(f"tauri-feed: {a.feed} feed -> host {release['version']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
