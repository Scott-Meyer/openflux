# Contributing to Keyglass

Thank you for helping. This page explains how to build and test each part
of Keyglass, how to write commits, and how releases are made.

[docs/architecture.md](docs/architecture.md) is the best place to start
reading about how the parts fit together.

## Repository layout

| Path | Contents |
|---|---|
| `host/daemon` | `keyglassd`, the background service (Rust) |
| `host/studio` | Keyglass Studio: the interface in `src/` (React, TypeScript), the Tauri app in `src-tauri/` |
| `host/macos-helpers` | menu bar and sensor helpers for macOS (Swift) |
| `host/packaging` | installers and service definitions for each operating system |
| `keyboard/firmware` | RP2040 key scanner firmware (C, Pico SDK) |
| `keyboard/bridge` | the keyboard computer's USB, serial and display service (Rust) |
| `keyboard/display` | the keyboard's display app, and the `keyglass-display` crate that draws Studio's preview (Rust) |
| `crates/keyglass-update` | release metadata, update trust and compatibility planning, and the `keyglass-release` tool |
| `tools/keyglass-dev` | `keyglass-dev`, a command-line client for the keyboard's USB commands, for diagnostics (Python) |
| `tests/e2e` | end-to-end tests that run the firmware core, bridge, display and `keyglass-dev` together on Linux |
| `scripts/build_ipk.py` | the `.ipk` packager the keyboard Makefiles use |
| `assets` | default configuration, fonts, icons and backgrounds |
| `docs` | documentation and protocol specifications |
| `third_party` | littlefs, the firmware's flash file system, used by the firmware unit tests |

## Prerequisites

- Rust, at the version in `rust-toolchain.toml` (rustup installs it
  automatically);
- Node.js 22 and npm, for Keyglass Studio;
- `make` and a C compiler, for the firmware tests: on macOS the Xcode
  Command Line Tools (`xcode-select --install`), which also provide `swiftc`
  for the macOS helpers; on Debian and Ubuntu `build-essential`;
- Python 3, for `keyglass-dev` and the end-to-end tests;
- Docker, for the firmware image and the keyboard packages;
- on Linux, the WebKitGTK and system libraries Tauri needs. On Debian and
  Ubuntu, as CI installs them:

  ```sh
  sudo apt-get install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
    libudev-dev libwayland-dev libxdo-dev
  ```

