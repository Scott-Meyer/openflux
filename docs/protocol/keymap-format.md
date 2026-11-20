# Keymap and profile settings

Interfaces: `keymap` and `profile-settings` in `compat.toml`, both 1.0.

A keymap exists in two forms. The **keymap document** is JSON: the host and
the display edit and read it, and it is stored on the keyboard as
`keymap.json`. The **keymap blob** is the binary form the firmware stores and
scans with. The host converts the document to a blob and sends it with the
firmware's `SEND_KEYMAP` command. **Profile settings** are a small binary
table, sent with `FILE_FROM_HOST`, that tells the firmware which actuation
mode and default layer each profile uses.

Implementations: `host/daemon/src/codecs/keymap.rs` (both directions),
`keyboard/firmware/src/fw_scanner.c` (validation and use),
`host/daemon/src/defaults.rs` (the default keymap).

## Keys and layouts

The keyboard has six rows of keys. The physical layout depends on the
attached keyframe:

| Layout | ID | Keys | Keys per row, top row first |
|---|---|---|---|
| ANSI | 1 | 85 | 16, 15, 15, 14, 14, 11 |
| ISO | 2 | 87 | 16, 15, 15, 15, 15, 11 |
| JIS | 3 | 90 | 16, 16, 15, 15, 15, 13 |

Keys are numbered from 0, left to right within a row and rows top to
bottom. Every per-key list in either form uses this order.

## Keymap document

```json
{
  "selectedFrameLayoutType": "ansi",
  "keymaps": {
    "ansi": {
      "name": "OpenFlux ANSI",
      "frameLayoutType": "ansi",
      "defaultLayer": 0,
      "layers": [
        {"id": "…", "name": "Base", "frameLayoutType": "ansi", "keyActions": [ … ]},
        {"id": "…", "name": "FN", "frameLayoutType": "ansi", "keyActions": [ … ]}
      ]
    },
    "iso": { … }
  },
  "macros": [ … ]
}
```

`keymaps` has one entry per layout (`ansi`, `iso`), each with its layers in
order. JIS exists in the blob format only; the keymap document has no `jis`
entry. Each layer's `keyActions` has one entry per key. A key action is one
of:

| `type` | Fields | Meaning |
|---|---|---|
| 0 | `keyCode`, `modifiers` (bitmask or `null`) | send a key, a consumer control or an OpenFlux function |
| 1 | `macroId` | play the macro with that index |
| 2 | `layerControlType` (`momentaryLayer`, `toggleLayer`, `switchToLayer`), `targetLayer` | change layers |

`macros` is a list of `{name, steps}`, each step `{keyCode, delayMs,
isDown}`. When the layouts have different macros, `macros` is an object with
one list per layout name.

Key codes use QMK's full names, of the form `KC_<name>`: every key of the
HID Keyboard/Keypad page from `KC_A` to `KC_EXSEL` (letters, digits,
punctuation, `KC_F1` to `KC_F24`, editing and navigation keys such as
`KC_INSERT`, `KC_PRINT_SCREEN`, `KC_SCROLL_LOCK` and `KC_PAUSE`, the keypad
`KC_NUM_LOCK` and `KC_KP_*`, the ISO keys `KC_NONUS_BACKSLASH` and
`KC_NONUS_HASH`, `KC_APPLICATION` and `KC_MENU`, `KC_INTERNATIONAL_1` to `_9`,
`KC_LANGUAGE_1` to `_9`, and the eight modifiers `KC_LEFT_CTRL` …
`KC_RIGHT_GUI`), the consumer controls below, `KC_TRANSPARENT`, `KC_NO`, and
the OpenFlux functions below. QMK's short and older names (`KC_INS`,
`KC_PSCR`, `KC_P1`, `KC_INT1`, `KC_LNG1`, `KC_LANG1`, `KC_LCTL`, `KC_MUTE`, …)
are accepted too; documents OpenFlux writes use the full names. The tables,
with every alias and the word of every name, are in
`host/daemon/src/codecs/keycodes.rs` and
`host/daemon/tests/fixtures/keycodes.json`.

