# Display channel

Interface: `display-ipc` in `compat.toml`, 1.0.

The display learns about the keyboard and the computer through the bridge.
The bridge listens on a local socket and sends the display key state from
the firmware and live data from the host. Everything else the display needs
is in its configuration files, which it reads directly (see
[Configuration files](#configuration-files)).

Implementations: `keyboard/bridge/src/display_ipc.rs` (server),
`keyboard/display/src/ipc.rs` (client), `host/daemon/src/codecs/simple_hdlc.rs`
(live-data encoding).

## Socket

| | |
|---|---|
| Path | `/tmp/keyglass_bridge.sock` (`KEYGLASS_BRIDGE_SOCK` overrides it for the display) |
| Type | Unix domain stream socket |
| Clients | up to 8 at a time; further connections are closed |

Both directions carry newline-delimited JSON: one UTF-8 JSON object per line,
ended by `\n`. The bridge discards a client line longer than 511 bytes; the
display discards its buffer if a line grows past 64 KiB.

## Client to bridge

| Message | Effect |
|---|---|
| `{"cmd":"get_state"}` | The bridge answers this client with the latest `key_state`, if it has received one. The display sends this right after connecting. |
| `{"cmd":"set_qc","enabled":true}` | Sets the sensor-mode flag the bridge sends to the firmware. While on, the firmware sends sensor readings and the bridge broadcasts `key_qc`. |
| `{"cmd":"set_stress","enabled":true}` | Sets the stress-test flag the bridge sends to the firmware. |

Lines that are not JSON objects with a known `cmd` are ignored.

## Bridge to clients

The answer to `get_state` goes only to the client that asked. Every other
message is broadcast to all connected clients.

### `key_state`

Sent for every key-state message from the firmware (see [uart.md](uart.md)).

```json
{"type":"key_state","seq":1234,"timestamp_ms":567890,"keyframe_layout":1,
 "keyframe_attached":true,"switch_feel":0,"active_layer_bitfield":1,
 "active_actuation_mode":0,"language_code":0,"platform_code":2,
 "active_profile":0,"num_lock":false,"caps_lock":false,"scroll_lock":false,
 "of_events_seq":0,"of_events_ring":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
 "of_events_profiles":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
 "key_bitmap":"0000000000000000000000000000000000000000000000000000000000000000"}
```

`key_bitmap` is the 32-byte pressed-key bitmap as 64 lowercase hex digits,
byte 0 first. When the firmware fills the Keyglass event ring (its
capability byte at key-state offset `0x2F` is nonzero; see
[uart.md](uart.md)), `of_events_seq` counts Keyglass function-key presses,
`of_events_ring` holds the last 16 function codes in press order, and
`of_events_profiles` holds the profile that was active after each of those
presses. The other fields are the firmware's key-state fields; the three
lock flags are the bits of the host lock LED byte.

### `key_qc`

Sent for every sensor-readings message.

```json
{"type":"key_qc","seq":1235,"sensors":[2051,2047, ...]}
```

`sensors` has 198 entries; the first 160 are the raw reading of each sensor
slot and the rest are zero.

### `live_data`

Sent for every complete live-data message from the host.

```json
{"type":"live_data","data_type":0,"payload":{ ... }}
{"type":"live_data","data_type":4}
```

Data type 0 carries a JSON object in `payload`; data type 4 is a heartbeat
without a payload.

## Live data from the host

The host sends live data with the bridge's `SEND_LIVE_DATA` USB command. The
command payload is a sequence of frames in the framing described in
[uart.md](uart.md):

1. A control frame of 5 bytes: `0xFF` followed by the message length as a
   big-endian `u32`.
2. The message, split into frames of at most 8000 bytes.

The message is one data-type byte followed by the data: `0` for UTF-8 JSON,
`4` for a heartbeat. Messages longer than 1 MiB are dropped, and a frame that
would overrun the announced length discards the message. The bridge forwards
each complete message as one `live_data` line.

The JSON object can contain these members, each of them optional:

| Member | Contents |
|---|---|
| `hardwareData` | `cpu` (%), `memory` and `totalMemory` (GB), `gpuUsage` (%), `gpuUnified`, `vramUsed` and `totalVram` (GB or `null`), `netDown` and `netUp` |
| `mediaFile` | `title`, `artists`, `album`, `length` and `played` (seconds), `playing`, `base64Image` and `trackId` (see below) |
| `weather` | `city`, `temp`, `high`, `low`, `condition`, `isDay`, `celsius`, `ready`, `sunrise`, `sunset`, `aqi`, `utcOffsetSec`, and `hourly`: up to three `{hour, temp, condition, isDay}` |
| `customGraphData` | a generic Mosaic module update: `index` (slot 0 to 3), `title`, `subtitle`, `value`, `progress`, `type` |
| `shortcutOverlays` | up to 64 `{mods, keyCode, label, icon}`: labels for shortcuts of the active shortcut pack |
| `activeThemeId`, `activeThemeProfileSlot`, `themeRev` | the theme of the profile the firmware has active, that profile's index, and the revision of the host's theme choice (see below) |
| `hostDisplayAsleep` | `true` while the computer's display is asleep |

`base64Image` is the track's album art: a JPEG, square and at most 160 pixels
a side, as base64 with padding, at most 32 KiB of text. It is an empty string
when the player has no art, and while nothing plays. `trackId` names the
image: the same id always comes with the same image, so a display can decode
an image once and keep it while the id stays the same. It is empty when
`base64Image` is. The host sends the art with every message, keepalives
included, so a display that connects later gets it too.

The host sends `activeThemeId` with every message, keepalives included, so
the display applies it only when `themeRev` differs from the last one it
applied; a theme picked on the keyboard stays until the host chooses a theme
again. `themeRev` is a `u64` that changes when the host chooses:

- a profile is picked in Studio or through the API, even the profile already
  in effect;
- the profile in effect changes on the host (an app rule, an app reporting
  its process) to one with a different theme, or its theme is edited.

It doesn't change for keepalives, for a profile picked on the keyboard, for
profiles or a `selectedId` read back from the keyboard, or across daemon
restarts: the host stores it and sends the same value again.

## Configuration files

The display reads its configuration from `/home/root/keyglass/config/`
(`KEYGLASS_CONFIG_DIR` overrides it) and checks the files for changes four
times a second. The host writes them with the bridge's `PUT_FILE` command,
which replaces a file atomically.

| File | Contents |
|---|---|
| `appearance.json` | `selectedId`, `defaultTheme` and `customThemes`: font, background, key effect, colors and per-key styles of each theme |
| `settings.json` | display timeout, parallax and eye position, foot stand, display offset, locale |
| `mosaic.json` | `tileTypes` and `tileConfigs` of the four Mosaic slots. A weather tile's config has `city` and the unit flag `celcius`, which is the spelling stored configurations use; the display also accepts `celsius`. |
| `keymap.json` | the keymap document, see [keymap-format.md](keymap-format.md) |
| `profiles.json` | profiles with their theme, shortcut contexts and actuation mode |

`assets/default_config.json` holds the defaults the host starts from.
Backgrounds named in a theme are looked up in `/home/root/keyglass/media/`
and the display's own resources; a name ending in `.frag` selects one of the
display's procedural backgrounds (`aurora_drift`, `cobalt_waves`,
`horizon_split`, `tidal_caustics`, `perimeter_glow`).

The display writes to `/home/root/keyglass/logs/`:

| File | Contents |
|---|---|
| `display_heartbeat.log` | the current time in Unix seconds, rewritten each second; the keyboard's watchdog restarts a display whose heartbeat stops changing |
| `pomo_state.json` | the Pomodoro timer's state, read by the host |
| `soc_stats.json` | the keyboard computer's load, written every 2 seconds, read by the host |