- on Windows, the Microsoft C++ Build Tools and WebView2 that Tauri needs
  ([Tauri's prerequisites](https://v2.tauri.app/start/prerequisites/)). The
  firmware tests and the container builds use `make` and a POSIX shell; run
  them in WSL.

## Building and testing

The commands below run from the repository root.
[CI](.github/workflows/ci.yml) runs the same builds, tests and checks on
every pull request, except the end-to-end and `keyglass-dev` tests.

### Host

The Studio crate embeds the built interface, so build the interface before
building the Rust workspace:

```sh
npm ci --prefix host/studio
npm run build --prefix host/studio
cargo test --workspace --locked
```

Studio's interface on its own:

```sh
cd host/studio
npm run typecheck
npm test
npm exec -- biome ci .
```

### Running Studio from source

Run the daemon in one terminal and Studio in another, the daemon first: a
Studio that finds no daemon running starts one itself.

```sh
# terminal 1
cargo run -p keyglassd

# terminal 2
npm --prefix host/studio run tauri dev
```

If Keyglass is installed, its daemon is already running. There is one
daemon per data directory: a second `keyglassd` prints `another keyglassd is
already running` and exits, and Studio connects to whichever daemon owns the
data directory. Either stop the installed daemon first:

```sh
cargo run -p keyglassd -- --request-stop
```

It stays stopped until your next login, or until a Studio starts it. Or give
your daemon a data directory of its own and leave the keyboard to the
installed daemon with `--no-usb`:

```sh
# both terminals
export KEYGLASS_DATA_DIR="$PWD/build/dev-data"

# terminal 1
cargo run -p keyglassd -- --no-usb

# terminal 2
npm --prefix host/studio run tauri dev
```

A daemon with its own data directory uses a free port when the default one
(52323) is taken, and third-party apps don't find it.

**FFmpeg.** Video imports need FFmpeg. A debug build (`cargo run`) uses one
installed in `/opt/homebrew/bin`, `/usr/local/bin` or `/usr/bin`;
`KEYGLASS_HOST_FFMPEG` and `KEYGLASS_HOST_FFPROBE` name others. Without
FFmpeg, importing a video that needs converting fails with a message.
`host/packaging/ffmpeg/build-ffmpeg.sh` builds the `keyglass-ffmpeg` and
`keyglass-ffprobe` the installers carry.

**macOS helpers.** On macOS the menu bar icon, the active application, the
input language, media and system sensors come from two helpers. Build them
and point the daemon at them:

```sh
make -C host/macos-helpers BUILD_DIR="$PWD/build/macos-helpers"
KEYGLASS_TRAY_BIN="$PWD/build/macos-helpers/keyglass-tray" \
KEYGLASS_MACOS_SENSORS_BIN="$PWD/build/macos-helpers/keyglass-sensors" \
  cargo run -p keyglassd
```

The daemon also finds them next to its own executable, where the installers
put them.

### Update library

`crates/keyglass-update` is part of the workspace. Its tests don't need
Studio's interface to be built:

```sh
cargo test -p keyglass-update --locked
cargo run -p keyglass-update --bin keyglass-release -- compat --compat compat.toml
```

The second command checks `compat.toml`; run it whenever you change an
interface between components (see
[docs/compatibility.md](docs/compatibility.md#for-contributors)). The
crate's [README](crates/keyglass-update/README.md) describes the library and
the tool.

### Firmware

Unit tests run on your computer, with AddressSanitizer and
UndefinedBehaviorSanitizer:

```sh
make -C keyboard/firmware test
```

The RP2040 image is built in a container, into
`build/firmware/keyglass_firmware.uf2`:

```sh
docker build -t keyglass-firmware-build -f keyboard/firmware/Dockerfile.build keyboard/firmware
docker run --rm -v "$PWD:/work" -w /work keyglass-firmware-build \
  sh -c "cmake -S keyboard/firmware -B build/firmware -G Ninja -DCMAKE_BUILD_TYPE=Release && cmake --build build/firmware"
```

### Bridge and display

Tests run on your computer:

```sh
cargo test -p keyglass-bridge
cargo test -p keyglass-display
```

The keyboard binaries and packages are built for aarch64 Linux in the
component's container, `keyboard/<component>/Dockerfile.build`, as the
release workflow builds them. This writes
`build/out/keyglass-bridge_1.0.0_aarch64.ipk`:

```sh
docker build -t keyglass-bridge-build -f keyboard/bridge/Dockerfile.build keyboard/bridge
docker run --rm -v "$PWD:/work" -w /work keyglass-bridge-build \
  make -C keyboard/bridge ipk VERSION=1.0.0 OUT_DIR=/work/build/out
```

Replace `bridge` with `display` for the display package. The display's
container also builds the keyboard's FFmpeg, which the package includes as
`keyglass-ffmpeg`. With your own aarch64 cross toolchain, run the same `make`
commands directly; the Makefiles say what they expect.

### Loading a build onto a keyboard

On a keyboard that runs Keyglass, Studio's **Device** page installs files
you built:

- **Install Key Scanner Firmware from File…** takes the `.uf2`;
- **Install Keyboard Package from File…** takes a bridge or display `.ipk`.
  The keyboard installs it and reconnects.

Studio knows a component by the version it reports. Keep a local build at
the version in the source (the default): your Studio knows that version's
interfaces from its own build. With a version it doesn't know, Studio
won't change the parts of the keyboard that depend on it.

### End-to-end tests

`tests/e2e` runs the firmware core, a native bridge and display, and
`keyglass-dev` together, without a keyboard. It needs Linux with Weston
installed:

```sh
cargo build --release -p keyglass-bridge -p keyglass-display
KEYGLASS_TEST_BRIDGE_BIN="$PWD/target/release/keyglass-bridge" \
KEYGLASS_TEST_DISPLAY_BIN="$PWD/target/release/keyglass-display" \
  python3 tests/e2e/test_product_ux.py
```

### keyglass-dev

`tools/keyglass-dev` talks to a connected keyboard over its USB commands,
for diagnostics. It needs libusb 1.0 (`brew install libusb`, or
`libusb-1.0-0` on Debian and Ubuntu) and runs on macOS and Linux, alongside
a running daemon:

```sh
python3 tools/keyglass-dev/keyglass_dev.py versions
python3 tools/keyglass-dev/keyglass_dev.py --help
```

Its own tests need no keyboard:

```sh
python3 -m unittest discover -s tools/keyglass-dev/tests
```

### Formatting and checks

CI runs these; run them before you push.

Rust:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

`cargo fmt --all` fixes the formatting.

TypeScript, in `host/studio`:

```sh
npm exec -- biome ci .
```

`npm exec -- biome check --write .` fixes what it can.

C, with clang-format 18 (`clang-format-18` on Ubuntu):

```sh
git ls-files -z 'keyboard/firmware/*.c' 'keyboard/firmware/*.h' \
  | xargs -0 --no-run-if-empty clang-format-18 --dry-run --Werror
```

Replace `--dry-run --Werror` with `-i` to format the files.

Shell scripts:

```sh
git ls-files -z '*.sh' .github/scripts/testing/gh | xargs -0 --no-run-if-empty shellcheck
git ls-files -z '.github/scripts/testing/hooks/*' | xargs -0 --no-run-if-empty shellcheck -s sh
git ls-files -z 'host/packaging/linux/deb/*' 'keyboard/*/packaging/preinst' 'keyboard/*/packaging/postinst' \
  'keyboard/*/packaging/prerm' 'keyboard/*/packaging/postrm' | xargs -0 --no-run-if-empty shellcheck -s sh
```

GitHub workflows:

```sh
go install github.com/rhysd/actionlint/cmd/actionlint@v1.7.12
"$(go env GOPATH)/bin/actionlint"
```

## Changes

- Keep a change focused on one thing, with tests for the behavior it adds or
  fixes. Tests describe behavior, not implementation details.
- Comments and documentation state what is true and why, in the present
  tense.
- When a change crosses a boundary between components (a USB command, a
  file format, a socket message), update its specification in
  `docs/protocol/` and its version in `compat.toml` in the same change.
- The third-party app API (`host/daemon/src/api/third_party.rs`) is a
  fixed external contract. Its contract tests in
  `host/daemon/tests/third_party_api.rs` must keep passing unchanged.

## Commits

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/).
The files a commit changes decide which component it releases, and its type
decides how that component's version changes. The scope names the component
for readers:

```
feat(display): show the next three hours in the weather widget
fix(bridge): refuse uploads that would fill the disk
docs: describe the display channel
```

- `feat` makes a minor release, `fix`, `perf` and `deps` a patch release.
- `!` after the type, or a `BREAKING CHANGE:` footer, makes a major release.
- A commit belongs to a component by the files it changes: `host/`,
  `keyboard/firmware/`, `keyboard/bridge/` or `keyboard/display/`.
- Shared files (`crates/`, `assets/`, `compat.toml`, the root `Cargo.toml`
  and `Cargo.lock`) belong to no component. A commit that changes only
  those releases nothing by itself; the change ships with the next release
  of each component that uses it.
- `docs`, `test`, `ci`, `build`, `style`, `refactor` and `chore` don't release
  anything.

## Releases

Releases are made only by the GitHub Actions workflows of this repository.
Tags, release artifacts, signatures and the update index all come from those
workflows, and the signing keys exist only in GitHub environments. Nothing
built on a developer's machine is ever published. Pull requests never need
to change version numbers; release-please does that.
[docs/releasing.md](docs/releasing.md) describes the process.

## License of contributions

Keyglass is licensed under either of the MIT License or the Apache License,
Version 2.0, at your option. Unless you explicitly state otherwise, any
contribution you intentionally submit for inclusion in Keyglass is licensed
the same way, as defined in the Apache License, Version 2.0, without any
additional terms or conditions. No contributor license agreement or
sign-off is required.

## Reporting security issues

Please don't open public issues for security problems. See
[SECURITY.md](SECURITY.md).
