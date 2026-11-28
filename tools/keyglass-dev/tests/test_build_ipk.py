#!/usr/bin/env python3
"""Tests for scripts/build_ipk.py."""

import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "scripts"))
from build_ipk import build_ipk


class BuildIpkTests(unittest.TestCase):
    def test_builds_valid_ar_and_tar_gz_ipk(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            ctrl = root / "ctrl"
            data = root / "data"
            ctrl.mkdir()
            (data / "usr" / "bin").mkdir(parents=True)
            (ctrl / "control").write_text("Package: testpkg\nVersion: 1.0.0\nArchitecture: aarch64\n")
            (data / "usr" / "bin" / "hello").write_text("#!/bin/sh\necho hi\n")
            out_ipk = root / "testpkg_1.0.0_aarch64.ipk"

            build_ipk(ctrl, data, out_ipk, mtime=1700000000)

            raw = out_ipk.read_bytes()
            self.assertTrue(raw.startswith(b"!<arch>\n"))

            # Parse ar members
            pos = 8
            members = {}
            while pos < len(raw):
                hdr = raw[pos : pos + 60]
                pos += 60
                name = hdr[:16].decode("ascii").strip()
                size = int(hdr[48:58].decode("ascii").strip())
                payload = raw[pos : pos + size]
                pos += size + (size % 2)
                members[name] = payload

            self.assertEqual(members["debian-binary"], b"2.0\n")
            self.assertIn("control.tar.gz", members)
            self.assertIn("data.tar.gz", members)

            # Verify tar.gz payloads extract cleanly
            ctrl_tgz = root / "out_ctrl.tar.gz"
            ctrl_tgz.write_bytes(members["control.tar.gz"])
            with tarfile.open(ctrl_tgz, "r:gz") as tf:
                self.assertIn("./control", tf.getnames())

            data_tgz = root / "out_data.tar.gz"
            data_tgz.write_bytes(members["data.tar.gz"])
            with tarfile.open(data_tgz, "r:gz") as tf:
                self.assertIn("./usr/bin/hello", tf.getnames())


if __name__ == "__main__":
    unittest.main()
