#!/usr/bin/env python3
"""Contract tests for SimpleHDLC, LiveData, PROFILE_SETTINGS_V1, and firmware keymap/actuation codecs."""

import json
import struct
import sys
import tempfile
import unittest
import zlib
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

# Shared with the daemon's codec tests (host/daemon/src/codecs/keymap.rs).
FIXTURES = Path(__file__).resolve().parents[3] / "host" / "daemon" / "tests" / "fixtures"

import openflux_dev
from keymap_codec import (
    ACTUATION_RAPID_TRIGGER,
    ACTUATION_TRADITIONAL,
    CONSUMER_CONTROLS,
    KEY_ALIASES,
    KEYBOARD_KEYS,
    NONE_KEYCODE,
    OPENFLUX_KEYS,
    FirmwareKeymapBlob,
    KeymapSection,
    actuation_entry_json,
    decode_actuation_modes,
    decode_actuation_word,
    decode_keycode,
    decode_keymap_blob,
    decode_macro_step,
    dist_um_to_flux_density,
    encode_actuation_entry,
    encode_actuation_word,
    encode_keycode,
    encode_macro_step,
    keycode_name,
    keycode_word,
)
from profile_settings import ProfileSetting, ProfileSettingsFile, decode_profile_settings
from simple_hdlc import (
    DataType,
    SimpleHdlcDecoder,
    compute_crc32,
    decode_frame,
    decode_live_data,
    encode_frame,
    encode_live_data,
    encode_live_json,
)


class SimpleHdlcTests(unittest.TestCase):
    def test_roundtrip_with_escape_bytes_and_chunked_stream(self) -> None:
        payload = bytes([0x00, 0x7E, 0x7D, 0x20, 0xFF, 0x7E, 0x7D, 0x01])
        frame = encode_frame(payload)
        self.assertEqual(frame[0], 0x7E)
        self.assertNotIn(0x7E, frame[1:])
        self.assertEqual(decode_frame(frame), payload)

        # Feed byte-by-byte across two concatenated frames with leading noise
        frame2 = encode_frame(b"flux")
        dec = SimpleHdlcDecoder()
        out = []
        for b in b"\x00\x55" + frame + frame2:
            out.extend(dec.feed(bytes([b])))
        self.assertEqual(out, [payload, b"flux"])

    def test_crc_mismatch_rejected(self) -> None:
        frame = bytearray(encode_frame(b"hello"))
        frame[-1] ^= 0x01
        with self.assertRaises(ValueError):
            decode_frame(bytes(frame))

    def test_crc32_matches_independent_vectors_and_reversed_ieee_crc32(self) -> None:

        def rev32(v: int) -> int:
            return int(f"{v:032b}"[::-1], 2)

        # Standard test vector under bit-reversed-input / MSB-first / ~crc (equiv. bitrev32(IEEE CRC32))
        self.assertEqual(compute_crc32(b""), 0x00000000)
        self.assertEqual(compute_crc32(b"123456789"), 0x649C2FD3)
        for sample in [b"", b"123456789", b"\xff\x00\x00\x00\x01", b"\x04", b"\x7e\x7d\x00\xff"]:
            self.assertEqual(compute_crc32(sample), rev32(zlib.crc32(sample)))

    def test_fixed_wire_frames_match_protocol_layout(self) -> None:
        # 5-byte live-data length header [0xFF, uint32_be(1)] -> CRC=0xE00082DE
        expected_len_wire = bytes.fromhex("7e0005ff00000001e00082de")
        # 1-byte DataType.HEARTBEAT [0x04] -> CRC=0x29D4F6AB
        expected_hb_wire = bytes.fromhex("7e00010429d4f6ab")
        self.assertEqual(encode_live_data(DataType.HEARTBEAT), [expected_len_wire, expected_hb_wire])
        self.assertEqual(decode_live_data([expected_len_wire, expected_hb_wire]), (DataType.HEARTBEAT, b""))

        # Escaping vector: length=2, payload=[0x7E, 0x7D], CRC=0x60D28B7B (rev32(zlib.crc32(b"\x7e\x7d")))
        expected_esc_wire = bytes.fromhex("7e00027d5e7d5d60d28b7b")
        self.assertEqual(encode_frame(b"\x7e\x7d"), expected_esc_wire)
        self.assertEqual(decode_frame(expected_esc_wire), b"\x7e\x7d")

    def test_live_data_json_single_and_multi_chunk_roundtrip(self) -> None:
        small = {"hardwareData": {"isMac": True, "cpu": 12}}
        frames = encode_live_json(small)
        self.assertEqual(len(frames), 2)
        dtype, raw = decode_live_data(frames)
        self.assertEqual(dtype, DataType.JSON)
        self.assertEqual(json.loads(raw), small)

        # Multi-chunk (> 8000 bytes)
        large = {"mediaFile": {"title": "t", "base64Image": "A" * 17000}}
        frames_big = encode_live_json(large)
        self.assertGreater(len(frames_big), 3)
        dtype_big, raw_big = decode_live_data(frames_big)
        self.assertEqual(dtype_big, DataType.JSON)
        self.assertEqual(json.loads(raw_big), large)

        hb_frames = encode_live_data(DataType.HEARTBEAT)
        self.assertEqual(decode_live_data(hb_frames), (DataType.HEARTBEAT, b""))


