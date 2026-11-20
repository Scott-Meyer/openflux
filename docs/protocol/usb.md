# USB command protocol

Interfaces in `compat.toml`: `bridge-usb` 1.0 and `firmware-usb` 1.1.

The keyboard connects to the computer through an internal USB hub with two
devices behind it: the **bridge**, a USB gadget run by the keyboard's
computer, and the **firmware**, the RP2040 key scanner. Both answer the same
command framing on a bulk interface; they differ in packet size and
command set.

Implementations: `host/daemon/src/usb/protocol.rs` (host side),
`keyboard/bridge/src/usb_commands.rs` and `usb_rx.rs` (bridge),
`keyboard/firmware/src/fw_usb_cmd.c` (firmware).

## Devices

| | Bridge | Firmware |
|---|---|---|
| USB VID | `0x3761` | `0x3761` |
| USB PID | `0x0104` | `0x4015` |
| Manufacturer string | `OpenFlux` | `OpenFlux` |
| Product string | `OpenFlux Keyboard` | `OpenFlux Firmware` |
| Command interface | 0 (FunctionFS) | 1 (class `0xFF`) |
| Bulk OUT / IN | `0x01` / `0x81` | `0x01` / `0x84` |
| Packet size | 512 bytes (high speed) | 64 bytes (full speed) |

The firmware is a composite device. Besides the command interface it has:

| Interface | Class | Endpoints | Use |
|---|---|---|---|
| 0 | HID, boot keyboard protocol | IN `0x83` | Report ID 1: NKRO keyboard (1 reserved byte + 256-bit usage bitmap `0x00..0xFF`), with 8-byte boot keyboard fallback on `SET_PROTOCOL(0)`. Report ID 2: consumer control (one 16-bit usage). Output report: lock LEDs. |
| 2, 3 | CDC ACM | notification `0x81`, data `0x02` / `0x82` | Diagnostic log |
| 4 | HID, vendor (`HID_ITF_PROTOCOL_NONE`) | IN `0x85` | Vendor page `0xFF00` OpenFlux actions (4-byte LE 32-bit pressed-function mask, no Report ID) |

The firmware's serial number string is the RP2040's unique board ID.

When the firmware is told to enter its boot loader, the RP2040 USB boot ROM
appears instead (`0x2E8A:0x0003`). The host uses its PICOBOOT interface to
read and write flash; see [calibration-storage.md](calibration-storage.md)
for the region a firmware image may never overwrite.

### HID reports (`IN 0x83` and `IN 0x85`)

Interface `0` (`IN 0x83`) has two application collections:

1. **Report ID 1** (`0x01`): the keyboard, with full n-key rollover. The
   input report is 1 reserved byte and a 32-byte bitmask of the HID
   Keyboard/Keypad usages `0x00..0xFF`; output report `0x01` is a 1-byte
   lock LED mask. When the host selects the boot protocol
   (`SET_PROTOCOL(0)`), `IN 0x83` sends standard 8-byte boot keyboard
   reports (`modifiers, 0x00, keycodes[0..5]`) without a report ID.
2. **Report ID 2** (`0x02`): consumer controls (Consumer Page `0x0C`), one
   16-bit little-endian usage.

Interface `4` (`IN 0x85`, `HID_ITF_PROTOCOL_NONE`) is a separate vendor HID
interface, so host software can read it without macOS Input Monitoring
permission. It has one application collection and no report ID:

```text
Usage Page (0xFF00), Usage (0x01), Collection (Application):
  Usage Page (0xFF00), Usage Minimum (0x00), Usage Maximum (0x1F)
  Logical Minimum (0), Logical Maximum (1)
  Report Size (1), Report Count (32)
  Input (Data, Variable, Absolute)
End Collection
```

The report is a 4-byte little-endian bitmask of the held OpenFlux function
keys (see [keymap-format.md](keymap-format.md)): bit `c` is 1 while
`0x5000_0000 | c` is held. The report has room for 32 function codes; codes
2 to 15 are assigned. The firmware queues every press and release, so a
brief tap is still reported, in order.

## Framing

All integers are little-endian. `P` is the device's packet size.

### Request

1. A header packet of exactly `P` bytes:

   | Offset | Size | Field |
   |---|---|---|
   | 0 | 1 | command number |
   | 1 | 4 | `packets`: `ceil(len(payload) / P)`, 0 without a payload |
   | 5 | 4 | `bytes`: `len(payload)` |
   | 9 | `P - 9` | zero |

2. The payload, split into `packets` packets of `P` bytes. The last packet
   is zero-padded.
3. A zero-length packet, which ends the bulk OUT transfer.

### Response

1. A header packet of `P` bytes with the same layout, where byte 0 is the
   response type: `1` (ACK) or `0` (NAK).
2. `packets` packets of `P` bytes; the first `bytes` bytes are the response
   payload and the rest is padding.
