#!/usr/bin/env python3
"""Encapsulated full-stack E2E harness wiring OpenFlux firmware core, bridge, Weston display, and tools/openflux-dev."""

from __future__ import annotations

import json
import os
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools/openflux-dev"))

import openflux_dev  # noqa: E402
from keymap_codec import FirmwareKeymapBlob, decode_keymap_blob  # noqa: E402
from profile_settings import ProfileSetting, ProfileSettingsFile  # noqa: E402
from simple_hdlc import DataType, encode_live_data  # noqa: E402

BRIDGE_BIN = Path(os.environ.get("OPENFLUX_TEST_BRIDGE_BIN") or (ROOT / "keyboard/bridge/build/openflux-bridge"))
DISPLAY_BIN = Path(os.environ.get("OPENFLUX_TEST_DISPLAY_BIN") or (ROOT / "keyboard/display/build/openflux-display"))
FW_SIM_BIN = ROOT / "tests/e2e/build/fw_sim"
EXPECTED_VERSION = os.environ.get("OPENFLUX_TEST_VERSION", "1.0.0")


def compile_fw_sim() -> Path:
    build_dir = FW_SIM_BIN.parent
    build_dir.mkdir(parents=True, exist_ok=True)
    fw_src = ROOT / "keyboard/firmware/src"
    lfs_dir = ROOT / "third_party/littlefs"
    cc = shutil.which("cc") or shutil.which("gcc") or shutil.which("clang")
    if not cc:
        raise RuntimeError("No C compiler available to build fw_sim")
    srcs = [
        str(ROOT / "tests/e2e/fw_sim.c"),
        str(fw_src / "fw_calibration.c"),
        str(fw_src / "fw_classic_row.c"),
        str(fw_src / "fw_classic_neighbors.c"),
        str(fw_src / "fw_classic_keys.c"),
        str(fw_src / "fw_depth.c"),
        str(fw_src / "fw_hdlc.c"),
        str(fw_src / "fw_keyframe.c"),
        str(fw_src / "fw_lifecycle.c"),
        str(fw_src / "fw_rapid.c"),
        str(fw_src / "fw_refine.c"),
        str(fw_src / "fw_refinement_record.c"),
        str(fw_src / "fw_rest.c"),
        str(fw_src / "fw_rest_runtime.c"),
        str(fw_src / "fw_scanner.c"),
        str(fw_src / "fw_thermal.c"),
        str(fw_src / "fw_usb_cmd.c"),
        str(fw_src / "lfs_store.c"),
        str(lfs_dir / "lfs.c"),
        str(lfs_dir / "lfs_util.c"),
    ]
    subprocess.run(
        [
            cc,
            "-O2",
            "-std=c11",
            "-DHOST_TEST",
            f'-DFW_PROJECT_VERSION="{EXPECTED_VERSION}"',
            f"-I{fw_src}",
            f"-I{lfs_dir}",
            *srcs,
            "-o",
            str(FW_SIM_BIN),
        ],
        check=True,
    )
    return FW_SIM_BIN


class _BridgePipeAdapter:
    """Connects host openflux-dev USB framing to openflux-bridge over record-preserving SOCK_SEQPACKET."""

    def __init__(self, ep2_sock: socket.socket, ep1_sock: socket.socket) -> None:
        self.ep2_sock = ep2_sock
        self.ep1_sock = ep1_sock
        self.client = openflux_dev.OpenFluxUsbClient.__new__(openflux_dev.OpenFluxUsbClient)

    def send_raw_record(self, data: bytes) -> None:
        self.ep2_sock.send(data)

    def transact(self, cmd: int, payload: bytes = b"") -> bytes:
        tx_buf = bytearray()

        def write_fn(_ep: int, data: bytes, _to: int) -> None:
            if not data:
                off = 0
                while off + 16384 <= len(tx_buf):
                    self.ep2_sock.send(bytes(tx_buf[off : off + 16384]))
                    off += 16384
                if off < len(tx_buf):
                    self.ep2_sock.send(bytes(tx_buf[off:]))
                elif tx_buf:
                    self.ep2_sock.send(b"")
                tx_buf.clear()
                return
            tx_buf.extend(data)

        def read_fn(_ep: int, length: int, timeout_ms: int) -> bytes:
            self.ep1_sock.settimeout(max(0.05, timeout_ms / 1000.0))
            try:
                return self.ep1_sock.recv(length)
            except (TimeoutError, socket.timeout):
                raise RuntimeError("seqpacket read timeout (ZLP/idle)")

        return self.client.transact_io(write_fn, read_fn, openflux_dev.BRIDGE_TARGET, cmd, payload, timeout_ms=2000)


