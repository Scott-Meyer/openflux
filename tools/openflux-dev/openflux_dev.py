#!/usr/bin/env python3
"""openflux-dev: USB control & diagnostic client for the OpenFlux Keyboard (Bridge + RP2040 Firmware).

Communicates directly with:
  - Bridge   (Linux USB FunctionFS gadget): VID=0x3761, PID=0x0104, IF=0, EP_OUT=0x01, EP_IN=0x81, 512B packets
  - Firmware (RP2040 MCU vendor bulk):      VID=0x3761, PID=0x4015, IF=1, EP_OUT=0x01, EP_IN=0x84, 64B packets

The USB commands run on macOS and Linux: they serialize access to each device with a POSIX file lock.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import fcntl
import json
import math
import struct
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from keymap_codec import FirmwareKeymapBlob, decode_keymap_blob
from profile_settings import ProfileSetting, ProfileSettingsFile, decode_profile_settings
from simple_hdlc import DataType, encode_live_data, encode_live_json

VENDOR_ID = 0x3761
BRIDGE_PID = 0x0104
FW_PID = 0x4015

# Bridge USB transfer commands
BRIDGE_CMD_BRIDGE_VERSION = 1
BRIDGE_CMD_DISPLAY_VERSION = 2
BRIDGE_CMD_OS_VERSION = 3
BRIDGE_CMD_SET_SYSTEM_TIME = 4
BRIDGE_CMD_GET_FILE_LIST = 100
BRIDGE_CMD_GET_FILE = 101
BRIDGE_CMD_PUT_FILE = 102
BRIDGE_CMD_DELETE_FILE = 103
BRIDGE_CMD_GET_FREE_SPACE = 104
BRIDGE_CMD_SEND_LIVE_DATA = 200
BRIDGE_CMD_GET_MEDIA_STATUS = 201
BRIDGE_CMD_GET_SHORTCUT_CONTEXT = 202

# RP2040 firmware USB transfer commands
FW_CMD_VERSION = 1
FW_CMD_SEND_KEYMAP = 100
FW_CMD_MCU_BOOTLOADER = 101
FW_CMD_SOC_BOOTLOADER = 102
FW_CMD_GET_KEYMAP = 103
FW_CMD_SET_LANGUAGE = 104
FW_CMD_SET_PLATFORM = 105
FW_CMD_SET_ACTIVE_PROFILE = 106
FW_CMD_FILE_FROM_HOST = 107
FW_CMD_FILE_TO_HOST = 108

RESP_NAK = 0
RESP_ACK = 1


class NakError(RuntimeError):
    """Raised when the device explicitly returns RESP_NAK after draining its response."""


class LibUsbDeviceDescriptor(ctypes.Structure):
    _fields_ = [
        ("bLength", ctypes.c_uint8),
        ("bDescriptorType", ctypes.c_uint8),
        ("bcdUSB", ctypes.c_uint16),
        ("bDeviceClass", ctypes.c_uint8),
        ("bDeviceSubClass", ctypes.c_uint8),
        ("bDeviceProtocol", ctypes.c_uint8),
        ("bMaxPacketSize0", ctypes.c_uint8),
        ("idVendor", ctypes.c_uint16),
        ("idProduct", ctypes.c_uint16),
        ("bcdDevice", ctypes.c_uint16),
        ("iManufacturer", ctypes.c_uint8),
        ("iProduct", ctypes.c_uint8),
        ("iSerialNumber", ctypes.c_uint8),
        ("bNumConfigurations", ctypes.c_uint8),
    ]


def _load_libusb() -> ctypes.CDLL:
    candidates = [
        "/opt/homebrew/opt/libusb/lib/libusb-1.0.dylib",
        "/usr/local/opt/libusb/lib/libusb-1.0.dylib",
        ctypes.util.find_library("usb-1.0"),
    ]
    for c in candidates:
        if c and Path(c).exists():
            lib = ctypes.CDLL(c)
            break
    else:
        raise RuntimeError("Could not locate libusb-1.0 dynamic library")

    lib.libusb_init.argtypes = [ctypes.POINTER(ctypes.c_void_p)]
    lib.libusb_init.restype = ctypes.c_int
    lib.libusb_exit.argtypes = [ctypes.c_void_p]
    lib.libusb_get_device_list.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.POINTER(ctypes.c_void_p))]
    lib.libusb_get_device_list.restype = ctypes.c_ssize_t
    lib.libusb_free_device_list.argtypes = [ctypes.POINTER(ctypes.c_void_p), ctypes.c_int]
    lib.libusb_get_device_descriptor.argtypes = [ctypes.c_void_p, ctypes.POINTER(LibUsbDeviceDescriptor)]
    lib.libusb_get_device_descriptor.restype = ctypes.c_int
    lib.libusb_open.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_void_p)]
    lib.libusb_open.restype = ctypes.c_int
    lib.libusb_close.argtypes = [ctypes.c_void_p]
    lib.libusb_claim_interface.argtypes = [ctypes.c_void_p, ctypes.c_int]
    lib.libusb_claim_interface.restype = ctypes.c_int
    lib.libusb_release_interface.argtypes = [ctypes.c_void_p, ctypes.c_int]
    lib.libusb_release_interface.restype = ctypes.c_int
    lib.libusb_bulk_transfer.argtypes = [
        ctypes.c_void_p,
        ctypes.c_uint8,
        ctypes.POINTER(ctypes.c_uint8),
        ctypes.c_int,
        ctypes.POINTER(ctypes.c_int),
        ctypes.c_uint,
    ]
    lib.libusb_bulk_transfer.restype = ctypes.c_int
    lib.libusb_strerror.argtypes = [ctypes.c_int]
    lib.libusb_strerror.restype = ctypes.c_char_p
    return lib


@dataclass
class UsbTarget:
    pid: int
    interface: int
    ep_out: int
    ep_in: int
    packet_size: int


BRIDGE_TARGET = UsbTarget(pid=BRIDGE_PID, interface=0, ep_out=0x01, ep_in=0x81, packet_size=512)
FW_TARGET = UsbTarget(pid=FW_PID, interface=1, ep_out=0x01, ep_in=0x84, packet_size=64)


class OpenFluxUsbClient:
    def __init__(self) -> None:
        self.lib = _load_libusb()
        self.ctx = ctypes.c_void_p()
        rc = self.lib.libusb_init(ctypes.byref(self.ctx))
        if rc != 0:
            raise RuntimeError(f"libusb_init failed: {rc}")

    def close(self) -> None:
        if self.ctx:
            self.lib.libusb_exit(self.ctx)
            self.ctx = ctypes.c_void_p()

    def _err(self, rc: int) -> str:
        msg = self.lib.libusb_strerror(rc)
        return f"{rc} ({msg.decode('utf-8', 'replace') if msg else 'unknown'})"

    def _bulk_write(self, handle: ctypes.c_void_p, ep: int, data: bytes, timeout_ms: int = 3000) -> None:
        buf = (ctypes.c_uint8 * len(data))(*data) if data else (ctypes.c_uint8 * 1)()
        transferred = ctypes.c_int(0)
        rc = self.lib.libusb_bulk_transfer(handle, ep, buf, len(data), ctypes.byref(transferred), timeout_ms)
        if rc != 0:
            raise RuntimeError(f"bulk_write(ep=0x{ep:02x}, len={len(data)}) failed: {self._err(rc)}")
        if transferred.value != len(data):
            raise RuntimeError(f"short bulk_write: {transferred.value} != {len(data)}")

    def _bulk_read(self, handle: ctypes.c_void_p, ep: int, length: int, timeout_ms: int = 3000) -> bytes:
        buf = (ctypes.c_uint8 * max(length, 1))()
        transferred = ctypes.c_int(0)
        rc = self.lib.libusb_bulk_transfer(handle, ep, buf, length, ctypes.byref(transferred), timeout_ms)
        if rc != 0:
            raise RuntimeError(f"bulk_read(ep=0x{ep:02x}, len={length}) failed: {self._err(rc)}")
        return bytes(buf[: transferred.value])

    def transact_io(
        self,
        write_fn: Callable[[int, bytes, int], None],
        read_fn: Callable[[int, int, int], bytes],
        target: UsbTarget,
        cmd: int,
        payload: bytes = b"",
        timeout_ms: int = 5000,
    ) -> bytes:
        pkt_sz = target.packet_size
        num_packets = math.ceil(len(payload) / pkt_sz) if payload else 0
        hdr = bytearray(pkt_sz)
        struct.pack_into("<BII", hdr, 0, cmd, num_packets, len(payload))
        write_fn(target.ep_out, bytes(hdr), timeout_ms)
        if payload:
            for off in range(0, len(payload), pkt_sz):
                chunk = payload[off : off + pkt_sz]
                if len(chunk) < pkt_sz:
                    chunk = chunk + b"\x00" * (pkt_sz - len(chunk))
                write_fn(target.ep_out, chunk, timeout_ms)
        # Terminate exact-packet-multiple bulk OUT transfer with a ZLP
        write_fn(target.ep_out, b"", timeout_ms)

        resp_hdr = read_fn(target.ep_in, pkt_sz, timeout_ms)
        if len(resp_hdr) < 9:
            raise RuntimeError(f"Short response header: {len(resp_hdr)} bytes")
        resp_type, resp_packets, resp_bytes = struct.unpack_from("<BII", resp_hdr, 0)
        if resp_type != RESP_ACK:
            # Drain any trailing payload/ZLP packets from the NAK response so the next transaction stays synchronized.
            if resp_bytes == 0:
                try:
                    read_fn(target.ep_in, pkt_sz, 1000)
                except RuntimeError:
                    pass
            elif resp_packets > 0:
                rem = resp_packets * pkt_sz
                while rem > 0:
                    try:
                        chunk = read_fn(target.ep_in, min(rem, 16384), 1000)
                    except RuntimeError:
                        break
                    if not chunk:
                        break
                    rem -= len(chunk)
                try:
                    read_fn(target.ep_in, pkt_sz, 200)
                except RuntimeError:
                    pass
            raise NakError(f"Device returned NAK (type={resp_type}, packets={resp_packets}, bytes={resp_bytes})")

        if resp_bytes == 0:
            if resp_packets not in (0, 1):
                raise RuntimeError(f"Invalid response header: bytes=0 with packets={resp_packets}")
            try:
                d = read_fn(target.ep_in, pkt_sz, 1000)
                if d and resp_packets == 1:
                    try:
                        read_fn(target.ep_in, pkt_sz, 1000)
                    except RuntimeError:
                        pass
            except RuntimeError:
                pass
            return b""

        if resp_packets == 0 or resp_bytes > resp_packets * pkt_sz:
            # Drain any queued packets + ZLP on a malformed header so subsequent USB queries stay synchronized.
            if resp_packets > 0:
                rem = resp_packets * pkt_sz
                while rem > 0:
                    try:
                        chunk = read_fn(target.ep_in, min(rem, 16384), 200)
                    except RuntimeError:
                        break
                    if not chunk:
                        break
                    rem -= len(chunk)
                try:
                    read_fn(target.ep_in, pkt_sz, 200)
                except RuntimeError:
                    pass
            raise RuntimeError(
                f"Invalid response header: bytes={resp_bytes} exceeds packets={resp_packets} * {pkt_sz}"
            )

        total_read = resp_packets * pkt_sz
        chunks = []
        remaining = total_read
        total_received = 0
        zlp_consumed = False
        while remaining > 0:
            req_len = min(remaining, 16384)
            d = read_fn(target.ep_in, req_len, timeout_ms)
            if not d:
                if total_received >= resp_bytes:
                    zlp_consumed = True
                    break
                raise RuntimeError(
                    f"Premature zero-length packet while reading response: "
                    f"received {total_read - remaining} of {total_read} bytes (advertised payload={resp_bytes})"
                )
            chunks.append(d)
            total_received += len(d)
            remaining -= len(d)
            if len(d) < req_len and total_received >= resp_bytes:
                zlp_consumed = True
                break
        raw = b"".join(chunks)
        if len(raw) < resp_bytes:
            raise RuntimeError(f"Truncated response payload: received {len(raw)} bytes, expected {resp_bytes}")
        if not zlp_consumed:
            try:
                read_fn(target.ep_in, pkt_sz, 1000)
            except RuntimeError:
                pass
        return raw[:resp_bytes]

    def transact(self, target: UsbTarget, cmd: int, payload: bytes = b"", timeout_ms: int = 5000) -> bytes:
        lock_path = Path(f"/tmp/openflux_usb_{target.pid:04x}.lock")
        with open(lock_path, "a+") as lock_fp:
            fcntl.flock(lock_fp.fileno(), fcntl.LOCK_EX)
            try:
                return self._transact_locked(target, cmd, payload, timeout_ms)
            finally:
                fcntl.flock(lock_fp.fileno(), fcntl.LOCK_UN)

    def _transact_locked(self, target: UsbTarget, cmd: int, payload: bytes = b"", timeout_ms: int = 5000) -> bytes:
        dev_list = ctypes.POINTER(ctypes.c_void_p)()
        count = self.lib.libusb_get_device_list(self.ctx, ctypes.byref(dev_list))
        if count < 0:
            raise RuntimeError(f"libusb_get_device_list failed: {self._err(count)}")

        handle = ctypes.c_void_p()
        try:
            for i in range(count):
                dev = dev_list[i]
                desc = LibUsbDeviceDescriptor()
                if self.lib.libusb_get_device_descriptor(dev, ctypes.byref(desc)) == 0:
                    if desc.idVendor == VENDOR_ID and desc.idProduct == target.pid:
                        rc = self.lib.libusb_open(dev, ctypes.byref(handle))
                        if rc != 0:
                            raise RuntimeError(f"libusb_open(0x{target.pid:04x}) failed: {self._err(rc)}")
                        break
        finally:
            self.lib.libusb_free_device_list(dev_list, 1)

        if not handle:
            raise RuntimeError(f"Device 0x{VENDOR_ID:04x}:0x{target.pid:04x} not found")

        rc = self.lib.libusb_claim_interface(handle, target.interface)
        if rc != 0:
            self.lib.libusb_close(handle)
            raise RuntimeError(f"claim_interface({target.interface}) failed: {self._err(rc)}")

        try:
            return self.transact_io(
                lambda ep, data, to: self._bulk_write(handle, ep, data, to),
                lambda ep, ln, to: self._bulk_read(handle, ep, ln, to),
                target,
                cmd,
                payload,
                timeout_ms,
            )
        finally:
            self.lib.libusb_release_interface(handle, target.interface)
            self.lib.libusb_close(handle)


def build_put_file_payload(remote_path: str, data: bytes) -> bytes:
    if "\x00" in remote_path:
        raise ValueError("remote_path must not contain NUL")
    return remote_path.encode("utf-8") + b"\x00" + data


def build_set_time_payload(timestamp: str, timezone_name: str) -> bytes:
    return f"{timestamp} {timezone_name}".encode("utf-8") + b"\x00"


def main() -> None:
    parser = argparse.ArgumentParser(prog="openflux-dev", description="OpenFlux Keyboard USB control & diagnostic tool")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("versions", help="Query firmware, bridge, display, and OS versions")
    sub.add_parser("df", help="Query free disk space on the keyboard SoC")
    p_ls = sub.add_parser("ls", help="List files in a directory on the keyboard")
    p_ls.add_argument("path", default="/home/root/openflux", nargs="?")
    p_pull = sub.add_parser("pull", help="Read a file from the keyboard to stdout or -o file")
    p_pull.add_argument("remote_path")
    p_pull.add_argument("-o", "--output", help="Write to local file instead of stdout")
    p_push = sub.add_parser("push", help="Upload a local file to keyboard sandbox (/home/root/...)")
    p_push.add_argument("local_file")
    p_push.add_argument("remote_path")
    p_rm = sub.add_parser("rm", help="Delete a file inside keyboard sandbox (/home/root/...)")
    p_rm.add_argument("remote_path")
    p_time = sub.add_parser("set-time", help="Set keyboard system clock (UTC) and timezone")
    p_time.add_argument("datetime_str", help="UTC timestamp 'YYYY-MM-DD HH:MM:SS'")
    p_time.add_argument("timezone_name", help="IANA timezone, e.g. America/Los_Angeles")
    sub.add_parser("media", help="Query bridge media status")
    sub.add_parser("shortcut", help="Query bridge shortcut context")
    p_km = sub.add_parser("keymap-dump", help="Dump firmware binary keymap from device")
    p_km.add_argument("-o", "--output", help="Save binary keymap to local file")
    p_send_km = sub.add_parser("send-keymap", help="Validate and flash a binary keymap blob to firmware")
    p_send_km.add_argument("file")
    p_dk = sub.add_parser("decode-keymap", help="Decode a dumped binary keymap file to JSON")
    p_dk.add_argument("file")
    p_ek = sub.add_parser("encode-keymap", help="Encode and validate a JSON keymap spec to binary blob")
    p_ek.add_argument("json_file")
    p_ek.add_argument("-o", "--output", required=True, help="Output binary keymap file")
    p_prof = sub.add_parser("set-profile", help="Set active profile index on firmware (uint32)")
    p_prof.add_argument("profile_index", type=int)
    p_lang = sub.add_parser("set-language", help="Set language code on firmware (uint32)")
    p_lang.add_argument("language_code", type=int)
    p_plat = sub.add_parser("set-platform", help="Set platform type on firmware (uint32)")
    p_plat.add_argument("platform_type", type=int)
    p_gps = sub.add_parser("get-profile-settings", help="Read PROFILE_SETTINGS_V1 from firmware (0x6c)")
    p_gps.add_argument("-o", "--output", help="Save raw bytes instead of JSON")
    p_sps = sub.add_parser("send-profile-settings", help="Send PROFILE_SETTINGS_V1 JSON spec to firmware (0x6b)")
    p_sps.add_argument(
        "json_file",
        help='JSON object {"settings": [{"actuation_mode_index": N, "default_layer_bitfield": N}, ...]}',
    )
    p_dps = sub.add_parser("decode-profile-settings", help="Decode PROFILE_SETTINGS_V1 binary file")
    p_dps.add_argument("file")
    p_dps.add_argument("--transfer", action="store_true", help="Expect 16-byte FileHeader prefix")
    p_live = sub.add_parser("send-live-data", help="Send a SimpleHDLC live-data JSON message or heartbeat via CMD 200")
    p_live.add_argument("--heartbeat", action="store_true", help="Send DataType(4) heartbeat")
    p_live.add_argument("--json", dest="json_str", help="JSON object payload for DataType(0)")
    p_bk = sub.add_parser("backup", help="Back up device versions, config files, and live firmware keymap")
    p_bk.add_argument("out_dir")
    p_rpbk = sub.add_parser("rp2040-backup", help="Reboot RP2040 to PICOBOOT, dump 16MiB flash + LittleFS calibration, and reboot back")
    p_rpbk.add_argument("out_dir")
    p_rpf = sub.add_parser("rp2040-flash", help="Flash an RP2040 .uf2 via PICOBOOT while preserving 0x10F00000 calibration")
    p_rpf.add_argument("uf2_file")
    args = parser.parse_args()

    if args.command == "rp2040-backup":
        from rp2040_flash_backup import backup_live_rp2040_flash

        out = backup_live_rp2040_flash(Path(args.out_dir))
        print(f"RP2040 16MiB flash + LittleFS calibration backup complete -> {out}")
        return

    if args.command == "rp2040-flash":
        from rp2040_flash_backup import flash_live_rp2040_uf2

        res = flash_live_rp2040_uf2(Path(args.uf2_file))
        print(f"Flashed and verified RP2040 UF2 via PICOBOOT -> {json.dumps(res)}")
        return

    if args.command == "decode-keymap":
        blob = decode_keymap_blob(Path(args.file).read_bytes())
        print(json.dumps(blob.to_dict(), indent=2))
        return
    if args.command == "encode-keymap":
        raw_blob = FirmwareKeymapBlob.from_dict(json.loads(Path(args.json_file).read_text())).to_bytes()
        decode_keymap_blob(raw_blob)
        Path(args.output).write_bytes(raw_blob)
        return
    if args.command == "decode-profile-settings":
        ps = decode_profile_settings(Path(args.file).read_bytes(), has_file_header=args.transfer)
        print(json.dumps(ps.to_dict(), indent=2))
        return

    validated_keymap_raw: bytes | None = None
    if args.command == "send-keymap":
        validated_keymap_raw = Path(args.file).read_bytes()
        decode_keymap_blob(validated_keymap_raw)

    validated_ps: ProfileSettingsFile | None = None
    if args.command == "send-profile-settings":
        spec = json.loads(Path(args.json_file).read_text())
        items = [
            ProfileSetting(
                actuation_mode_index=int(item["actuation_mode_index"]),
                default_layer_bitfield=int(item["default_layer_bitfield"]),
            )
            for item in spec["settings"]
        ]
        validated_ps = ProfileSettingsFile(settings=items)
        validated_ps.to_transfer_bytes()

    validated_live_frames: list[bytes] | None = None
    if args.command == "send-live-data":
        if args.heartbeat:
            validated_live_frames = encode_live_data(DataType.HEARTBEAT)
        elif args.json_str:
            validated_live_frames = encode_live_json(json.loads(args.json_str))
        else:
            raise SystemExit("Specify --heartbeat or --json '{...}'")

    client = OpenFluxUsbClient()
    try:
        if args.command == "versions":
            firmware = client.transact(FW_TARGET, FW_CMD_VERSION).rstrip(b"\x00").decode("utf-8", "replace")
            bridge = client.transact(BRIDGE_TARGET, BRIDGE_CMD_BRIDGE_VERSION).rstrip(b"\x00").decode("utf-8", "replace")
            display = client.transact(BRIDGE_TARGET, BRIDGE_CMD_DISPLAY_VERSION).rstrip(b"\x00").decode("utf-8", "replace")
            os_ver = client.transact(BRIDGE_TARGET, BRIDGE_CMD_OS_VERSION).rstrip(b"\x00").decode("utf-8", "replace")
            print(f"firmware: {firmware}")
            print(f"bridge:   {bridge}")
            print(f"display:  {display}")
            print(f"os:       {os_ver}")
        elif args.command == "df":
            raw = client.transact(BRIDGE_TARGET, BRIDGE_CMD_GET_FREE_SPACE).rstrip(b"\x00").decode("utf-8", "replace")
            print(raw)
        elif args.command == "ls":
            raw = client.transact(BRIDGE_TARGET, BRIDGE_CMD_GET_FILE_LIST, args.path.encode("utf-8") + b"\x00")
            text = raw.rstrip(b"\x00").decode("utf-8", "replace")
            for line in text.replace("\x00", "\n").splitlines():
                if line:
                    print(line)
        elif args.command == "pull":
            raw = client.transact(BRIDGE_TARGET, BRIDGE_CMD_GET_FILE, args.remote_path.encode("utf-8") + b"\x00")
            if args.output:
                Path(args.output).write_bytes(raw)
                print(f"Saved {len(raw)} bytes -> {args.output}")
            else:
                sys.stdout.buffer.write(raw)
        elif args.command == "push":
            data = Path(args.local_file).read_bytes()
            client.transact(BRIDGE_TARGET, BRIDGE_CMD_PUT_FILE, build_put_file_payload(args.remote_path, data))
            print(f"Uploaded {len(data)} bytes -> {args.remote_path}")
        elif args.command == "rm":
            client.transact(BRIDGE_TARGET, BRIDGE_CMD_DELETE_FILE, args.remote_path.encode("utf-8") + b"\x00")
            print(f"Deleted {args.remote_path}")
        elif args.command == "set-time":
            client.transact(
                BRIDGE_TARGET,
                BRIDGE_CMD_SET_SYSTEM_TIME,
                build_set_time_payload(args.datetime_str, args.timezone_name),
            )
            print(f"Set system time -> {args.datetime_str} {args.timezone_name}")
        elif args.command == "media":
            raw = client.transact(BRIDGE_TARGET, BRIDGE_CMD_GET_MEDIA_STATUS).rstrip(b"\x00").decode("utf-8", "replace")
            print(raw)
        elif args.command == "shortcut":
            raw = client.transact(BRIDGE_TARGET, BRIDGE_CMD_GET_SHORTCUT_CONTEXT).rstrip(b"\x00").decode("utf-8", "replace")
            print(raw)
        elif args.command == "keymap-dump":
            raw = client.transact(FW_TARGET, FW_CMD_GET_KEYMAP)
            if args.output:
                Path(args.output).write_bytes(raw)
                print(f"Saved {len(raw)} bytes -> {args.output}")
            else:
                print(f"Received {len(raw)} bytes; head={raw[:32].hex()}")
        elif args.command == "send-keymap":
            assert validated_keymap_raw is not None
            client.transact(FW_TARGET, FW_CMD_SEND_KEYMAP, validated_keymap_raw)
            print(f"Sent validated keymap ({len(validated_keymap_raw)} bytes) to firmware")
        elif args.command == "set-profile":
            client.transact(FW_TARGET, FW_CMD_SET_ACTIVE_PROFILE, struct.pack("<I", args.profile_index))
            print(f"Set active profile -> {args.profile_index}")
        elif args.command == "set-language":
            client.transact(FW_TARGET, FW_CMD_SET_LANGUAGE, struct.pack("<I", args.language_code))
            print(f"Set language -> {args.language_code}")
        elif args.command == "set-platform":
            client.transact(FW_TARGET, FW_CMD_SET_PLATFORM, struct.pack("<I", args.platform_type))
            print(f"Set platform -> {args.platform_type}")
        elif args.command == "get-profile-settings":
            try:
                raw = client.transact(FW_TARGET, FW_CMD_FILE_TO_HOST, struct.pack("<I", 0))
            except NakError:
                raw = client.transact(FW_TARGET, FW_CMD_FILE_TO_HOST)
            if args.output:
                Path(args.output).write_bytes(raw)
                print(f"Saved {len(raw)} bytes -> {args.output}")
            else:
                data = raw
                if len(raw) >= 32:
                    w0, w1 = struct.unpack_from("<2I", raw, 0)
                    if w0 == 0 and w1 == len(raw) - 16:
                        data = raw[16:]
                print(json.dumps(decode_profile_settings(data).to_dict(), indent=2))
        elif args.command == "send-profile-settings":
            assert validated_ps is not None
            payload = validated_ps.to_transfer_bytes()
            client.transact(FW_TARGET, FW_CMD_FILE_FROM_HOST, payload)
            print(f"Sent PROFILE_SETTINGS_V1 ({len(validated_ps.settings)} profiles, {len(payload)} bytes)")
        elif args.command == "send-live-data":
            assert validated_live_frames is not None
            for frame in validated_live_frames:
                client.transact(BRIDGE_TARGET, BRIDGE_CMD_SEND_LIVE_DATA, frame)
            print(f"Sent {len(validated_live_frames)} SimpleHDLC frame(s) via CMD 200")
        elif args.command == "backup":
            out = Path(args.out_dir)
            out.mkdir(parents=True, exist_ok=True)
            versions = {
                "firmware": client.transact(FW_TARGET, FW_CMD_VERSION).rstrip(b"\x00").decode("utf-8", "replace"),
                "bridge": client.transact(BRIDGE_TARGET, BRIDGE_CMD_BRIDGE_VERSION).rstrip(b"\x00").decode("utf-8", "replace"),
                "display": client.transact(BRIDGE_TARGET, BRIDGE_CMD_DISPLAY_VERSION).rstrip(b"\x00").decode("utf-8", "replace"),
                "os": client.transact(BRIDGE_TARGET, BRIDGE_CMD_OS_VERSION).rstrip(b"\x00").decode("utf-8", "replace"),
            }
            (out / "versions.json").write_text(json.dumps(versions, indent=2) + "\n")
            km = client.transact(FW_TARGET, FW_CMD_GET_KEYMAP)
            (out / "firmware_keymap.bin").write_bytes(km)
            (out / "firmware_keymap.json").write_text(json.dumps(decode_keymap_blob(km).to_dict(), indent=2) + "\n")
            cfg_dir = out / "config"
            cfg_dir.mkdir(exist_ok=True)
            listing = client.transact(BRIDGE_TARGET, BRIDGE_CMD_GET_FILE_LIST, b"/home/root/openflux/config\x00")
            for entry in listing.rstrip(b"\x00").decode("utf-8", "replace").replace("\x00", "\n").splitlines():
                if ":" not in entry:
                    continue
                _, fname = entry.split(":", 1)
                data = client.transact(BRIDGE_TARGET, BRIDGE_CMD_GET_FILE, f"/home/root/openflux/config/{fname}".encode("utf-8") + b"\x00")
                (cfg_dir / fname).write_bytes(data)
            print(f"Backup complete -> {out}")
    finally:
        client.close()


if __name__ == "__main__":
    main()
