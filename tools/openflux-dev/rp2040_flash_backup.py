#!/usr/bin/env python3
"""Back up and flash the RP2040 over PICOBOOT without touching the keyboard's factory calibration."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import struct
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

RP2040_PICOBOOT_VID = 0x2E8A
RP2040_PICOBOOT_PID = 0x0003
RP2040_XIP_BASE = 0x10000000
RP2040_FLASH_SIZE = 16 * 1024 * 1024  # 16 MiB

FW_LFS_OFFSET = 0x00F00000
FW_LFS_BLOCK_SIZE = 4096
FW_LFS_BLOCK_COUNT = 256
FW_LFS_SIZE = FW_LFS_BLOCK_SIZE * FW_LFS_BLOCK_COUNT

PICOBOOT_MAGIC = 0x431FD10B
PICOBOOT_IF_RESET = 0x41
PICOBOOT_IF_CMD_STATUS = 0x42

PC_EXCLUSIVE_ACCESS = 0x01
PC_REBOOT = 0x02
PC_FLASH_ERASE = 0x03
PC_READ = 0x84
PC_WRITE = 0x05
PC_EXIT_XIP = 0x06

# PC_EXCLUSIVE_ACCESS modes.
PICOBOOT_EXCLUSIVE = 1
PICOBOOT_EXCLUSIVE_AND_EJECT = 2

FACTORY_CALIB_MAGIC = 0xFAC5CA1E

_LFS_TOOL_BIN: Path | None = None


def build_picoboot_cmd(token: int, cmd_id: int, cmd_size: int, xfer_len: int, args: bytes = b"") -> bytes:
    """Packs a 32-byte pico-sdk struct picoboot_cmd (<IIBBHI16s)."""
    if len(args) > 16:
        raise ValueError(f"PICOBOOT args exceed 16 bytes: {len(args)}")
    return struct.pack(
        "<IIBBHI16s",
        PICOBOOT_MAGIC,
        token & 0xFFFFFFFF,
        cmd_id & 0xFF,
        cmd_size & 0xFF,
        0,
        xfer_len & 0xFFFFFFFF,
        args.ljust(16, b"\x00"),
    )


@dataclass(frozen=True)
class CalibrationInfo:
    magic: int
    version: int
    num_sensors: int
    reserved: int
    checksum: int
    raw_factors: list[int]
    effective_factors_q8: list[int]


def _ensure_lfs_tool() -> Path:
    global _LFS_TOOL_BIN
    if _LFS_TOOL_BIN is not None and _LFS_TOOL_BIN.exists():
        return _LFS_TOOL_BIN

    bin_name = "lfs_tool.exe" if os.name == "nt" else "lfs_tool"
    bundled = Path(__file__).resolve().parent / bin_name
    if bundled.exists() and (os.name == "nt" or os.access(bundled, os.X_OK)):
        _LFS_TOOL_BIN = bundled
        return bundled

    cargo_dir = Path(__file__).resolve().parent / "lfs-tool"
    manifest = cargo_dir / "Cargo.toml"
    if not manifest.exists():
        raise RuntimeError(f"Missing Rust lfs-tool crate at {cargo_dir}")

    target_dir = Path(os.environ.get("CARGO_TARGET_DIR") or (cargo_dir / "target"))
    bin_path = target_dir / "release" / bin_name
    need_build = not bin_path.exists()
    if not need_build:
        mtime_bin = bin_path.stat().st_mtime
        for p in (manifest, cargo_dir / "src" / "main.rs"):
            if p.stat().st_mtime > mtime_bin:
                need_build = True
                break
    if need_build:
        cargo = shutil.which("cargo")
        if not cargo:
            raise RuntimeError("No cargo compiler found to build Rust lfs_tool")
        subprocess.run(
            [cargo, "build", "--release", "--manifest-path", str(manifest), "--target-dir", str(target_dir)],
            check=True,
        )
    if not bin_path.is_file():
        raise RuntimeError(f"Expected built lfs_tool binary at {bin_path}")
    _LFS_TOOL_BIN = bin_path
    return bin_path


def extract_littlefs_files(lfs_image: bytes) -> dict[str, bytes]:
    """Extracts the root files from a 1 MiB LittleFS v2 image using lfs-tool."""
    if len(lfs_image) != FW_LFS_SIZE:
        raise ValueError(f"Expected {FW_LFS_SIZE} bytes LittleFS image, got {len(lfs_image)}")
    tool = _ensure_lfs_tool()
    with tempfile.TemporaryDirectory(prefix="flux_lfs_extract_") as td:
        td_path = Path(td)
        img_path = td_path / "lfs.bin"
        out_dir = td_path / "out"
        img_path.write_bytes(lfs_image)
        subprocess.run([str(tool), "extract", str(img_path), str(out_dir)], check=True, capture_output=True)
        files: dict[str, bytes] = {}
        for p in sorted(out_dir.iterdir()):
            if p.is_file():
                files[p.name] = p.read_bytes()
        return files


def create_littlefs_image(files: dict[str, bytes]) -> bytes:
    """Builds a 1 MiB LittleFS v2 image from files using lfs-tool (used by tests)."""
    tool = _ensure_lfs_tool()
    with tempfile.TemporaryDirectory(prefix="flux_lfs_create_") as td:
        td_path = Path(td)
        in_dir = td_path / "in"
        in_dir.mkdir(parents=True, exist_ok=True)
        for name, data in files.items():
            (in_dir / name).write_bytes(data)
        img_path = td_path / "lfs.bin"
        subprocess.run([str(tool), "create", str(img_path), str(in_dir)], check=True, capture_output=True)
        return img_path.read_bytes()


def parse_factory_calibration(info_bytes: bytes, factors_bytes: bytes) -> CalibrationInfo:
    if len(info_bytes) != 20:
        raise ValueError(f"factory_calibration_info must be 20 bytes, got {len(info_bytes)}")
    magic, version, num_sensors, reserved, checksum = struct.unpack("<5I", info_bytes)
    if magic != FACTORY_CALIB_MAGIC:
        raise ValueError(f"Invalid calibration magic 0x{magic:08x}")
    if len(factors_bytes) != num_sensors * 2:
        raise ValueError(f"Expected {num_sensors * 2} factor bytes, got {len(factors_bytes)}")
    raw_factors = list(struct.unpack(f"<{num_sensors}H", factors_bytes))
    actual_sum = sum(raw_factors)
    if actual_sum != checksum:
        raise ValueError(f"Calibration checksum mismatch: expected 0x{checksum:08x}, got 0x{actual_sum:08x}")
    if version == 2:
        effective = [(f * 0xAA) >> 8 for f in raw_factors]
    else:
        effective = list(raw_factors)
    return CalibrationInfo(
        magic=magic,
        version=version,
        num_sensors=num_sensors,
        reserved=reserved,
        checksum=checksum,
        raw_factors=raw_factors,
        effective_factors_q8=effective,
    )


class PicobootClient:
    """USB PICOBOOT ROM protocol client matching pico-sdk boot/picoboot.h (pure ctypes libusb-1.0)."""

    def __init__(self, timeout_s: float = 10.0) -> None:
        import ctypes
        from openflux_dev import LibUsbDeviceDescriptor, _load_libusb

        class _EpDesc(ctypes.Structure):
            _fields_ = [
                ("bLength", ctypes.c_uint8), ("bDescriptorType", ctypes.c_uint8),
                ("bEndpointAddress", ctypes.c_uint8), ("bmAttributes", ctypes.c_uint8),
                ("wMaxPacketSize", ctypes.c_uint16), ("bInterval", ctypes.c_uint8),
                ("bRefresh", ctypes.c_uint8), ("bSynchAddress", ctypes.c_uint8),
                ("extra", ctypes.c_void_p), ("extra_length", ctypes.c_int),
            ]

        class _IntfDesc(ctypes.Structure):
            _fields_ = [
                ("bLength", ctypes.c_uint8), ("bDescriptorType", ctypes.c_uint8),
                ("bInterfaceNumber", ctypes.c_uint8), ("bAlternateSetting", ctypes.c_uint8),
                ("bNumEndpoints", ctypes.c_uint8), ("bInterfaceClass", ctypes.c_uint8),
                ("bInterfaceSubClass", ctypes.c_uint8), ("bInterfaceProtocol", ctypes.c_uint8),
                ("iInterface", ctypes.c_uint8), ("endpoint", ctypes.POINTER(_EpDesc)),
                ("extra", ctypes.c_void_p), ("extra_length", ctypes.c_int),
            ]

        class _Intf(ctypes.Structure):
            _fields_ = [("altsetting", ctypes.POINTER(_IntfDesc)), ("num_altsetting", ctypes.c_int)]

        class _CfgDesc(ctypes.Structure):
            _fields_ = [
                ("bLength", ctypes.c_uint8), ("bDescriptorType", ctypes.c_uint8),
                ("wTotalLength", ctypes.c_uint16), ("bNumInterfaces", ctypes.c_uint8),
                ("bConfigurationValue", ctypes.c_uint8), ("iConfiguration", ctypes.c_uint8),
                ("bmAttributes", ctypes.c_uint8), ("MaxPower", ctypes.c_uint8),
                ("interface", ctypes.POINTER(_Intf)), ("extra", ctypes.c_void_p), ("extra_length", ctypes.c_int),
            ]

        lib = _load_libusb()
        lib.libusb_get_active_config_descriptor.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.POINTER(_CfgDesc))]
        lib.libusb_get_active_config_descriptor.restype = ctypes.c_int
        lib.libusb_free_config_descriptor.argtypes = [ctypes.POINTER(_CfgDesc)]
        lib.libusb_clear_halt.argtypes = [ctypes.c_void_p, ctypes.c_uint8]
        lib.libusb_clear_halt.restype = ctypes.c_int
        lib.libusb_control_transfer.argtypes = [
            ctypes.c_void_p, ctypes.c_uint8, ctypes.c_uint8, ctypes.c_uint16,
            ctypes.c_uint16, ctypes.POINTER(ctypes.c_uint8), ctypes.c_uint16, ctypes.c_uint,
        ]
        lib.libusb_control_transfer.restype = ctypes.c_int

        ctx = ctypes.c_void_p()
        if lib.libusb_init(ctypes.byref(ctx)) != 0:
            raise RuntimeError("libusb_init failed")

        self._ctypes = ctypes
        self._lib = lib
        self._ctx = ctx
        self._handle = ctypes.c_void_p()
        self.intf_num = -1
        self.out_ep = 0
        self.in_ep = 0

        deadline = time.monotonic() + timeout_s
        found_dev = None
        while time.monotonic() < deadline and not self._handle:
            dev_list = ctypes.POINTER(ctypes.c_void_p)()
            cnt = lib.libusb_get_device_list(ctx, ctypes.byref(dev_list))
            try:
                for i in range(max(0, cnt)):
                    desc = LibUsbDeviceDescriptor()
                    if lib.libusb_get_device_descriptor(dev_list[i], ctypes.byref(desc)) == 0:
                        if desc.idVendor == RP2040_PICOBOOT_VID and desc.idProduct == RP2040_PICOBOOT_PID:
                            cfg = ctypes.POINTER(_CfgDesc)()
                            if lib.libusb_get_active_config_descriptor(dev_list[i], ctypes.byref(cfg)) == 0:
                                try:
                                    for ii in range(cfg.contents.bNumInterfaces):
                                        alt = cfg.contents.interface[ii].altsetting[0]
                                        if alt.bInterfaceClass == 0xFF:
                                            oe = ie = 0
                                            for ei in range(alt.bNumEndpoints):
                                                addr = int(alt.endpoint[ei].bEndpointAddress)
                                                if addr & 0x80:
                                                    if not ie: ie = addr
                                                elif not oe: oe = addr
                                            if oe and ie:
                                                self.intf_num = int(alt.bInterfaceNumber)
                                                self.out_ep = oe
                                                self.in_ep = ie
                                                break
                                finally:
                                    lib.libusb_free_config_descriptor(cfg)
                            rc = lib.libusb_open(dev_list[i], ctypes.byref(self._handle))
                            if rc != 0:
                                raise RuntimeError(f"libusb_open PICOBOOT failed: {rc}")
                            found_dev = True
                            break
            finally:
                if cnt >= 0:
                    lib.libusb_free_device_list(dev_list, 1)
            if not found_dev:
                time.sleep(0.15)

        if not self._handle or self.intf_num < 0:
            self.close()
            raise RuntimeError("RP2040 PICOBOOT device (2e8a:0003) not found")

        rc = lib.libusb_claim_interface(self._handle, self.intf_num)
        if rc != 0:
            self.close()
            raise RuntimeError(f"claim_interface({self.intf_num}) failed: {rc}")
        self._reset_interface()
        self._token = 1

    def _bulk_write(self, ep: int, data: bytes, timeout_ms: int = 5000) -> int:
        ctypes = self._ctypes
        buf = (ctypes.c_uint8 * len(data))(*data) if data else (ctypes.c_uint8 * 1)()
        transferred = ctypes.c_int(0)
        rc = self._lib.libusb_bulk_transfer(self._handle, ep, buf, len(data), ctypes.byref(transferred), timeout_ms)
        if rc != 0:
            raise RuntimeError(f"PICOBOOT bulk_write(ep=0x{ep:02x}, len={len(data)}) failed: {rc}")
        return transferred.value

    def _bulk_read(self, ep: int, length: int, timeout_ms: int = 5000) -> bytes:
        ctypes = self._ctypes
        buf = (ctypes.c_uint8 * max(length, 1))()
        transferred = ctypes.c_int(0)
        rc = self._lib.libusb_bulk_transfer(self._handle, ep, buf, length, ctypes.byref(transferred), timeout_ms)
        if rc != 0:
            raise RuntimeError(f"PICOBOOT bulk_read(ep=0x{ep:02x}, len={length}) failed: {rc}")
        return bytes(buf[: transferred.value])

    def _ctrl_transfer(self, req_type: int, req: int, val: int, idx: int, data_or_len, timeout_ms: int = 2000) -> bytes:
        ctypes = self._ctypes
        if isinstance(data_or_len, int):
            buf = (ctypes.c_uint8 * max(data_or_len, 1))()
            rc = self._lib.libusb_control_transfer(self._handle, req_type, req, val, idx, buf, data_or_len, timeout_ms)
            if rc < 0:
                raise RuntimeError(f"PICOBOOT ctrl_transfer IN failed: {rc}")
            return bytes(buf[:rc])
        buf = (ctypes.c_uint8 * 1)()
        rc = self._lib.libusb_control_transfer(self._handle, req_type, req, val, idx, buf, 0, timeout_ms)
        if rc < 0:
            raise RuntimeError(f"PICOBOOT ctrl_transfer OUT failed: {rc}")
        return b""

    def reset_session(self) -> None:
        self._reset_interface()

    def _reset_interface(self) -> None:
        self._lib.libusb_clear_halt(self._handle, self.out_ep)
        self._lib.libusb_clear_halt(self._handle, self.in_ep)
        self._ctrl_transfer(0x41, PICOBOOT_IF_RESET, 0, self.intf_num, None, timeout_ms=2000)

    def _cmd_status(self) -> tuple[int, int, int, int]:
        raw = self._ctrl_transfer(0xC1, PICOBOOT_IF_CMD_STATUS, 0, self.intf_num, 16, timeout_ms=2000)
        d_token, d_status, b_cmd_id, b_in_prog = struct.unpack("<IIBB6x", raw)
        return d_token, d_status, b_cmd_id, b_in_prog

    def _cmd(
        self,
        cmd_id: int,
        cmd_size: int,
        xfer_len: int,
        args: bytes = b"",
        data_in: bool = False,
        data_out: bytes | None = None,
    ) -> bytes:
        tok = self._token
        self._token += 1
        pkt = build_picoboot_cmd(tok, cmd_id, cmd_size, xfer_len, args)
        self._bulk_write(self.out_ep, pkt, timeout_ms=5000)
        res = b""
        if data_out is not None and xfer_len > 0:
            pos = 0
            while pos < len(data_out):
                part = data_out[pos : pos + 4096]
                w = self._bulk_write(self.out_ep, part, timeout_ms=10000)
                if w <= 0:
                    raise RuntimeError(f"PICOBOOT short OUT write at {pos}/{len(data_out)}")
                pos += w
        elif xfer_len > 0 and data_in:
            chunks: list[bytes] = []
            got = 0
            while got < xfer_len:
                part = self._bulk_read(self.in_ep, min(65536, xfer_len - got), timeout_ms=10000)
                if not part:
                    break
                chunks.append(part)
                got += len(part)
            res = b"".join(chunks)
            if len(res) != xfer_len:
                raise RuntimeError(f"PICOBOOT short IN read: expected {xfer_len}, got {len(res)}")
        # Status stage: OUT ZLP when data_in is True, IN ZLP otherwise (zero-len or data_out).
        if data_in and xfer_len > 0:
            self._bulk_write(self.out_ep, b"", timeout_ms=5000)
        else:
            self._bulk_read(self.in_ep, 64, timeout_ms=5000)
        st_tok, st_code, _cid, _inprog = self._cmd_status()
        if st_tok != tok or st_code != 0:
            raise RuntimeError(f"PICOBOOT cmd 0x{cmd_id:02x} failed: status={st_code}, tok={st_tok}/{tok}")
        return res

    def exclusive_access(self, mode: int = PICOBOOT_EXCLUSIVE) -> None:
        self._cmd(PC_EXCLUSIVE_ACCESS, 1, 0, struct.pack("<B", mode))

    def exit_xip(self) -> None:
        self._cmd(PC_EXIT_XIP, 0, 0)

    def flash_erase(self, offset: int, size: int) -> None:
        if offset % 4096 != 0 or size <= 0 or size % 4096 != 0:
            raise ValueError(f"PICOBOOT flash_erase must be 4 KiB aligned (offset=0x{offset:x}, size=0x{size:x})")
        if offset + size > FW_LFS_OFFSET:
            raise ValueError(f"Refusing to erase into LittleFS calibration region (end=0x{offset + size:x})")
        self._cmd(PC_FLASH_ERASE, 8, 0, struct.pack("<II", RP2040_XIP_BASE + offset, size))

    def write_flash(self, offset: int, data: bytes) -> None:
        if offset % 256 != 0 or len(data) == 0 or len(data) % 256 != 0:
            raise ValueError(f"PICOBOOT write_flash must be 256 B aligned (offset=0x{offset:x}, len={len(data)})")
        if offset + len(data) > FW_LFS_OFFSET:
            raise ValueError(f"Refusing to write into LittleFS calibration region (end=0x{offset + len(data):x})")
        pos = 0
        while pos < len(data):
            chunk = data[pos : pos + 4096]
            addr = RP2040_XIP_BASE + offset + pos
            self._cmd(PC_WRITE, 8, len(chunk), struct.pack("<II", addr, len(chunk)), data_out=chunk)
            pos += len(chunk)

    def read_flash(self, offset: int, size: int) -> bytes:
        chunks: list[bytes] = []
        pos = 0
        while pos < size:
            chunk = min(65536, size - pos)
            addr = RP2040_XIP_BASE + offset + pos
            chunks.append(self._cmd(PC_READ, 8, chunk, struct.pack("<II", addr, chunk), data_in=True))
            pos += chunk
        return b"".join(chunks)

    def reboot(self, delay_ms: int = 100) -> None:
        tok = self._token
        self._token += 1
        pkt = build_picoboot_cmd(tok, PC_REBOOT, 12, 0, struct.pack("<III", 0, 0, delay_ms))
        self._bulk_write(self.out_ep, pkt, timeout_ms=2000)
        try:
            self._bulk_read(self.in_ep, 64, timeout_ms=1000)
        except Exception:
            pass

    def close(self) -> None:
        if getattr(self, "_handle", None):
            try:
                if self.intf_num >= 0:
                    self._lib.libusb_release_interface(self._handle, self.intf_num)
            except Exception:
                pass
            try:
                self._lib.libusb_close(self._handle)
            except Exception:
                pass
            self._handle = None
        if getattr(self, "_ctx", None):
            try:
                self._lib.libusb_exit(self._ctx)
            except Exception:
                pass
            self._ctx = None


def _find_usb_device(vid: int, pid: int) -> bool:
    import ctypes
    from openflux_dev import LibUsbDeviceDescriptor, _load_libusb

    lib = _load_libusb()
    ctx = ctypes.c_void_p()
    if lib.libusb_init(ctypes.byref(ctx)) != 0:
        return False
    try:
        dev_list = ctypes.POINTER(ctypes.c_void_p)()
        cnt = lib.libusb_get_device_list(ctx, ctypes.byref(dev_list))
        if cnt < 0:
            return False
        try:
            for i in range(cnt):
                desc = LibUsbDeviceDescriptor()
                if lib.libusb_get_device_descriptor(dev_list[i], ctypes.byref(desc)) == 0:
                    if desc.idVendor == vid and desc.idProduct == pid:
                        return True
            return False
        finally:
            lib.libusb_free_device_list(dev_list, 1)
    finally:
        lib.libusb_exit(ctx)


def backup_live_rp2040_flash(out_dir: Path) -> Path:
    """Backs up full 16 MiB RP2040 flash + validates calibration, attempting BOOTSEL recovery on exit."""
    from openflux_dev import FW_CMD_MCU_BOOTLOADER, FW_PID, FW_TARGET, OpenFluxUsbClient, VENDOR_ID

    out_dir.mkdir(parents=True, exist_ok=True)

    if not _find_usb_device(RP2040_PICOBOOT_VID, RP2040_PICOBOOT_PID):
        client = OpenFluxUsbClient()
        try:
            client.transact(FW_TARGET, FW_CMD_MCU_BOOTLOADER)
        finally:
            client.close()

    pb: PicobootClient | None = None
    primary_exc: BaseException | None = None
    reboot_exc: BaseException | None = None
    try:
        pb = PicobootClient(timeout_s=10.0)
        pb.exclusive_access(PICOBOOT_EXCLUSIVE)
        pb.exit_xip()
        flash_bin = pb.read_flash(0, RP2040_FLASH_SIZE)
        if len(flash_bin) != RP2040_FLASH_SIZE:
            raise RuntimeError(f"Expected {RP2040_FLASH_SIZE} bytes, got {len(flash_bin)}")
        lfs_bin = flash_bin[FW_LFS_OFFSET : FW_LFS_OFFSET + FW_LFS_SIZE]
        lfs_verify = pb.read_flash(FW_LFS_OFFSET, FW_LFS_SIZE)
        if hashlib.sha256(lfs_bin).digest() != hashlib.sha256(lfs_verify).digest():
            raise RuntimeError("LittleFS partition SHA-256 mismatch across repeated reads")

        # Persist raw dump immediately so any later parse failure preserves the full raw flash image.
        (out_dir / "firmware_flash_16mib.bin").write_bytes(flash_bin)
        (out_dir / "firmware_lfs_1mib.bin").write_bytes(lfs_bin)
        (out_dir / "firmware_flash_16mib.sha256").write_text(hashlib.sha256(flash_bin).hexdigest() + "\n")

        files = extract_littlefs_files(lfs_bin)
        lfs_dir = out_dir / "lfs_files"
        lfs_dir.mkdir(exist_ok=True)
        for name, data in files.items():
            (lfs_dir / name).write_bytes(data)

        if "factory_calibration_info" not in files or "factory_scaling_factors" not in files:
            raise RuntimeError(
                f"LittleFS calibration backup incomplete; missing factory files (found: {sorted(files.keys())})"
            )

        cal = parse_factory_calibration(files["factory_calibration_info"], files["factory_scaling_factors"])
        meta = {
            "flash_bytes": len(flash_bin),
            "flash_sha256": hashlib.sha256(flash_bin).hexdigest(),
            "lfs_sha256": hashlib.sha256(lfs_bin).hexdigest(),
            "lfs_files": sorted(files.keys()),
            "calibration": {
                "magic": f"0x{cal.magic:08x}",
                "version": cal.version,
                "num_sensors": cal.num_sensors,
                "checksum": f"0x{cal.checksum:08x}",
                "sample_factors_q8": cal.effective_factors_q8[:10],
            },
        }
        (out_dir / "manifest.json").write_text(json.dumps(meta, indent=2) + "\n")
    except BaseException as exc:
        primary_exc = exc
        raise
    finally:
        if pb is not None:
            if primary_exc is not None:
                try:
                    pb.reset_session()
                except BaseException:
                    pass
            try:
                pb.reboot(100)
            except BaseException as exc:
                reboot_exc = exc
            finally:
                pb.close()

        # Verify the keyboard's normal firmware USB enumeration was restored.
        deadline = time.monotonic() + 6.0
        restored = False
        while time.monotonic() < deadline:
            if _find_usb_device(VENDOR_ID, FW_PID):
                restored = True
                break
            time.sleep(0.15)
        if not restored:
            msg = (
                f"RP2040 did not re-enumerate as normal firmware ({VENDOR_ID:04x}:{FW_PID:04x}) after BOOTSEL"
                + (f" (reboot error: {reboot_exc})" if reboot_exc else "")
            )
            if primary_exc is None:
                raise RuntimeError(msg)
            raise RuntimeError(f"{primary_exc}; additionally: {msg}") from primary_exc
    return out_dir


def parse_uf2_image(uf2_bytes: bytes) -> tuple[int, bytes]:
    """Parses an RP2040 UF2 image into (flash_offset, 4KiB-padded payload), refusing LittleFS overlap."""
    if len(uf2_bytes) == 0 or len(uf2_bytes) % 512 != 0:
        raise ValueError(f"Invalid UF2 size: {len(uf2_bytes)}")
    pages: dict[int, bytes] = {}
    nblocks = len(uf2_bytes) // 512
    for i in range(nblocks):
        blk = uf2_bytes[i * 512 : (i + 1) * 512]
        m0, m1, _flags, addr, psz, bno, tot, _fam = struct.unpack_from("<8I", blk, 0)
        mend = struct.unpack_from("<I", blk, 508)[0]
        if m0 != 0x0A324655 or m1 != 0x9E5D5157 or mend != 0x0AB16F30:
            raise ValueError(f"Bad UF2 magic in block {i}")
        if psz != 256 or addr % 256 != 0:
            raise ValueError(f"Unexpected UF2 page size/align in block {i}: size={psz}, addr=0x{addr:x}")
        if bno != i or tot != nblocks:
            raise ValueError(f"Unexpected UF2 block sequence {bno}/{tot} at index {i}")
        if addr < RP2040_XIP_BASE or addr + psz > RP2040_XIP_BASE + FW_LFS_OFFSET:
            raise ValueError(
                f"UF2 block {i} address 0x{addr:x} touches reserved/LittleFS calibration region (>=0x{RP2040_XIP_BASE + FW_LFS_OFFSET:x})"
            )
        pages[addr - RP2040_XIP_BASE] = blk[32 : 32 + 256]

    min_off = min(pages.keys()) & ~0xFFF
    max_end = (max(pages.keys()) + 256 + 0xFFF) & ~0xFFF
    if max_end > FW_LFS_OFFSET:
        raise ValueError(f"UF2 padded end 0x{max_end:x} overlaps LittleFS region")
    buf = bytearray(b"\xff" * (max_end - min_off))
    for off, pdata in pages.items():
        pos = off - min_off
        buf[pos : pos + 256] = pdata
    return min_off, bytes(buf)


def flash_live_rp2040_uf2(uf2_path: Path) -> dict[str, object]:
    """Flashes an RP2040 .uf2 via PICOBOOT while preserving 0x10F00000 LittleFS calibration, with readback verification."""
    from openflux_dev import FW_CMD_MCU_BOOTLOADER, FW_PID, FW_TARGET, OpenFluxUsbClient, VENDOR_ID

    offset, image = parse_uf2_image(uf2_path.read_bytes())
    expected_sha = hashlib.sha256(image).hexdigest()

    if not _find_usb_device(RP2040_PICOBOOT_VID, RP2040_PICOBOOT_PID):
        client = OpenFluxUsbClient()
        try:
            client.transact(FW_TARGET, FW_CMD_MCU_BOOTLOADER)
        finally:
            client.close()

    pb: PicobootClient | None = None
    lfs_after_sha = ""
    try:
        pb = PicobootClient(timeout_s=10.0)
        pb.exclusive_access(PICOBOOT_EXCLUSIVE_AND_EJECT)
        pb.exit_xip()
        lfs_before_sha = hashlib.sha256(pb.read_flash(FW_LFS_OFFSET, FW_LFS_SIZE)).hexdigest()
        pb.flash_erase(offset, len(image))
        pb.write_flash(offset, image)
        verify = pb.read_flash(offset, len(image))
        if hashlib.sha256(verify).hexdigest() != expected_sha:
            raise RuntimeError(
                "Read-back verification failed after writing the firmware; the keyboard is still in BOOTSEL mode, "
                "rerun rp2040-flash"
            )
        lfs_after_sha = hashlib.sha256(pb.read_flash(FW_LFS_OFFSET, FW_LFS_SIZE)).hexdigest()
        if lfs_after_sha != lfs_before_sha:
            raise RuntimeError(
                "The calibration partition changed during flashing; the keyboard is still in BOOTSEL mode, "
                "back up before retrying"
            )
        # Reboot only after the firmware and the calibration partition both verify.
        pb.reboot(100)
    except BaseException:
        if pb is not None:
            try:
                pb.reset_session()
            except BaseException:
                pass
        raise
    finally:
        if pb is not None:
            pb.close()

    deadline = time.monotonic() + 8.0
    while time.monotonic() < deadline:
        if _find_usb_device(VENDOR_ID, FW_PID):
            return {
                "offset": f"0x{offset:08x}",
                "bytes": len(image),
                "sha256": expected_sha,
                "lfs_sha256": lfs_after_sha,
            }
        time.sleep(0.15)
    raise RuntimeError(f"RP2040 did not enumerate as {VENDOR_ID:04x}:{FW_PID:04x} after PICOBOOT flash")
