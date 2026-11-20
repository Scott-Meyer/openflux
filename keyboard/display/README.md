# OpenFlux display

`openflux-display` shows your keymap, themes, profiles, Mosaic widgets, and
media on the keyboard's 1920×1080 screen. It receives live key state and host
metrics from `openflux-bridge`, and draws with the GPU and hardware video
decoding, falling back to software for either when needed. It is written in
Rust and ships with its own FFmpeg build, `openflux-ffmpeg`.

## Build and test

Unit and render tests run on your computer:

```sh
cargo test -p openflux-display
```

The aarch64 Linux package, `openflux-display_<version>_aarch64.ipk` (along
with `openflux-ffmpeg-source-*.tar.xz` and `openflux-ffmpeg-build.txt`), is
built in the container described by `Dockerfile.build`, which installs the
cross toolchain and builds `openflux-ffmpeg`:

```sh
docker build -t openflux-display-build -f keyboard/display/Dockerfile.build keyboard/display
docker run --rm -v "$PWD:/work" -w /work openflux-display-build \
  make -C keyboard/display ipk VERSION=1.0.0 OUT_DIR=/work/build/out
```

`tests/test_linux_display.py` runs headless PPM render checks and a real
Weston + `openflux-bridge` IPC test on Linux. It expects the display binary at
`keyboard/display/build/openflux-display` (or `OPENFLUX_TEST_DISPLAY_BIN`) and
the bridge binary at `keyboard/bridge/build/openflux-bridge` (or
`OPENFLUX_TEST_BRIDGE_BIN`). Without Weston or the bridge binary the IPC test
is skipped; set `OPENFLUX_REQUIRE_FULL_E2E=1` to make that a failure instead.

## Documentation

- [architecture.md](../../docs/architecture.md): where the display fits
- [display-ipc.md](../../docs/protocol/display-ipc.md): the bridge-to-display
  socket protocol