3. A zero-length packet.

Every command gets exactly one response. A NAK carries no payload. Readers
accept a `packets` value of `ceil(bytes / P)` or one more, and an empty ACK
with `packets` 0 or 1; any other combination is a protocol error, after which
the host drains the trailing packets before sending the next command.

### Limits

| | Bridge | Firmware |
|---|---|---|
| Request payload, `PUT_FILE` | 128 MiB, streamed to disk | n/a |
| Request payload, other commands | 16 MiB | 16384 bytes |
| Response payload accepted by the host | 128 MiB | 128 MiB |

A device that receives a header announcing more than its limit reads and
discards the announced packets, then answers NAK.

The host gives each command a base timeout and extends it by the transfer
size at 2 MiB/s, so large files get proportionally longer.

Strings in payloads are UTF-8 and NUL-terminated unless stated otherwise.

## Bridge commands

| Number | Name | Request payload | ACK payload |
|---|---|---|---|
| 1 | `BRIDGE_VERSION` | none | `openflux-bridge <version>` |
| 2 | `DISPLAY_VERSION` | none | the installed display's version file, or `unknown` |
| 3 | `OS_VERSION` | none | the keyboard operating system's version file, or `unknown` |
| 4 | `SET_SYSTEM_TIME` | `YYYY-MM-DD HH:MM:SS <zone>`: UTC time and an IANA time zone name | none |
| 100 | `GET_FILE_LIST` | directory path, or no payload at all for the home directory | `<size>:<name>\0` for every regular file, concatenated |
| 101 | `GET_FILE` | file path | the file's bytes (not NUL-terminated) |
| 102 | `PUT_FILE` | `<path>\0<file bytes>` | none |
| 103 | `DELETE_FILE` | file path | none |
| 104 | `GET_FREE_SPACE` | none | `free:<bytes>,total:<bytes>` |
| 105 | `STAT_FILE` | file path | `exists:<size>` or `absent` |
| 200 | `SEND_LIVE_DATA` | live-data frames, see [display-ipc.md](display-ipc.md) | one byte, `0x01` |
| 201 | `GET_MEDIA_STATUS` | none | `/home/root/current_active_media.txt`, or empty |
| 202 | `GET_SHORTCUT_CONTEXT` | none | `/home/root/current_shortcut_context.txt`, or `Unknown` |

Unknown command numbers are answered with NAK.

Version and text-file responses have trailing line breaks removed, are at
most 4095 bytes, and are NUL-terminated.

### Paths

Every path a command names is checked before use:

