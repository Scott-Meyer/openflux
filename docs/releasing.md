# Releasing OpenFlux

OpenFlux is released as four independently versioned units:

| Unit | Tag | Artifacts (listed in the update index) | Other release files |
|---|---|---|---|
| host | `host-vX.Y.Z` | OpenFlux Studio with `openfluxd`: macOS universal `.dmg` and `.app.tar.gz`, Windows `_x64-setup.exe`, Linux x86_64 and aarch64 `.AppImage` and `.deb` | FFmpeg source, build information and license; SBOMs |
| firmware | `firmware-vX.Y.Z` | `openflux-firmware-X.Y.Z.uf2` | `openflux-firmware-licenses.txt`: license texts of littlefs, the Pico SDK and TinyUSB |
| bridge | `bridge-vX.Y.Z` | `openflux-bridge_X.Y.Z_aarch64.ipk`, `openflux-factory-restore_X.Y.Z_aarch64.ipk` | SBOM |
| display | `display-vX.Y.Z` | `openflux-display_X.Y.Z_aarch64.ipk` | `openflux-ffmpeg-source-*.tar.xz`, `openflux-ffmpeg-build.txt`; SBOM |

`openflux-factory-restore` is the package that starts a reset to the
factory software; the host installs it after staging the factory packages.

Releases are made only by the GitHub Actions workflows in this repository,
on GitHub-hosted runners. Tags, release artifacts, signatures and the
update index all come from those workflows, and the signing keys exist only
as secrets of GitHub environments. A build on a developer machine is for
development: it can't sign, tag or publish anything.

## How a release happens

Commits on `main` follow [Conventional Commits](https://www.conventionalcommits.org/).
release-please keeps one release pull request open per unit, named
`chore(main): release <unit> <version>`. It collects the unit's changes into
its `CHANGELOG.md` and sets the next version everywhere it is recorded:

- host: `[workspace.package].version` in `Cargo.toml`, the host crates'
  entries in `Cargo.lock`, `host/studio/package.json` and its lock file.
  Studio's `tauri.conf.json` has no version of its own and uses the crate's.
- firmware: the `set(OPENFLUX_FIRMWARE_VERSION "X.Y.Z")` line in
  `keyboard/firmware/CMakeLists.txt`, marked `# x-release-please-version`
- bridge, display: `keyboard/<unit>/Cargo.toml` and `Cargo.lock`
- every unit: `components.<unit>.version` in `compat.toml`

A commit belongs to a unit when it changes files under the unit's
directory (`host/`, `keyboard/firmware/`, `keyboard/bridge/`,
`keyboard/display/`). `feat` makes a minor release, `fix`, `perf` and
`deps` a patch release, and a `!` or `BREAKING CHANGE:` footer a major
release. Commits that change only shared files (`crates/`, `assets/`,
`compat.toml`, the root `Cargo.toml` and `Cargo.lock`) release nothing by
themselves; they ship with the next release of the units that use them.

Merging a release pull request starts the release (`.github/workflows/release-please.yml`).
Tags created by the workflow token do not start other workflows, so this
workflow calls the build workflows directly, as reusable workflows:

1. release-please tags the merge commit and creates the GitHub release.
2. The unit's build workflow runs (`release-host.yml`, `release-firmware.yml`
   or `release-keyboard.yml`). It builds from the tag, applies platform
   signing, checks the result, and uploads the artifacts, `SHA256SUMS`,
   SBOMs and build information to the release, with a build provenance
   attestation for every file. `artifacts.txt`, the list of artifacts, is
   uploaded last; it marks the build as finished.
3. `publish-index.yml` waits for approval in the `release-signing`
   environment. Once approved, it signs each new artifact with the release
   key (`<artifact>.sig` on the release), writes the release's
   `release.json`, merges it into the next `index.json` together with the
   factory software reference (`factory-reference.toml`), signs the index,
   updates the Studio update feeds and deploys everything to GitHub Pages.
   It then checks that the site serves the new index and records its
   sequence number.

Until step 3 is approved, the release exists on GitHub but no OpenFlux
installation offers it.

Published release files are immutable. An upload that finds a file of the
same name already on the release skips it when the contents are identical
and otherwise fails with "published artifacts are immutable; cut a new
version". No workflow replaces a release file.

### Approving the signing step

The approval is the point where a release becomes available to users.
Before approving, check in the run that:

