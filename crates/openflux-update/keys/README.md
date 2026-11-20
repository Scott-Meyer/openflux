# Embedded public keys

The host trusts updates signed by the keys in this directory. The crate
embeds both files at build time and exposes them as
`openflux_update::keys::trust_anchors()`, so the daemon and `openflux-release`
share one source.

| File | Key | Signs |
|---|---|---|
| `root.pub` | root key | key statements only: trusting a new release key, revoking one |
| `release.pub` | initial release key | `index.json` and every artifact |

Both files are minisign public keys.

## Key handling

The root secret key is kept offline and is used only to sign key statements
(`openflux-release statement`). The release secret key exists only as the
signing secret of the release environment, which only the release workflow can
read. Release builds enable the `production-keys` feature, which refuses to
build if either file holds one of the placeholder development keys listed in
`src/keys.rs`, for which no secret key exists.

Tests never use these keys; they generate their own.

Rotating the release key does not change these files: the root key signs a
statement trusting the next key, and the release workflow adds it to the
index (`openflux-release index --statement`). Hosts built with this
`release.pub` learn the next key from that statement.
[docs/releasing.md](../../../docs/releasing.md#keys-and-rotation) describes
the procedure.
