# Firmware storage and calibration

Interface: `calibration-lfs` in `compat.toml`, 1.0.

Each keyboard is calibrated individually: every Hall-effect sensor has its
own scaling factor, measured for that unit. The factors are stored in the
key scanner's flash, in a littlefs file system that also holds the keymap
and the firmware's settings. Firmware updates never touch this file system,
and the firmware never writes the calibration files.

Implementations: `keyboard/firmware/src/lfs_store.c`, `fw_calibration.c`
and `fw_rest.c` (rest reference); `host/daemon/src/usb/picoboot.rs`
(protected region during flashing).

## Flash layout

The key scanner has 16 MiB of external flash, mapped at `0x10000000`.

| Flash offset | Address | Size | Contents |
|---|---|---|---|
| `0x000000` | `0x10000000` | 15 MiB | firmware image |
| `0xF00000` | `0x10F00000` | 1 MiB | littlefs file system |

littlefs configuration:

| Parameter | Value |
|---|---|
| block size | 4096 bytes |
| block count | 256 |
| read size | 1 byte |
| program size | 256 bytes |
| cache size | 4096 bytes |
| lookahead size | 16 bytes |
| block cycles | 500 |

The firmware mounts the file system at start and never formats it. A file
system formatted with 255 blocks mounts as well. If it cannot be mounted,
the firmware runs with defaults and stores nothing, so a damaged file system
is left as it is for recovery.

## Files

| Name | Size | Written by | Contents |
|---|---|---|---|
| `factory_calibration_info` | 20 | calibration only | calibration header, below |
| `factory_scaling_factors` | 320 | calibration only | 160 × `u16` scaling factors |
| `keymap` | up to 16384 | `SEND_KEYMAP` | the keymap blob ([keymap-format.md](keymap-format.md)) |
| `profile_settings_v1` | `16 + 8 × N` | `FILE_FROM_HOST` | profile settings without the file header |
| `language_code` | 4 | `SET_LANGUAGE` | `u32` |
| `platform_type_code` | 4 | `SET_PLATFORM` | `u32` |
| `press_refinement_v1` | 1472 | `PUT_PRESS_REFINEMENT` | optional [press-refinement record](usb.md#optional-press-refinement-data), separate from per-unit sensor calibration |
| `rest_reference_<layout>_<feel>` | 236 | the firmware | resting sensor readings for one keyframe layout and switch feel, below |

All integers are little-endian. The firmware refuses to write either
calibration file.

### Calibration

`factory_calibration_info`, five `u32`:

| Offset | Field |
|---|---|
| 0 | magic, `0xFAC5CA1E` |
| 4 | version |
| 8 | number of sensors, 160 |
| 12 | reserved |
| 16 | checksum: the sum of the 160 factors, as `u32` |

`factory_scaling_factors` holds one `u16` per sensor slot, in Q8 fixed point
(`0x0100` is 1.0). For calibration version 2, each factor is used multiplied
by `0xAA / 0x100`; other versions use the factors as stored.

The calibration is used only when both files have the expected sizes, the
magic and sensor count match and the checksum is correct. Otherwise every
factor is 1.0.

The firmware turns a raw reading into a calibrated one as
`((raw - baseline) × factor) >> 8`, where `baseline` is the sensor's resting
reading.

### Rest reference

On keyframe attachment or a change of layout or switch feel, the firmware
measures eight sweeps. It compares the average to a saved reference for that
layout and feel after accounting for thermal drift. A held switch can then be
recognized without learning its displaced sensor values as the new baseline.
A reference inconsistent with more than 30% of the keys is not used.

Measuring a baseline does **not** write it to flash. The firmware starts
collecting rest snapshots after eight minutes without a pressed key, with
at least two minutes between snapshots. Once it has nine, their per-channel
median refreshes the baseline. A clean refresh writes the rest reference at
most once per keyframe attachment.

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | magic, `0x52455354` |
| 4 | 2 | version, 1 |
| 6 | 1 | keyframe layout |
| 7 | 1 | switch feel |
| 8 | 224 | 112 × `i16` resting readings, sensor slots 0 to 111 |
| 232 | 4 | rolling `u32` checksum: start with `magic + version + layout + feel`, then `h = h × 31 + sample` for the 112 samples interpreted as `u16` |

`<layout>` and `<feel>` in the file name are single decimal digits. A file
whose magic, version, layout, feel or checksum does not match is ignored.
The reader also accepts earlier Keyglass rest-reference records with a
rolling checksum over bytes 0 through 231 starting at zero, subject to the
same layout and feel checks. Loading one does not rewrite it or associate it
with another switch feel.

## Flashing firmware

The host writes firmware through the RP2040 boot ROM and keeps the file
system intact:

- A UF2 image is refused if any block lies outside the firmware image region
  (at or above `0x10F00000`), or if erasing the sectors it covers would reach
  that address.
- The host reads the whole file system region before flashing and again
  afterwards, and reports a failure if its SHA-256 changed.

Because flashing leaves the file system untouched, a keyboard keeps its
calibration when it is converted to Keyglass, updated, or returned to its
factory software.