- the release is one you merged, and its build jobs passed
- the release's files are the expected set (the run lists every release in
  the next index, with withdrawn releases marked)

Then approve the `release-signing` deployment in the run's summary page.

### When keyboard and host release together

Host installers bundle the newest stable firmware, bridge and display from
the published index, verified against its signature. When a single merge
releases keyboard units and the host, the workflow runs in this order:

1. keyboard builds
2. publish the index (first approval)
3. host build, which bundles the keyboard releases from step 2
4. publish the index again (second approval)

That is why one release run can ask for two approvals. If a keyboard build
of the run fails, the host is not built in that run: rebuild the keyboard
unit, then run Release host for the host tag. A host build fails with an
explanation when the index has no stable release of any of the three
keyboard units (nothing published yet, or only betas); it never builds
installers with an empty or unverified bundle.

### A repository without releases

In a repository that has published nothing yet (`.release-please-manifest.json`
is empty), every unit starts at 1.0.0, its `initial-version` in
`release-please-config.json`. release-please opens a release pull request
only for a unit with at least one releasable commit under its directory.
The releases are cut keyboard first, then host:

1. Merge the release pull requests for firmware, bridge and display and
   approve their index publication. This publishes the first index (the
   workflow finds no published index and no recorded sequence, and starts
   at sequence 1).
2. Merge the host release pull request and approve its publication. Its
   build bundles the keyboard releases from step 1.

If all four release pull requests are merged together, the run follows this
order by itself. A host release whose keyboard units have not been
published stops at the keyboard bundle step with "no stable firmware
release published yet; release keyboard components first"; publish the
keyboard units and run Release host for the tag again.

## Beta releases

Betas are SemVer prereleases, `X.Y.Z-beta.N`, on the same tags. They are
published to the beta channel; users opt in per installation. GitHub marks
them as prereleases, and host installers always bundle stable keyboard
software.

To start a beta series for a unit, commit a change to that unit with a
`Release-As` footer:

```
feat(display): weather widget layouts

Release-As: 1.3.0-beta.1
```

For the rest of the series, set the unit's versioning in
`release-please-config.json` so that each release pull request proposes
the next beta number:

```json
"keyboard/display": {
  "versioning": "prerelease",
  "prerelease-type": "beta",
  "prerelease": true
}
```

To release the final version, either remove `"prerelease": true` (the next
release pull request proposes `1.3.0`) or commit with
`Release-As: 1.3.0`, and then return the unit's `versioning` to the default.

## Rebuilding a release

If a build fails for reasons outside the code (a runner or network fault),
run the unit's workflow by hand (Actions > Release host, Release firmware or
Release keyboard package > Run workflow) with the existing tag. It rebuilds
from the tag and, unless unchecked, publishes the index afterwards.

A rebuild can only add files the release does not have yet, or upload files
identical to the ones it has. The keyboard and firmware builds are
reproducible, so a rebuild of those usually matches. Host installers carry
platform signatures with timestamps, so once any host installer has been
uploaded, a host rebuild fails; the fix is a new patch release. The index
likewise refuses a `release.json` whose contents differ from the published
one.

CI does not start on release pull requests by itself, because release-please
opens them with the workflow token. Run the CI workflow on the release
branch by hand when its checks are needed; the release workflow also checks
each release pull request's `Cargo.lock` and `compat.toml`.

## When a release goes wrong

**Withdraw it.** Run Actions > Publish index > Run workflow with
`withdraw` set to the release, for example `bridge@1.2.0` (several are
separated by spaces), and approve. The release stays in the index marked
withdrawn: no installation is offered it, and installations that
have it show that it was withdrawn. Withdrawals are carried into every later
index. While another publication runs, the withdrawal waits in the queue,
and a newer publication (a release or the monthly run) that queues behind
it cancels it; check that the run finished, and start it again if it was
cancelled. Then fix forward with a new patch release. Don't delete the tag or
the GitHub release; their files stay available for anyone who needs to
recover or compare.