class ProfileSettingsTests(unittest.TestCase):
    def test_roundtrip_data_and_transfer_bytes(self) -> None:
        ps = ProfileSettingsFile(
            settings=[
                ProfileSetting.from_layer(0, 0),
                ProfileSetting.from_layer(1, 2),
            ]
        )
        data = ps.to_data_bytes()
        self.assertEqual(len(data), 16 + 2 * 8)
        self.assertEqual(decode_profile_settings(data, has_file_header=False), ps)

        xfer = ps.to_transfer_bytes()
        self.assertEqual(len(xfer), 32 + 2 * 8)
        self.assertEqual(decode_profile_settings(xfer, has_file_header=True), ps)

    def test_rejects_over_64_profiles(self) -> None:
        ps = ProfileSettingsFile(settings=[ProfileSetting.from_layer(0, 0) for _ in range(65)])
        with self.assertRaises(ValueError):
            ps.to_data_bytes()


class KeymapAndActuationTests(unittest.TestCase):
    def test_actuation_um_matches_live_hardware_constants(self) -> None:
        self.assertEqual(dist_um_to_flux_density(-100), 0x000)
        self.assertEqual(dist_um_to_flux_density(450), 0x0FE)
        self.assertEqual(dist_um_to_flux_density(5000), 0x613)
        # The second field is the release depth: 0x253 - 0xFE for a 1.2 mm actuation releasing at 0.4 mm.
        self.assertEqual(encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 400), 0x40155253)
        self.assertEqual(encode_actuation_word(ACTUATION_RAPID_TRIGGER, 600, 400), 0x800FE150)

        dec = decode_actuation_word(0x40155253)
        self.assertEqual(dec["mode"], "traditional_trigger")
        self.assertEqual(dec["actuation_um"], 1200)
        self.assertEqual(dec["second_um"], 400)
        self.assertTrue(dec["exact"])

    def test_release_depth_is_zero_for_a_release_point_at_or_below_actuation(self) -> None:
        self.assertEqual(encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 1200), 0x40000253)
        self.assertEqual(encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 2800), 0x40000253)
        # A release depth beyond the actuation reading releases at the top.
        self.assertEqual(decode_actuation_word((ACTUATION_TRADITIONAL << 30) | (0x300 << 12) | 0x100)["second_um"], 0)

    def test_type_0_and_3_words_decode_as_the_firmware_default(self) -> None:
        for word in (0, 0x30000000, 0xC0000000, 0xC0123456, 0x00123456):
            dec = decode_actuation_word(word)
            self.assertEqual(dec["actuation_type"], ACTUATION_TRADITIONAL, hex(word))
            self.assertEqual((dec["actuation_density"], dec["second_density"]), (650, 216))
            self.assertEqual((dec["actuation_um"], dec["second_um"]), (1300, 800))
            self.assertTrue(dec["firmware_default"])
            self.assertFalse(dec["exact"])
            entry = actuation_entry_json(word)
            self.assertEqual(entry["firmwareWord"], word)
            self.assertEqual(encode_actuation_entry(entry)[0], word)

    def test_a_firmware_word_is_kept_only_while_its_values_are_unchanged(self) -> None:
        word = (ACTUATION_TRADITIONAL << 30) | (100 << 12) | 600
        entry = actuation_entry_json(word)
        self.assertEqual(entry["firmwareWord"], word)
        self.assertEqual(encode_actuation_entry({**entry, "touched": True})[0], word)
        moved = {**entry, "actuationPointMm": 1.5}
        self.assertEqual(encode_actuation_entry(moved)[0], encode_actuation_word(ACTUATION_TRADITIONAL, 1500, 1000))
        # Without a release point, a fixed entry releases 0.2 mm above actuation.
        self.assertEqual(
            encode_actuation_entry({"actuationPointMm": 2.0})[0],
            encode_actuation_word(ACTUATION_TRADITIONAL, 2000, 1800),
        )


