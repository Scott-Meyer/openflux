#!/usr/bin/env python3
"""Contract tests for openflux-dev USB transport & keymap binary codec."""

import multiprocessing
import struct
import sys
import tempfile
import time
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

import openflux_dev
from keymap_codec import FirmwareKeymapBlob, KeymapSection, decode_keymap_blob


def _transact_lock_worker(events_path: str, tag: str, hold_s: float, should_raise: bool, entered_evt=None, wait_evt=None) -> None:
    if wait_evt is not None:
        wait_evt.wait(timeout=5.0)
    client = openflux_dev.OpenFluxUsbClient.__new__(openflux_dev.OpenFluxUsbClient)
    client.lib = None
    client.ctx = None

    def fake_locked(_target, _cmd, _payload=b"", _timeout_ms=5000):
        with open(events_path, "a") as ep:
            ep.write(f"{tag}_enter\n")
        if entered_evt is not None:
            entered_evt.set()
        try:
            time.sleep(hold_s)
            if should_raise:
                raise RuntimeError("simulated usb failure")
            return b"ok"
        finally:
            with open(events_path, "a") as ep:
                ep.write(f"{tag}_exit\n")

    client._transact_locked = fake_locked  # type: ignore[method-assign]
    try:
        client.transact(openflux_dev.FW_TARGET, 1)
    except RuntimeError:
        pass


class FakeTransport:
    def __init__(self, rx_packets: list[bytes]) -> None:
        self.tx_packets: list[bytes] = []
        self.rx_packets = list(rx_packets)

    def write(self, _ep: int, data: bytes, _timeout_ms: int) -> None:
        self.tx_packets.append(data)

    def read(self, _ep: int, _length: int, _timeout_ms: int) -> bytes:
        if not self.rx_packets:
            raise RuntimeError("EOF in fake transport")
        return self.rx_packets.pop(0)


def make_resp_header(pkt_sz: int, resp_type: int, packets: int, nbytes: int) -> bytes:
    buf = bytearray(pkt_sz)
    struct.pack_into("<BII", buf, 0, resp_type, packets, nbytes)
    return bytes(buf)


