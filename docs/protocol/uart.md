# Serial link between firmware and bridge

Interface: `uart` in `compat.toml`, 1.0.

Inside the keyboard, the firmware and the bridge share a serial link. The
firmware reports key state over it and the bridge answers each report. The
bridge forwards what it receives to the display (see
[display-ipc.md](display-ipc.md)).

Implementations: `keyboard/firmware/src/fw_hdlc.c`, `fw_scanner.c`
(`fw_runtime_build_msg_update`, `fw_runtime_build_msg_qc`) and
`rp2040_main.c`; `keyboard/bridge/src/simple_hdlc.rs` and
`uart_bridge.rs`.

## Physical layer

| | |
|---|---|
| Firmware side | RP2040 `UART1`, TX on GPIO 8, RX on GPIO 9 |
| Bridge side | `/dev/ttyS3` |
| Format | 921 600 baud, 8N1, no flow control |

## Frames

Each message is one frame:

| Field | Size | |
|---|---|---|
| start flag | 1 | `0x7E` |
| length | 2 | payload length, big-endian |
| payload | length | |
| CRC | 4 | big-endian |

Every byte after the start flag that equals `0x7E` or `0x7D` is sent as
`0x7D` followed by the byte XOR `0x20`. A receiver that sees `0x7E` starts a
new frame, discarding any frame in progress.

The CRC covers the payload only. It is CRC-32 with polynomial `0x04C11DB7`
and initial value `0xFFFFFFFF`. Each input byte is reflected (its bit order
swapped) and then processed most-significant bit first; the result is not
reflected and is XORed with `0xFFFFFFFF`. The check value for the ASCII
bytes `123456789` is `0x649C2FD3`. Frames with a wrong CRC are dropped.

The firmware accepts payloads up to 512 bytes and the bridge up to 8192.

The same framing carries live data from the host to the display, inside the
`SEND_LIVE_DATA` USB command.

## Messages

The first payload byte is the message type. Multi-byte fields are
little-endian.

| Type | Direction | Length | Message |
|---|---|---|---|
| 2 | firmware to bridge | 139 | key state |
| 3 | firmware to bridge | 401 | sensor readings |
| 2, 3 | bridge to firmware | 4 | response |

### Key state (type 2)

The firmware sends key state whenever a key or a lock LED changes, and at
least once per second.

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | type, `2` |
| 1 | 4 | sequence number, incremented for every message the firmware sends |
| 5 | 134 | state, below |

State offsets are relative to byte 5:

| Offset | Size | Field |
|---|---|---|
| `0x00` | 32 | pressed keys: bit `k % 8` of byte `k / 8` is set while key `k` is down, in keymap key order |
| `0x20` | 4 | firmware uptime in milliseconds |
| `0x24` | 1 | keyframe layout: 1 ANSI, 2 ISO, 3 JIS |
| `0x25` | 1 | keyframe attached: 0 or 1 |
| `0x26` | 1 | switch feel |
| `0x27` | 4 | active layers: bit `n` set for each active layer, including layers held with a layer key |
| `0x2B` | 4 | active actuation mode index |
| `0x2F` | 1 | Keyglass event ring format: `1`; `0` means the ring fields below are not filled |
| `0x30` | 2 | `seq`: Keyglass function-key (`0x5000_00xx`) presses since boot, a `u16` that wraps at 65536 |
| `0x32` | 16 | ring of the last 16 Keyglass function codes in press order; the newest is at `ring[(seq - 1) & 15]`. A slot holds up to `0x1F`; the assigned codes are 2 to 15 (see [keymap-format.md](keymap-format.md)) |
| `0x42` | 16 | active profile index at the completion of each corresponding event in `0x32` |
| `0x5C` | 1 | host lock LEDs: `0x01` Num Lock, `0x02` Caps Lock, `0x04` Scroll Lock |
| `0x5D` | 4 | input language code (set with `SET_LANGUAGE`) |
| `0x61` | 4 | platform code (set with `SET_PLATFORM`) |
| `0x65` | 4 | active profile index |
| `0x6A` | 1 | keyframe phase: 0 absent, 1 field detected, 2 IDs settling, 3 attached, 4 attached and inhibiting new presses during recovery |
| `0x6B` | 1 | keyframe module types, 0 when none |
| `0x84` | 2 | keyframe module buttons, 0 when none |

All other bytes are zero.

### Sensor readings (type 3)

While sensor mode is on (see the response below), the firmware sends raw
readings about every 16 ms.

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | type, `3` |
| 1 | 4 | sequence number |
| 5 | 320 | 160 × `u16`: raw 12-bit ADC reading of each Hall-effect sensor, by sensor slot |
| 325 | 76 | zero |

### Response (bridge to firmware)

The bridge answers every key-state and sensor message:

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | type of the message being answered |
| 1 | 1 | error: 0 accepted, 1 wrong length |
| 2 | 1 | sensor mode: 1 asks the firmware to send sensor readings |
| 3 | 1 | stress test: 1 enables the firmware's stress test |

The firmware applies the sensor-mode and stress-test flags of every
response it receives.
