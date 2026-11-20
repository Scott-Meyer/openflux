# openflux-update

Release metadata, update trust and compatibility planning for OpenFlux. The
host daemon uses the library to decide what to offer and to verify what it
installs. The release workflow uses the `openflux-release` binary to produce
and sign what gets published.

The library is pure: it reads what it is given and never touches the
network, the keyboard or the disk. Persistence of trust state is behind the
`TrustStore` trait.

## Modules

| Module | Purpose |
|---|---|
| `compat` | Parses and validates `compat.toml`, derives each component's interface declarations and the host's `HostProfile`. |
| `manifest` | `release.json` (`Release`) and `index.json` (`Index`), including the merge rule for a release seen twice. |
| `trust` | Minisign keys and signatures, root key statements, `verify_index`, `verify_bundled_index`, `verify_artifact`. |
| `factory` | The factory software reference (`factory-reference.toml`) the signed index carries. |
| `keys` | The public keys built into every host (`keys/`), exposed as `trust_anchors()`. |
| `engine` | Keyboard mode, per-component status, validated update plans, downgrades and conversion proposals. |
| `signing` | Key generation and signing, for the release pipeline only (`signing` feature). |

[docs/compatibility.md](../../docs/compatibility.md) explains the model, and
[docs/releasing.md](../../docs/releasing.md) how the release workflows use
this crate.

## Features

| Feature | Default | Effect |
|---|---|---|
| `cli` | yes | Builds `openflux-release` (adds `clap`, `ureq`). Implies `signing`. |
| `signing` | via `cli` | Exposes the `signing` module. |
| `production-keys` | no | Fails the build if `keys/` holds a placeholder development key (`DEVELOPMENT_KEYS` in `src/keys.rs`). Release builds enable it. |

The daemon depends on the crate with `default-features = false`.

## Using it from the daemon

```rust
let compat = Compat::parse(openfluxd::updates::COMPAT_TOML)?;
let profile = compat.host_profile();
let anchors = openflux_update::trust_anchors();

// Bundled with the installer: same signature rules, no expiry or sequence check.
let bundled = verify_bundled_index(&bundled_json, &bundled_sig, &anchors, &store.load()?)?;
let mut catalog = Catalog::from_index(&bundled);
// Fetched online: full checks, and the store is updated before this returns.
let online = verify_index(&json, &sig, &anchors, &mut store, OffsetDateTime::now_utc())?;
catalog.add_index(&online)?;

let engine = Engine::new(&profile, &inventory, &catalog, Preferences { beta: false });
match engine.mode() {
    KeyboardMode::OpenFlux => { /* engine.status(), engine.plan(&PlanRequest::only(Component::Bridge)) */ }
    KeyboardMode::NeedsConversion { .. } => { /* engine.propose_conversion() for the conversion module */ }
}
// Before installing any artifact:
verify_artifact(file, &artifact, online.trusted_keys())?;
```

After every executed step, identify the keyboard again and plan again. The
engine is deterministic and the rest of a valid plan is again a valid plan.

## `openflux-release`

Releases are published only by the release workflows. The signing keys exist
only as secrets of the release environment, so a local build of this tool can
check and inspect but cannot publish.

```text
openflux-release manifest --component <host|firmware|bridge|display> --version <semver>
    --channel <stable|beta> --compat compat.toml --artifact <path>... --base-url <url>
    --notes-url <url> [--released <rfc3339>] -o release.json
openflux-release index --releases <dir> --previous <index.json|none> --expires-days <n>
    [--pubkey <release.pub>] [--statement <file>]... [--signature <artifact>.sig]...
    [--withdraw <component>@<version>]... [--factory factory-reference.toml]
    [--allow-unsigned] -o index.json
openflux-release sign --key-env <VAR> [--password-env <VAR>] <file>        # writes <file>.sig
openflux-release verify --pubkey <file> [--pubkey <file>]... [--signature <file>] <file>
openflux-release keygen --out-dir <dir> [--name <stem>]                     # offline use only
openflux-release statement --root-key-env <VAR> [--password-env <VAR>]
    (--trust <file.pub> | --revoke <key id> [--reason <text>]) -o <file>
openflux-release compat [--compat compat.toml]
openflux-release factory-check <factory-reference.toml>
openflux-release bundle --index <url|path> --pubkey <file>... [--root-pubkey <file>]...
    [--channel stable|beta] --component <c>... [--allow-missing] -o <dir>
```

Order in the release workflow: build, `sign` each artifact, `manifest`
(embeds `<artifact>.sig`), `index` (merges the published index with the new
releases and increments its sequence), `sign` the index. `index` refuses
unsigned artifacts unless `--allow-unsigned` is given for a test index. The
first index needs `--pubkey` so it can record the key that signs it; later
indexes inherit the previous key unless `--pubkey` names the active one.

`manifest` checks that `--version` matches the component's version in
`compat.toml`, which the release tooling keeps in step.

A published release is immutable. Merging the same release from two places
keeps it withdrawn if either says so, combines artifact signatures (one per
key), and fails on any other difference. The result does not depend on merge
order.

`index --factory factory-reference.toml` records the factory software
reference in the index, covered by its signature. Without `--factory` the
previous index's reference is kept. Hosts read it with
`VerifiedIndex::factory()` or `Catalog::factory()`; a catalog built from
several indexes uses the reference from the one with the highest sequence.

The same file's optional `[press_refinement_source]` table is recorded next to
it as `press_refinement_source`: where the keyboard maker's press-refinement
data is found (`sources`, each with a `kind`, such as `container`, with its URL,
size and SHA-256) and the size and SHA-256 of the member, the image and the
payload. Fields are strict; a source kind a host doesn't know is skipped
rather than rejected, and `factory-check` and `index` refuse to publish one.
Hosts read it with `Catalog::press_refinement_source()`, from the newest index
that records one. Readers that do not recognize this field ignore it.

Rotating the release key uses `statement --trust`, then
`index --pubkey <new key> --signature <artifact>.sig ...` to add the new
key's signature to every published artifact (each is checked against the
published file and the new key first), and finally `statement --revoke`.
Hosts built with only the original keys follow the rotation through the
root-signed statements.
[docs/releasing.md](../../docs/releasing.md#rotating-the-release-key) gives
the procedure.

`bundle` fetches `index.json` and `index.json.sig`, verifies them, picks the
newest eligible release of each component, downloads and verifies every
artifact, and writes the index, its signature and the artifacts into one flat
directory for the host installer. Relative artifact URLs are resolved against
a local index's directory, which is how the tests use it.

## Tests

From the repository root:

```sh
cargo test -p openflux-update
cargo clippy -p openflux-update --all-targets -- -D warnings
```

The engine, trust and CLI tests are table-driven. Engine tests use a fixed
registry in `tests/common/mod.rs`, so they describe the model independently
of the repository's versions; `tests/compat.rs` checks the repository's
`compat.toml`.