**A build failed.** A failed build may have uploaded some files, but the
index does not list the release until a build finishes. Follow
[Rebuilding a release](#rebuilding-a-release); if the uploaded files can't
be reproduced identically, fix the code and release the next patch version.

**The index is about to expire.** Every index expires (90 days by default)
so a frozen or replayed index is detected. The publish workflow runs on the
first of each month to issue a fresh index; approve it. It can also be run
by hand with a different `expires-days`.

**The published index is older than the last one.** Every publication
records the sequence it deployed as a deployment of the `update-index`
environment (repository > Deployments). The next publication refuses to
build on a published index whose sequence is lower than the newest record,
or on a missing index once one was recorded, because hosts that saw the
newer index would refuse whatever is built on the older one, and its
withdrawals would be lost. If this happens, find out what replaced the site,
then restore the newest index: open the publish run named in the newest
`update-index` deployment and re-run its `deploy` job, which deploys that
run's site again (its site artifact is kept for 90 days, longer than any
index stays valid when the monthly publication runs). Then publish again.

**Two publications at once.** Publications share one concurrency group that
covers reading the published index through recording the deployed one, so
they run one after another. A waiting run is replaced by a newer waiting
run. Every run collects all published releases again, so nothing is lost,
except the inputs of the replaced run (a withdrawal, for example): run it
again.

## Keys and rotation

| Key | Where it lives | Signs |
|---|---|---|
| Root key | offline only | statements that trust or revoke release keys |
| Release key | `OPENFLUX_RELEASE_KEY` in `release-signing` | `index.json`, every artifact |
| Updater key | `TAURI_SIGNING_PRIVATE_KEY` in `release-signing` | Studio update archives in the update feeds |
| Developer ID certificate | `release-build` | macOS app and disk image (Apple) |
| SignPath certificate | SignPath | Windows executables and the setup program |

Public halves in the repository:

| File | Use |
|---|---|
| `crates/openflux-update/keys/root.pub`, `release.pub` | Trust anchors compiled into `openfluxd`: the root key and the initial release key |
| `host/packaging/keys/active-release.pub` | The active release key. The publish workflow records it in the index (`index --pubkey`) and verifies every signature it makes against it |
| `host/packaging/keys/previous-release.pub` | Only during a rotation: the key being replaced, so the workflow can read the index it signed |
| `host/packaging/keys/statements/*.json` | Root-signed statements trusting or revoking release keys, added to every index |
| `host/packaging/keys/tauri-updater.pub` | The updater key, compiled into Studio |

Release builds of `openfluxd` and of the release tool enable the
`production-keys` feature, which fails the build if a trust anchor is one
of the placeholder development keys listed in `crates/openflux-update/src/keys.rs`.

Keys are generated offline:

```sh
cargo run -p openflux-update --bin openflux-release -- keygen --out-dir <offline dir> --name release
npm exec --prefix host/studio -- tauri signer generate -w <offline dir>/tauri-updater.key
```

### Rotating the release key

Rotating from key A to key B takes three publications. Hosts built with
only A follow it through the root-signed statements.

1. **Trust B.** Generate B offline. With the root key, write a statement
   that trusts it:
   `openflux-release statement --root-key-env OPENFLUX_ROOT_KEY --trust B.pub -o host/packaging/keys/statements/trust-<B id>.json`.
   Commit the statement, move `active-release.pub` (A) to
   `previous-release.pub`, and commit B as `active-release.pub`.
2. **Sign everything again with B.** Replace `OPENFLUX_RELEASE_KEY` (and its
   password) in `release-signing` with B. Run Actions > Publish index with
   `resign` checked, and approve. The run signs every artifact of every
   published release with B (`openflux-release sign`) and adds each
   signature with `openflux-release index --pubkey active-release.pub
   --signature <artifact>.sig`, which checks it against the published file
   and B first. Every artifact then carries both signatures in its
   `signatures` list, and the index, signed by B, carries the statement
   that trusts B. The run checks that a host built with only the root key
   and A accepts the result.
3. **Revoke A.** With the root key, write
   `openflux-release statement --root-key-env OPENFLUX_ROOT_KEY --revoke <A id> --reason "<why>" -o host/packaging/keys/statements/revoke-<A id>.json`,
   commit it, delete `previous-release.pub`, and publish again. Hosts then
   refuse any index signed by A and accept artifacts signed by B.

The `.sig` files on each GitHub release stay as they were uploaded
(release files are immutable); after a rotation the current signatures
are the ones in `index.json`. If the release key is compromised, do step 3
as soon as step 2 is published.

`.github/scripts/test-publish-index.sh` runs this whole procedure with
throwaway keys.

### The updater key

Studio trusts one updater key, built into the app, and requires every update
signature to name the version the feed announces (the updater's
`requireSignedVersion`). The publish workflow signs each update archive with
`tauri signer sign --app-version <version>` and checks that the signature
carries that version. Without this, a tampered feed could pair a new version
number with an older, validly signed release.

