#!/usr/bin/env python3
"""Generate default XZ-compressed Vial keyboard definition header for OpenFlux (ANSI / ISO / JIS)."""
import json
import lzma
import pathlib
import sys

# Default built-in OpenFlux actions mapped to QK_KB_0..13 (matching fw_protocol.h 0x02..0x0F).
DEFAULT_CUSTOM_KEYCODES = [
    {"name": "Screen\nBri+", "title": "Increase keyboard display brightness", "shortName": "OF_BRI+"},
    {"name": "Screen\nBri-", "title": "Decrease keyboard display brightness", "shortName": "OF_BRI-"},
    {"name": "Profile\nNext", "title": "Switch to next OpenFlux profile", "shortName": "OF_PRF+"},
    {"name": "Profile\nPrev", "title": "Switch to previous OpenFlux profile", "shortName": "OF_PRF-"},
    {"name": "Act\nNext", "title": "Cycle to next actuation mode", "shortName": "OF_ACT+"},
    {"name": "Act\nPrev", "title": "Cycle to previous actuation mode", "shortName": "OF_ACT-"},
    {"name": "Linux", "title": "Switch platform mode to Linux", "shortName": "OF_LNX"},
    {"name": "macOS", "title": "Switch platform mode to macOS", "shortName": "OF_MAC"},
    {"name": "Windows", "title": "Switch platform mode to Windows", "shortName": "OF_WIN"},
    {"name": "Profile\nLock", "title": "Toggle automatic profile switching lock", "shortName": "OF_LOCK"},
    {"name": "Pomo\nStart", "title": "Start or pause Pomodoro timer", "shortName": "OF_POMO"},
    {"name": "Pomo\nReset", "title": "Reset Pomodoro timer", "shortName": "OF_PRST"},
    {"name": "Theme\nNext", "title": "Switch to next display theme", "shortName": "OF_THM+"},
    {"name": "Theme\nPrev", "title": "Switch to previous display theme", "shortName": "OF_THM-"},
]