class SharedKeycodeFixtureTests(unittest.TestCase):
    """The Python key code names agree with the daemon's on `keycodes.json`."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.fixture = json.loads((FIXTURES / "keycodes.json").read_text())

    def test_tables_match_the_daemon(self) -> None:
        def table(rows: list[dict], key: str) -> list[tuple[str, int]]:
            return [(r["name"], r[key]) for r in rows]

        self.assertEqual(KEYBOARD_KEYS, table(self.fixture["keyboard"], "usage"))
        self.assertEqual(CONSUMER_CONTROLS, table(self.fixture["consumer"], "usage"))
        self.assertEqual(OPENFLUX_KEYS, table(self.fixture["openflux"], "word"))
        self.assertEqual(KEY_ALIASES, self.fixture["aliases"])

    def test_every_name_and_alias_encodes_like_the_daemon_and_names_round_trip(self) -> None:
        words = self.fixture["words"]
        self.assertEqual(len(words), 2 + len(KEYBOARD_KEYS) + len(CONSUMER_CONTROLS) + len(OPENFLUX_KEYS) + len(KEY_ALIASES))
        for name, word in words.items():
            self.assertEqual(keycode_word(name), word, name)
            if name not in KEY_ALIASES:
                self.assertEqual(keycode_name(word), (name, 0), name)
        for name, _ in KEYBOARD_KEYS + CONSUMER_CONTROLS:
            self.assertEqual(keycode_name(keycode_word(name, 0x05)), (name, 5), name)
        with self.assertRaises(ValueError):
            keycode_word("KC_FF_THEME_CYCLE_NEXT")
        self.assertIsNone(keycode_name(0x40000001))


class SharedActuationFixtureTests(unittest.TestCase):
    """The Python codec agrees with the daemon's on `actuation_words.json` and `default_keymap.bin`."""

    @classmethod
    def setUpClass(cls) -> None:
        cls.fixture = json.loads((FIXTURES / "actuation_words.json").read_text())

    def test_words_decode_like_the_daemon_and_encode_back_byte_identical(self) -> None:
        self.assertTrue(self.fixture["words"])
        for case in self.fixture["words"]:
            word = case["word"]
            dec = decode_actuation_word(word)
            self.assertEqual(
                (
                    dec["actuation_type"],
                    dec["actuation_density"],
                    dec["second_density"],
                    dec["actuation_um"],
                    dec["second_um"],
                    dec["firmware_default"],
                    dec["exact"],
                ),
                (
                    case["actuationType"],
                    case["actuationDensity"],
                    case["secondDensity"],
                    case["actuationUm"],
                    case["secondUm"],
                    case["firmwareDefault"],
                    case["exact"],
                ),
                hex(word),
            )
            self.assertEqual(actuation_entry_json(word), case["entry"], hex(word))
            self.assertEqual(encode_actuation_entry(case["entry"])[0], word, hex(word))

    def test_encodes_match_the_daemon(self) -> None:
        for case in self.fixture["encodes"]:
            self.assertEqual(
                encode_actuation_word(case["actuationType"], case["actuationUm"], case["secondUm"]),
                case["word"],
                case,
            )

    def test_default_keymap_actuation_modes_match_the_daemon(self) -> None:
        raw = (FIXTURES / "default_keymap.bin").read_bytes()
        blob = decode_keymap_blob(raw)
        self.assertEqual(blob.to_bytes(), raw)
        modes = decode_actuation_modes(blob)
        self.assertEqual(modes, self.fixture["defaultKeymapModes"])
        for idx, mode in enumerate(modes):
            for sec in blob.keymaps:
                self.assertEqual(set(sec.actuation_tables[idx]), {encode_actuation_entry(mode)[0]})


