#!/usr/bin/env python3
"""Writes or preserves the Tauri updater feed (latest.json) for one channel.

The feed points Keyglass Studio's built-in updater at the newest host release
of the channel that is not withdrawn: for "stable" the newest stable release,
for "beta" the newest stable or beta release.

When a host release was staged in the current run (--staged-host) and is the
newest eligible release for --feed:
  - Its macOS update archive (*_universal.app.tar.gz) is copied into
    <site>/updates/<feed>/ (replacing any older archive for that channel) and
    referenced at <pages-url>/updates/<feed>/<name>.
  - Its Windows and Linux update archives are signed directly from the staged
    artifacts directory and referenced at their clean GitHub Release URLs.
  - Each signature is bound to the release version (`tauri signer sign
    --app-version`) so Studio's `requireSignedVersion` accepts it.

When no matching host release was staged in this run (e.g. keyboard releases
or the monthly index refresh):
  - If the existing mirrored <site>/updates/<feed>/latest.json already points
    to the newest eligible host release, it is preserved untouched without
    re-downloading or re-signing any installers.
  - If the previously published host release was withdrawn, the stale feed (and
    its .app.tar.gz) is replaced if a fallback release's update archives are
    available, or removed so Studio never offers a withdrawn update.

Usage:
  tauri-feed.py --index <index.json> --feed <stable|beta> --pages-url <url>
      --site <site-dir> --work <dir> [--staged-host <dir>] -o <latest.json>
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import re
import shutil
import subprocess
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


def sign_archive(archive: Path, version: str) -> str:
    subprocess.run(
        ["npx", "--yes", TAURI_CLI, "signer", "sign", "--app-version", version, str(archive)],
        check=True,
    )
    signature = Path(f"{archive}.sig").read_text().strip()
    bound = signed_version(signature)
    if bound != version:
        raise SystemExit(
            f"tauri-feed: the signature of {archive.name} is bound to version {bound!r}, not {version}"
        )
    return signature


def clear_feed_dir(feed_dir: Path) -> None:
    if not feed_dir.exists():
        return
    for p in feed_dir.iterdir():
        if p.is_file() and (p.name == "latest.json" or p.name.endswith(".app.tar.gz")):
            p.unlink()


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--index", required=True, type=Path)
    p.add_argument("--feed", required=True, choices=["stable", "beta"])
    p.add_argument("--pages-url", required=True)
    p.add_argument("--site", required=True, type=Path)
    p.add_argument("--work", required=True, type=Path)
    p.add_argument("--staged-host", type=Path, default=None)
    p.add_argument("-o", "--output", required=True, type=Path)
    a = p.parse_args()

    pages_url = a.pages_url.rstrip("/")
    feed_dir = a.site / "updates" / a.feed

    index = json.loads(a.index.read_text())
    channels = ["stable"] if a.feed == "stable" else ["stable", "beta"]
    candidates = [
        r
        for ch in channels
        for r in index.get("releases", {}).get(ch, [])
        if r.get("component") == "host" and not r.get("withdrawn", False)
    ]
    if not candidates:
        clear_feed_dir(feed_dir)
        print(f"tauri-feed: no {a.feed} host release; feed cleared")
        return 0
    release = max(candidates, key=lambda r: version_key(r["version"]))
    target_version = release["version"]

    staged_meta = None
    if a.staged_host is not None and (a.staged_host / "meta.json").is_file():
        meta = json.loads((a.staged_host / "meta.json").read_text())
        if meta.get("component") == "host" and meta.get("version") == target_version:
            staged_meta = meta

    # Preserve existing mirrored feed when nothing changed for this channel.
    if staged_meta is None and a.output.is_file():
        try:
            existing = json.loads(a.output.read_text())
        except json.JSONDecodeError:
            existing = {}
        if existing.get("version") == target_version:
            print(f"tauri-feed: {a.feed} feed already at host {target_version}; preserved")
            return 0

    a.work.mkdir(parents=True, exist_ok=True)
    feed_dir.mkdir(parents=True, exist_ok=True)
    platforms = {}

    if staged_meta is not None:
        # Remove any stale macOS updater archive from an earlier version in this channel.
        for old in feed_dir.glob("*.app.tar.gz"):
            old.unlink()

        updater_dir = a.staged_host / "updater"
        app_archives = sorted(updater_dir.glob("*_universal.app.tar.gz")) if updater_dir.is_dir() else []
        if len(app_archives) != 1:
            raise SystemExit(
                f"tauri-feed: expected one *_universal.app.tar.gz in {updater_dir}, found {len(app_archives)}"
            )
        app_src = app_archives[0]
        app_dest = feed_dir / app_src.name
        shutil.copy2(app_src, app_dest)
        work_app = a.work / app_src.name
        shutil.copy2(app_src, work_app)
        app_sig = sign_archive(work_app, target_version)
        app_url = f"{pages_url}/updates/{a.feed}/{app_src.name}"
        for key in PLATFORMS["_universal.app.tar.gz"]:
            platforms[key] = {"signature": app_sig, "url": app_url}

        artifacts_dir = a.staged_host / "artifacts"
        for art in release["artifacts"]:
            keys = next((v for k, v in PLATFORMS.items() if art["name"].endswith(k)), None)
            if keys is None:
                continue
            local_src = artifacts_dir / art["name"]
            if not local_src.is_file():
                raise SystemExit(f"tauri-feed: staged artifact missing: {local_src}")
            if sha256(local_src) != art["sha256"]:
                raise SystemExit(f"tauri-feed: {art['name']} does not match the index")
            work_file = a.work / art["name"]
            if not work_file.exists():
                shutil.copy2(local_src, work_file)
            sig = sign_archive(work_file, target_version)
            for key in keys:
                platforms[key] = {"signature": sig, "url": art["url"]}
    else:
        # Fallback path: e.g. withdrawing the latest host release so the feed
        # must revert to a previous release whose archives are still downloadable.
        for art in release["artifacts"]:
            keys = next((v for k, v in PLATFORMS.items() if art["name"].endswith(k)), None)
            if keys is None:
                continue
            dest = a.work / art["name"]
            if not dest.exists():
                urllib.request.urlretrieve(art["url"], dest)
            if sha256(dest) != art["sha256"]:
                raise SystemExit(f"tauri-feed: {art['name']} does not match the index")
            sig = sign_archive(dest, target_version)
            for key in keys:
                platforms[key] = {"signature": sig, "url": art["url"]}
        missing = {k for v in PLATFORMS.values() for k in v} - platforms.keys()
        if missing:
            # Older release had its .app.tar.gz replaced by a newer release:
            # clear the feed rather than serving a withdrawn version.
            clear_feed_dir(feed_dir)
            print(
                f"tauri-feed: host {target_version} has no archived update bundle for {sorted(missing)}; feed cleared"
            )
            return 0

    missing = {k for v in PLATFORMS.values() for k in v} - platforms.keys()
    if missing:
        raise SystemExit(
            f"tauri-feed: host {target_version} has no update archive for {sorted(missing)}"
        )

    feed = {
        "version": target_version,
        "notes": release.get("notes_url", ""),
        "pub_date": release["released"],
        "platforms": dict(sorted(platforms.items())),
    }
    a.output.parent.mkdir(parents=True, exist_ok=True)
    a.output.write_text(json.dumps(feed, indent=2) + "\n")
    print(f"tauri-feed: {a.feed} feed -> host {target_version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
