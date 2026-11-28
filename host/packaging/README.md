# Host packaging

Files that turn the Keyglass host programs into installers, and that
register the daemon, `keyglassd`, with each operating system. The release
workflow (`.github/workflows/release-host.yml`) uses them; signed release
installers are built only there.

| Path | Purpose |
|---|---|
| `scripts/build-sidecars.sh` | Builds the executables bundled next to Keyglass Studio, named `<name>-<target triple>` for Tauri, and collects the FFmpeg license files |
| `scripts/tauri-release-config.sh` | Writes the Tauri configuration overlay for a release build |
| `ffmpeg/build-ffmpeg.sh` | Builds `keyglass-ffmpeg` and `keyglass-ffprobe` from pinned, verified FFmpeg and x264 sources, and writes the corresponding source archive |
| `macos/dev.keyglass.keyglassd.plist` | LaunchAgent for `keyglassd`, inside the app bundle |
| `macos/entitlements.plist` | Code-signing entitlements (hardened runtime, no exceptions) |
| `linux/keyglassd.service` | systemd user unit |
| `linux/70-keyglass.rules` | udev rules giving the logged-in user access to the keyboard |
| `linux/deb/` | Maintainer scripts for the `.deb` |
| `windows/installer-hooks.nsh` | NSIS hooks: Run-key autostart, daemon stop before files change |
| `keys/tauri-updater.pub` | Public key of the Studio update feed, built into Studio |
| `keys/active-release.pub` | Public half of the active release key, which signs the update index and artifacts (see [releasing.md](../../docs/releasing.md#keys-and-rotation)) |
| `keys/statements/` | Root-signed key statements carried in every index; present once a release key has been rotated |

## What each installer contains

Every installer holds Keyglass Studio, `keyglassd`, `keyglass-ffmpeg` and
`keyglass-ffprobe`, and the newest stable firmware, bridge and display
releases with the signed index that lists them (`$RESOURCE/keyboard/`), so
a keyboard can be converted to Keyglass without a network connection.
Resetting a keyboard to its factory software downloads the factory files
unless the computer already has a verified copy. On
macOS, the menu bar helper (`keyglass-tray`) and the system sensors helper
(`keyglass-sensors`) are separate executables. On Windows, `keyglassd`
shows the tray icon itself. The Linux packages include no tray helper; there
the daemon shows a plain notification icon when `zenity` is installed.

`keyglass-ffmpeg` and `keyglass-ffprobe` come from one FFmpeg build per
target, configured with libx264, so these two programs are licensed under
the GPL, version 2 or later. Their license and the configure lines of the
build are installed in `$RESOURCE/licenses/`; the complete source is
published with each host release. Keyglass's own code is MIT OR Apache-2.0.
`$RESOURCE/licenses/` also holds the license texts of the Rust crates
compiled into `keyglassd` and Studio (`keyglassd-rust-licenses.txt`,
`keyglass-studio-rust-licenses.txt`, written by cargo-about through
`scripts/licenses/rust-licenses.sh` for the build's targets) and of the npm
packages in Studio's interface (`keyglass-studio-ui-npm-licenses.txt`,
`scripts/licenses/npm-licenses.mjs`). `build-sidecars.sh` writes them; it
needs cargo-about (`scripts/licenses/install-cargo-about.sh`) and
`npm ci` in `host/studio`. Bundled dependencies and assets keep their own
licenses; see
[NOTICE](../../NOTICE) and the
[asset license manifest](../../assets/LICENSE_MANIFEST.md).

Sidecars are installed next to the main executable: `Contents/MacOS/` on
macOS, the install directory on Windows, `/usr/bin` for the `.deb`, and
`usr/bin` inside the AppImage. `keyglassd` finds `keyglass-ffmpeg`,
`keyglass-ffprobe` and the helpers there.

## The daemon and the service manager

`keyglassd` runs per user. The packaging relies on two commands of the
installed daemon, which talk to the running daemon over its local API, and
on its exit status:

| Command | 0 | 3 | 4 | 1 |
|---|---|---|---|---|
| `keyglassd --request-restart` | it was idle and exited with status 75; the service manager starts it again | it is flashing, converting or restoring the keyboard, and restarts by itself once idle | not running | the request failed |
| `keyglassd --request-stop` | it was idle and exited with status 0, and stays stopped | as above, then stays stopped | not running | the request failed |

Installers never kill the daemon, and their hooks fail closed: 0 and 4
continue, 3 waits up to 15 minutes with a message, and anything else stops
with an explanation.

The LaunchAgent (`KeepAlive` with `SuccessfulExit` false) and the systemd
unit (`Restart=on-failure`) start the daemon again after exit status 75 or a
crash, and leave it stopped after status 0. When the service manager itself
stops the daemon, the daemon first finishes the keyboard operation in
progress, so both allow 15 minutes before they kill it (`ExitTimeOut`,
`TimeoutStopSec`).

## Updates of Keyglass itself

On macOS, Windows and with the AppImage, Keyglass Studio installs host
updates with its built-in updater, from the update feed on the project's
site. The updater accepts an update only if it is signed with the key in
`keys/tauri-updater.pub` and the signature names the version the feed
announces. Studio:

1. downloads and verifies the update;
2. asks the daemon to stop (the daemon's lifecycle API). From then on the
   daemon refuses new keyboard work and exits once the work in progress is done;
   Studio shows what it is waiting for;
3. installs the update;
4. starts the updated daemon through the service manager and relaunches
   itself.

If the installation fails, or the user cancels after the stop, Studio starts
the existing daemon again and says so. The Windows setup program's hooks
then find the daemon already stopped.

The `.deb` is updated by the system package manager: Studio announces the
update and links to the release.

## macOS (`.dmg`, and `.app.tar.gz` for the updater)

Install: open the disk image and drag Keyglass Studio to Applications. On
first launch the app registers
`Contents/Library/LaunchAgents/dev.keyglass.keyglassd.plist` with
`SMAppService` (macOS 13 or later), which starts `keyglassd` immediately and
at every login. It is listed under the
app's name in System Settings > General > Login Items, where it can be
turned off.

Uninstall: choose Quit Keyglass Daemon in the menu bar icon (the daemon
stops after any keyboard work and stays stopped), turn off Keyglass Studio
in System Settings > General > Login Items, and move the app to the Bin.
The daemon's data in `~/.local/share/keyglass` stays until removed.

The app and every helper are signed with the hardened runtime and no
entitlement exceptions, and notarized.

## Windows (`-setup.exe`)

The setup program (NSIS) installs for the current user without
administrator rights into `%LOCALAPPDATA%\Keyglass Studio`. It sets the
`Keyglass` value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`
to start `keyglassd.exe` at sign-in, and starts it once installation ends.
The built-in updater uses the same setup program.

Before replacing or removing files, the setup program and the uninstaller
run `keyglassd --request-stop`. While the daemon finishes work on the
keyboard they wait, up to 15 minutes; after that, or if the daemon's answer
is anything other than stopped or not running, they stop without changing
anything and explain why.

Uninstall: Settings > Apps > Keyglass Studio. Removing the app removes the
Run value. The daemon's data in `%LOCALAPPDATA%\keyglass` is kept.

## Linux (`.deb`, `.AppImage`)

`.deb`: `sudo apt install ./keyglass-studio_<version>_<arch>.deb`. It
installs:

- `/usr/lib/udev/rules.d/70-keyglass.rules`, so the logged-in user can use
  the keyboard and its RP2040 boot loader without root. Its `hidraw` rule
  lets the daemon read the keyboard's function-key HID interface, which
  carries the Auto-Profile Lock key; without the rule that key does nothing,
  and the daemon logs this once
- `/usr/lib/systemd/user/keyglassd.service`, enabled for every user with
  `systemctl --global enable`

After installation or an upgrade, the daemon of each logged-in user who has
the service enabled is moved to the installed binary with
`keyglassd --request-restart` (and started if it was not running). If the
daemon is working on the keyboard, the package waits up to 15 minutes and
otherwise leaves the restart to the daemon, which restarts by itself when
it is done. A failed request stops the installation with an explanation;
`sudo dpkg --configure -a` repeats it. A user can opt out of the service
with `systemctl --user mask --now keyglassd.service`; `disable` does not
override the global enablement.

Remove: `sudo apt remove keyglass-studio`. Removal asks each logged-in
user's daemon to stop and waits for keyboard work to finish, up to 15
minutes; otherwise, or if the request fails, the removal stops and can be
repeated. User data in `~/.local/share/keyglass` is never removed by the
package.

`.AppImage`: an AppImage installs nothing on the system. Keyglass Studio
starts the daemon from the AppImage whenever it opens, and the daemon runs
until the session ends; it does not start at login by itself. The built-in
updater replaces the AppImage file in place. Keyboard access needs the udev
rules, which the AppImage carries as `usr/share/keyglass/70-keyglass.rules`
and the user installs by hand
([installing.md](../../docs/installing.md#appimage)). A daemon still running
from a deleted AppImage stops when the session ends.

## Building locally

The scripts run on a development machine as well, for testing packaging.
Local builds are unsigned and are never published. `--development-keys`
builds `keyglassd` without the `production-keys` check.

```sh
host/packaging/ffmpeg/build-ffmpeg.sh --target aarch64-apple-darwin --out build/ffmpeg-out/aarch64-apple-darwin
host/packaging/scripts/build-sidecars.sh --target aarch64-apple-darwin --ffmpeg-dir build/ffmpeg-out --out build/sidecars
```

Tests of these files run in CI (`.github/workflows/ci.yml`, job
`release-scripts`); [releasing.md](../../docs/releasing.md#testing-the-release-scripts)
lists them.
