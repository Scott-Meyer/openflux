#!/usr/bin/env python3
"""OpenFlux SimpleHDLC framing and display live-data packet codec.

Frame layout:
  - Start boundary flag: 0x7E
  - 2-byte big-endian unescaped payload length (byte-stuffed with 0x7D / ^0x20)
  - Payload bytes (byte-stuffed with 0x7D / ^0x20)
  - 4-byte big-endian CRC32 (MSB-first 0x04C11DB7 over 8-bit bit-reversed payload
    bytes, final bitwise NOT `~crc & 0xffffffff`, byte-stuffed with 0x7D / ^0x20)
"""

from __future__ import annotations

import json
import struct
from enum import IntEnum
from typing import Any

HDLC_FLAG = 0x7E
HDLC_ESCAPE = 0x7D
HDLC_ESCAPE_XOR = 0x20
HDLC_MAX_PAYLOAD = 8192
LIVE_DATA_MAX_CHUNK = 8000


class DataType(IntEnum):
    JSON = 0
    HEARTBEAT = 4


def _build_reverse_table() -> tuple[int, ...]:
    table = []
    for i in range(256):
        rev = 0
        for bit in range(8):
            rev = (rev << 1) | ((i >> bit) & 1)
        table.append(rev)
    return tuple(table)


def _build_crc32_table() -> tuple[int, ...]:
    poly = 0x04C11DB7
    table = []
    for i in range(256):
        crc = (i << 24) & 0xFFFFFFFF
        for _ in range(8):
            if crc & 0x80000000:
                crc = ((crc << 1) ^ poly) & 0xFFFFFFFF
            else:
                crc = (crc << 1) & 0xFFFFFFFF
        table.append(crc)
    return tuple(table)


_REVERSE_TABLE = _build_reverse_table()
_CRC32_TABLE = _build_crc32_table()


def compute_crc32(data: bytes) -> int:
    """Compute SimpleHDLC CRC32 over unescaped payload bytes."""
    crc = 0xFFFFFFFF
    for b in data:
        idx = ((crc >> 24) ^ _REVERSE_TABLE[b]) & 0xFF
        crc = (_CRC32_TABLE[idx] ^ ((crc << 8) & 0xFFFFFFFF)) & 0xFFFFFFFF
    return (~crc) & 0xFFFFFFFF


def _escape_byte(out: bytearray, byte: int) -> None:
    if byte in (HDLC_FLAG, HDLC_ESCAPE):
        out.append(HDLC_ESCAPE)
        out.append(byte ^ HDLC_ESCAPE_XOR)
    else:
        out.append(byte)


def encode_frame(payload: bytes) -> bytes:
    """Encode a raw payload into a SimpleHDLC frame."""
    if len(payload) > 0xFFFF:
        raise ValueError(f"Payload too large for SimpleHDLC: {len(payload)}")
    out = bytearray([HDLC_FLAG])
    _escape_byte(out, (len(payload) >> 8) & 0xFF)
    _escape_byte(out, len(payload) & 0xFF)
    for b in payload:
        _escape_byte(out, b)
    crc = compute_crc32(payload)
    _escape_byte(out, (crc >> 24) & 0xFF)
    _escape_byte(out, (crc >> 16) & 0xFF)
    _escape_byte(out, (crc >> 8) & 0xFF)
    _escape_byte(out, crc & 0xFF)
    return bytes(out)