def _recv_exact(sock: socket.socket, n: int) -> bytes:
    """Reads exactly n bytes from a stream socket; a short read is an error."""
    buf = bytearray()
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise RuntimeError(f"Firmware simulator closed the connection after {len(buf)} of {n} bytes")
        buf.extend(chunk)
    return bytes(buf)


class _FwUnixAdapter:
    """Connects host openflux-dev USB framing to simulated RP2040 firmware over Unix socket."""

    def __init__(self, usb_sock: str) -> None:
        self.usb_sock = usb_sock

    def transact(self, cmd: int, payload: bytes = b"") -> bytes:
        pkt_sz = 64
        n_pkts = (len(payload) + pkt_sz - 1) // pkt_sz if payload else 0
        hdr = bytearray(pkt_sz)
        struct.pack_into("<BII", hdr, 0, cmd, n_pkts, len(payload))
        pkts = [bytes(hdr)]
        for off in range(0, len(payload), pkt_sz):
            chunk = payload[off : off + pkt_sz]
            pkts.append(chunk.ljust(pkt_sz, b"\x00"))
        pkts.append(b"")

        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(5.0)
            s.connect(self.usb_sock)
            s.sendall(struct.pack("<I", len(pkts)))
            for p in pkts:
                s.sendall(struct.pack("<I", len(p)) + p)
            total = struct.unpack("<I", _recv_exact(s, 4))[0]
            buf = _recv_exact(s, total)
        if len(buf) < 9:
            raise RuntimeError("Short firmware response header")
        rtype, _rpkts, rbytes = struct.unpack_from("<BII", buf, 0)
        if rtype != openflux_dev.RESP_ACK:
            raise RuntimeError(f"Firmware returned NAK ({rtype})")
        return bytes(buf[64 : 64 + rbytes])


class _E2EHostBridgeTransport:
    """Routes simulated host bridge events straight into simulated firmware + openflux-bridge."""

    def __init__(self, fw_usb: _FwUnixAdapter, bridge_usb: _BridgePipeAdapter) -> None:
        self.fw = fw_usb
        self.bridge = bridge_usb

    def set_profile(self, profile_idx: int) -> None:
        self.fw.transact(openflux_dev.FW_CMD_SET_ACTIVE_PROFILE, struct.pack("<I", profile_idx))

    def set_language(self, lang_code: int) -> None:
        self.fw.transact(openflux_dev.FW_CMD_SET_LANGUAGE, struct.pack("<I", lang_code))

    def set_platform(self, platform_code: int) -> None:
        self.fw.transact(openflux_dev.FW_CMD_SET_PLATFORM, struct.pack("<I", platform_code))

    def send_live_data(self, data_type: DataType, json_payload: dict[str, Any] | None = None) -> None:
        body = json.dumps(json_payload, separators=(",", ":")).encode("utf-8") if json_payload is not None else b""
        for frame in encode_live_data(data_type, body):
            self.bridge.transact(openflux_dev.BRIDGE_CMD_SEND_LIVE_DATA, frame)


