#!/usr/bin/env python3
"""Linux integration tests for the compiled openflux-display binary."""

import json
import os
import shutil
import select
import signal
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "tools/openflux-dev"))
import simple_hdlc

DISPLAY_BIN = Path(os.environ.get("OPENFLUX_TEST_DISPLAY_BIN") or (ROOT / "keyboard/display/build/openflux-display"))
BRIDGE_BIN = Path(os.environ.get("OPENFLUX_TEST_BRIDGE_BIN") or (ROOT / "keyboard/bridge/build/openflux-bridge"))


def write_test_configs(cfg: Path, bg_path: str = "") -> None:
    cfg.mkdir(exist_ok=True)
    (cfg / "appearance.json").write_text(json.dumps({
        "selectedId": "default",
        "defaultTheme": {
            "id": "default",
            "name": "Open Test",
            "backgroundImagePath": bg_path,
            "backgroundImageDimming": 15.0,
            "moduleBackgroundDimming": 45.0,
            "selectedKeyEffect": "glowing_rings",
            "ringGlowColor": 0xFF66CCFF,
            "legendColor": 0xFFFFFFFF,
        },
        "customThemes": [],
    }))
    (cfg / "settings.json").write_text(json.dumps({
        "parallaxActive": True,
        "eyeHeight": 425.0,
        "eyeDistanceFromFront": 225.0,
        "footStandMode": "low",
    }))
    (cfg / "keymap.json").write_text(json.dumps({
        "selectedFrameLayoutType": "ansi",
        "keymaps": {
            "ansi": {
                "defaultLayer": 0,
                "layers": [
                    {"name": "Base", "keyActions": [{"keyCode": "KC_ESCAPE", "type": 0}, {"keyCode": "KC_F1", "type": 0}]},
                    {"name": "FN", "keyActions": [{"keyCode": "KC_TRANSPARENT", "type": 0}, {"keyCode": "KC_BRIGHTNESS_DOWN", "type": 0}]},
                ],
            }
        },
    }))
    (cfg / "mosaic.json").write_text(json.dumps({
        "tileTypes": [12, 8, 15, 13],
        "tileConfigs": [{}, {}, {"city": "Denver", "celcius": False}, {"main": "5", "shortBreak": "5", "longBreak": "15"}],
    }))


