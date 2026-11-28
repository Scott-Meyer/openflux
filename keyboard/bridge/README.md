# Keyglass bridge

`keyglass-bridge` runs on the keyboard's Linux computer and connects it to
everything else. It is the USB gadget the host talks to (versions, files in
`/home`, the clock, live data), it reads the firmware's key state from the
serial link, and it serves both to the display over a local socket. The
package also installs the launcher the keyboard runs at boot, which starts
the bridge and the display and recovers from failed updates. It is written
in Rust.

## Build and test

Tests run on your computer:

```sh
cargo test -p keyglass-bridge
```

The binary is a static aarch64 Linux executable. The package,
`keyglass-bridge_<version>_aarch64.ipk`, and the factory restore package,
`keyglass-factory-restore_<version>_aarch64.ipk`, are built in the
container described by `Dockerfile.build`:

```sh
docker build -t keyglass-bridge-build -f keyboard/bridge/Dockerfile.build keyboard/bridge
docker run --rm -v "$PWD:/work" -w /work keyglass-bridge-build \
  make -C keyboard/bridge ipk factory-restore-ipk VERSION=1.0.0 OUT_DIR=/work/build/out
```

`tests/test_linux_sigterm.py` runs a built bridge on Linux against
simulated USB endpoints and serial port; it expects the binary at
`keyboard/bridge/build/keyglass-bridge`, or at `KEYGLASS_TEST_BRIDGE_BIN`.

## Documentation

- [architecture.md](../../docs/architecture.md): where the bridge fits
- [usb.md](../../docs/protocol/usb.md): USB commands and the files on the
  keyboard
- [uart.md](../../docs/protocol/uart.md): the serial link to the firmware
- [display-ipc.md](../../docs/protocol/display-ipc.md): the display socket
  and live data