To change the updater key, release a host version that contains the next
public key while the feed is signed with the current key; once that version is
widely installed, switch the `TAURI_SIGNING_PRIVATE_KEY` secret.
Installations older than that version update through a manual download.

## Repository configuration

Environments:

- `release-build`: secrets for platform signing. Deployment branches: `main`.
- `release-signing`: release and updater keys. Required reviewers: the
  maintainers who approve releases. Deployment branches: `main`.
- `github-pages`: created by GitHub; Pages source set to GitHub Actions.
- `update-index`: created by the publish workflow when it first records a
  sequence. It holds no secrets.

Secrets (names only):

| Name | Environment |
|---|---|
| `OPENFLUX_RELEASE_KEY`, `OPENFLUX_RELEASE_KEY_PASSWORD` | `release-signing` |
| `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | `release-signing` |
| `APPLE_CERTIFICATE` (base64 `.p12`), `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY` | `release-build` |
| `APPLE_API_KEY_ID`, `APPLE_API_ISSUER`, `APPLE_API_PRIVATE_KEY` (App Store Connect API key for notarization) | `release-build` |
| `SIGNPATH_API_TOKEN` | `release-build` |

Variables (repository): `SIGNPATH_ORGANIZATION_ID`, `SIGNPATH_PROJECT_SLUG`,
`SIGNPATH_SIGNING_POLICY_SLUG`, and optionally `OPENFLUX_PAGES_URL` when the
site is served from a custom domain. The SignPath project has two artifact
configurations, `executables` (the `.exe` files in a zip) and `installers`
(the setup program).

Without the Apple or SignPath secrets the host builds run unsigned,
with a warning in the run; that is meant for forks and testing, not for
releases.

## Published files

GitHub Pages (`https://scott-meyer.github.io/openflux/`):

| Path | Contents |
|---|---|
| `index.json`, `index.json.sig` | update index used by `openfluxd` |
| `updates/stable/latest.json` | Studio update feed, stable channel |
| `updates/beta/latest.json` | Studio update feed, beta channel (newest stable or beta) |

Each GitHub release holds its artifacts and their `.sig` files,
`release.json`, `artifacts.txt`, `SHA256SUMS` and SBOMs.

Host releases also hold the files for the bundled FFmpeg tools,
`openflux-ffmpeg` and `openflux-ffprobe`. They are built with libx264 and
are therefore licensed under the GPL, version 2 or later, unlike the rest
of OpenFlux (MIT OR Apache-2.0); each host release's notes say so.

- `openflux-ffmpeg-source-<version>.tar.xz`: the complete corresponding
  source (the FFmpeg release, x264 at the pinned commit, and the build
  script)
- `openflux-ffmpeg-<target>.buildinfo.txt`: the configure lines and
  compiler of each build
- `openflux-ffmpeg-COPYING.GPLv2.txt`: the license

Display releases hold the source (`openflux-ffmpeg-source-*.tar.xz`) and
build configuration (`openflux-ffmpeg-build.txt`) of the LGPL FFmpeg build
inside the display package; the package carries its license.

A downloaded file can be checked by hand:

```sh
sha256sum --check --ignore-missing SHA256SUMS
openflux-release verify --pubkey host/packaging/keys/active-release.pub <file>
gh attestation verify <file> --repo Scott-Meyer/openflux
```

## Testing the release scripts

CI (`ci.yml`, job `release-scripts`) runs dry runs of the release scripts;
each also runs from the repository root on a development machine.

| Script | Checks |
|---|---|
| `.github/scripts/test-publish-index.sh` | first index, later indexes, refusal of an older published index, immutable uploads, key rotation with re-signing and revocation |
| `.github/scripts/test-upload-release-assets.sh` | identical files skipped, changed files refused |
| `.github/scripts/check-bundle-states.sh` | host bundling refuses an empty or beta-only index |
| `.github/scripts/test-package-hooks.sh` | `.deb` maintainer scripts against scripted `openfluxd` answers; service restart and stop settings |
| `.github/scripts/test-nsis-hooks.sh` | the NSIS hooks compile (needs `makensis`) |
| `.github/scripts/test-check-keyboard-ipk.sh` | keyboard package checks (needs an AArch64 cross compiler) |
| `.github/scripts/test-tauri-release-config.sh` | the Tauri release overlay for each OS |