def test_version_and_headless_ppm_render() -> None:
    res = subprocess.run([str(DISPLAY_BIN), "--version"], capture_output=True, text=True, check=True)
    assert res.stdout.strip().startswith("openflux-display "), res.stdout

    with tempfile.TemporaryDirectory() as tmp:
        cfg = Path(tmp) / "config"
        mp4 = Path(tmp) / "theme.mp4"
        has_ffmpeg = bool(shutil.which("ffmpeg"))
        if has_ffmpeg:
            # Generate 1920x1080 multi-color MP4: 0.2s Red (3 frames) + 0.2s Blue (3 frames) @ 15fps
            subprocess.run([
                "ffmpeg", "-y", "-loglevel", "error",
                "-f", "lavfi", "-i", "color=c=0xFF0000:duration=0.2:size=1920x1080:rate=15",
                "-f", "lavfi", "-i", "color=c=0x0000FF:duration=0.2:size=1920x1080:rate=15",
                "-filter_complex", "[0:v][1:v]concat=n=2:v=1:a=0",
                str(mp4),
            ], check=True)
            write_test_configs(cfg, bg_path=str(mp4))
        else:
            write_test_configs(cfg, bg_path="")

        live1 = Path(tmp) / "live1.json"
        live1.write_text(
            json.dumps({"type": "key_state", "seq": 1, "active_layer_bitfield": 1, "key_bitmap": "00" * 32}) + "\n"
            + json.dumps({"type": "live_data", "data_type": 0, "payload": {"hardwareData": {"cpu": 37}}}) + "\n"
        )
        ppm1 = Path(tmp) / "frame1.ppm"
        disp_env = os.environ.copy()
        if has_ffmpeg and not Path("/usr/lib/openflux/display/openflux-ffmpeg").is_file():
            disp_env["OPENFLUX_FFMPEG"] = shutil.which("ffmpeg") or ""
        subprocess.run([
            str(DISPLAY_BIN),
            "--config-dir", str(cfg),
            "--media-dir", tmp,
            "--live-json", str(live1),
            "--render-ppm", str(ppm1),
        ], env=disp_env, check=True)
        raw1 = ppm1.read_bytes()
        hdr = b"P6\n1920 1080\n255\n"
        assert raw1.startswith(hdr)
        assert len(raw1) == len(hdr) + 1920 * 1080 * 3
        if has_ffmpeg:
            off = len(hdr) + (1070 * 1920 + 1910) * 3
            r_px, g_px, b_px = raw1[off], raw1[off + 1], raw1[off + 2]
            assert r_px > 180 and r_px > g_px + 100 and r_px > b_px + 100, (r_px, g_px, b_px)

            # Target frame 5 is blue; target frame 8 is red after looping.
            env_f5 = disp_env.copy()
            env_f5["OPENFLUX_VIDEO_TARGET_FRAME"] = "5"
            env_f5["OPENFLUX_VIDEO_FPS"] = "15"
            ppm5 = Path(tmp) / "frame5.ppm"
            t0 = time.monotonic()
            subprocess.run([
                str(DISPLAY_BIN),
                "--config-dir", str(cfg),
                "--media-dir", tmp,
                "--live-json", str(live1),
                "--render-ppm", str(ppm5),
            ], env=env_f5, check=True)
            dt_f5 = time.monotonic() - t0
            raw5 = ppm5.read_bytes()
            assert raw5[off + 2] > 180 and raw5[off + 2] > raw5[off] + 100, (raw5[off], raw5[off + 1], raw5[off + 2])
            assert dt_f5 < 3.0, f"Video pipe drain throttled: {dt_f5:.2f}s"

            env_f8 = disp_env.copy()
            env_f8["OPENFLUX_VIDEO_TARGET_FRAME"] = "8"
            env_f8["OPENFLUX_VIDEO_FPS"] = "15"
            ppm8 = Path(tmp) / "frame8.ppm"
            subprocess.run([
                str(DISPLAY_BIN),
                "--config-dir", str(cfg),
                "--media-dir", tmp,
                "--live-json", str(live1),
                "--render-ppm", str(ppm8),
            ], env=env_f8, check=True)
            raw8 = ppm8.read_bytes()
            assert raw8[off] > 180 and raw8[off] > raw8[off + 2] + 100, (raw8[off], raw8[off + 1], raw8[off + 2])

        # Layer switch via active_layer_bitfield=2 under identical static config/env/timestamp/live-data must change the rendered frame
        cfg_static = Path(tmp) / "cfg_static"
        write_test_configs(cfg_static, bg_path="")
        env_fixed = os.environ.copy()
        env_fixed["OPENFLUX_FIXED_TIME_SEC"] = "1763414400"
        live_layer1 = Path(tmp) / "live_layer1.json"
        live_layer1.write_text(
            json.dumps({"type": "key_state", "seq": 1, "active_layer_bitfield": 1, "key_bitmap": "00" * 32}) + "\n"
        )
        live_layer2 = Path(tmp) / "live_layer2.json"
        live_layer2.write_text(
            json.dumps({"type": "key_state", "seq": 2, "active_layer_bitfield": 2, "key_bitmap": "00" * 32}) + "\n"
        )
        ppm_layer1 = Path(tmp) / "frame_layer1.ppm"
        ppm_layer2 = Path(tmp) / "frame_layer2.ppm"
        for live_p, ppm_p in ((live_layer1, ppm_layer1), (live_layer2, ppm_layer2)):
            subprocess.run([
                str(DISPLAY_BIN),
                "--config-dir", str(cfg_static),
                "--media-dir", tmp,
                "--live-json", str(live_p),
                "--render-ppm", str(ppm_p),
            ], env=env_fixed, check=True)
        assert ppm_layer1.read_bytes() != ppm_layer2.read_bytes(), "Layer switch did not change rendered frame"

        # Deterministic pixel-for-pixel reproducibility at fixed timestamp
        ppm_disp_a = Path(tmp) / "disp_fixed_a.ppm"
        ppm_disp_b = Path(tmp) / "disp_fixed_b.ppm"
        for out_p in (ppm_disp_a, ppm_disp_b):
            subprocess.run([
                str(DISPLAY_BIN),
                "--config-dir", str(cfg_static),
                "--media-dir", tmp,
                "--live-json", str(live_layer2),
                "--render-ppm", str(out_p),
            ], env=env_fixed, check=True)
        assert ppm_disp_a.read_bytes() == ppm_disp_b.read_bytes(), "openflux-display fixed-time renders differ"
        print("openflux-display headless 1920x1080 PPM + video/layer + deterministic fixed-time render OK")