class KeymapBlobTests(unittest.TestCase):
    def test_keycodes_and_macro_pool_roundtrip(self) -> None:
        self.assertEqual(encode_keycode("none"), NONE_KEYCODE)
        self.assertEqual(decode_keycode(NONE_KEYCODE)["kind"], "none")

        kc_hid = encode_keycode("hid", code=0x04, modifiers=0x0005)
        self.assertEqual(decode_keycode(kc_hid), {"kind": "hid", "code": 0x04, "modifiers": 5, "raw": f"0x{kc_hid:08x}"})

        kc_macro = encode_keycode("macro", code=0)
        self.assertEqual(decode_keycode(kc_macro)["kind"], "macro")
        self.assertEqual(decode_keycode(kc_macro)["macro_id"], 0)

        step = encode_macro_step(0x28, 15, is_down=True)
        self.assertEqual(decode_macro_step(step), {"is_down": True, "delay_ms": 15, "hid_usage": 0x28, "raw": f"0x{step:08x}"})

        sec = KeymapSection(
            num_actuation_modes=1,
            num_keys=4,
            num_layers=1,
            layers=[[kc_hid, kc_macro, NONE_KEYCODE, encode_keycode("layer_switch", code=1, switch_type=1)]],
            actuation_tables=[[encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 400)] * 4],
            macros=[[step, encode_macro_step(0x28, 0, is_down=False)]],
        )
        blob = FirmwareKeymapBlob(
            num_keymaps=1,
            layout_bitmask=1,
            keymaps=[sec],
        )
        raw = blob.to_bytes()
        decoded = decode_keymap_blob(raw)
        self.assertEqual(decoded.to_bytes(), raw)
        self.assertEqual(decoded.keymaps[0].macros, sec.macros)

        # Removing all macros with dangling MacroKeyCode must fail; after clearing the key it must re-encode cleanly.
        decoded.keymaps[0].macros.clear()
        with self.assertRaises(ValueError):
            decoded.to_bytes()
        decoded.keymaps[0].layers[0][1] = NONE_KEYCODE
        reencoded = decoded.to_bytes()
        redecoded = decode_keymap_blob(reencoded)
        self.assertEqual(redecoded.keymaps[0].macros, [])
        # The section's macro count (twice) and key-code offset are all zero again.
        self.assertEqual(struct.unpack_from("<3I", reencoded, 4 * (6 + 3)), (0, 0, 0))

    def test_corrupted_macro_offset_pointer_rejected_before_usb_io(self) -> None:
        step = encode_macro_step(0x28, 10, is_down=True)
        sec = KeymapSection(
            num_actuation_modes=1,
            num_keys=2,
            num_layers=1,
            layers=[[encode_keycode("macro", code=0), NONE_KEYCODE]],
            actuation_tables=[[encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 400)] * 2],
            macros=[[step]],
        )
        blob = FirmwareKeymapBlob(
            num_keymaps=1,
            layout_bitmask=1,
            keymaps=[sec],
        )
        raw = bytearray(blob.to_bytes())
        # Word 13 (byte offset 52 = 6 list hdr + 7 keymap hdr words) is macro_offsets[0].
        struct.pack_into("<I", raw, 52, 0xFFFFFFFF)
        with self.assertRaises(ValueError) as ctx:
            decode_keymap_blob(bytes(raw))
        self.assertIn("macro_offsets[0]", str(ctx.exception))

        # Also verify `send-keymap` rejects the mutated file before opening libusb / doing USB I/O.
        with tempfile.NamedTemporaryFile(suffix=".bin") as tf:
            tf.write(raw)
            tf.flush()
            with patch.object(openflux_dev, "OpenFluxUsbClient") as mock_client, patch.object(
                sys, "argv", ["openflux_dev.py", "send-keymap", tf.name]
            ):
                with self.assertRaises(ValueError):
                    openflux_dev.main()
                mock_client.assert_not_called()

    def test_nonzero_macro_count_without_macro_pool_rejected_before_usb_io(self) -> None:
        sec = KeymapSection(
            num_actuation_modes=1,
            num_keys=2,
            num_layers=1,
            layers=[[encode_keycode("hid", code=0x04), NONE_KEYCODE]],
            actuation_tables=[[encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 400)] * 2],
        )
        blob = FirmwareKeymapBlob(
            num_keymaps=1,
            layout_bitmask=1,
            keymaps=[sec],
        )
        raw = bytearray(blob.to_bytes())
        # Word 9 (byte offset 36 = 6 list hdr + 3 section words) is n_macros; set to 1 while keycodes_off=0.
        struct.pack_into("<I", raw, 36, 1)
        with self.assertRaises(ValueError) as ctx:
            decode_keymap_blob(bytes(raw))
        self.assertIn("n_macros=1", str(ctx.exception))

        with tempfile.NamedTemporaryFile(suffix=".bin") as tf:
            tf.write(raw)
            tf.flush()
            with patch.object(openflux_dev, "OpenFluxUsbClient") as mock_client, patch.object(
                sys, "argv", ["openflux_dev.py", "send-keymap", tf.name]
            ):
                with self.assertRaises(ValueError):
                    openflux_dev.main()
                mock_client.assert_not_called()

    def test_two_layout_edit_recomputes_second_layout_offset_and_rejects_stale(self) -> None:
        sec1 = KeymapSection(
            num_actuation_modes=1,
            num_keys=4,
            num_layers=1,
            layers=[[encode_keycode("hid", code=0x04)] * 4],
            actuation_tables=[[encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 400)] * 4],
        )
        sec2 = KeymapSection(
            num_actuation_modes=1,
            num_keys=5,
            num_layers=1,
            layers=[[encode_keycode("hid", code=0x05)] * 5],
            actuation_tables=[[encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 400)] * 5],
        )
        blob = FirmwareKeymapBlob(
            num_keymaps=2,
            layout_bitmask=0x3,  # ANSI (1) + ISO (2)
            keymaps=[sec1, sec2],
        )
        raw1 = blob.to_bytes()
        dec1 = decode_keymap_blob(raw1)
        self.assertEqual(dec1.offsets_by_layout, (0, 0, len(sec1.to_words()), 0))

        # Enlarge first layout by adding a macro; second layout offset must advance automatically.
        step = encode_macro_step(0x28, 10, is_down=True)
        dec1.keymaps[0].macros.append([step, encode_macro_step(0x28, 0, is_down=False)])
        raw2 = dec1.to_bytes()
        dec2 = decode_keymap_blob(raw2)
        expected_off2 = len(dec1.keymaps[0].to_words())
        self.assertGreater(expected_off2, len(sec1.to_words()))
        self.assertEqual(dec2.offsets_by_layout, (0, 0, expected_off2, 0))

        # Corrupt offset_layout_2 back to stale size -> decoder must reject before send-keymap.
        stale = bytearray(raw2)
        struct.pack_into("<I", stale, 4 * 4, len(sec1.to_words()))
        with self.assertRaises(ValueError):
            decode_keymap_blob(bytes(stale))


class PayloadBuilderTests(unittest.TestCase):
    def test_openflux_dev_payload_builders(self) -> None:
        self.assertEqual(
            openflux_dev.build_put_file_payload("/home/root/a.txt", b"hi"),
            b"/home/root/a.txt\x00hi",
        )
        self.assertEqual(
            openflux_dev.build_set_time_payload("2026-11-16 12:00:00", "UTC"),
            b"2026-11-16 12:00:00 UTC\x00",
        )


if __name__ == "__main__":
    unittest.main()
