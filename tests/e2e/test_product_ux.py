#!/usr/bin/env python3
"""End-to-end tests of the OpenFlux keyboard stack, written as user stories.

Each test exercises the real compiled components together end-to-end
(RP2040 firmware core <-> UART <-> openflux-bridge <-> openflux-display Wayland/Weston <-> tools/openflux-dev).
"""

import concurrent.futures
import json
import os
import struct
import subprocess
import sys
import time
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "tools/openflux-dev"))

from flux_stack_harness import OpenFluxKeyboardE2ECluster  # noqa: E402
from keymap_codec import FirmwareKeymapBlob, KeymapSection, encode_actuation_word, encode_keycode  # noqa: E402
from profile_settings import ProfileSetting  # noqa: E402


EXPECTED_VERSION = os.environ.get("OPENFLUX_TEST_VERSION", "1.0.0")


class OpenFluxProductUxTests(unittest.TestCase):
    def test_01_user_queries_all_versions_and_pushes_custom_theme(self) -> None:
        """UX: Host connects over USB, inspects all component versions, and pushes a new background theme."""
        with OpenFluxKeyboardE2ECluster() as kb:
            versions = kb.user_queries_versions()
            self.assertEqual(versions["firmware"], f"openflux-firmware {EXPECTED_VERSION}")
            self.assertEqual(versions["bridge"], f"openflux-bridge {EXPECTED_VERSION}")
            self.assertEqual(versions["display"], f"openflux-display {EXPECTED_VERSION}")
            self.assertEqual(versions["os"], "0.0.9")

            # Push a solid-blue PPM background + verify PUT -> LIST -> GET -> DELETE inside OPENFLUX_SYSROOT
            ppm_header = b"P6\n16 9\n255\n"
            blue_ppm = ppm_header + (b"\x00\x00\xff" * (16 * 9))
            kb.user_pushes_file("/home/root/openflux/media/temp_check.ppm", blue_ppm)
            self.assertIn("temp_check.ppm", kb.user_lists_dir("/home/root/openflux/media"))
            self.assertEqual(kb.user_pulls_file("/home/root/openflux/media/temp_check.ppm"), blue_ppm)
            kb.user_deletes_file("/home/root/openflux/media/temp_check.ppm")
            self.assertFalse((kb.media_dir / "temp_check.ppm").exists())

            kb.user_pushes_file("/home/root/openflux/media/ocean.ppm", blue_ppm)
            self.assertEqual((kb.media_dir / "ocean.ppm").read_bytes(), blue_ppm)

            # Verify multi-16KiB upload (>80 KiB) & exact 16 KiB transfer + ZLP boundary over USB
            large_blob = bytes((i % 251 for i in range(80 * 1024 + 123)))
            kb.user_pushes_file("/home/root/openflux/media/large.bin", large_blob)
            self.assertEqual(kb.user_pulls_file("/home/root/openflux/media/large.bin"), large_blob)
            path_part = b"/home/root/openflux/media/exact16k.bin\x00"
            exact_data = b"Z" * (31 * 512 - len(path_part))
            kb.user_pushes_file("/home/root/openflux/media/exact16k.bin", exact_data)
            self.assertEqual(kb.user_pulls_file("/home/root/openflux/media/exact16k.bin"), exact_data)

            # Packet-aligned 1024-byte truncated transfer must NAK cleanly and leave subsequent query working
            bad_hdr = bytearray(512)
            struct.pack_into("<BII", bad_hdr, 0, 102, 2, 700)
            kb.bridge_usb.send_raw_record(bytes(bad_hdr) + b"\x00" * 512)
            nak_resp = kb.bridge_usb.ep1_sock.recv(512)
            self.assertEqual(nak_resp[0], 0)
            self.assertEqual(kb.bridge_usb.ep1_sock.recv(512), b"")
            with self.assertRaises(RuntimeError):
                kb.bridge_usb.transact(250)
            self.assertEqual(kb.user_queries_versions()["bridge"], f"openflux-bridge {EXPECTED_VERSION}")

            before_ppm = kb.read_live_display_ppm()
            app_cfg = json.loads(kb.user_pulls_file("/home/root/openflux/config/appearance.json"))
            app_cfg["defaultTheme"]["backgroundImagePath"] = "ocean.ppm"
            app_cfg["defaultTheme"]["backgroundImageDimming"] = 0.0
            kb.user_pushes_file("/home/root/openflux/config/appearance.json", json.dumps(app_cfg).encode())

            hdr = b"P6\n1920 1080\n255\n"
            off = len(hdr) + (1070 * 1920 + 1910) * 3
            ppm = kb.wait_for_live_display_change(
                before_ppm,
                pred=lambda f: f.startswith(hdr) and f[off + 2] > 200 and f[off] < 40,
            )
            self.assertGreater(ppm[off + 2], 200)
            self.assertLess(ppm[off], 40)

    def test_02_user_flashes_rapid_trigger_keymap_and_types_end_to_end(self) -> None:
        """UX: User flashes a 2-layer rapid-trigger keymap, reboots firmware, presses Fn+Key, and gets expected HID + layer switch."""
        with OpenFluxKeyboardE2ECluster() as kb:
            base = [encode_keycode("hid", code=0x04)] * 85
            base[16] = encode_keycode("layer_switch", code=1, switch_type=1)
            fn_layer = [encode_keycode("hid", code=0x05)] + [encode_keycode("none")] * 84
            trad_table = [encode_actuation_word(1, 1200, 400)] * 85
            rt_table = [encode_actuation_word(2, 600, 200)] * 85
            blob = FirmwareKeymapBlob(
                num_keymaps=1,
                layout_bitmask=0x1,
                keymaps=[KeymapSection(2, 85, 2, [base, fn_layer], [trad_table, rt_table])],
            )

            kb.user_flashes_keymap(blob)
            kb.user_uploads_profiles([
                ProfileSetting(actuation_mode_index=0, default_layer_bitfield=1),
                ProfileSetting(actuation_mode_index=1, default_layer_bitfield=1),
            ])

            kb.user_power_cycles_firmware()
            self.assertEqual(kb.user_dumps_keymap().to_bytes(), blob.to_bytes())

            def key0_region(ppm_bytes: bytes) -> bytes:
                hdr_len = len(b"P6\n1920 1080\n255\n")
                out = bytearray()
                for y in range(342, 436):
                    row_base = hdr_len + y * 1920 * 3
                    out.extend(ppm_bytes[row_base + 28 * 3 : row_base + 122 * 3])
                return bytes(out)

            kb.user_activates_profile(1)
            pressed = kb.user_sets_adc(s6=2100, s13=1500)
            self.assertEqual(pressed["active_actuation_mode"], 1)
            self.assertEqual(pressed["active_layer_bitfield"], 1)
            self.assertEqual(pressed["keycodes"][0], 0x04)
            # Holding Key 0 while pressing Fn keeps Key 0 press-latched at 0x04 while activating layer 1
            held_then_fn = kb.user_sets_adc(s4=2100, s14=1500, s6=2100, s13=1500)
            self.assertEqual(held_then_fn["active_layer_bitfield"], 3)
            self.assertEqual(held_then_fn["keycodes"][0], 0x04)
            # Release Key 0 while holding Fn, then press Key 0 on layer 1 -> emits 0x05
            rel_key0 = kb.user_sets_adc(s4=2100, s14=1500, s6=1800, s13=1800)
            self.assertEqual(rel_key0["active_layer_bitfield"], 3)
            self.assertEqual(rel_key0["keycodes"][0], 0)
            fn_pressed = kb.user_sets_adc(s4=2100, s14=1500, s6=2100, s13=1500)
            self.assertEqual(fn_pressed["active_layer_bitfield"], 3)
            self.assertEqual(fn_pressed["keycodes"][0], 0x05)

            kb.user_sets_adc()
            time.sleep(0.60)
            ppm_base = kb.read_live_display_ppm()
            fn_only = kb.user_sets_adc(s4=2100, s14=1500)
            self.assertEqual(fn_only["active_layer_bitfield"], 3)
            ppm_fn = kb.wait_for_live_display_change(
                ppm_base,
                pred=lambda f: key0_region(f) != key0_region(ppm_base),
            )
            self.assertNotEqual(key0_region(ppm_base), key0_region(ppm_fn))

            released_fn = kb.user_sets_adc()
            self.assertEqual(released_fn["active_layer_bitfield"], 1)
            ppm_reverted = kb.wait_for_live_display_change(
                ppm_fn,
                pred=lambda f: key0_region(f) == key0_region(ppm_base),
            )
            self.assertEqual(key0_region(ppm_reverted), key0_region(ppm_base))

            re_pressed = kb.user_sets_adc(s6=2100, s13=1500)
            self.assertEqual(re_pressed["active_layer_bitfield"], 1)
            self.assertEqual(re_pressed["keycodes"][0], 0x04)

            # NKRO: assign distinct usages 0x04..0x0B across 8 independent keys, press all 8 simultaneously,
            # and verify nkro_bitmap reports every single held key (not dropped after 6).
            nk_base = [encode_keycode("none")] * 85
            nk_keys = [0, 2, 16, 18, 31, 33, 46, 48]
            for i, key_idx in enumerate(nk_keys):
                nk_base[key_idx] = encode_keycode("hid", code=0x04 + i)
            nk_blob = FirmwareKeymapBlob(
                num_keymaps=1,
                layout_bitmask=0x1,
                keymaps=[KeymapSection(2, 85, 2, [nk_base, [encode_keycode("none")] * 85], [trad_table, rt_table])],
            )
            kb.user_flashes_keymap(nk_blob)
            nk_state = kb.user_sets_adc(
                s6=2550, s13=1050,
                s22=2550, s16=1050,
                s4=2550, s14=1050,
                s9=2550, s17=1050,
                s3=2550, s11=1050,
                s19=2550, s31=1050,
                s2=2550, s10=1050,
                s18=2550, s28=1050,
            )
            nk_bm = bytes.fromhex(nk_state["nkro_bitmap"])
            for code in range(0x04, 0x0C):
                self.assertTrue((nk_bm[code >> 3] >> (code & 7)) & 1, f"NKRO missing usage 0x{code:02x}")
            rel_nk = kb.user_sets_adc()
            self.assertEqual(rel_nk["nkro_bitmap"], "00" * 32)

            # A large keymap (8 layers, about 3 KiB) is read back while keys are being pressed: the
            # firmware sends the response over several scans and keeps reporting key presses meanwhile.
            big_layers = [[encode_keycode("hid", code=0x04 + (l % 20)) for _ in range(85)] for l in range(8)]
            big_tables = [[encode_actuation_word(1, 1200, 400)] * 85 for _ in range(8)]
            big_blob = FirmwareKeymapBlob(
                num_keymaps=1,
                layout_bitmask=0x1,
                keymaps=[KeymapSection(8, 85, 8, big_layers, big_tables)],
            )
            kb.user_flashes_keymap(big_blob)
            with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                dump_fut = pool.submit(kb.user_dumps_keymap)
                press_state = kb.user_sets_adc(s6=2550, s13=1050)
                self.assertEqual(press_state["keycodes"][0], 0x04)
                dumped = dump_fut.result(timeout=5.0)
            self.assertEqual(dumped.to_bytes(), big_blob.to_bytes())
            after_dump = kb.user_sets_adc()
            self.assertEqual(after_dump["nkro_bitmap"], "00" * 32)

    def test_03_host_bridges_switch_profile_language_and_stream_live_data(self) -> None:
        """UX: Host bridge commands switch firmware profile/language and stream media+CPU to display."""
        with OpenFluxKeyboardE2ECluster() as kb:
            def tile_region(ppm_bytes: bytes, x0: int, x1: int, y0: int = 30, y1: int = 190) -> bytes:
                hdr_len = len(b"P6\n1920 1080\n255\n")
                out = bytearray()
                for y in range(y0, y1):
                    row_base = hdr_len + y * 1920 * 3
                    out.extend(ppm_bytes[row_base + x0 * 3 : row_base + x1 * 3])
                return bytes(out)

            before_ppm = kb.read_live_display_ppm()
            time.sleep(0.2)
            self.assertEqual(kb.read_live_display_ppm(), before_ppm)

            kb.run_host_bridge_tick(
                profile_idx=3,
                language_code=334,
                platform_code=2,
                media_title="Open Stack Jam",
                cpu_pct=42.5,
            )

            state = kb.user_sets_adc()
            self.assertEqual(state["active_profile"], 3)
            self.assertEqual(state["language_code"], 334)
            self.assertEqual(state["platform_code"], 2)

            after_ppm = kb.wait_for_live_display_change(
                before_ppm,
                pred=lambda f: tile_region(f, 80, 440) != tile_region(before_ppm, 80, 440)
                and tile_region(f, 520, 880) != tile_region(before_ppm, 520, 880),
            )
            self.assertNotEqual(tile_region(before_ppm, 80, 440), tile_region(after_ppm, 80, 440))
            self.assertNotEqual(tile_region(before_ppm, 520, 880), tile_region(after_ppm, 520, 880))
            self.assertEqual(tile_region(before_ppm, 1440, 1800), tile_region(after_ppm, 1440, 1800))

    def test_04_uart_handoff_marker_and_clean_service_shutdown(self) -> None:
        """UX: Display advertises active marker so Bridge owns UART cleanly, and SIGTERM tears down both cleanly."""
        with OpenFluxKeyboardE2ECluster() as kb:
            self.assertTrue(kb.display_marker.exists())
            d_rc, b_rc = kb.shutdown_display_and_bridge()
            self.assertEqual(d_rc, 0)
            self.assertEqual(b_rc, 0)
            self.assertFalse(kb.display_marker.exists())

    def test_05_partial_startup_failure_and_hung_child_leave_no_survivors(self) -> None:
        """UX/Ops: Startup readiness failure and SIGTERM-ignoring children must both clean up cleanly."""
        cluster = OpenFluxKeyboardE2ECluster()
        orig_wait = cluster._wait_for
        started_pids: list[int] = []

        def fail_on_display(pred, desc, timeout_s=5.0):
            if desc == "display ready":
                for p in (cluster.weston, cluster.fw, cluster.bridge, cluster.display):
                    if p is not None:
                        started_pids.append(p.pid)
                raise RuntimeError("injected display readiness failure")
            return orig_wait(pred, desc, timeout_s)

        with mock.patch.object(cluster, "_wait_for", side_effect=fail_on_display):
            with self.assertRaises(RuntimeError):
                cluster.__enter__()
        self.assertGreaterEqual(len(started_pids), 3)
        for pid in started_pids:
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)

        c2 = OpenFluxKeyboardE2ECluster()
        hung = subprocess.Popen(
            [sys.executable, "-c", "import signal, sys, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); sys.stdout.write('READY\\n'); sys.stdout.flush(); time.sleep(30)"],
            stdout=subprocess.PIPE,
            text=True,
        )
        assert hung.stdout is not None and hung.stdout.readline().strip() == "READY"
        hung.stdout.close()
        sibling = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])
        c2.display = hung
        c2.bridge = sibling
        c2.__exit__(None, None, None)
        self.assertEqual(hung.returncode, -9)
        for pid in (hung.pid, sibling.pid):
            with self.assertRaises(ProcessLookupError):
                os.kill(pid, 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)
