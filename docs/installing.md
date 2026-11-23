# Installing Keyglass

Keyglass for your computer is released as **host** releases, tagged
`host-vX.Y.Z` on the project's
[GitHub Releases](https://github.com/keyglass-dev/keyglass/releases) page.
Each installer contains Keyglass Studio, the background service `openfluxd`,
and the newest stable keyboard software at the time of the release, so a
keyboard can be set up without an internet connection.

`openfluxd` runs in the background for your user from login (with the
AppImage, from when Studio first opens in a session). It keeps the
keyboard's live information (system load, media, weather, the active
application) up to date even while Studio is closed.

After installing, open Keyglass Studio and connect the keyboard. If the
keyboard is not running Keyglass yet, Studio offers to convert it; see
[converting-a-keyboard.md](converting-a-keyboard.md).

Keyglass Studio was previously called OpenFlux Studio. Update your existing
installation normally. Some installer filenames, paths, and shortcuts still
use the old name so they keep working across the update.

## Checking a download

Every release lists the SHA-256 of its files in `SHA256SUMS`. Download it
into the same folder as the installer, then run:

```sh
# macOS
shasum -a 256 --check --ignore-missing SHA256SUMS

# Linux
sha256sum --check --ignore-missing SHA256SUMS
```

Each file you downloaded is listed with `OK`. On Windows, in PowerShell:

```powershell
Get-FileHash .\openflux-studio_<version>_x64-setup.exe
```

The `Hash` it prints must match the installer's line in `SHA256SUMS`
(letter case doesn't matter).

The installers are also signed with the project's release key; maintainers
check those signatures as described in
[releasing.md](releasing.md#published-files).

## Before uninstalling

A keyboard is returned to its factory software from Keyglass Studio. If you
want that, [reset the keyboard](converting-a-keyboard.md#reset-to-factory-software)
before removing Keyglass.

## macOS

Requires macOS 13 or later, on Apple silicon or Intel.

1. Download `openflux-studio_<version>_universal.dmg` from the release.
2. Open it and drag **Keyglass Studio** to **Applications**. The app file may still be named `OpenFlux Studio.app`.
3. Open Keyglass Studio. On first launch it registers `openfluxd` to start at
   login. It appears in **System Settings > General > Login Items** under the
   app's name, where you can turn it off.

The app is signed and notarized.

**Updates.** Studio checks for updates and installs them when you choose
to; the background service restarts from the updated app.

**Uninstalling.** Choose **Quit Keyglass Daemon** from the Keyglass icon in
the menu bar, turn off Keyglass Studio in **System Settings > General >
Login Items**, then move the Studio app from Applications to the Trash. Your
settings stay in `~/Library/Application Support/org.openflux.studio` and
`~/.local/share/openflux` until you delete them.

## Windows

Requires 64-bit Windows 10 or later.

1. Download `openflux-studio_<version>_x64-setup.exe` from the release.
2. Run it. It installs for your user, without administrator rights, into
   `%LOCALAPPDATA%\OpenFlux Studio`, and starts `openfluxd` at sign-in.

**Updates.** Studio checks for updates and installs them with the setup
program when you choose to.

**Uninstalling.** Open **Settings > Apps**, select Keyglass Studio and
choose Uninstall. Selecting **Delete the application data** removes Studio's
own settings in `%APPDATA%\org.openflux.studio`. The daemon's data in
`%LOCALAPPDATA%\openflux` is kept until you delete it.

Installers and the uninstaller wait while Keyglass is working on the
keyboard (for example installing a keyboard update) and continue once it
has finished.

## Linux

Packages are published for x86_64 and aarch64 (`amd64` and `arm64` in `.deb`
file names). They run on distributions with the C library of Ubuntu 22.04 or
newer. Use the `.deb` on Debian, Ubuntu and their derivatives, and the
AppImage on other distributions.

If `zenity` is installed, the daemon shows an icon while it runs; without
it, the daemon runs the same, with no icon.

### Debian, Ubuntu and derivatives (`.deb`)

```sh
sudo apt install ./openflux-studio_<version>_<arch>.deb
```

The package installs:

- udev rules (`/usr/lib/udev/rules.d/70-openflux.rules`) that let the
  logged-in user use the keyboard and its boot loader without root;
- a systemd user service, `openfluxd.service`, enabled for every user.

The service is enabled system-wide, so `systemctl --user disable` doesn't
stop it from starting. To stop it and keep it from starting for your user:

```sh
systemctl --user mask --now openfluxd.service
```

`systemctl --user unmask openfluxd.service` undoes this; the service then
starts again at your next login.

**Updates.** Studio tells you when a release is available and links to
its package; install it the same way.

**Uninstalling.** `sudo apt remove open-flux-studio`. Your data in
`~/.local/share/openflux` is kept.

### AppImage

1. Download `openflux-studio_<version>_<arch>.AppImage` and make it
   executable:

   ```sh
   chmod +x openflux-studio_*.AppImage
   ```

2. Once per computer, install the udev rules the AppImage carries. They let
   you use the keyboard without root:

   ```sh
   ./openflux-studio_*.AppImage --appimage-extract usr/share/openflux/70-openflux.rules
   sudo cp squashfs-root/usr/share/openflux/70-openflux.rules /etc/udev/rules.d/
   sudo udevadm control --reload-rules && sudo udevadm trigger
   rm -rf squashfs-root
   ```

   If the keyboard was already plugged in and Studio can't reach it,
   unplug it and plug it in again.

3. Run it:

   ```sh
   ./openflux-studio_*.AppImage
   ```

The AppImage installs nothing else on the system. Studio starts `openfluxd`
from the AppImage whenever it opens, and the daemon then runs until your
session ends; it does not start at login by itself.

**Updates.** Studio replaces the AppImage in place when you choose to
update.

**Uninstalling.** Delete the AppImage, and
`/etc/udev/rules.d/70-openflux.rules` if you installed it. Your data in
`~/.local/share/openflux` is kept.

## Beta releases

Beta releases (versions such as `1.1.0-beta.2`) are offered only after you
choose **Beta** as the **Update channel** on Studio's **Updates** page.
Leaving the beta channel moves you to the next stable release. See
[compatibility.md](compatibility.md#channels).

## Building from source

See [CONTRIBUTING.md](../CONTRIBUTING.md). Builds made on your own machine
are unsigned and update only by rebuilding.
