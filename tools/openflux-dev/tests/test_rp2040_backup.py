import hashlib
import struct
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import rp2040_flash_backup as rfb


class TestRp2040Backup(unittest.TestCase):
    def test_picoboot_cmd_wire_layout_and_large_token(self) -> None:
        pkt = rfb.build_picoboot_cmd(
            token=5000,
            cmd_id=rfb.PC_READ,
            cmd_size=8,
            xfer_len=65536,
            args=struct.pack("<II", 0x10F00000, 65536),
        )
        self.assertEqual(len(pkt), 32)
        magic, token, cmd_id, cmd_size, unused, xfer_len, args = struct.unpack("<IIBBHI16s", pkt)
        self.assertEqual(magic, rfb.PICOBOOT_MAGIC)
        self.assertEqual(token, 5000)
        self.assertEqual(cmd_id, 0x84)
        self.assertEqual(cmd_size, 8)
        self.assertEqual(unused, 0)
        self.assertEqual(xfer_len, 65536)
        self.assertEqual(args[:8], struct.pack("<II", 0x10F00000, 65536))

    def test_parse_factory_calibration_v2(self) -> None:
        factors = [300 + i for i in range(160)]
        checksum = sum(factors)
        info = struct.pack("<5I", rfb.FACTORY_CALIB_MAGIC, 2, 160, 0, checksum)
        fbytes = struct.pack("<160H", *factors)
        cal = rfb.parse_factory_calibration(info, fbytes)
        self.assertEqual(cal.version, 2)
        self.assertEqual(cal.num_sensors, 160)
        self.assertEqual(cal.effective_factors_q8[0], (300 * 0xAA) >> 8)

        bad_info = struct.pack("<5I", rfb.FACTORY_CALIB_MAGIC, 2, 160, 0, checksum + 1)
        with self.assertRaises(ValueError):
            rfb.parse_factory_calibration(bad_info, fbytes)

    def test_extract_roundtrips_littlefs_image(self) -> None:
        factors = [256] * 160
        info = struct.pack("<5I", rfb.FACTORY_CALIB_MAGIC, 2, 160, 0, sum(factors))
        fbytes = struct.pack("<160H", *factors)
        keymap = b"TESTKEYMAP" * 32
        img = rfb.create_littlefs_image(
            {
                "factory_calibration_info": info,
                "factory_scaling_factors": fbytes,
                "keymap": keymap,
            }
        )
        self.assertEqual(len(img), rfb.FW_LFS_SIZE)
        files = rfb.extract_littlefs_files(img)
        self.assertEqual(files["factory_calibration_info"], info)
        self.assertEqual(files["factory_scaling_factors"], fbytes)
        self.assertEqual(files["keymap"], keymap)
        cal = rfb.parse_factory_calibration(files["factory_calibration_info"], files["factory_scaling_factors"])
        self.assertEqual(cal.effective_factors_q8[0], (256 * 0xAA) >> 8)

    def test_backup_recovers_bootsel_and_fails_on_missing_calibration(self) -> None:
        empty_lfs = rfb.create_littlefs_image({"keymap": b"1234"})
        full_flash = b"\xff" * rfb.FW_LFS_OFFSET + empty_lfs
        assert len(full_flash) == rfb.RP2040_FLASH_SIZE

        events: list[str] = []

        class FakePicoboot:
            def __init__(self, timeout_s: float = 10.0) -> None:
                events.append("open_pb")

            def exclusive_access(self, mode: int = 1) -> None:
                events.append(f"excl:{mode}")

            def exit_xip(self) -> None:
                events.append("exit_xip")

            def read_flash(self, off: int, sz: int) -> bytes:
                events.append(f"read:{off}:{sz}")
                return full_flash[off : off + sz]

            def reset_session(self) -> None:
                events.append("reset_session")

            def reboot(self, delay_ms: int = 100) -> None:
                events.append("reboot")

            def close(self) -> None:
                events.append("close")

        enum_state = {"after_reboot": False}

        def fake_find(vid: int, pid: int) -> bool:
            if (vid, pid) == (rfb.RP2040_PICOBOOT_VID, rfb.RP2040_PICOBOOT_PID):
                return not enum_state["after_reboot"]
            enum_state["after_reboot"] = True
            return True

        with tempfile.TemporaryDirectory() as td:
            out_dir = Path(td)
            with mock.patch.object(rfb, "PicobootClient", FakePicoboot), mock.patch.object(
                rfb, "_find_usb_device", side_effect=fake_find
            ):
                with self.assertRaises(RuntimeError) as ctx:
                    rfb.backup_live_rp2040_flash(out_dir)
                self.assertIn("missing factory files", str(ctx.exception))
            self.assertIn("reset_session", events)
            self.assertIn("reboot", events)
            self.assertIn("close", events)
            # Raw 16 MiB dump was still preserved despite extraction failure.
            self.assertEqual((out_dir / "firmware_flash_16mib.bin").stat().st_size, rfb.RP2040_FLASH_SIZE)
            self.assertEqual(
                (out_dir / "firmware_flash_16mib.sha256").read_text().strip(),
                hashlib.sha256(full_flash).hexdigest(),
            )

    def test_stalled_read_resets_session_before_reboot(self) -> None:
        events: list[str] = []

        class StallingPicoboot:
            def __init__(self, timeout_s: float = 10.0) -> None:
                events.append("open_pb")

            def exclusive_access(self, mode: int = 1) -> None:
                events.append("excl")

            def exit_xip(self) -> None:
                events.append("exit_xip")

            def read_flash(self, off: int, sz: int) -> bytes:
                events.append("read_stall")
                raise RuntimeError("LIBUSB_ERROR_PIPE")

            def reset_session(self) -> None:
                events.append("reset_session")

            def reboot(self, delay_ms: int = 100) -> None:
                events.append("reboot")

            def close(self) -> None:
                events.append("close")

        enum_state = {"after_reboot": False}

        def fake_find(vid: int, pid: int) -> bool:
            if (vid, pid) == (rfb.RP2040_PICOBOOT_VID, rfb.RP2040_PICOBOOT_PID):
                return not enum_state["after_reboot"]
            enum_state["after_reboot"] = True
            return True

        with tempfile.TemporaryDirectory() as td:
            with mock.patch.object(rfb, "PicobootClient", StallingPicoboot), mock.patch.object(
                rfb, "_find_usb_device", side_effect=fake_find
            ):
                with self.assertRaises(RuntimeError) as ctx:
                    rfb.backup_live_rp2040_flash(Path(td))
                self.assertIn("LIBUSB_ERROR_PIPE", str(ctx.exception))
            self.assertLess(events.index("reset_session"), events.index("reboot"))
            self.assertEqual(events[-1], "close")

    def test_parse_uf2_image_valid_and_rejects_littlefs_overlap(self) -> None:
        def make_uf2(blocks: list[tuple[int, bytes]]) -> bytes:
            out = bytearray()
            tot = len(blocks)
            for idx, (addr, data) in enumerate(blocks):
                blk = bytearray(512)
                struct.pack_into("<8I", blk, 0, 0x0A324655, 0x9E5D5157, 0x00002000, addr, 256, idx, tot, 0xE48BFF56)
                blk[32 : 32 + 256] = data.ljust(256, b"\x00")
                struct.pack_into("<I", blk, 508, 0x0AB16F30)
                out.extend(blk)
            return bytes(out)

        good = make_uf2([(rfb.RP2040_XIP_BASE, b"A" * 256), (rfb.RP2040_XIP_BASE + 256, b"B" * 256)])
        off, img = rfb.parse_uf2_image(good)
        self.assertEqual(off, 0)
        self.assertEqual(len(img), 4096)
        self.assertEqual(img[:512], b"A" * 256 + b"B" * 256)

        bad = make_uf2([(rfb.RP2040_XIP_BASE + rfb.FW_LFS_OFFSET, b"X" * 256)])
        with self.assertRaises(ValueError):
            rfb.parse_uf2_image(bad)

    def test_flash_uf2_never_reboots_on_erase_write_or_verify_failure(self) -> None:
        blk = bytearray(512)
        struct.pack_into("<8I", blk, 0, 0x0A324655, 0x9E5D5157, 0x00002000, rfb.RP2040_XIP_BASE, 256, 0, 1, 0xE48BFF56)
        blk[32 : 32 + 256] = b"A" * 256
        struct.pack_into("<I", blk, 508, 0x0AB16F30)

        for fail_stage in ("erase", "write", "verify", "lfs_after"):
            events: list[str] = []

            class FaultPicoboot:
                def __init__(self, timeout_s: float = 10.0) -> None:
                    events.append("open")

                def exclusive_access(self, mode: int = 1) -> None:
                    events.append("excl")

                def exit_xip(self) -> None:
                    events.append("exit_xip")

                def flash_erase(self, off: int, sz: int) -> None:
                    events.append("erase")
                    if fail_stage == "erase":
                        raise RuntimeError("erase failed")

                def write_flash(self, off: int, data: bytes) -> None:
                    events.append("write")
                    if fail_stage == "write":
                        raise RuntimeError("write failed")

                def read_flash(self, off: int, sz: int) -> bytes:
                    events.append(f"read:{off:x}")
                    if off == rfb.FW_LFS_OFFSET:
                        if fail_stage == "lfs_after" and "write" in events:
                            return b"C" * sz
                        return b"L" * sz
                    if fail_stage == "verify":
                        return b"Z" * sz
                    return (b"A" * 256) + (b"\xff" * (sz - 256))

                def reset_session(self) -> None:
                    events.append("reset_session")

                def reboot(self, delay_ms: int = 100) -> None:
                    events.append("reboot")

                def close(self) -> None:
                    events.append("close")

            with tempfile.TemporaryDirectory() as td:
                uf2_p = Path(td) / "fw.uf2"
                uf2_p.write_bytes(bytes(blk))
                with mock.patch.object(rfb, "PicobootClient", FaultPicoboot), mock.patch.object(
                    rfb, "_find_usb_device", return_value=True
                ):
                    with self.assertRaises(RuntimeError):
                        rfb.flash_live_rp2040_uf2(uf2_p)
                self.assertNotIn("reboot", events, f"rebooted after {fail_stage} failure")
                self.assertIn("reset_session", events)
                self.assertEqual(events[-1], "close")


if __name__ == "__main__":
    unittest.main()
