# Compatibility and updates

OpenFlux has four parts that are released separately:

| Component | Runs on | Updated by |
|---|---|---|
| host | your computer: the daemon and OpenFlux Studio | Studio's updater, or the system package manager for the `.deb` |
| firmware | the key scanner in the keyboard | the host, over the firmware's USB boot interface |
| bridge | the keyboard's computer, linking USB, the key scanner and the display | the host, over the bridge's USB connection |
| display | the keyboard's computer, drawing the screen | the host, through the bridge |

Because each part has its own version, OpenFlux needs a clear rule for which
versions work together. That rule is the interface registry, `compat.toml`,
at the root of the repository.

## Interfaces

Wherever two components talk to each other, or one reads data the other
writes, there is a named interface with a `MAJOR.MINOR` version:

| Interface | Between | Kind |
|---|---|---|
| `bridge-usb` | host and bridge | protocol |
| `firmware-usb` | host and firmware | protocol |
| `uart` | bridge and firmware | protocol |
| `display-ipc` | bridge and display | protocol |
| `keymap` | host and firmware | data |
| `profile-settings` | host and firmware | data |
| `calibration-lfs` | host and firmware | data |
| `config` | host and display | data |

Each interface has exactly one component that **provides** it (implements it
and defines its version) and one or more components that **require** it.

- A **minor** version only adds. A provider at `1.4` works with every
  consumer that needs anything from `1.0` to `1.4`.
- A **major** version breaks. `2.0` does not work with consumers of `1.x`.
- A requirement therefore always stays within one major: `>=1.2, <2.0`. A
  component that works with two majors lists two requirements.

**Protocol** interfaces are conversations. A consumer that knows `1.2` keeps
working with a provider at `1.4`; it just does not use the additions.

**Data** interfaces are stored formats. A host that knows `keymap 1.2` can
read a keymap stored as `1.4`, but writing it back could drop what `1.3` and
`1.4` added. So when the keyboard's data is newer than the host understands,
the host shows that area read-only and suggests updating OpenFlux.

Every release records what its component provides and requires, and which
version of each required interface it was built against. Those declarations
come from `compat.toml` at the commit the release was built from.

## What you see

For each component, OpenFlux shows one of these states:

| State | Meaning |
|---|---|
| Up to date | Nothing newer is available for your channel. |
| Update available | A newer version can be installed safely. If it is marked as required, the installed version does not fully work with the rest of your setup. |
| Needs a newer OpenFlux first | A newer version exists, but your OpenFlux app must be updated before it can be installed. |
| Keyboard newer than this OpenFlux | The keyboard uses interfaces this app does not fully understand. Affected areas are read-only or unavailable until you update OpenFlux. |
| Update blocked | A newer version exists but no safe way to install it was found. The reason is shown. |
| Incompatible | The installed version does not work with the rest of your setup and no release fixes that. |
| Conversion required | The keyboard is not fully running OpenFlux. The only keyboard action is converting it. |

Keyboard updates are never installed automatically. You choose when to
install, and the app shows every step before it starts.

## Fully OpenFlux keyboards and conversion

OpenFlux updates a keyboard component by component only when every keyboard
component runs OpenFlux 1.0 or later and identifies itself. Such a keyboard
is in the **OpenFlux** mode described in the rest of this page.

If any component runs something else (the keyboard's factory software, an
OpenFlux development build, or software that cannot be identified),
the keyboard is in the **conversion** mode. OpenFlux does not mix its
components with other software, so in this mode:

- the only keyboard action offered is converting the whole keyboard to
  OpenFlux, which installs firmware, bridge and display together;
- no individual keyboard update, companion update or downgrade is planned;
- updates of the OpenFlux app itself remain available.

