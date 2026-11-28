#!/usr/bin/env python3
"""Binary keymap codec for the Keyglass keyboard RP2040 firmware."""

from __future__ import annotations

import json
import math
import struct
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any

# Calibration LUT mapping actuation/reset travel in micrometers to firmware flux density units.
DIST_UM_TO_FLUX_DENSITY: dict[int, int] = {
    0: 0x000,
    100: 0x0AB,
    200: 0x0BE,
    300: 0x0DE,
    400: 0x0FE,
    500: 0x126,
    600: 0x150,
    700: 0x17A,
    800: 0x1A6,
    900: 0x1D1,
    1000: 0x1FD,
    1100: 0x228,
    1200: 0x253,
    1300: 0x281,
    1400: 0x2B3,
    1500: 0x2E5,
    1600: 0x318,
    1700: 0x34C,
    1800: 0x381,
    1900: 0x3C0,
    2000: 0x402,
    2100: 0x444,
    2200: 0x48A,
    2300: 0x4D1,
    2400: 0x518,
    2500: 0x561,
    2600: 0x5AB,
    2700: 0x5D7,
    2800: 0x613,
}
_FLUX_DENSITY_TO_DIST_UM: dict[int, int] = {v: k for k, v in DIST_UM_TO_FLUX_DENSITY.items()}

ACTUATION_TRADITIONAL = 1
ACTUATION_RAPID_TRIGGER = 2
NONE_KEYCODE = 0xF0000000