The actuation modes stored alongside a keymap come from the host
configuration's `actuationModes` list:

```json
{"id": "mode_0", "name": "Typing", "type": 1, "actuationPointMm": 2.0,
 "releasePointMm": 1.8, "rapidTrigger": false}
{"id": "mode_1", "name": "Gaming", "type": 2, "actuationPointMm": 1.2,
 "rapidTrigger": true, "rapidTriggerSensitivityMm": 0.3}
```

A fixed mode's `releasePointMm` is where a held key releases; it is above
`actuationPointMm`. See [actuation words](#actuation-words).

`perKeyOverrides` can give individual keys their own values, keyed by layout
name and then key number.

## Keymap blob

The blob is a sequence of little-endian `u32` words; its length is a
multiple of 4 and at most 16384 bytes.

### Header (6 words)

| Word | Field |
|---|---|
| 0 | number of layouts in the blob, 1 to 3 |
| 1 | layout mask: bit `id - 1` set for every layout present |
| 2 | 0 |
| 3 to 5 | offset of the ANSI, ISO and JIS section, in words from word 6; 0 for a layout that is absent |

Sections follow the header in layout ID order, with no gaps: the first
present layout is at offset 0 and each following one starts where the
previous one ends.

### Section

| Word | Field |
|---|---|
| 0 | `L`: number of layers, 1 to 8 |
| 1 | `K`: number of keys, at most 96 |
| 2 | `A`: number of actuation modes, 1 to 8 |
| 3 | `M`: number of macros, at most 512 |
| 4 | the number of macros again; readers ignore it |
| 5 | `kc`: offset of the key codes, in words from word 7 of the section |
| 6 | `act`: offset of the actuation table, in words from word 7; always `kc + K × L` |
| 7 … | macro pool, `kc` words |
| `7 + kc` … | key codes, `L × K` words: layer 0 for every key, then layer 1, … |
| `7 + act` … | actuation table, `A × K` words: mode 0 for every key, then mode 1, … |

The macro pool begins with `M` words giving each macro's offset within the
pool, followed by the macros in order, each a length word and that many
step words. The first macro's offset is therefore `M`. Without macros the
pool is empty and `kc` is 0.

A macro step is `(down << 31) | (delay_ms << 8) | usage`: `down` is 1 for a
key press and 0 for a release, `delay_ms` (16 bits) is the wait before the
step, and `usage` is a HID keyboard usage.

The firmware refuses a blob unless every count is within range, every
offset is consistent, every macro offset points at the next macro, and the
sections fill the blob exactly.

### Key codes

The top four bits of a key code select its kind:

| Top bits | Kind | Encoding |
|---|---|---|
| `0x0` | keyboard key | bits 0–15: HID keyboard usage; bits 16–23: modifiers to add (bit 0 Left Ctrl … bit 7 Right GUI) |
| `0x1` | macro | bits 0–15: macro index |
| `0x2` | layer key | bits 24–27: 1 momentary, 2 toggle, 3 switch to; bits 0–7: target layer |
| `0x4` | consumer control | bits 0–15: HID Consumer Page usage (page `0x0C`), sent as is in the consumer report |
| `0x5` | OpenFlux function | bits 0–7: function, below |
| `0xF` | none | exactly `0xF0000000` |

A word of 0 also does nothing.

Consumer controls with a name:

| Name | Usage | Word |
|---|---|---|
| `KC_SYSTEM_SLEEP` | `0x0032` | `0x40000032` |
| `KC_BRIGHTNESS_UP` | `0x006F` | `0x4000006F` |
| `KC_BRIGHTNESS_DOWN` | `0x0070` | `0x40000070` |
| `KC_MEDIA_NEXT_TRACK` | `0x00B5` | `0x400000B5` |
| `KC_MEDIA_PREV_TRACK` | `0x00B6` | `0x400000B6` |
| `KC_MEDIA_STOP` | `0x00B7` | `0x400000B7` |
| `KC_MEDIA_EJECT` | `0x00B8` | `0x400000B8` |
| `KC_MEDIA_PLAY_PAUSE` | `0x00CD` | `0x400000CD` |
| `KC_AUDIO_MUTE` | `0x00E2` | `0x400000E2` |
| `KC_AUDIO_BASS_BOOST` | `0x00E5` | `0x400000E5` |
| `KC_AUDIO_LOUDNESS` | `0x00E7` | `0x400000E7` |
| `KC_AUDIO_VOL_UP` | `0x00E9` | `0x400000E9` |
| `KC_AUDIO_VOL_DOWN` | `0x00EA` | `0x400000EA` |
| `KC_AUDIO_BASS_UP` | `0x0152` | `0x40000152` |
| `KC_AUDIO_BASS_DOWN` | `0x0153` | `0x40000153` |
| `KC_AUDIO_TREBLE_UP` | `0x0154` | `0x40000154` |
| `KC_AUDIO_TREBLE_DOWN` | `0x0155` | `0x40000155` |
| `KC_MEDIA_SELECT` | `0x0183` | `0x40000183` |
| `KC_MAIL` | `0x018A` | `0x4000018A` |
| `KC_CALCULATOR` | `0x0192` | `0x40000192` |
| `KC_MY_COMPUTER` | `0x0194` | `0x40000194` |
| `KC_AC_FIND` | `0x021F` | `0x4000021F` |
| `KC_WWW_SEARCH` | `0x0221` | `0x40000221` |
| `KC_WWW_HOME` | `0x0223` | `0x40000223` |
| `KC_WWW_BACK` | `0x0224` | `0x40000224` |
| `KC_WWW_FORWARD` | `0x0225` | `0x40000225` |
| `KC_WWW_STOP` | `0x0226` | `0x40000226` |
| `KC_WWW_REFRESH` | `0x0227` | `0x40000227` |
| `KC_WWW_FAVORITES` | `0x022A` | `0x4000022A` |
| `KC_SCROLL_UP` | `0x0233` | `0x40000233` |
| `KC_SCROLL_DOWN` | `0x0234` | `0x40000234` |
| `KC_EDIT` | `0x023D` | `0x4000023D` |
| `KC_GLOBE` | `0x029D` | `0x4000029D` |
| `KC_MISSION_CONTROL` | `0x029F` | `0x4000029F` |
| `KC_LAUNCHPAD` | `0x02A0` | `0x400002A0` |

| Function | Name | | Function | Name |
|---|---|---|---|---|
| 0 | `KC_TRANSPARENT` | | 8 | `KC_OF_PLATFORM_LINUX` |
| 2 | `KC_OF_BRIGHTNESS_UP` | | 9 | `KC_OF_PLATFORM_MACOS` |
| 3 | `KC_OF_BRIGHTNESS_DOWN` | | 10 | `KC_OF_PLATFORM_WINDOWS` |
| 4 | `KC_OF_PROFILE_CYCLE_NEXT` | | 11 | `KC_OF_AUTO_PROFILE_LOCK` |
| 5 | `KC_OF_PROFILE_CYCLE_PREV` | | 12 | `KC_OF_START_PAUSE_POMODORO` |
| 6 | `KC_OF_ACTUATION_MODE_NEXT` | | 13 | `KC_OF_RESET_POMODORO` |
| 7 | `KC_OF_ACTUATION_MODE_PREV` | | 14 | `KC_OF_THEME_CYCLE_NEXT` |
| | | | 15 | `KC_OF_THEME_CYCLE_PREV` |

Each newly pressed key is looked up from the highest active layer down,
falling through only on `KC_TRANSPARENT`. The active layers are the profile's
default layer and the layers that layer keys have turned on. Keyboard keys and
modifiers are sent in the keyboard report, consumer controls in the consumer
report (see [usb.md](usb.md)). OpenFlux functions are handled where they
belong:

- profile, actuation-mode and platform keys by the firmware, which reports
  the result to the bridge and the display in the key-state message;
- brightness, theme and Pomodoro keys by the display, which resolves the
  pressed keys against `keymap.json` with the same layer rules;
- `KC_OF_AUTO_PROFILE_LOCK` by the host, which reads it from vendor HID
  interface 4.

### Actuation words

Each actuation table entry describes when one key triggers in one mode:

```
(type << 30) | (R << 12) | A
```

| Field | Bits | |
|---|---|---|
| `type` | 30–31 | 1 fixed actuation point, 2 rapid trigger; 0 and 3 select the firmware's default fixed setting |
| `R` | 12–23 | fixed: the release depth, how far the reading falls below `A` before a held key releases; rapid trigger: how far the reading must fall from its peak to release |
| `A` | 0–11 | the reading above which the key presses |

`A` and `R` are calibrated key-signal units, 0 to 4095, not raw ADC values
or distances.

A fixed key becomes a press candidate when its reading `F` rises above `A`.
Once held, it releases when `F <= A - R`. `R` is a depth below the
actuation reading, not a release position, and `R = 0` means no hysteresis.
The firmware scales `A` and `R` by the same per-position gain (Q8, applied as
`× gain >> 8`) before comparing. A type 0 or 3 entry, such as an all-zero
word, uses `A = 650` and `R = 216` before scaling.

The host converts key travel to readings in steps of 0.1 mm with this table,
rounding travel down to the step and limiting it to 0–2.8 mm:

| mm | reading | mm | reading | mm | reading |
|---|---|---|---|---|---|
| 0.0 | `0x000` | 1.0 | `0x1FD` | 2.0 | `0x402` |
| 0.1 | `0x0AB` | 1.1 | `0x228` | 2.1 | `0x444` |
| 0.2 | `0x0BE` | 1.2 | `0x253` | 2.2 | `0x48A` |
| 0.3 | `0x0DE` | 1.3 | `0x281` | 2.3 | `0x4D1` |
| 0.4 | `0x0FE` | 1.4 | `0x2B3` | 2.4 | `0x518` |
| 0.5 | `0x126` | 1.5 | `0x2E5` | 2.5 | `0x561` |
| 0.6 | `0x150` | 1.6 | `0x318` | 2.6 | `0x5AB` |
| 0.7 | `0x17A` | 1.7 | `0x34C` | 2.7 | `0x5D7` |
| 0.8 | `0x1A6` | 1.8 | `0x381` | 2.8 | `0x613` |
| 0.9 | `0x1D1` | 1.9 | `0x3C0` | | |

A fixed mode has an actuation point and a release point in mm of travel. The
release point is above the actuation point (less travel). The host stores

```
A = reading(actuation point)
R = reading(actuation point) - reading(release point)
```

limited to `0 <= R <= A`: a release point as deep as the actuation point or
deeper gives `R = 0`, and a release point at 0.0 mm gives `R = A`. A fixed mode
without a release point releases 0.2 mm above its actuation point.

Reading a word back, the host shows the 0.1 mm steps nearest to `A` and to
`A - R`, and decodes a type 0 or 3 entry as the default `A = 650`,
`R = 216` (about 1.3 mm, releasing at about 0.8 mm), marked
`firmwareDefault`. When those steps don't encode back to the same word, the
entry carries the word as `firmwareWord`, and the host writes that word
unchanged for as long as the entry's travel settings stay the same.

For rapid trigger, `R` holds the sensitivity converted with the same table.

## Profile settings

The firmware keeps up to 64 profiles. For each it stores the actuation mode
and the default layers; `SET_ACTIVE_PROFILE` selects one.

`FILE_FROM_HOST` sends:

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | file version, 0 |
| 4 | 4 | file type, 0 (profile settings) |
| 8 | 8 | zero |
| 16 | 4 | `N`: number of profiles, at most 64 |
| 20 | 12 | zero |
| 32 | `8 × N` | per profile: `u32` actuation mode index, `u32` default layer mask (`1 << layer`) |

The length must be exactly `32 + 8 × N`. `FILE_TO_HOST` returns the same
bytes from offset 16 on, without the 16-byte file header.

When a profile becomes active, its actuation mode becomes the active mode,
and its default layer mask, if not 0, becomes the active layers. An
actuation mode index beyond the keymap's modes selects mode 0.