The conversion carries the keyboard's settings over and leaves its
calibration untouched. It does not keep a copy of the software it replaces:
[resetting to factory software](converting-a-keyboard.md#reset-to-factory-software)
downloads the factory version listed in the signed update index.

## Update plans

An update can involve more than one component, and the order matters. The
host plans each update as a sequence of single-component steps and checks
every state along the way, not just the final one:

- **The host never loses its way in.** Every component the host can reach
  before the update stays reachable after every step, over the interface it
  is installed and recovered through. A step that fails can always be retried
  or rolled back.
- **Nothing that works breaks.** The final state meets every requirement that
  was met before, and leaves no area read-only that was editable before.
- **Short interruptions are named.** Sometimes two components must change
  together and no order avoids a moment where one waits for the other, for
  example the display showing no live data until it is updated right after
  the bridge. Orders without such moments are preferred; when one is
  unavoidable, the step says what pauses.
- **No fixed order.** The order is found for each update. Sometimes the
  firmware must go first, sometimes the bridge.
- **Companions.** If the version you asked for needs another component to
  change too, that component is included and marked as such. Components are
  not updated without need.
- **Transitional releases.** Moving an interface to a new major can need an
  intermediate release that speaks both majors, so each side can move in
  turn. The plan includes such a release when one exists.
- **Refusal.** When no safe order exists, nothing is installed and the reason
  is shown, for example "display needs display-ipc >=1.0, <2.0 but bridge
  provides 2.0".

Updates only move forward. Going back to an earlier version is a separate,
explicit action, and it is checked by the same rules.

## Channels

Stable releases are offered to everyone. Beta releases are versions such as
`1.1.0-beta.2` and are offered only after you opt in. Versions are ordered as
SemVer orders them: `1.1.0-beta.2` comes before `1.1.0-beta.10`, which comes
before `1.1.0`. If you leave the beta channel while running a beta, you get
the next stable release that is newer than it.

A release can be withdrawn by its publisher. A withdrawn release is never
offered, and if it is installed the app says so.

## Trust

The keyboard cannot check what it is given, so the host checks everything
before it reaches the keyboard.

- Releases are published only by the project's release workflow. The signing
  key exists only as a secret of that workflow's protected environment.
- `index.json` lists every release with the SHA-256, size and signatures of
  each file. It is signed by the **release key**, and every file is signed by
  it too. After the release key changes, files are signed again by the next
  key and carry both signatures; a host accepts a file signed by any key it
  trusts. Signatures use the minisign format, so anyone can check a download
  with the `minisign` tool and the published public key.
- The **root key** is kept offline. It signs only statements that trust a new
  release key or revoke one. The host is built with the root public key and
  the initial release public key, and learns later release keys from
  root-signed statements in the index. A revoked key is never trusted again.
- Each index has a sequence number and an expiry time. The host remembers the
  highest sequence it has accepted and refuses older ones, so an earlier index
  cannot be replayed to hide newer releases. It refuses expired indexes, so
  a stale index cannot be served indefinitely.
- Installers carry keyboard software with the signed index that lists it, for
  installing without a network connection. That bundled index is checked
  against the same keys and revocations, but not for expiry or sequence,
  since it is expected to be older than what the host has seen online.

## For contributors

When you change a boundary between components:

1. Find its interface in `compat.toml`.
2. If the change only adds (a new command, an optional field), bump the
   interface's minor version. Consumers keep working.
3. If a consumer starts relying on the addition, raise its requirement, for
   example from `>=1.0, <2.0` to `>=1.1, <2.0`.
4. If the change breaks existing consumers, read the next section first.
5. Run `openflux-release compat` to check the registry. It reports any
   requirement that the provider does not satisfy.

Interfaces are never renamed or reused; a different meaning gets a different
name.

### Breaking changes

A major bump cuts off every consumer of the previous major. For an update
transport (`bridge-usb`, `firmware-usb`) that would leave the host unable to
reach the keyboard, so the update engine refuses such updates unless a
transitional path exists. To move an interface to a new major:

1. Release the provider with both majors, by listing the previous one under
   `also-provides`:

   ```toml
   [interfaces.bridge-usb]
   version = "2.0"

   [components.bridge]
   provides = ["bridge-usb", "display-ipc"]
   also-provides = { bridge-usb = ["1.4"] }
   ```

2. Release consumers that accept the new major, alone or alongside the previous
   one (`bridge-usb = [">=1.4, <2.0", ">=2.0, <3.0"]`).
3. Only after that, drop the previous major from the provider.

The update engine finds the order that uses these releases.

### Versions in `compat.toml`

Each component's `version` in `compat.toml` is kept in step with its releases
by the release tooling. The release workflow refuses to describe a release
whose version does not match it.