def test_real_weston_compositor_and_bridge_ipc() -> None:
    """Run compiled openflux-display against real headless Weston compositor + openflux-bridge IPC."""
    require_full = os.environ.get("OPENFLUX_REQUIRE_FULL_E2E") == "1"
    if not shutil.which("weston") or not BRIDGE_BIN.exists():
        msg = f"Missing Weston ({shutil.which('weston')}) or Bridge binary ({BRIDGE_BIN})"
        if require_full:
            raise RuntimeError(f"OPENFLUX_REQUIRE_FULL_E2E=1 set, cannot skip cross-stack test: {msg}")
        print(f"Skipping Weston compositor test ({msg})")
        return

    with tempfile.TemporaryDirectory() as tmp:
        xdg_dir = Path(tmp) / "xdg"
        xdg_dir.mkdir(mode=0o700)
        cfg = Path(tmp) / "config"
        log_dir = Path(tmp) / "logs"
        log_dir.mkdir()
        write_test_configs(cfg, bg_path="")

        sock_name = "wayland-openflux-test"
        w_env = os.environ.copy()
        w_env["XDG_RUNTIME_DIR"] = str(xdg_dir)
        weston = subprocess.Popen(
            ["weston", "--backend=headless-backend.so", f"--socket={sock_name}", "--width=1920", "--height=1080", "--idle-time=0"],
            env=w_env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        bridge = None
        disp = None
        in_r, in_w = os.pipe()
        out_r, out_w = os.pipe()
        uart_rx_r, uart_rx_w = os.pipe()
        uart_tx_r, uart_tx_w = os.pipe()
        for fd in (out_w, in_r, uart_rx_r, uart_tx_w):
            os.set_inheritable(fd, True)

        try:
            deadline = time.monotonic() + 5.0
            while time.monotonic() < deadline and not (xdg_dir / sock_name).exists():
                time.sleep(0.02)
            assert (xdg_dir / sock_name).exists(), "Headless weston did not create Wayland socket"

            ready_file = Path(tmp) / "bridge_ready"
            bridge_sock = Path(tmp) / "bridge.sock"
            marker_file = Path(tmp) / "openflux_display_active"

            b_env = os.environ.copy()
            b_env["OPENFLUX_READY_FILE"] = str(ready_file)
            b_env["OPENFLUX_TEST_EP_FDS"] = f"{out_w},{in_r}"
            b_env["OPENFLUX_TEST_UART_FDS"] = f"{uart_rx_r},{uart_tx_w}"
            b_env["OPENFLUX_BRIDGE_SOCK"] = str(bridge_sock)
            bridge = subprocess.Popen([str(BRIDGE_BIN)], env=b_env, pass_fds=(out_w, in_r, uart_rx_r, uart_tx_w))
            os.close(out_w); os.close(in_r); os.close(uart_rx_r); os.close(uart_tx_w)

            while time.monotonic() < deadline and not (ready_file.exists() and bridge_sock.exists()):
                time.sleep(0.02)
            assert bridge_sock.exists(), "openflux-bridge IPC socket missing"

            d_env = os.environ.copy()
            d_env["XDG_RUNTIME_DIR"] = str(xdg_dir)
            d_env["WAYLAND_DISPLAY"] = sock_name
            d_env["OPENFLUX_CONFIG_DIR"] = str(cfg)
            d_env["OPENFLUX_MEDIA_DIR"] = tmp
            d_env["OPENFLUX_LOG_DIR"] = str(log_dir)
            d_env["OPENFLUX_BRIDGE_SOCK"] = str(bridge_sock)
            d_env["OPENFLUX_DISPLAY_MARKER"] = str(marker_file)
            disp = subprocess.Popen(
                [str(DISPLAY_BIN), "--width=1920", "--height=1080"],
                env=d_env,
            )

            hb_file = log_dir / "display_heartbeat.log"
            while time.monotonic() < deadline and not (hb_file.exists() and marker_file.exists()):
                time.sleep(0.02)
            assert hb_file.exists() and marker_file.exists(), "openflux-display did not write heartbeat or active marker"

            # Feed a canonical 139-byte UART MSG_UPDATE and verify ACK with bounded polling while display consumes IPC over Weston
            msg = bytearray(139)
            msg[0] = 2
            struct.pack_into("<I", msg, 1, 119)
            struct.pack_into("<I", msg, 5 + 0x27, 2)  # active_layer_bitfield = 2
            os.write(uart_rx_w, simple_hdlc.encode_frame(bytes(msg)))
            dec = simple_hdlc.SimpleHdlcDecoder()
            ack_frame = None
            ack_deadline = time.monotonic() + 5.0
            while time.monotonic() < ack_deadline:
                rem = max(0.05, ack_deadline - time.monotonic())
                readable, _, _ = select.select([uart_tx_r], [], [], rem)
                if not readable:
                    continue
                chunk = os.read(uart_tx_r, 64)
                if not chunk:
                    break
                frames = dec.feed(chunk)
                if frames:
                    ack_frame = frames[0]
                    break
            assert ack_frame == bytes([2, 0, 0, 0]), f"Unexpected UART ACK: {ack_frame!r}"

            # Shut down Weston compositor -> openflux-display must detect disconnect and exit cleanly without spinning
            weston.send_signal(signal.SIGTERM)
            weston.wait(timeout=3.0)
            disp_rc = disp.wait(timeout=3.0)
            assert disp.poll() is not None, "openflux-display did not exit on compositor shutdown"
            assert not marker_file.exists(), "openflux-display did not unlink active marker on compositor exit"

            bridge.send_signal(signal.SIGTERM)
            assert bridge.wait(timeout=2.0) == 0
            print(f"openflux-display real Weston + bridge IPC + disconnect OK (exit={disp_rc})")
        finally:
            for p in (disp, bridge, weston):
                if p is not None and p.poll() is None:
                    p.kill()
                    p.wait(timeout=2.0)
            for fd in (in_w, out_r, uart_rx_w, uart_tx_r):
                try:
                    os.close(fd)
                except OSError:
                    pass


if __name__ == "__main__":
    if not DISPLAY_BIN.exists():
        raise FileNotFoundError(f"Missing compiled binary at {DISPLAY_BIN}")
    test_version_and_headless_ppm_render()
    test_real_weston_compositor_and_bridge_ipc()