def dist_um_to_flux_density(dist_um: int) -> int:
    """Clamp to [0, 2800] um, round down to 100 um, and map to firmware flux density."""
    clamped = max(0, min(2800, dist_um))
    stepped = (clamped // 100) * 100
    return DIST_UM_TO_FLUX_DENSITY.get(stepped, 0xAB)


def flux_density_to_dist_um(density: int) -> int | None:
    """Invert a firmware flux density table value to um, or return None if custom."""
    return _FLUX_DENSITY_TO_DIST_UM.get(density)


def nearest_dist_um(density: int) -> int:
    """The 0.1 mm step whose reading is closest to `density`; the shallower step on a tie."""
    return min(DIST_UM_TO_FLUX_DENSITY.items(), key=lambda kv: (abs(kv[1] - density), kv[0]))[0]


# The fixed actuation reading the firmware uses for a type 0 or 3 entry, before position gain.
DEFAULT_FIXED_ACTUATION_DENSITY = 650
# The fixed release depth the firmware uses for a type 0 or 3 entry, before position gain.
DEFAULT_FIXED_RELEASE_DEPTH = 216
# A fixed mode with no release point releases this far above its actuation point.
DEFAULT_RELEASE_GAP_UM = 200


def encode_actuation_word(actuation_type: int, actuation_um: int, second_um: int) -> int:
    """Encode `(type << 30) | (field << 12) | actuation_density`.

    Fixed (`ACTUATION_TRADITIONAL`): `second_um` is the release point. The field is the release
    depth `R = density(actuation) - density(release)`, so a held key releases once its reading
    falls to `A - R`. A release point as deep as the actuation point or deeper gives `R = 0`, and
    `R` never exceeds `A`.

    Rapid trigger: `second_um` is the sensitivity, stored as its reading.
    """
    act_d = dist_um_to_flux_density(actuation_um) & 0xFFF
    second_d = dist_um_to_flux_density(second_um) & 0xFFF
    if actuation_type == ACTUATION_TRADITIONAL:
        fld = max(0, act_d - second_d)
    elif actuation_type == ACTUATION_RAPID_TRIGGER:
        fld = second_d
    else:
        raise ValueError(f"Unsupported actuation_type: {actuation_type}")
    return (actuation_type << 30) | (fld << 12) | act_d


def decode_actuation_word(word: int) -> dict[str, Any]:
    """Decode one actuation word the way the firmware applies it, with travel in 0.1 mm steps.

    - `actuation_type`: 1 (fixed) or 2 (rapid trigger); type 0 and 3 decode as fixed.
    - `actuation_density`: the actuation reading, before position gain.
    - `second_density`: fixed, the release depth `R`; rapid trigger, the sensitivity reading.
    - `actuation_um`: the step closest to the actuation reading.
    - `second_um`: fixed, the step closest to the release reading `A - R`; rapid trigger, the
      step closest to the sensitivity reading.
    - `firmware_default`: a type 0 or 3 entry, which the firmware applies as its default fixed
      setting (`A = 650`, `R = 216`).
    - `exact`: encoding `actuation_um` and `second_um` gives back this word.
    """
    raw_type = (word >> 30) & 0x3
    firmware_default = raw_type not in (ACTUATION_TRADITIONAL, ACTUATION_RAPID_TRIGGER)
    if firmware_default:
        actuation_type = ACTUATION_TRADITIONAL
        act_d = DEFAULT_FIXED_ACTUATION_DENSITY
        second_d = DEFAULT_FIXED_RELEASE_DEPTH
    else:
        actuation_type = raw_type
        act_d = word & 0xFFF
        second_d = (word >> 12) & 0xFFF
    actuation_um = nearest_dist_um(act_d)
    if actuation_type == ACTUATION_TRADITIONAL:
        second_um = nearest_dist_um(max(0, act_d - second_d))
    else:
        second_um = nearest_dist_um(second_d)
    exact = encode_actuation_word(actuation_type, actuation_um, second_um) == word
    return {
        "mode": "rapid_trigger" if actuation_type == ACTUATION_RAPID_TRIGGER else "traditional_trigger",
        "actuation_type": actuation_type,
        "actuation_density": act_d,
        "second_density": second_d,
        "actuation_um": actuation_um,
        "second_um": second_um,
        "firmware_default": firmware_default,
        "exact": exact,
        "raw": f"0x{word:08x}",
    }


def _um_to_mm(um: int) -> float:
    return um / 1000.0


def _mm_to_um(mm: float, min_um: float) -> int:
    # Rounds half away from zero, like Rust's `f64::round`; the result is clamped non-negative.
    um = math.floor(mm * 1000.0 + 0.5) if mm >= 0 else -math.floor(-mm * 1000.0 + 0.5)
    return int(max(min_um, min(2800.0, um)))


def _dist_um_to_step(dist_um: int) -> int:
    """Travel rounded down to its 0.1 mm step, as the encoder applies it."""
    return (min(dist_um, 2800) // 100) * 100


def actuation_entry_json(word: int) -> dict[str, Any]:
    """The editable keymap-document entry for one actuation word.

    A word the 0.1 mm values don't reproduce also carries `firmwareWord`, which
    `encode_actuation_entry` keeps while those values stay unchanged.
    """
    d = decode_actuation_word(word)
    rapid = d["actuation_type"] == ACTUATION_RAPID_TRIGGER
    entry: dict[str, Any] = {
        "actuationPointMm": _um_to_mm(d["actuation_um"]),
        "rapidTrigger": rapid,
        "touched": False,
    }
    if rapid:
        entry["rapidTriggerSensitivityMm"] = _um_to_mm(d["second_um"])
    else:
        entry["releasePointMm"] = _um_to_mm(d["second_um"])
    if d["firmware_default"]:
        entry["firmwareDefault"] = True
    if not d["exact"]:
        entry["firmwareWord"] = word
    return entry


def _number(v: Any) -> float | None:
    return float(v) if isinstance(v, (int, float)) and not isinstance(v, bool) else None


# What an actuation mode falls back to for fields it leaves out.
MODE_DEFAULTS: dict[str, Any] = {"rapid": False, "act_mm": 2.0, "sens_mm": 0.3}


def encode_actuation_entry(entry: dict[str, Any], defaults: dict[str, Any] = MODE_DEFAULTS) -> tuple[int, dict[str, Any]]:
    """Encode one mode or per-key actuation entry.

    Returns the word and the entry's settings, which per-key entries of the mode inherit as their
    `defaults`. A `firmwareWord` is written as it is while the entry's values still decode from it.
    """
    rapid_v = entry.get("rapidTrigger")
    rapid = rapid_v if isinstance(rapid_v, bool) else defaults["rapid"]
    act_mm = _number(entry.get("actuationPointMm"))
    act_mm = defaults["act_mm"] if act_mm is None else act_mm
    sens_mm = _number(entry.get("rapidTriggerSensitivityMm"))
    sens_mm = defaults["sens_mm"] if sens_mm is None else sens_mm
    act_um = _mm_to_um(act_mm, 100.0)
    if rapid:
        atype, second_mm, second_um = ACTUATION_RAPID_TRIGGER, sens_mm, _mm_to_um(sens_mm, 100.0)
    else:
        release_mm = _number(entry.get("releasePointMm"))
        if release_mm is not None:
            atype, second_mm, second_um = ACTUATION_TRADITIONAL, release_mm, _mm_to_um(release_mm, 0.0)
        else:
            um = max(0, act_um - DEFAULT_RELEASE_GAP_UM)
            atype, second_mm, second_um = ACTUATION_TRADITIONAL, _um_to_mm(um), um
    word = encode_actuation_word(atype, act_um, second_um)
    source = entry.get("firmwareWord")
    if isinstance(source, int) and not isinstance(source, bool) and 0 <= source <= 0xFFFFFFFF:
        d = decode_actuation_word(source)
        if (
            d["actuation_type"] == atype
            and d["actuation_um"] == _dist_um_to_step(_mm_to_um(act_mm, 0.0))
            and d["second_um"] == _dist_um_to_step(_mm_to_um(second_mm, 0.0))
        ):
            word = source
    return word, {"rapid": rapid, "act_mm": act_mm, "sens_mm": sens_mm}


def encode_macro_step(hid_usage: int, delay_ms: int = 0, *, is_down: bool = True) -> int:
    """Encode `((is_down ? 1 : 0) << 31) | ((delay_ms & 0xffff) << 8) | (hid_usage & 0xff)`."""
    if not (0 <= hid_usage <= 0xFF):
        raise ValueError(f"hid_usage out of range: {hid_usage}")
    if not (0 <= delay_ms <= 0xFFFF):
        raise ValueError(f"delay_ms out of range: {delay_ms}")
    return ((1 if is_down else 0) << 31) | (delay_ms << 8) | hid_usage


def decode_macro_step(word: int) -> dict[str, Any]:
    return {
        "is_down": bool((word >> 31) & 1),
        "delay_ms": (word >> 8) & 0xFFFF,
        "hid_usage": word & 0xFF,
        "raw": f"0x{word:08x}",
    }


TRANSPARENT_KEYCODE = 0x50000000
CONSUMER_KIND = 0x40000000

# Key code names, as the daemon's `codecs/keycodes.rs` has them; both are checked against
# host/daemon/tests/fixtures/keycodes.json. Keyboard/Keypad Page (0x07) usages:
KEYBOARD_KEYS: list[tuple[str, int]] = [
    ("KC_A", 0x04),
    ("KC_B", 0x05),
    ("KC_C", 0x06),
    ("KC_D", 0x07),
    ("KC_E", 0x08),
    ("KC_F", 0x09),
    ("KC_G", 0x0A),
    ("KC_H", 0x0B),
    ("KC_I", 0x0C),
    ("KC_J", 0x0D),
    ("KC_K", 0x0E),
    ("KC_L", 0x0F),
    ("KC_M", 0x10),
    ("KC_N", 0x11),
    ("KC_O", 0x12),
    ("KC_P", 0x13),
    ("KC_Q", 0x14),
    ("KC_R", 0x15),
    ("KC_S", 0x16),
    ("KC_T", 0x17),
    ("KC_U", 0x18),
    ("KC_V", 0x19),
    ("KC_W", 0x1A),
    ("KC_X", 0x1B),
    ("KC_Y", 0x1C),
    ("KC_Z", 0x1D),
    ("KC_1", 0x1E),
    ("KC_2", 0x1F),
    ("KC_3", 0x20),
    ("KC_4", 0x21),
    ("KC_5", 0x22),
    ("KC_6", 0x23),
    ("KC_7", 0x24),
    ("KC_8", 0x25),
    ("KC_9", 0x26),
    ("KC_0", 0x27),
    ("KC_ENTER", 0x28),
    ("KC_ESCAPE", 0x29),
    ("KC_BACKSPACE", 0x2A),
    ("KC_TAB", 0x2B),
    ("KC_SPACE", 0x2C),
    ("KC_MINUS", 0x2D),
    ("KC_EQUAL", 0x2E),
    ("KC_LEFT_BRACKET", 0x2F),
    ("KC_RIGHT_BRACKET", 0x30),
    ("KC_BACKSLASH", 0x31),
    ("KC_NONUS_HASH", 0x32),
    ("KC_SEMICOLON", 0x33),
    ("KC_QUOTE", 0x34),
    ("KC_GRAVE", 0x35),
    ("KC_COMMA", 0x36),
    ("KC_DOT", 0x37),
    ("KC_SLASH", 0x38),
    ("KC_CAPS_LOCK", 0x39),
    ("KC_F1", 0x3A),
    ("KC_F2", 0x3B),
    ("KC_F3", 0x3C),
    ("KC_F4", 0x3D),
    ("KC_F5", 0x3E),
    ("KC_F6", 0x3F),
    ("KC_F7", 0x40),
    ("KC_F8", 0x41),
    ("KC_F9", 0x42),
    ("KC_F10", 0x43),
    ("KC_F11", 0x44),
    ("KC_F12", 0x45),
    ("KC_PRINT_SCREEN", 0x46),
    ("KC_SCROLL_LOCK", 0x47),
    ("KC_PAUSE", 0x48),
    ("KC_INSERT", 0x49),
    ("KC_HOME", 0x4A),
    ("KC_PAGE_UP", 0x4B),
    ("KC_DELETE", 0x4C),
    ("KC_END", 0x4D),
    ("KC_PAGE_DOWN", 0x4E),
    ("KC_RIGHT", 0x4F),
    ("KC_LEFT", 0x50),
    ("KC_DOWN", 0x51),
    ("KC_UP", 0x52),
    ("KC_NUM_LOCK", 0x53),
    ("KC_KP_SLASH", 0x54),
    ("KC_KP_ASTERISK", 0x55),
    ("KC_KP_MINUS", 0x56),
    ("KC_KP_PLUS", 0x57),
    ("KC_KP_ENTER", 0x58),
    ("KC_KP_1", 0x59),
    ("KC_KP_2", 0x5A),
    ("KC_KP_3", 0x5B),
    ("KC_KP_4", 0x5C),
    ("KC_KP_5", 0x5D),
    ("KC_KP_6", 0x5E),
    ("KC_KP_7", 0x5F),
    ("KC_KP_8", 0x60),
    ("KC_KP_9", 0x61),
    ("KC_KP_0", 0x62),
    ("KC_KP_DOT", 0x63),
    ("KC_NONUS_BACKSLASH", 0x64),
    ("KC_APPLICATION", 0x65),
    ("KC_KB_POWER", 0x66),
    ("KC_KP_EQUAL", 0x67),
    ("KC_F13", 0x68),
    ("KC_F14", 0x69),
    ("KC_F15", 0x6A),
    ("KC_F16", 0x6B),
    ("KC_F17", 0x6C),
    ("KC_F18", 0x6D),
    ("KC_F19", 0x6E),
    ("KC_F20", 0x6F),
    ("KC_F21", 0x70),
    ("KC_F22", 0x71),
    ("KC_F23", 0x72),
    ("KC_F24", 0x73),
    ("KC_EXECUTE", 0x74),
    ("KC_HELP", 0x75),
    ("KC_MENU", 0x76),
    ("KC_SELECT", 0x77),
    ("KC_STOP", 0x78),
    ("KC_AGAIN", 0x79),
    ("KC_UNDO", 0x7A),
    ("KC_CUT", 0x7B),
    ("KC_COPY", 0x7C),
    ("KC_PASTE", 0x7D),
    ("KC_FIND", 0x7E),
    ("KC_KB_MUTE", 0x7F),
    ("KC_KB_VOLUME_UP", 0x80),
    ("KC_KB_VOLUME_DOWN", 0x81),
    ("KC_LOCKING_CAPS_LOCK", 0x82),
    ("KC_LOCKING_NUM_LOCK", 0x83),
    ("KC_LOCKING_SCROLL_LOCK", 0x84),
    ("KC_KP_COMMA", 0x85),
    ("KC_KP_EQUAL_AS400", 0x86),
    ("KC_INTERNATIONAL_1", 0x87),
    ("KC_INTERNATIONAL_2", 0x88),
    ("KC_INTERNATIONAL_3", 0x89),
    ("KC_INTERNATIONAL_4", 0x8A),
    ("KC_INTERNATIONAL_5", 0x8B),
    ("KC_INTERNATIONAL_6", 0x8C),
    ("KC_INTERNATIONAL_7", 0x8D),
    ("KC_INTERNATIONAL_8", 0x8E),
    ("KC_INTERNATIONAL_9", 0x8F),
    ("KC_LANGUAGE_1", 0x90),
    ("KC_LANGUAGE_2", 0x91),
    ("KC_LANGUAGE_3", 0x92),
    ("KC_LANGUAGE_4", 0x93),
    ("KC_LANGUAGE_5", 0x94),
    ("KC_LANGUAGE_6", 0x95),
    ("KC_LANGUAGE_7", 0x96),
    ("KC_LANGUAGE_8", 0x97),
    ("KC_LANGUAGE_9", 0x98),
    ("KC_ALTERNATE_ERASE", 0x99),
    ("KC_SYSTEM_REQUEST", 0x9A),
    ("KC_CANCEL", 0x9B),
    ("KC_CLEAR", 0x9C),
    ("KC_PRIOR", 0x9D),
    ("KC_RETURN", 0x9E),
    ("KC_SEPARATOR", 0x9F),
    ("KC_OUT", 0xA0),
    ("KC_OPER", 0xA1),
    ("KC_CLEAR_AGAIN", 0xA2),
    ("KC_CRSEL", 0xA3),
    ("KC_EXSEL", 0xA4),
    ("KC_LEFT_CTRL", 0xE0),
    ("KC_LEFT_SHIFT", 0xE1),
    ("KC_LEFT_ALT", 0xE2),
    ("KC_LEFT_GUI", 0xE3),
    ("KC_RIGHT_CTRL", 0xE4),
    ("KC_RIGHT_SHIFT", 0xE5),
    ("KC_RIGHT_ALT", 0xE6),
    ("KC_RIGHT_GUI", 0xE7),
]

# Consumer Page (0x0C) usages; a consumer key code's low 16 bits are the usage.
CONSUMER_CONTROLS: list[tuple[str, int]] = [
    ("KC_SYSTEM_SLEEP", 0x0032),
    ("KC_BRIGHTNESS_UP", 0x006F),
    ("KC_BRIGHTNESS_DOWN", 0x0070),
    ("KC_MEDIA_NEXT_TRACK", 0x00B5),
    ("KC_MEDIA_PREV_TRACK", 0x00B6),
    ("KC_MEDIA_STOP", 0x00B7),
    ("KC_MEDIA_EJECT", 0x00B8),
    ("KC_MEDIA_PLAY_PAUSE", 0x00CD),
    ("KC_AUDIO_MUTE", 0x00E2),
    ("KC_AUDIO_BASS_BOOST", 0x00E5),
    ("KC_AUDIO_LOUDNESS", 0x00E7),
    ("KC_AUDIO_VOL_UP", 0x00E9),
    ("KC_AUDIO_VOL_DOWN", 0x00EA),
    ("KC_AUDIO_BASS_UP", 0x0152),
    ("KC_AUDIO_BASS_DOWN", 0x0153),
    ("KC_AUDIO_TREBLE_UP", 0x0154),
    ("KC_AUDIO_TREBLE_DOWN", 0x0155),
    ("KC_MEDIA_SELECT", 0x0183),
    ("KC_MAIL", 0x018A),
    ("KC_CALCULATOR", 0x0192),
    ("KC_MY_COMPUTER", 0x0194),
    ("KC_AC_FIND", 0x021F),
    ("KC_WWW_SEARCH", 0x0221),
    ("KC_WWW_HOME", 0x0223),
    ("KC_WWW_BACK", 0x0224),
    ("KC_WWW_FORWARD", 0x0225),
    ("KC_WWW_STOP", 0x0226),
    ("KC_WWW_REFRESH", 0x0227),
    ("KC_WWW_FAVORITES", 0x022A),
    ("KC_SCROLL_UP", 0x0233),
    ("KC_SCROLL_DOWN", 0x0234),
    ("KC_EDIT", 0x023D),
    ("KC_GLOBE", 0x029D),
    ("KC_MISSION_CONTROL", 0x029F),
    ("KC_LAUNCHPAD", 0x02A0),
]

# Keyglass's keyboard functions and their key codes.
KEYGLASS_KEYS: list[tuple[str, int]] = [
    ("KC_OF_BRIGHTNESS_UP", 0x50000002),
    ("KC_OF_BRIGHTNESS_DOWN", 0x50000003),
    ("KC_OF_PROFILE_CYCLE_NEXT", 0x50000004),
    ("KC_OF_PROFILE_CYCLE_PREV", 0x50000005),
    ("KC_OF_ACTUATION_MODE_NEXT", 0x50000006),
    ("KC_OF_ACTUATION_MODE_PREV", 0x50000007),
    ("KC_OF_PLATFORM_LINUX", 0x50000008),
    ("KC_OF_PLATFORM_MACOS", 0x50000009),
    ("KC_OF_PLATFORM_WINDOWS", 0x5000000A),
    ("KC_OF_AUTO_PROFILE_LOCK", 0x5000000B),
    ("KC_OF_START_PAUSE_POMODORO", 0x5000000C),
    ("KC_OF_RESET_POMODORO", 0x5000000D),
    ("KC_OF_THEME_CYCLE_NEXT", 0x5000000E),
    ("KC_OF_THEME_CYCLE_PREV", 0x5000000F),
]

# Other names accepted for a key code; documents are written with the name.
KEY_ALIASES: dict[str, str] = {
    "KC_ENT": "KC_ENTER",
    "KC_ESC": "KC_ESCAPE",
    "KC_BSPC": "KC_BACKSPACE",
    "KC_SPC": "KC_SPACE",
    "KC_MINS": "KC_MINUS",
    "KC_EQL": "KC_EQUAL",
    "KC_LBRC": "KC_LEFT_BRACKET",
    "KC_RBRC": "KC_RIGHT_BRACKET",
    "KC_BSLS": "KC_BACKSLASH",
    "KC_NUHS": "KC_NONUS_HASH",
    "KC_SCLN": "KC_SEMICOLON",
    "KC_QUOT": "KC_QUOTE",
    "KC_GRV": "KC_GRAVE",
    "KC_COMM": "KC_COMMA",
    "KC_SLSH": "KC_SLASH",
    "KC_CAPS": "KC_CAPS_LOCK",
    "KC_PSCR": "KC_PRINT_SCREEN",
    "KC_SCRL": "KC_SCROLL_LOCK",
    "KC_BRMD": "KC_SCROLL_LOCK",
    "KC_PAUS": "KC_PAUSE",
    "KC_BRK": "KC_PAUSE",
    "KC_BRMU": "KC_PAUSE",
    "KC_INS": "KC_INSERT",
    "KC_PGUP": "KC_PAGE_UP",
    "KC_DEL": "KC_DELETE",
    "KC_PGDN": "KC_PAGE_DOWN",
    "KC_RGHT": "KC_RIGHT",
    "KC_NUM": "KC_NUM_LOCK",
    "KC_PSLS": "KC_KP_SLASH",
    "KC_PAST": "KC_KP_ASTERISK",
    "KC_PMNS": "KC_KP_MINUS",
    "KC_PPLS": "KC_KP_PLUS",
    "KC_PENT": "KC_KP_ENTER",
    "KC_P1": "KC_KP_1",
    "KC_P2": "KC_KP_2",
    "KC_P3": "KC_KP_3",
    "KC_P4": "KC_KP_4",
    "KC_P5": "KC_KP_5",
    "KC_P6": "KC_KP_6",
    "KC_P7": "KC_KP_7",
    "KC_P8": "KC_KP_8",
    "KC_P9": "KC_KP_9",
    "KC_P0": "KC_KP_0",
    "KC_PDOT": "KC_KP_DOT",
    "KC_NUBS": "KC_NONUS_BACKSLASH",
    "KC_APP": "KC_APPLICATION",
    "KC_PEQL": "KC_KP_EQUAL",
    "KC_EXEC": "KC_EXECUTE",
    "KC_SLCT": "KC_SELECT",
    "KC_AGIN": "KC_AGAIN",
    "KC_PSTE": "KC_PASTE",
    "KC_LCAP": "KC_LOCKING_CAPS_LOCK",
    "KC_LNUM": "KC_LOCKING_NUM_LOCK",
    "KC_LSCR": "KC_LOCKING_SCROLL_LOCK",
    "KC_PCMM": "KC_KP_COMMA",
    "KC_INT1": "KC_INTERNATIONAL_1",
    "KC_INT2": "KC_INTERNATIONAL_2",
    "KC_INT3": "KC_INTERNATIONAL_3",
    "KC_INT4": "KC_INTERNATIONAL_4",
    "KC_INT5": "KC_INTERNATIONAL_5",
    "KC_INT6": "KC_INTERNATIONAL_6",
    "KC_INT7": "KC_INTERNATIONAL_7",
    "KC_INT8": "KC_INTERNATIONAL_8",
    "KC_INT9": "KC_INTERNATIONAL_9",
    "KC_LNG1": "KC_LANGUAGE_1",
    "KC_LNG2": "KC_LANGUAGE_2",
    "KC_LNG3": "KC_LANGUAGE_3",
    "KC_LNG4": "KC_LANGUAGE_4",
    "KC_LNG5": "KC_LANGUAGE_5",
    "KC_LNG6": "KC_LANGUAGE_6",
    "KC_LNG7": "KC_LANGUAGE_7",
    "KC_LNG8": "KC_LANGUAGE_8",
    "KC_LNG9": "KC_LANGUAGE_9",
    "KC_LANG1": "KC_LANGUAGE_1",
    "KC_LANG2": "KC_LANGUAGE_2",
    "KC_LANG3": "KC_LANGUAGE_3",
    "KC_LANG4": "KC_LANGUAGE_4",
    "KC_LANG5": "KC_LANGUAGE_5",
    "KC_LANG6": "KC_LANGUAGE_6",
    "KC_LANG7": "KC_LANGUAGE_7",
    "KC_LANG8": "KC_LANGUAGE_8",
    "KC_LANG9": "KC_LANGUAGE_9",
    "KC_ERAS": "KC_ALTERNATE_ERASE",
    "KC_SYRQ": "KC_SYSTEM_REQUEST",
    "KC_CNCL": "KC_CANCEL",
    "KC_CLR": "KC_CLEAR",
    "KC_PRIR": "KC_PRIOR",
    "KC_RETN": "KC_RETURN",
    "KC_SEPR": "KC_SEPARATOR",
    "KC_CLAG": "KC_CLEAR_AGAIN",
    "KC_CRSL": "KC_CRSEL",
    "KC_EXSL": "KC_EXSEL",
    "KC_LCTL": "KC_LEFT_CTRL",
    "KC_LSFT": "KC_LEFT_SHIFT",
    "KC_LSHIFT": "KC_LEFT_SHIFT",
    "KC_LALT": "KC_LEFT_ALT",
    "KC_LOPT": "KC_LEFT_ALT",
    "KC_LGUI": "KC_LEFT_GUI",
    "KC_LCMD": "KC_LEFT_GUI",
    "KC_LWIN": "KC_LEFT_GUI",
    "KC_RCTL": "KC_RIGHT_CTRL",
    "KC_RSFT": "KC_RIGHT_SHIFT",
    "KC_RSHIFT": "KC_RIGHT_SHIFT",
    "KC_RALT": "KC_RIGHT_ALT",
    "KC_ROPT": "KC_RIGHT_ALT",
    "KC_ALGR": "KC_RIGHT_ALT",
    "KC_RGUI": "KC_RIGHT_GUI",
    "KC_RCMD": "KC_RIGHT_GUI",
    "KC_RWIN": "KC_RIGHT_GUI",
    "KC_SLEP": "KC_SYSTEM_SLEEP",
    "KC_BRIU": "KC_BRIGHTNESS_UP",
    "KC_BRID": "KC_BRIGHTNESS_DOWN",
    "KC_MNXT": "KC_MEDIA_NEXT_TRACK",
    "KC_MPRV": "KC_MEDIA_PREV_TRACK",
    "KC_MSTP": "KC_MEDIA_STOP",
    "KC_EJCT": "KC_MEDIA_EJECT",
    "KC_MPLY": "KC_MEDIA_PLAY_PAUSE",
    "KC_MUTE": "KC_AUDIO_MUTE",
    "KC_BSBT": "KC_AUDIO_BASS_BOOST",
    "KC_LDNS": "KC_AUDIO_LOUDNESS",
    "KC_VOLU": "KC_AUDIO_VOL_UP",
    "KC_VOLD": "KC_AUDIO_VOL_DOWN",
    "KC_BSSU": "KC_AUDIO_BASS_UP",
    "KC_BSSD": "KC_AUDIO_BASS_DOWN",
    "KC_TRBU": "KC_AUDIO_TREBLE_UP",
    "KC_TRBD": "KC_AUDIO_TREBLE_DOWN",
    "KC_MSEL": "KC_MEDIA_SELECT",
    "KC_CALC": "KC_CALCULATOR",
    "KC_MYCM": "KC_MY_COMPUTER",
    "KC_WSCH": "KC_WWW_SEARCH",
    "KC_WHOM": "KC_WWW_HOME",
    "KC_WBAK": "KC_WWW_BACK",
    "KC_WFWD": "KC_WWW_FORWARD",
    "KC_WSTP": "KC_WWW_STOP",
    "KC_WREF": "KC_WWW_REFRESH",
    "KC_WFAV": "KC_WWW_FAVORITES",
    "KC_SCLU": "KC_SCROLL_UP",
    "KC_SCLD": "KC_SCROLL_DOWN",
    "KC_GLBE": "KC_GLOBE",
    "KC_MCTL": "KC_MISSION_CONTROL",
    "KC_LPAD": "KC_LAUNCHPAD",
    "KC_TRNS": "KC_TRANSPARENT",
}

_KEYBOARD_USAGE = dict(KEYBOARD_KEYS)
_KEYBOARD_NAME = {u: n for n, u in KEYBOARD_KEYS}
_CONSUMER_USAGE = dict(CONSUMER_CONTROLS)
_CONSUMER_NAME = {u: n for n, u in CONSUMER_CONTROLS}
_KEYGLASS_WORD = dict(KEYGLASS_KEYS)
_KEYGLASS_NAME = {w: n for n, w in KEYGLASS_KEYS}


def keycode_word(name: str, modifiers: int = 0) -> int:
    """The firmware word of a key action's `keyCode` (a name or an alias) with `modifiers`."""
    name = KEY_ALIASES.get(name, name)
    if name == "KC_TRANSPARENT":
        word = TRANSPARENT_KEYCODE
    elif name == "KC_NO":
        word = NONE_KEYCODE
    elif name in _KEYBOARD_USAGE:
        word = _KEYBOARD_USAGE[name]
    elif name in _KEYGLASS_WORD:
        word = _KEYGLASS_WORD[name]
    elif name in _CONSUMER_USAGE:
        word = CONSUMER_KIND | _CONSUMER_USAGE[name]
    else:
        raise ValueError(f"Unsupported keyCode for the firmware keymap: {name}")
    if modifiers:
        if word >> 28 not in (0x0, 0x4):
            raise ValueError("Modifiers not supported on firmware/transparent keyCode")
        if not (0 <= modifiers <= 0xFFF):
            raise ValueError(f"modifiers out of range: {modifiers}")
        word |= modifiers << 16
    return word


def keycode_name(word: int) -> tuple[str, int] | None:
    """`(keyCode, modifiers)` of a key word, or None when the word isn't a named key."""
    if word == NONE_KEYCODE:
        return ("KC_NO", 0)
    if word == TRANSPARENT_KEYCODE:
        return ("KC_TRANSPARENT", 0)
    kind = word >> 28
    if kind == 0x5:
        name = _KEYGLASS_NAME.get(word)
        return (name, 0) if name else None
    if kind in (0x0, 0x4):
        usage, mods = word & 0xFFFF, (word >> 16) & 0xFFF
        name = (_CONSUMER_NAME if kind == 0x4 else _KEYBOARD_NAME).get(usage)
        return (name, mods) if name else None
    return None


def encode_keycode(kind: str, *, code: int = 0, modifiers: int = 0, switch_type: int = 1) -> int:
    if kind == "hid":
        return ((modifiers & 0xFFF) << 16) | (code & 0xFFFF)
    if kind == "macro":
        return 0x10000000 | (code & 0xFFFF)
    if kind == "layer_switch":
        return 0x20000000 | ((switch_type & 0xF) << 24) | (code & 0xFF)
    if kind == "hid_consumer":
        return 0x40000000 | ((modifiers & 0xFFF) << 16) | (code & 0xFFFF)
    if kind == "flux_function":
        return 0x50000000 | (code & 0xFF)
    if kind == "none":
        return NONE_KEYCODE
    raise ValueError(f"Unknown keycode kind: {kind}")


def decode_keycode(code: int) -> dict[str, Any]:
    if code == NONE_KEYCODE:
        return {"kind": "none", "raw": f"0x{code:08x}"}
    top_nibble = (code >> 28) & 0xF
    if top_nibble == 0x0:
        return {
            "kind": "hid",
            "code": code & 0xFFFF,
            "modifiers": (code >> 16) & 0xFFF,
            "raw": f"0x{code:08x}",
        }
    if top_nibble == 0x1:
        return {"kind": "macro", "macro_id": code & 0xFFFF, "raw": f"0x{code:08x}"}
    if top_nibble == 0x2:
        return {
            "kind": "layer_switch",
            "switch_type": (code >> 24) & 0xF,
            "layer": code & 0xFF,
            "raw": f"0x{code:08x}",
        }
    if top_nibble == 0x4:
        return {
            "kind": "hid_consumer",
            "usage": code & 0xFFFF,
            "modifiers": (code >> 16) & 0xFFF,
            "raw": f"0x{code:08x}",
        }
    if top_nibble == 0x5:
        return {"kind": "flux_function", "fn": code & 0xFF, "raw": f"0x{code:08x}"}
    return {"kind": "unknown", "raw": f"0x{code:08x}"}


@dataclass
class KeymapSection:
    num_actuation_modes: int
    num_keys: int
    num_layers: int
    layers: list[list[int]]
    actuation_tables: list[list[int]]
    macros: list[list[int]] = field(default_factory=list)

    def to_words(self) -> list[int]:
        if self.num_keys <= 0:
            raise ValueError(f"num_keys must be positive, got {self.num_keys}")
        if len(self.layers) != self.num_layers:
            raise ValueError(f"layers count {len(self.layers)} != num_layers {self.num_layers}")
        if len(self.actuation_tables) != self.num_actuation_modes:
            raise ValueError(
                f"actuation_tables count {len(self.actuation_tables)} != num_actuation_modes {self.num_actuation_modes}"
            )
        macro_pool: list[int] = []
        macro_offsets: list[int] = []
        running = len(self.macros)
        for macro in self.macros:
            macro_offsets.append(running)
            macro_pool.append(len(macro))
            macro_pool.extend(macro)
            running += 1 + len(macro)

        macro_section_words = len(macro_offsets) + len(macro_pool)
        keycodes_offset = macro_section_words
        actuations_offset = keycodes_offset + (self.num_keys * self.num_layers)
        words = [
            self.num_actuation_modes,
            self.num_keys,
            self.num_layers,
            len(self.macros),
            len(self.macros),
            keycodes_offset,
            actuations_offset,
        ]
        if self.macros:
            words.extend(macro_offsets)
            words.extend(macro_pool)
        for layer in self.layers:
            if len(layer) != self.num_keys:
                raise ValueError(f"Layer length {len(layer)} != num_keys {self.num_keys}")
            for code in layer:
                if code != NONE_KEYCODE and ((code >> 28) & 0xF) == 0x1:
                    macro_id = code & 0xFFFF
                    if macro_id >= len(self.macros):
                        raise ValueError(
                            f"MacroKeyCode references macro_id={macro_id}, but only {len(self.macros)} macro(s) exist"
                        )
            words.extend(layer)
        for table in self.actuation_tables:
            if len(table) != self.num_keys:
                raise ValueError(f"Actuation table length {len(table)} != num_keys {self.num_keys}")
            words.extend(table)
        return words

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "KeymapSection":
        return cls(
            num_actuation_modes=int(d["num_actuation_modes"]),
            num_keys=int(d["num_keys"]),
            num_layers=int(d["num_layers"]),
            layers=[[int(v) for v in layer] for layer in d["layers"]],
            actuation_tables=[[int(v) for v in tbl] for tbl in d["actuation_tables"]],
            macros=[[int(step) for step in m] for m in d.get("macros", [])],
        )


@dataclass
class FirmwareKeymapBlob:
    # Header words: [num_layouts, layout_bitmask, 0, ANSI offset, ISO offset, JIS offset].
    # offsets_by_layout holds words 2 to 5, indexed by layout ID; encoding recomputes them.
    num_keymaps: int
    layout_bitmask: int
    offsets_by_layout: tuple[int, int, int, int] = (0, 0, 0, 0)
    keymaps: list[KeymapSection] = field(default_factory=list)
    trailing_words: list[int] = field(default_factory=list)

    def recomputed_offsets(self) -> tuple[int, int, int, int]:
        """Compute layout offsets_by_layout[0..3] from layout_bitmask and section sizes."""
        active = [lid for lid in (1, 2, 3) if (self.layout_bitmask >> (lid - 1)) & 1]
        if len(active) != len(self.keymaps):
            raise ValueError(
                f"layout_bitmask 0x{self.layout_bitmask:x} selects {len(active)} layout(s) {active}, "
                f"but keymaps has {len(self.keymaps)} section(s)"
            )
        offsets = [0, 0, 0, 0]
        running = 0
        for lid, km in zip(active, self.keymaps):
            offsets[lid] = running
            running += len(km.to_words())
        return (offsets[0], offsets[1], offsets[2], offsets[3])

    def to_bytes(self) -> bytes:
        if not self.keymaps:
            raise ValueError("At least one keymap section is required")
        if self.num_keymaps != len(self.keymaps):
            raise ValueError(f"num_keymaps {self.num_keymaps} != len(keymaps) {len(self.keymaps)}")
        off0, off1, off2, off3 = self.recomputed_offsets()
        words = [
            self.num_keymaps,
            self.layout_bitmask,
            off0,
            off1,
            off2,
            off3,
        ]
        for km in self.keymaps:
            words.extend(km.to_words())
        words.extend(self.trailing_words)
        return struct.pack(f"<{len(words)}I", *words)

    def to_dict(self) -> dict[str, Any]:
        return asdict(self)

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> "FirmwareKeymapBlob":
        return cls(
            num_keymaps=int(d["num_keymaps"]),
            layout_bitmask=int(d["layout_bitmask"]),
            offsets_by_layout=_four_offsets(d.get("offsets_by_layout", (0, 0, 0, 0))),
            keymaps=[KeymapSection.from_dict(s) for s in d["keymaps"]],
            trailing_words=[int(w) for w in d.get("trailing_words", [])],
        )


def _four_offsets(values: Any) -> tuple[int, int, int, int]:
    offsets = tuple(int(v) for v in values)
    if len(offsets) != 4:
        raise ValueError(f"Expected 4 layout offsets, got {len(offsets)}")
    return (offsets[0], offsets[1], offsets[2], offsets[3])


def decode_keymap_blob(data: bytes) -> FirmwareKeymapBlob:
    if len(data) < 24 or len(data) % 4 != 0:
        raise ValueError(f"Invalid keymap blob size: {len(data)} bytes")

    words = list(struct.unpack(f"<{len(data) // 4}I", data))
    num_keymaps, layout_bitmask, off0, off1, off2, off3 = words[:6]

    pos = 6
    keymaps: list[KeymapSection] = []
    if num_keymaps == 0 or num_keymaps > 16:
        raise ValueError(f"Invalid num_keymaps: {num_keymaps}")
    for idx in range(num_keymaps):
        if pos + 7 > len(words):
            raise ValueError(f"Truncated keymap[{idx}] header at word {pos}")
        n_act, n_keys, n_layers, n_macros, _n_macros_again, keycodes_off, actuations_off = words[pos : pos + 7]
        pos += 7
        if n_keys == 0 or n_keys > 512 or n_layers > 64 or n_act > 64 or n_macros > 512:
            raise ValueError(
                f"Invalid keymap[{idx}] dimensions: n_keys={n_keys}, n_layers={n_layers}, n_act={n_act}, n_macros={n_macros}"
            )
        if actuations_off != keycodes_off + n_keys * n_layers:
            raise ValueError(
                f"Keymap[{idx}] actuations_off={actuations_off} != keycodes_off({keycodes_off}) + {n_keys}*{n_layers}"
            )
        if n_macros == 0 and keycodes_off != 0:
            raise ValueError(
                f"Keymap[{idx}] has n_macros=0 but nonzero keycodes_off={keycodes_off}"
            )
        if n_macros > 0 and keycodes_off < n_macros:
            raise ValueError(
                f"Keymap[{idx}] has n_macros={n_macros} but keycodes_off={keycodes_off} < n_macros"
            )

        macros: list[list[int]] = []
        if keycodes_off > 0:
            if pos + keycodes_off > len(words):
                raise ValueError(f"Truncated keymap[{idx}] macro pool at word {pos}")
            pool_words = words[pos : pos + keycodes_off]
            if n_macros > len(pool_words):
                raise ValueError(f"Keymap[{idx}] n_macros={n_macros} exceeds macro pool {len(pool_words)}")
            macro_offsets = pool_words[:n_macros]
            m_pos = n_macros
            for m_idx in range(n_macros):
                if macro_offsets[m_idx] != m_pos:
                    raise ValueError(
                        f"Keymap[{idx}] macro_offsets[{m_idx}]={macro_offsets[m_idx]} != expected word offset {m_pos}"
                    )
                if m_pos >= len(pool_words):
                    raise ValueError(f"Truncated macro[{m_idx}] length in keymap[{idx}]")
                m_len = pool_words[m_pos]
                m_pos += 1
                if m_pos + m_len > len(pool_words):
                    raise ValueError(f"Truncated macro[{m_idx}] steps in keymap[{idx}]")
                macros.append(pool_words[m_pos : m_pos + m_len])
                m_pos += m_len
            if m_pos != len(pool_words):
                raise ValueError(
                    f"Keymap[{idx}] macro pool has trailing/unreferenced words: parsed {m_pos} != {len(pool_words)}"
                )
            pos += keycodes_off

        expected_tail = n_keys * n_layers + n_act * n_keys
        if pos + expected_tail > len(words):
            raise ValueError(
                f"Truncated keymap[{idx}] body: need {expected_tail} words at {pos}, have {len(words) - pos}"
            )
        layers = [words[pos + l * n_keys : pos + (l + 1) * n_keys] for l in range(n_layers)]
        for l_idx, layer in enumerate(layers):
            for k_idx, code in enumerate(layer):
                if code != NONE_KEYCODE and ((code >> 28) & 0xF) == 0x1:
                    macro_id = code & 0xFFFF
                    if macro_id >= n_macros:
                        raise ValueError(
                            f"Keymap[{idx}] layer[{l_idx}][{k_idx}] references macro_id={macro_id} >= n_macros={n_macros}"
                        )
        pos += n_keys * n_layers
        act_tables = [words[pos + m * n_keys : pos + (m + 1) * n_keys] for m in range(n_act)]
        pos += n_act * n_keys

        keymaps.append(
            KeymapSection(
                num_actuation_modes=n_act,
                num_keys=n_keys,
                num_layers=n_layers,
                layers=layers,
                actuation_tables=act_tables,
                macros=macros,
            )
        )

    trailing = words[pos:]
    blob = FirmwareKeymapBlob(
        num_keymaps=num_keymaps,
        layout_bitmask=layout_bitmask,
        offsets_by_layout=(off0, off1, off2, off3),
        keymaps=keymaps,
        trailing_words=trailing,
    )
    expected_offsets = blob.recomputed_offsets()
    if (off0, off1, off2, off3) != expected_offsets:
        raise ValueError(
            f"Keymap list offsets {(off0, off1, off2, off3)} != expected {expected_offsets} "
            f"for layout_bitmask=0x{layout_bitmask:x}"
        )
    return blob


def _dominant_table_word(table: list[int]) -> int:
    """The most common word of a table; the one seen first on a tie."""
    counts: dict[int, tuple[int, int]] = {}
    for idx, w in enumerate(table):
        c, first = counts.get(w, (0, idx))
        counts[w] = (c + 1, first)
    if not counts:
        return 0
    return max(counts.items(), key=lambda kv: (kv[1][0], -kv[1][1], kv[0]))[0]


def decode_actuation_modes(blob: FirmwareKeymapBlob) -> list[dict[str, Any]]:
    """The keymap document's `actuationModes` for a blob, as the daemon decodes them.

    Each mode is its first layout's most common word; keys of any layout with another word are
    per-key overrides under `ansi` or `iso`. Everything is marked `touched: false`.
    """
    sec0 = blob.keymaps[0]
    out: list[dict[str, Any]] = []
    for idx, tbl0 in enumerate(sec0.actuation_tables):
        first = _dominant_table_word(tbl0)
        mixed = False
        scoped: dict[str, dict[str, Any]] = {}
        for sec in blob.keymaps:
            if idx >= len(sec.actuation_tables):
                continue
            ov = {str(k): actuation_entry_json(w) for k, w in enumerate(sec.actuation_tables[idx]) if w != first}
            if ov:
                mixed = True
                scoped["iso" if sec.num_keys == 87 else "ansi"] = ov
        name = {0: "Typing", 1: "Gaming"}.get(idx, f"Mode {idx + 1}")
        mode = actuation_entry_json(first)
        mode.update(
            {
                "id": f"mode_{idx}",
                "name": name,
                "type": decode_actuation_word(first)["actuation_type"],
                "mixedPerKey": mixed,
                "perKeyOverrides": scoped,
            }
        )
        out.append(mode)
    return out


if __name__ == "__main__":
    import sys

    blob = decode_keymap_blob(Path(sys.argv[1]).read_bytes())
    print(json.dumps(blob.to_dict(), indent=2))
