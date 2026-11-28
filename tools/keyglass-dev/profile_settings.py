#!/usr/bin/env python3
"""Binary codec for Keyglass `PROFILE_SETTINGS_V1` (firmware `0x6b` / `0x6c`).

Wire layout:
  - `FileHeader` (16 B): 4 x uint32_le `[version=0, file_type=0, 0, 0]`
  - `ProfileSettingsHeader` (16 B): 4 x uint32_le `[num_profiles, 0, 0, 0]`
  - `ProfileSetting` (8 B x num_profiles): 2 x uint32_le `[actuation_mode_index, default_layer_bitfield]`
"""

from __future__ import annotations

import struct
from dataclasses import asdict, dataclass
from typing import Any

MAX_PROFILES = 64
FILE_TYPE_PROFILE_SETTINGS_V1 = 0


@dataclass
class ProfileSetting:
    actuation_mode_index: int
    default_layer_bitfield: int

    @classmethod
    def from_layer(cls, actuation_mode_index: int, default_layer: int) -> "ProfileSetting":
        if actuation_mode_index < 0:
            raise ValueError(f"actuation_mode_index must be >= 0, got {actuation_mode_index}")
        if not (0 <= default_layer <= 31):
            raise ValueError(f"default_layer must be in [0, 31], got {default_layer}")
        return cls(
            actuation_mode_index=actuation_mode_index,
            default_layer_bitfield=1 << default_layer,
        )


@dataclass
class ProfileSettingsFile:
    settings: list[ProfileSetting]
    file_version: int = 0
    file_type: int = FILE_TYPE_PROFILE_SETTINGS_V1

    def to_data_bytes(self) -> bytes:
        if len(self.settings) > MAX_PROFILES:
            raise ValueError(f"Maximum {MAX_PROFILES} profiles allowed, got {len(self.settings)}")
        out = bytearray(16 + 8 * len(self.settings))
        struct.pack_into("<4I", out, 0, len(self.settings), 0, 0, 0)
        for i, item in enumerate(self.settings):
            struct.pack_into("<2I", out, 16 + i * 8, item.actuation_mode_index, item.default_layer_bitfield)
        return bytes(out)

    def to_transfer_bytes(self) -> bytes:
        data = self.to_data_bytes()
        header = struct.pack("<4I", self.file_version, self.file_type, 0, 0)
        return header + data

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)


def decode_profile_settings(raw: bytes, *, has_file_header: bool = False) -> ProfileSettingsFile:
    """Decode PROFILE_SETTINGS_V1 from either data bytes (0x6c) or transfer bytes (0x6b)."""
    pos = 0
    file_version = 0
    file_type = FILE_TYPE_PROFILE_SETTINGS_V1
    if has_file_header:
        if len(raw) < 32:
            raise ValueError(f"Transfer payload too small for headers: {len(raw)} bytes")
        file_version, file_type, _, _ = struct.unpack_from("<4I", raw, 0)
        if file_type != FILE_TYPE_PROFILE_SETTINGS_V1:
            raise ValueError(f"Unexpected file_type {file_type}, expected {FILE_TYPE_PROFILE_SETTINGS_V1}")
        pos = 16
    elif len(raw) < 16:
        raise ValueError(f"Data payload too small for ProfileSettingsHeader: {len(raw)} bytes")

    num_profiles, _, _, _ = struct.unpack_from("<4I", raw, pos)
    if num_profiles > MAX_PROFILES:
        raise ValueError(f"Invalid num_profiles: {num_profiles} > {MAX_PROFILES}")
    expected = pos + 16 + num_profiles * 8
    if len(raw) != expected:
        raise ValueError(f"Size mismatch: expected {expected} bytes for {num_profiles} profiles, got {len(raw)}")

    settings: list[ProfileSetting] = []
    for i in range(num_profiles):
        act_idx, layer_mask = struct.unpack_from("<2I", raw, pos + 16 + i * 8)
        settings.append(ProfileSetting(actuation_mode_index=act_idx, default_layer_bitfield=layer_mask))
    return ProfileSettingsFile(settings=settings, file_version=file_version, file_type=file_type)