class OpenFluxDevTransportTests(unittest.TestCase):
    def setUp(self) -> None:
        self.client = openflux_dev.OpenFluxUsbClient.__new__(openflux_dev.OpenFluxUsbClient)

    def test_valid_multi_packet_response_returns_trimmed_payload(self) -> None:
        pkt_sz = 64
        payload = b"A" * 80
        rx = [
            make_resp_header(pkt_sz, openflux_dev.RESP_ACK, 2, len(payload)),
            payload[:64],
            payload[64:].ljust(64, b"\x00"),
            b"",
        ]
        fake = FakeTransport(rx)
        out = self.client.transact_io(fake.write, fake.read, openflux_dev.FW_TARGET, 103)
        self.assertEqual(out, payload)
        self.assertEqual(len(fake.tx_packets), 2)
        self.assertEqual(fake.tx_packets[1], b"")

    def test_accepts_one_extra_advertised_packet_on_exact_multiple_responses(self) -> None:
        # A response may advertise one packet more than its payload needs (see docs/protocol/usb.md).
        for size in (512, 16384, 32768):
            payload = bytes((i % 251 for i in range(size)))
            rx = [make_resp_header(512, openflux_dev.RESP_ACK, (size // 512) + 1, size)]
            off = 0
            while off < size:
                chunk = payload[off : off + 16384]
                rx.append(chunk)
                off += len(chunk)
            rx.append(b"")  # ZLP
            fake = FakeTransport(rx)
            out = self.client.transact_io(fake.write, fake.read, openflux_dev.BRIDGE_TARGET, 101, b"/home/root/x\x00")
            self.assertEqual(out, payload, f"Failed for exact size={size}")

    def test_bridge_zero_byte_ack_with_packets_one_and_rejects_packets_two(self) -> None:
        fake_ok = FakeTransport([make_resp_header(512, openflux_dev.RESP_ACK, 1, 0), b""])
        self.assertEqual(self.client.transact_io(fake_ok.write, fake_ok.read, openflux_dev.BRIDGE_TARGET, 200, b"hi"), b"")
        fake_bad = FakeTransport([make_resp_header(512, openflux_dev.RESP_ACK, 2, 0)])
        with self.assertRaises(RuntimeError):
            self.client.transact_io(fake_bad.write, fake_bad.read, openflux_dev.BRIDGE_TARGET, 200, b"hi")

    def test_rejects_premature_zlp_when_header_advertises_more_bytes(self) -> None:
        pkt_sz = 512
        rx = [
            make_resp_header(pkt_sz, openflux_dev.RESP_ACK, 2, 900),
            b"X" * 512,
            b"",  # premature ZLP instead of 2nd packet
        ]
        fake = FakeTransport(rx)
        with self.assertRaises(RuntimeError) as ctx:
            self.client.transact_io(fake.write, fake.read, openflux_dev.BRIDGE_TARGET, 101, b"/home/root/a\x00")
        self.assertIn("Premature zero-length packet", str(ctx.exception))

    def test_rejects_short_payload_chunk(self) -> None:
        pkt_sz = 512
        rx = [
            make_resp_header(pkt_sz, openflux_dev.RESP_ACK, 1, 400),
            b"Y" * 100,  # 100 of the 400 advertised bytes, then the end of the transfer
            b"",
        ]
        fake = FakeTransport(rx)
        with self.assertRaises(RuntimeError) as ctx:
            self.client.transact_io(fake.write, fake.read, openflux_dev.BRIDGE_TARGET, 101, b"/home/root/a\x00")
        self.assertIn("Premature zero-length packet", str(ctx.exception))

    def test_rejects_nak_response(self) -> None:
        rx = [
            make_resp_header(512, openflux_dev.RESP_NAK, 0, 0),
            b"",
            make_resp_header(512, openflux_dev.RESP_ACK, 1, 22),
            b"openflux-bridge 1.0.0\x00".ljust(512, b"\x00"),
            b"",
        ]
        fake = FakeTransport(rx)
        with self.assertRaises(RuntimeError) as ctx:
            self.client.transact_io(fake.write, fake.read, openflux_dev.BRIDGE_TARGET, 101, b"/etc/passwd\x00")
        self.assertIn("NAK", str(ctx.exception))
        # Trailing ZLP from the NAK response was drained so the subsequent query succeeds.
        res = self.client.transact_io(fake.write, fake.read, openflux_dev.BRIDGE_TARGET, 1)
        self.assertEqual(res, b"openflux-bridge 1.0.0\x00")

    def test_malformed_firmware_header_drains_queued_packet_so_next_query_stays_synchronized(self) -> None:
        fake = FakeTransport([
            make_resp_header(64, openflux_dev.RESP_ACK, 1, 96),
            b"P" * 64,
            b"",
            make_resp_header(64, openflux_dev.RESP_ACK, 1, 24),
            b"openflux-firmware 1.0.0\x00".ljust(64, b"\x00"),
            b"",
        ])
        with self.assertRaises(RuntimeError) as ctx:
            self.client.transact_io(fake.write, fake.read, openflux_dev.FW_TARGET, openflux_dev.FW_CMD_FILE_TO_HOST, struct.pack("<I", 0))
        self.assertNotIsInstance(ctx.exception, openflux_dev.NakError)
        ver = self.client.transact_io(fake.write, fake.read, openflux_dev.FW_TARGET, openflux_dev.FW_CMD_VERSION)
        self.assertEqual(ver, b"openflux-firmware 1.0.0\x00")

    def test_usb_lock_serializes_concurrent_processes(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            events_path = str(Path(td) / "events.log")
            entered = multiprocessing.Event()
            p1 = multiprocessing.Process(target=_transact_lock_worker, args=(events_path, "bridge", 0.25, True, entered, None))
            p2 = multiprocessing.Process(target=_transact_lock_worker, args=(events_path, "gui", 0.05, False, None, entered))
            p1.start()
            p2.start()
            p1.join(timeout=5)
            p2.join(timeout=5)
            lines = Path(events_path).read_text().splitlines()
            self.assertEqual(lines, ["bridge_enter", "bridge_exit", "gui_enter", "gui_exit"])


class KeymapCodecTests(unittest.TestCase):
    def test_roundtrip_synthetic_ansi_iso_blob(self) -> None:
        ansi = KeymapSection(
            num_actuation_modes=1,
            num_keys=85,
            num_layers=2,
            layers=[[0x29] * 85, [0x50000000] * 85],
            actuation_tables=[[0x400FE253] * 85],
        )
        iso = KeymapSection(
            num_actuation_modes=1,
            num_keys=87,
            num_layers=2,
            layers=[[0x29] * 87, [0x50000000] * 87],
            actuation_tables=[[0x800FE150] * 87],
        )
        blob = FirmwareKeymapBlob(
            num_keymaps=2,
            layout_bitmask=0x3,
            offsets_by_layout=(0, 0, 262, 0),
            keymaps=[ansi, iso],
            trailing_words=[],
        )
        raw = blob.to_bytes()
        decoded = decode_keymap_blob(raw)
        self.assertEqual(decoded.to_bytes(), raw)
        self.assertEqual(decoded.offsets_by_layout, (0, 0, 262, 0))
        self.assertEqual(decoded.keymaps[0].num_keys, 85)
        self.assertEqual(decoded.keymaps[1].num_keys, 87)

    def test_rejects_mismatched_counts_and_zero_keys(self) -> None:
        bad_layers = KeymapSection(
            num_actuation_modes=1,
            num_keys=85,
            num_layers=2,
            layers=[[0x29] * 85],  # only 1 layer instead of 2
            actuation_tables=[[0x400FE253] * 85],
        )
        with self.assertRaises(ValueError):
            bad_layers.to_words()

        bad_modes = KeymapSection(
            num_actuation_modes=2,
            num_keys=85,
            num_layers=1,
            layers=[[0x29] * 85],
            actuation_tables=[[0x400FE253] * 85],  # only 1 table instead of 2
        )
        with self.assertRaises(ValueError):
            bad_modes.to_words()

        bad_blob = FirmwareKeymapBlob(
            num_keymaps=2,
            layout_bitmask=3,
            keymaps=[
                KeymapSection(1, 85, 1, [[0x29] * 85], [[0x400FE253] * 85])
            ],
            trailing_words=[],
        )
        with self.assertRaises(ValueError):
            bad_blob.to_bytes()

        # Zero-key section with huge layer count must be rejected by decoder
        crafted = struct.pack("<6I7I", 1, 1, 0, 0, 0, 0, 1, 0, 1000000, 0, 0, 0, 0)
        with self.assertRaises(ValueError):
            decode_keymap_blob(crafted)


if __name__ == "__main__":
    unittest.main()
