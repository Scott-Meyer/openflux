#!/usr/bin/env python3
"""Pure-Python .ipk (opkg / Debian binary package 2.0) builder."""

from __future__ import annotations

import argparse
import gzip
import io
import os
import tarfile
import time
from pathlib import Path


def _ar_entry(name: str, payload: bytes, mtime: int) -> bytes:
    header = (
        f"{name:<16}"
        f"{mtime:<12}"
        f"{0:<6}"
        f"{0:<6}"
        f"{0o100644:<8o}"
        f"{len(payload):<10}"
        "`\n"
    ).encode("ascii")
    if len(header) != 60:
        raise ValueError(f"Bad ar header length {len(header)}")
    pad = b"\n" if len(payload) % 2 == 1 else b""
    return header + payload + pad


def _tar_gz_dir(src_dir: Path, mtime: int, executable_names: set[str] | None = None) -> bytes:
    buf = io.BytesIO()
    with gzip.GzipFile(fileobj=buf, mode="wb", compresslevel=9, mtime=mtime) as gz:
        with tarfile.open(fileobj=gz, mode="w") as tf:
            for path in sorted(src_dir.rglob("*")):
                rel = path.relative_to(src_dir)
                arcname = f"./{rel.as_posix()}"
                info = tf.gettarinfo(str(path), arcname=arcname)
                info.uid = 0
                info.gid = 0
                info.uname = "root"
                info.gname = "root"
                info.mtime = mtime
                if executable_names and path.is_file() and path.name in executable_names:
                    info.mode = 0o755
                if path.is_file():
                    with path.open("rb") as f:
                        tf.addfile(info, f)
                else:
                    tf.addfile(info)
    return buf.getvalue()


def build_ipk(control_dir: Path, data_dir: Path, out_ipk: Path, mtime: int | None = None) -> None:
    if mtime is None:
        env_epoch = os.environ.get("SOURCE_DATE_EPOCH")
        mtime = int(env_epoch) if env_epoch and env_epoch.isdigit() else int(time.time())
    if not (control_dir / "control").is_file():
        raise FileNotFoundError(f"Missing control file in {control_dir}")

    debian_binary = b"2.0\n"
    control_tgz = _tar_gz_dir(control_dir, mtime, executable_names={"preinst", "postinst", "prerm", "postrm"})
    data_tgz = _tar_gz_dir(data_dir, mtime)

    out_ipk.parent.mkdir(parents=True, exist_ok=True)
    with out_ipk.open("wb") as f:
        f.write(b"!<arch>\n")
        f.write(_ar_entry("debian-binary", debian_binary, mtime))
        f.write(_ar_entry("control.tar.gz", control_tgz, mtime))
        f.write(_ar_entry("data.tar.gz", data_tgz, mtime))


def main() -> None:
    p = argparse.ArgumentParser(description="Build an opkg .ipk package")
    p.add_argument("--control-dir", required=True, type=Path)
    p.add_argument("--data-dir", required=True, type=Path)
    p.add_argument("-o", "--output", required=True, type=Path)
    args = p.parse_args()
    build_ipk(args.control_dir, args.data_dir, args.output)
    print(f"Built {args.output} ({args.output.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