# OpenFlux 6x16 KLE geometry with Vial Layout Option 0: ["Physical Layout", "ANSI", "ISO", "JIS"].
# Covers all 85 ANSI, 87 ISO, and 90 JIS physical keys cleanly.
KEYMAP = [
    ["0,0", "0,1", "0,2", "0,3", "0,4", "0,5", "0,6", "0,7", "0,8", "0,9", "0,10", "0,11", "0,12", "0,13", "0,14", "0,15"],
    [
        "1,0", "1,1", "1,2", "1,3", "1,4", "1,5", "1,6", "1,7", "1,8", "1,9", "1,10", "1,11", "1,12",
        {"w": 2}, "1,13\n\n\n0,0", "1,14\n\n\n0,0",
        {"x": -3, "w": 2}, "1,13\n\n\n0,1", "1,14\n\n\n0,1",
        {"x": -3}, "1,13\n\n\n0,2", "1,14\n\n\n0,2", "1,15\n\n\n0,2",
    ],
    [{"w": 1.5}, "2,0", "2,1", "2,2", "2,3", "2,4", "2,5", "2,6", "2,7", "2,8", "2,9", "2,10", "2,11", "2,12", {"w": 1.5}, "2,13", "2,14"],
    [
        {"w": 1.75}, "3,0", "3,1", "3,2", "3,3", "3,4", "3,5", "3,6", "3,7", "3,8", "3,9", "3,10", "3,11",
        {"w": 2.25}, "3,12\n\n\n0,0", "3,13\n\n\n0,0",
        {"x": -3.25}, "3,12\n\n\n0,1", {"w": 1.25}, "3,13\n\n\n0,1", "3,14\n\n\n0,1",
        {"x": -3.25}, "3,12\n\n\n0,2", {"w": 1.25}, "3,13\n\n\n0,2", "3,14\n\n\n0,2",
    ],
    [
        {"w": 2.25}, "4,0\n\n\n0,0", "4,1\n\n\n0,0", "4,2\n\n\n0,0", "4,3\n\n\n0,0", "4,4\n\n\n0,0",
        "4,5\n\n\n0,0", "4,6\n\n\n0,0", "4,7\n\n\n0,0", "4,8\n\n\n0,0", "4,9\n\n\n0,0", "4,10\n\n\n0,0",
        {"w": 1.75}, "4,11\n\n\n0,0", "4,12\n\n\n0,0", "4,13\n\n\n0,0",
        {"x": -16, "w": 1.25}, "4,0\n\n\n0,1", "4,1\n\n\n0,1", "4,2\n\n\n0,1", "4,3\n\n\n0,1", "4,4\n\n\n0,1",
        "4,5\n\n\n0,1", "4,6\n\n\n0,1", "4,7\n\n\n0,1", "4,8\n\n\n0,1", "4,9\n\n\n0,1", "4,10\n\n\n0,1",
        "4,11\n\n\n0,1", {"w": 1.75}, "4,12\n\n\n0,1", "4,13\n\n\n0,1", "4,14\n\n\n0,1",
        {"x": -16, "w": 2.25}, "4,0\n\n\n0,2", "4,1\n\n\n0,2", "4,2\n\n\n0,2", "4,3\n\n\n0,2", "4,4\n\n\n0,2",
        "4,5\n\n\n0,2", "4,6\n\n\n0,2", "4,7\n\n\n0,2", "4,8\n\n\n0,2", "4,9\n\n\n0,2", "4,10\n\n\n0,2",
        "4,11\n\n\n0,2", {"w": 0.75}, "4,12\n\n\n0,2", "4,13\n\n\n0,2", "4,14\n\n\n0,2",
    ],
    [
        {"w": 1.25}, "5,0\n\n\n0,0", {"w": 1.25}, "5,1\n\n\n0,0", {"w": 1.25}, "5,2\n\n\n0,0", {"w": 1.25}, "5,3\n\n\n0,0",
        {"w": 5}, "5,4\n\n\n0,0", "5,5\n\n\n0,0", "5,6\n\n\n0,0", "5,7\n\n\n0,0", "5,8\n\n\n0,0", "5,9\n\n\n0,0", "5,10\n\n\n0,0",
        {"x": -16, "w": 1.25}, "5,0\n\n\n0,1", {"w": 1.25}, "5,1\n\n\n0,1", {"w": 1.25}, "5,2\n\n\n0,1", {"w": 1.25}, "5,3\n\n\n0,1",
        {"w": 5}, "5,4\n\n\n0,1", "5,5\n\n\n0,1", "5,6\n\n\n0,1", "5,7\n\n\n0,1", "5,8\n\n\n0,1", "5,9\n\n\n0,1", "5,10\n\n\n0,1",
        {"x": -16}, "5,0\n\n\n0,2", "5,1\n\n\n0,2", "5,2\n\n\n0,2", "5,3\n\n\n0,2", "5,4\n\n\n0,2",
        {"w": 3}, "5,5\n\n\n0,2", "5,6\n\n\n0,2", "5,7\n\n\n0,2", "5,8\n\n\n0,2", "5,9\n\n\n0,2", "5,10\n\n\n0,2", "5,11\n\n\n0,2", "5,12\n\n\n0,2",
    ],
]

VIAL_DEF = {
    "name": "OpenFlux Keyboard",
    "vendorId": "0x3761",
    "productId": "0x4015",
    "matrix": {"rows": 6, "cols": 16},
    "layouts": {
        "labels": [["Physical Layout", "ANSI", "ISO", "JIS"]],
        "keymap": KEYMAP,
    },
    "customKeycodes": DEFAULT_CUSTOM_KEYCODES,
}


def main() -> int:
    out_path = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else pathlib.Path(
        "keyboard/firmware/src/qmk/qmk_platform/vial_generated_keyboard_definition.h"
    )
    raw = json.dumps(VIAL_DEF, separators=(",", ":")).encode("utf-8")
    compressed = lzma.compress(raw, format=lzma.FORMAT_XZ, preset=6)

    lines = [
        "/* Auto-generated by keyboard/firmware/tools/gen_vial_def.py — do not hand-edit. */",
        "#pragma once",
        "#include <stdint.h>",
        "",
        f"/* Uncompressed: {len(raw)} bytes, XZ compressed: {len(compressed)} bytes */",
        "static const uint8_t keyboard_definition[] = {",
    ]
    for i in range(0, len(compressed), 16):
        chunk = ", ".join(f"0x{b:02X}" for b in compressed[i : i + 16])
        lines.append(f"    {chunk},")
    lines.append("};")
    lines.append("")

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text("\n".join(lines), encoding="utf-8")
    print(f"Wrote {out_path} ({len(raw)} -> {len(compressed)} bytes XZ)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
