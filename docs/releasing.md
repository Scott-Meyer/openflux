# Releasing Keyglass

Keyglass is released as four independently versioned units, with a clear split between what humans download and what `keyglassd` / Studio fetch automatically:

| Unit | Git tag | Where it publishes | Artifacts |
|---|---|---|---|
| **host** | `host-vX.Y.Z` | **Public GitHub Release** (`Keyglass Studio vX.Y.Z`, always `Latest`) + Tauri feed on GitHub Pages | 6 Studio installers (`.dmg`, `_x64-setup.exe`, 2× `.AppImage`, 2× `.deb`) + `SHA256SUMS`; `.app.tar.gz` updater archive and FFmpeg GPL source on GitHub Pages |
| **firmware** | `firmware-vX.Y.Z` | **GitHub Pages** (`packages/firmware/X.Y.Z/`) | `keyglass-firmware-X.Y.Z.uf2` |
| **bridge** | `bridge-vX.Y.Z` | **GitHub Pages** (`packages/bridge/X.Y.Z/`) | `keyglass-bridge_X.Y.Z_aarch64.ipk`, `keyglass-factory-restore_X.Y.Z_aarch64.ipk` |
| **display** | `display-vX.Y.Z` | **GitHub Pages** (`packages/display/X.Y.Z/`) | `keyglass-display_X.Y.Z_aarch64.ipk` (LGPL FFmpeg source under `source/`) |

Keyboard components never create public GitHub Releases and never steal the `Latest` badge from Keyglass Studio. Users download Keyglass Studio from GitHub Releases; Studio bundles the latest stable keyboard software and `keyglassd` delivers independent keyboard updates over the air from GitHub Pages.

## How a release happens

Commits on `main` follow [Conventional Commits](https://www.conventionalcommits.org/). `release-please` keeps one release pull request open per unit (`chore(main): release <unit> <version>`), updating its `CHANGELOG.md` and version files (`Cargo.toml`, `Cargo.lock`, `host/studio/package.json`, `keyboard/firmware/CMakeLists.txt`, and `compat.toml`).

Merging a release pull request runs `.github/workflows/release-please.yml` automatically from start to finish (no manual approval clicks):

1. **Tagging**: `release-please` creates the tag and GitHub Release for `host`. Keyboard units have `"skip-github-release": true`; the workflow diffs `.release-please-manifest.json` and pushes their git tags (`firmware-v*`, `bridge-v*`, `display-v*`) without creating GitHub Releases.
2. **Building**:
   - `release-firmware.yml` and `release-keyboard.yml` build reproducibly twice in clean Docker environments, verify packages, and upload a `staged-keyboard-*` workflow artifact.
   - `release-host.yml` bundles the latest stable keyboard software from the live signed index, builds and platform-signs the 6 Studio installers + macOS `.app.tar.gz` updater archive + FFmpeg GPL source, uploads **only the 6 installers + `SHA256SUMS`** (with build provenance attestations) to the GitHub Release, and uploads a `staged-host` workflow artifact.
3. **Publishing (`publish-index.yml`)**:
   - Runs in the `release-signing` environment (branch-protected to `main`).
   - Signs newly staged artifacts (`stage-releases.sh`), mirrors existing live GitHub Pages static files (`site-manifest.json`), merges new releases into `index.json` (365-day expiry, monotonic sequence), signs `index.json`, updates Studio's Tauri updater feeds (`updates/<channel>/latest.json`) when a host release is staged, and deploys to GitHub Pages.

When keyboard components and `host` are released together, keyboard builds publish to Pages first, then `host` bundles those newly published keyboard packages and publishes the updated index and Studio feed.

## Beta releases

Betas use SemVer prereleases (`X.Y.Z-beta.N`) on the same tags and publish to the `beta` channel. Add a `Release-As: X.Y.Z-beta.1` footer to start a beta series, and set `"versioning": "prerelease", "prerelease-type": "beta", "prerelease": true` on the package in `release-please-config.json` while iterating.

## Rebuilding or withdrawing a release

- **Rebuild an existing tag**: run Actions → `Release host`, `Release firmware`, or `Release keyboard package` → Run workflow with the existing tag. Published GitHub Release assets are immutable; if a host installer was already uploaded with different bytes, cut a new patch version.
- **Withdraw a release**: run Actions → `Publish index` → Run workflow with `withdraw` set to `<component>@<version>` (space-separated for multiple).
- **Monthly refresh**: `publish-index.yml` runs automatically on the 1st of each month to refresh `index.json`'s 365-day expiry without re-downloading Studio installers.

## Keys and rotation

| Key | Where it lives | Signs |
|---|---|---|
| Root key | offline only | statements that trust or revoke release keys |
| Release key | `KEYGLASS_RELEASE_KEY` in `release-signing` | `index.json`, every artifact |
| Updater key | `TAURI_SIGNING_PRIVATE_KEY` in `release-signing` | Studio update archives in `updates/*/latest.json` |
| Developer ID certificate | `release-build` | macOS app and disk image (Apple) |
| SignPath certificate | SignPath | Windows executables and the setup program |

To rotate the release key from A to B:
1. Trust B with an offline root-signed statement in `host/packaging/keys/statements/`, move `active-release.pub` to `previous-release.pub`, and commit B as `active-release.pub`.
2. Update `KEYGLASS_RELEASE_KEY` in `release-signing` to B and run Actions → `Publish index` with `resign` checked.
3. Commit a root-signed revocation statement for A, remove `previous-release.pub`, and run `Publish index` once more.

## Published files

GitHub Releases (`https://github.com/keyglass-dev/keyglass/releases`):
- Only `Keyglass Studio vX.Y.Z` releases, containing the 6 installers + `SHA256SUMS`.

GitHub Pages (`https://scott-meyer.github.io/keyglass/`):
- `index.json`, `index.json.sig`: signed update index for `keyglassd`
- `site-manifest.json`: sha256 inventory used across deploys
- `packages/<component>/<version>/...`: keyboard `.uf2` and `.ipk` OTA packages
- `updates/stable/latest.json` (+ active `.app.tar.gz`) and `updates/beta/latest.json`: Tauri updater feeds
- `source/`: FFmpeg GPL/LGPL corresponding source archives, licenses, and build info

## Testing the release scripts

CI (`ci.yml`, job `release-scripts` and `shell`) runs `shellcheck`, `actionlint`, and dry runs of the release scripts, including `.github/scripts/test-publish-index.sh` and `.github/scripts/test-upload-release-assets.sh`.
