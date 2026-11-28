# Keyglass firmware

Firmware for the keyboard's key scanner, an RP2040 that reads the 160
Hall-effect sensors, acts as the USB keyboard and reports key state to the
bridge. It is written in C with the Pico SDK and TinyUSB.

## Build and test

Unit tests run on your computer, with AddressSanitizer and
UndefinedBehaviorSanitizer:

```sh
make -C keyboard/firmware test
```

The RP2040 image (`build/firmware/*.uf2`) is built in a container; the
commands are in [CONTRIBUTING.md](../../CONTRIBUTING.md#firmware). C sources
are formatted with `clang-format` (`.clang-format` in this directory).

## Hardware

| Function | RP2040 pins |
|---|---|
| Sensor readings | ADC3 on GPIO 29, through a 16-way column multiplexer on GPIO 10 to 17 and ten active-low bank enables |
| Serial link to the bridge | `UART1`, TX on GPIO 8, RX on GPIO 9 |
| Control of the keyboard's computer | GPIO 3, GPIO 4 (power) and GPIO 5 (reset) |

The pin numbers are defined in `src/fw_protocol.h` and `src/fw_scanner.c`.

## Flash and calibration

The top 1 MiB of the 16 MiB flash holds the littlefs file system with the
keyboard's factory calibration. A firmware image must never reach it.
Keyglass Studio refuses images that would and checks the region before and
after flashing. When flashing by other means, back up the flash first, for
example with `python3 tools/keyglass-dev/keyglass_dev.py rp2040-backup <dir>`, which
saves the whole flash and the calibration files.

## Documentation

- [architecture.md](../../docs/architecture.md): where the firmware fits
- [usb.md](../../docs/protocol/usb.md): USB interfaces and commands
- [uart.md](../../docs/protocol/uart.md): the serial link to the bridge
- [keymap-format.md](../../docs/protocol/keymap-format.md): keymap blob,
  actuation and profile settings
- [calibration-storage.md](../../docs/protocol/calibration-storage.md): flash
  layout, file system and calibration