class SimpleHdlcDecoder:
    """Incremental byte-stream SimpleHDLC decoder."""

    def __init__(self, max_payload: int = HDLC_MAX_PAYLOAD) -> None:
        self.max_payload = max_payload
        self._state = 0  # 0=wait flag, 1=len_hi, 2=len_lo, 3=body+crc
        self._escaped = False
        self._expected_len = 0
        self._buf = bytearray()

    def reset(self) -> None:
        self._state = 0
        self._escaped = False
        self._expected_len = 0
        self._buf.clear()

    def feed(self, data: bytes) -> list[bytes]:
        frames: list[bytes] = []
        for b in data:
            if b == HDLC_FLAG:
                # Start of a new frame always resets receiver state
                self._state = 1
                self._escaped = False
                self._expected_len = 0
                self._buf.clear()
                continue
            if self._state == 0:
                continue
            if self._escaped:
                b ^= HDLC_ESCAPE_XOR
                self._escaped = False
            elif b == HDLC_ESCAPE:
                self._escaped = True
                continue

            if self._state == 1:
                self._expected_len = b << 8
                self._state = 2
            elif self._state == 2:
                self._expected_len |= b
                if self._expected_len > self.max_payload:
                    self.reset()
                    raise ValueError(
                        f"SimpleHDLC length {self._expected_len} exceeds max {self.max_payload}"
                    )
                self._buf.clear()
                self._state = 3
            elif self._state == 3:
                self._buf.append(b)
                if len(self._buf) == self._expected_len + 4:
                    payload = bytes(self._buf[: self._expected_len])
                    rx_crc = struct.unpack_from(">I", self._buf, self._expected_len)[0]
                    calc_crc = compute_crc32(payload)
                    self.reset()
                    if rx_crc != calc_crc:
                        raise ValueError(
                            f"SimpleHDLC CRC32 mismatch: got 0x{rx_crc:08x}, expected 0x{calc_crc:08x}"
                        )
                    frames.append(payload)
        return frames


def decode_frame(frame: bytes, max_payload: int = HDLC_MAX_PAYLOAD) -> bytes:
    """Decode a single complete SimpleHDLC frame."""
    dec = SimpleHdlcDecoder(max_payload=max_payload)
    frames = dec.feed(frame)
    if len(frames) != 1:
        raise ValueError(f"Expected 1 complete SimpleHDLC frame, decoded {len(frames)}")
    return frames[0]


def encode_live_data(data_type: DataType, payload: bytes = b"") -> list[bytes]:
    """Encode a CMD 200 live-data sequence (length frame + chunk frames)."""
    if data_type == DataType.JSON and not payload:
        raise ValueError("JSON live-data message requires non-empty payload")
    full_payload = bytes([int(data_type)]) + (payload if data_type == DataType.JSON else b"")
    len_pkt = bytes([0xFF]) + struct.pack(">I", len(full_payload))
    frames = [encode_frame(len_pkt)]
    for offset in range(0, len(full_payload), LIVE_DATA_MAX_CHUNK):
        frames.append(encode_frame(full_payload[offset : offset + LIVE_DATA_MAX_CHUNK]))
    return frames


def encode_live_json(payload_obj: dict[str, Any]) -> list[bytes]:
    """Serialize a JSON dict and wrap it in CMD 200 SimpleHDLC frames."""
    raw = json.dumps(payload_obj, separators=(",", ":")).encode("utf-8")
    return encode_live_data(DataType.JSON, raw)


def decode_live_data(frames: list[bytes]) -> tuple[DataType, bytes]:
    """Decode a CMD 200 SimpleHDLC frame list back to (DataType, raw_json_bytes)."""
    if len(frames) < 2:
        raise ValueError(f"Expected length frame + >=1 data frame, got {len(frames)}")
    len_hdr = decode_frame(frames[0])
    if len(len_hdr) != 5 or len_hdr[0] != 0xFF:
        raise ValueError(f"Invalid live-data length header: {len_hdr.hex()}")
    expected_len = struct.unpack_from(">I", len_hdr, 1)[0]
    body = b"".join(decode_frame(f) for f in frames[1:])
    if len(body) != expected_len:
        raise ValueError(f"Live-data length mismatch: expected {expected_len}, got {len(body)}")
    if not body:
        raise ValueError("Empty live-data payload (missing DataType tag)")
    return DataType(body[0]), body[1:]