- A path that is empty, contains `..` or contains `\` is refused. (A
  `GET_FILE_LIST` without any payload names no path and lists the home
  directory.)
- `.` is the home directory, `/home/root`.
- A relative path is resolved under `/home/root`.
- An absolute path must be inside `/home`.

`GET_FILE_LIST` skips names that start with a dot. `PUT_FILE` requires the
path to end within the first 255 bytes of the payload, creates missing
parent directories, writes to a staging file, syncs it and renames it over
the target, so a reader never sees a partial file. It is refused when the
file would leave less than 64 MiB free on a filesystem larger than 512 MiB.
`GET_FREE_SPACE` reports the free space minus 64 MiB, whatever the
filesystem's size.
`DELETE_FILE` is acknowledged when the file does not exist.

### Time

`SET_SYSTEM_TIME` is refused unless the date and time are valid, the year is
1970 or later and the zone exists in the keyboard's time zone database. The
bridge points `/etc/localtime` at the zone and then sets the clock; if
setting the clock fails, the previous zone is restored.

### Files on the keyboard

| Path | Contents |
|---|---|
| `/home/root/openflux/config/` | display configuration: `appearance.json`, `settings.json`, `mosaic.json`, `keymap.json`, `profiles.json` |
| `/home/root/openflux/media/` | background images and videos; key icons in `key_icons/` |
| `/home/root/openflux/logs/` | logs, and state the display publishes for the host: `pomo_state.json`, `soc_stats.json` |
| `/home/root/.openflux/versions/<component>` | installed version of each OpenFlux keyboard component |
| `/home/root/.openflux/rollback/bridge/` | the previous bridge, kept while an update is unconfirmed |
| `/home/root/*.ipk` | packages placed here are installed by the keyboard |

Installing a package means uploading it to `/home/root/` with `PUT_FILE`;
the keyboard installs it and the bridge restarts if it was replaced. After a
bridge update, the previous bridge is restored at the next start unless the
host confirms the update by deleting
`/home/root/.openflux/rollback/bridge/armed`.

## Firmware commands

| Number | Name | Request payload | ACK payload |
|---|---|---|---|
| 1 | `VERSION` | none | `openflux-firmware <version>` |
| 100 | `SEND_KEYMAP` | keymap blob | none |
| 101 | `ENTER_BOOTLOADER` | none | none |
| 102 | `ENTER_OS_BOOTLOADER` | none | none |
| 103 | `GET_KEYMAP` | none | the stored keymap blob |
| 104 | `SET_LANGUAGE` | `u32` input language code | none |
| 105 | `SET_PLATFORM` | `u32` platform code: 1 Windows, 2 macOS, 3 Linux | none |
| 106 | `SET_ACTIVE_PROFILE` | `u32` profile index | none |
| 107 | `FILE_FROM_HOST` | file header and profile settings | none |
| 108 | `FILE_TO_HOST` | optional `u32` file type, which must be 0 | profile settings, without the file header |
| 109 | `GET_RUNTIME_STATE` | none | 12 bytes: `active_profile`, `active_actuation_mode` and `platform_code`, each a `u32` |
| 112 | `PRESS_REFINEMENT_STATUS` | none | 16-byte [refinement status](#optional-press-refinement-data) |
| 113 | `PUT_PRESS_REFINEMENT` | complete press-refinement record | none |
| 114 | `GET_PRESS_REFINEMENT` | none | complete validated press-refinement record |

The keymap blob and the profile settings are specified in
[keymap-format.md](keymap-format.md).

- `SEND_KEYMAP` and `FILE_FROM_HOST` validate the payload completely, store
  it, and only then apply it. An invalid payload is answered with NAK and
  changes nothing.
- `GET_KEYMAP` answers NAK when no keymap is stored.
- `SET_LANGUAGE` and `SET_PLATFORM` are stored and reported back in the
  key-state message (see [uart.md](uart.md)). Language codes are listed in
  `host/daemon/data/input_language_codes.json`.
- `SET_ACTIVE_PROFILE` selects the profile's actuation mode and default
  layers from the stored profile settings. It is not stored; the host sets
  it again after the keyboard restarts.
- `ENTER_BOOTLOADER` is acknowledged, and the RP2040 restarts into its USB
  boot ROM shortly after.
- `ENTER_OS_BOOTLOADER` is acknowledged, and the firmware resets the
  keyboard's computer: it drives GPIO 5 (`FW_SOC_RST_GPIO`, the computer's
  reset line) high for 200 ms while holding GPIO 4 (`FW_SOC_PWR_GPIO`, its
  power-enable line) high, so the computer stays powered across the reset.

### Optional press-refinement data

These commands install optional, keyboard-wide press-refinement data. The
data is separate from the keymap and from the keyboard's own sensor
calibration. Clients never send a path, and installing needs no restart.
Firmware without these commands answers them with NAK.

`PUT_PRESS_REFINEMENT` (113) accepts exactly 1472 bytes:

| Offset | Bytes | Contents |
|---|---|---|
| 0 | 4 | ASCII `OFRM` |
| 4 | 4 | format `u32` = 1 |
| 8 | 4 | payload length `u32` = 1456 |
| 12 | 4 | payload CRC-32 `u32` (CRC-32/ISO-HDLC, as returned by `zlib.crc32`) |
| 16 | 1456 | opaque press-refinement payload |

The CRC covers only the 1456 payload bytes. All numeric fields are little-endian.
The CRC detects corruption, not provenance. The host should verify its source
before installation. The firmware validates the record, writes and reads back a
staging file on littlefs, and atomically replaces `press_refinement_v1`. Only the
live file is loaded at boot. A rejected or interrupted upload does not activate
partial data. If the final ACK was lost, check the status or read the record back.

`PRESS_REFINEMENT_STATUS` (112) returns four `u32` values:

| Offset | Contents |
|---|---|
| 0 | Supported record format (1) |
| 4 | State: 0 absent, 1 installed and active, 2 invalid, 3 storage unavailable, 4 installed but inactive |
| 8 | Installed payload length (1456 in states 1 and 4; zero otherwise) |
| 12 | Installed payload CRC (zero unless state 1 or 4) |

A successful install means the validated data is stored. Firmware built
with press refinement (`FW_ENABLE_PRESS_REFINEMENT`) uses it from then on and
reports state 1. Firmware built without it, as release firmware is, reports
state 4 and scans keys without the data. `GET_PRESS_REFINEMENT` (114) reads
back the full validated record in either installed state, and answers NAK
otherwise.

The host never trusts a `PUT_PRESS_REFINEMENT` ACK alone. After every put it
reads `PRESS_REFINEMENT_STATUS`, which must report state 1 or 4 with the
payload's CRC, and `GET_PRESS_REFINEMENT`, which must return exactly the record
it sent.

## Versions

Every OpenFlux component reports its version as `openflux-<component>
<version>`, for example `openflux-bridge 1.0.0`, with `<version>` the SemVer
version of the release. The host treats a keyboard whose firmware, bridge
or display reports anything else as one that needs converting (see
[converting-a-keyboard.md](../converting-a-keyboard.md)).