class OpenFluxKeyboardE2ECluster:
    """Full end-to-end virtual OpenFlux Keyboard (firmware + UART + bridge + Weston + display + host)."""

    def __init__(self) -> None:
        self._td = tempfile.TemporaryDirectory(prefix="openflux_e2e_")
        self.root = Path(self._td.name)
        self.cfg_dir = self.root / "home/root/openflux/config"
        self.media_dir = self.root / "home/root/openflux/media"
        self.log_dir = self.root / "home/root/openflux/logs"
        self.ver_dir = self.root / "home/root/.openflux/versions"
        self.cfg_dir.mkdir(parents=True)
        self.media_dir.mkdir(parents=True)
        self.log_dir.mkdir(parents=True)
        self.ver_dir.mkdir(parents=True)
        display_ver = subprocess.check_output([str(DISPLAY_BIN), "--version"], text=True).strip()
        (self.ver_dir / "display").write_text(f"{display_ver}\n")
        # The keyboard's base OS image keeps its version in this file (see OS_VERSION_FILE in the bridge).
        (self.root / "home/root/gazpacho_version.txt").write_text("0.0.9\n")
        self.xdg_dir = self.root / "xdg"
        self.xdg_dir.mkdir(mode=0o700)

        self.weston: subprocess.Popen | None = None
        self.fw: subprocess.Popen | None = None
        self.bridge: subprocess.Popen | None = None
        self.display: subprocess.Popen | None = None
        self._open_fds: set[int] = set()
        self._open_sockets: list[socket.socket] = []

    def _seed_configs(self) -> None:
        (self.cfg_dir / "appearance.json").write_text(json.dumps({
            "selectedId": "default",
            "defaultTheme": {
                "id": "default",
                "name": "Aurora",
                "backgroundImagePath": "",
                "backgroundImageDimming": 10.0,
                "moduleBackgroundDimming": 40.0,
                "selectedKeyEffect": "glowing_rings",
                "ringGlowColor": 0xFF34D399,
                "legendColor": 0xFFFFFFFF,
            },
            "customThemes": [],
        }))
        (self.cfg_dir / "settings.json").write_text(json.dumps({
            "parallaxActive": True,
            "eyeHeight": 425.0,
            "eyeDistanceFromFront": 225.0,
            "footStandMode": "low",
        }))
        (self.cfg_dir / "keymap.json").write_text(json.dumps({
            "selectedFrameLayoutType": "ansi",
            "keymaps": {
                "ansi": {
                    "defaultLayer": 0,
                    "layers": [
                        {"name": "Base", "keyActions": [{"keyCode": "KC_A", "type": 0} for _ in range(85)]},
                        {"name": "FN", "keyActions": [{"keyCode": "KC_F1", "type": 0} for _ in range(85)]},
                    ],
                }
            },
        }))
        (self.cfg_dir / "mosaic.json").write_text(json.dumps({
            "tileTypes": [8, 16, 15, 13],  # Hardware, Active Media, Weather, Pomodoro
            "tileConfigs": [{}, {}, {"city": "Denver", "celcius": False}, {"main": "25", "shortBreak": "5", "longBreak": "15"}],
        }))

    def _close_fd(self, fd: int) -> None:
        if fd in self._open_fds:
            self._open_fds.remove(fd)
            try:
                os.close(fd)
            except OSError:
                pass

    @staticmethod
    def _stop_proc(proc: subprocess.Popen | None) -> int:
        if proc is None:
            return 0
        if proc.poll() is not None:
            return proc.returncode
        try:
            proc.send_signal(signal.SIGTERM)
            return proc.wait(timeout=1.5)
        except subprocess.TimeoutExpired:
            try:
                proc.kill()
                return proc.wait(timeout=2.0)
            except Exception:
                return -1
        except Exception:
            return -1

    def __enter__(self) -> "OpenFluxKeyboardE2ECluster":
        if not shutil.which("weston") or not BRIDGE_BIN.exists() or not DISPLAY_BIN.exists():
            raise RuntimeError("Full E2E cluster requires weston, compiled openflux-bridge, and compiled openflux-display")
        try:
            compile_fw_sim()
            self._seed_configs()

            sock_name = "wayland-e2e-openflux"
            w_env = os.environ.copy()
            w_env["XDG_RUNTIME_DIR"] = str(self.xdg_dir)
            self.weston = subprocess.Popen(
                ["weston", "--backend=headless-backend.so", f"--socket={sock_name}", "--width=1920", "--height=1080", "--idle-time=0"],
                env=w_env,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            self._wait_for(lambda: (self.xdg_dir / sock_name).exists(), "Weston Wayland socket")

            f2b_r, f2b_w = os.pipe()
            b2f_r, b2f_w = os.pipe()
            ep2_host, ep2_dev = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
            ep1_host, ep1_dev = socket.socketpair(socket.AF_UNIX, socket.SOCK_SEQPACKET)
            for s in (ep2_host, ep2_dev, ep1_host, ep1_dev):
                s.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 262144)
                s.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 262144)
            self._open_sockets.extend([ep2_host, ep2_dev, ep1_host, ep1_dev])
            ep2_r = ep2_dev.fileno()
            ep1_w = ep1_dev.fileno()
            self._open_fds.update([f2b_r, f2b_w, b2f_r, b2f_w])
            for fd in (f2b_w, b2f_r, f2b_r, b2f_w, ep2_r, ep1_w):
                os.set_inheritable(fd, True)

            self.fw_usb_sock = str(self.root / "fw_usb.sock")
            self.fw_ctrl_sock = str(self.root / "fw_ctrl.sock")
            f_env = os.environ.copy()
            f_env["OPENFLUX_SIM_UART_FDS"] = f"{b2f_r},{f2b_w}"
            f_env["OPENFLUX_SIM_USB_SOCK"] = self.fw_usb_sock
            f_env["OPENFLUX_SIM_CTRL_SOCK"] = self.fw_ctrl_sock
            self.fw = subprocess.Popen([str(FW_SIM_BIN)], env=f_env, pass_fds=(b2f_r, f2b_w))
            self._close_fd(b2f_r)
            self._close_fd(f2b_w)
            self._wait_for(lambda: Path(self.fw_usb_sock).exists() and Path(self.fw_ctrl_sock).exists(), "fw_sim sockets")

            self.bridge_sock = str(self.root / "openflux_bridge.sock")
            self.display_marker = self.root / "openflux_display_active"
            self.bridge_ready = self.root / "openflux_bridge_ready"
            b_env = os.environ.copy()
            b_env["OPENFLUX_SYSROOT"] = str(self.root)
            b_env["OPENFLUX_READY_FILE"] = str(self.bridge_ready)
            b_env["OPENFLUX_TEST_EP_FDS"] = f"{ep1_w},{ep2_r}"
            b_env["OPENFLUX_TEST_UART_FDS"] = f"{f2b_r},{b2f_w}"
            b_env["OPENFLUX_BRIDGE_SOCK"] = self.bridge_sock
            b_env["OPENFLUX_DISPLAY_MARKER"] = str(self.display_marker)
            self.bridge = subprocess.Popen([str(BRIDGE_BIN)], env=b_env, pass_fds=(ep2_r, ep1_w, f2b_r, b2f_w))
            for child_fd in (f2b_r, b2f_w):
                self._close_fd(child_fd)
            ep2_dev.close()
            ep1_dev.close()
            self._wait_for(lambda: self.bridge_ready.exists() and Path(self.bridge_sock).exists(), "bridge ready")

            self.live_ppm_path = self.root / "live_display.ppm"
            d_env = os.environ.copy()
            d_env["XDG_RUNTIME_DIR"] = str(self.xdg_dir)
            d_env["WAYLAND_DISPLAY"] = sock_name
            d_env["OPENFLUX_CONFIG_DIR"] = str(self.cfg_dir)
            d_env["OPENFLUX_MEDIA_DIR"] = str(self.media_dir)
            d_env["OPENFLUX_LOG_DIR"] = str(self.log_dir)
            d_env["OPENFLUX_BRIDGE_SOCK"] = self.bridge_sock
            d_env["OPENFLUX_DISPLAY_MARKER"] = str(self.display_marker)
            d_env["OPENFLUX_LIVE_PPM_PATH"] = str(self.live_ppm_path)
            self.display = subprocess.Popen([str(DISPLAY_BIN), "--width=1920", "--height=1080"], env=d_env)
            self._wait_for(
                lambda: (self.log_dir / "display_heartbeat.log").exists()
                and self.display_marker.exists()
                and self.live_ppm_path.exists(),
                "display ready",
            )

            self.fw_usb = _FwUnixAdapter(self.fw_usb_sock)
            self.bridge_usb = _BridgePipeAdapter(ep2_sock=ep2_host, ep1_sock=ep1_host)
            return self
        except BaseException:
            self.__exit__(None, None, None)
            raise

    def __exit__(self, *_exc: Any) -> None:
        try:
            self.shutdown_display_and_bridge()
        finally:
            for proc in (self.fw, self.weston):
                self._stop_proc(proc)
            for fd in list(self._open_fds):
                self._close_fd(fd)
            for s in self._open_sockets:
                try:
                    s.close()
                except OSError:
                    pass
            self._open_sockets.clear()
            self._td.cleanup()

    def shutdown_display_and_bridge(self) -> tuple[int, int]:
        d_rc = self._stop_proc(self.display)
        b_rc = self._stop_proc(self.bridge)
        return d_rc, b_rc

    def _wait_for(self, pred: Any, desc: str, timeout_s: float = 5.0) -> None:
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            if pred():
                return
            time.sleep(0.02)
        raise AssertionError(f"Timed out waiting for {desc}")

    def _fw_ctrl(self, cmd: str) -> dict[str, Any]:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as s:
            s.settimeout(5.0)
            s.connect(self.fw_ctrl_sock)
            s.sendall(cmd.encode("utf-8"))
            raw = s.recv(4096).decode("utf-8").strip()
        return json.loads(raw)

    # --- High-level Product UX API ---

    def user_queries_versions(self) -> dict[str, str]:
        return {
            "firmware": self.fw_usb.transact(openflux_dev.FW_CMD_VERSION).rstrip(b"\x00").decode(),
            "bridge": self.bridge_usb.transact(openflux_dev.BRIDGE_CMD_BRIDGE_VERSION).rstrip(b"\x00").decode(),
            "display": self.bridge_usb.transact(openflux_dev.BRIDGE_CMD_DISPLAY_VERSION).rstrip(b"\x00").decode(),
            "os": self.bridge_usb.transact(openflux_dev.BRIDGE_CMD_OS_VERSION).rstrip(b"\x00").decode(),
        }

    def user_pushes_file(self, remote_path: str, data: bytes) -> None:
        self.bridge_usb.transact(openflux_dev.BRIDGE_CMD_PUT_FILE, openflux_dev.build_put_file_payload(remote_path, data))

    def user_pulls_file(self, remote_path: str) -> bytes:
        return self.bridge_usb.transact(openflux_dev.BRIDGE_CMD_GET_FILE, remote_path.encode("utf-8") + b"\x00")

    def user_lists_dir(self, remote_dir: str) -> str:
        return self.bridge_usb.transact(openflux_dev.BRIDGE_CMD_GET_FILE_LIST, remote_dir.encode("utf-8") + b"\x00").decode("utf-8", "replace")

    def user_deletes_file(self, remote_path: str) -> None:
        self.bridge_usb.transact(openflux_dev.BRIDGE_CMD_DELETE_FILE, remote_path.encode("utf-8") + b"\x00")

    def user_flashes_keymap(self, blob: FirmwareKeymapBlob) -> None:
        raw = blob.to_bytes()
        decode_keymap_blob(raw)
        self.fw_usb.transact(openflux_dev.FW_CMD_SEND_KEYMAP, raw)

    def user_dumps_keymap(self) -> FirmwareKeymapBlob:
        return decode_keymap_blob(self.fw_usb.transact(openflux_dev.FW_CMD_GET_KEYMAP))

    def user_uploads_profiles(self, settings: list[ProfileSetting]) -> None:
        xfer = ProfileSettingsFile(settings=settings).to_transfer_bytes()
        self.fw_usb.transact(openflux_dev.FW_CMD_FILE_FROM_HOST, xfer)

    def user_activates_profile(self, profile_idx: int) -> None:
        self.fw_usb.transact(openflux_dev.FW_CMD_SET_ACTIVE_PROFILE, struct.pack("<I", profile_idx))

    def user_sets_adc(self, **sensor_vals: int) -> dict[str, Any]:
        if not sensor_vals:
            return self._fw_ctrl("RESET_ADC")
        pairs = ",".join(f"{int(k[1:]) if k.startswith('s') else int(k)}={v}" for k, v in sensor_vals.items())
        return self._fw_ctrl(f"SCAN {pairs}")

    def user_power_cycles_firmware(self) -> dict[str, Any]:
        return self._fw_ctrl("REBOOT")

    def run_host_bridge_tick(self, profile_idx: int, language_code: int, platform_code: int, media_title: str, cpu_pct: float) -> None:
        tr = _E2EHostBridgeTransport(self.fw_usb, self.bridge_usb)
        tr.set_profile(profile_idx)
        tr.set_language(language_code)
        tr.set_platform(platform_code)
        tr.send_live_data(DataType.JSON, {
            "hardwareData": {"cpu": round(cpu_pct, 1), "memory": "8.0/32 GB", "netDown": "0 MB/s", "netUp": "0 MB/s"},
            "mediaFile": {"title": media_title, "artists": "Open Artist", "album": "LP", "playing": True, "length": 180, "played": 10},
        })

    def read_live_display_ppm(self) -> bytes:
        return self.live_ppm_path.read_bytes()

    def wait_for_live_display_change(self, prev_ppm: bytes, pred=None, timeout_s: float = 8.0) -> bytes:
        deadline = time.monotonic() + timeout_s
        last_cur = prev_ppm
        while time.monotonic() < deadline:
            try:
                cur = self.live_ppm_path.read_bytes()
                last_cur = cur
                if cur != prev_ppm and (pred is None or pred(cur)):
                    return cur
            except OSError:
                pass
            time.sleep(0.05)
        rc = self.display.poll() if self.display else None
        raise AssertionError(f"Timed out waiting for live running display frame update (display_rc={rc}, same={last_cur == prev_ppm})")
