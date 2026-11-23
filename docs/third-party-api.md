# Third-party app API

Keyglass implements the published Flux keyboard third-party API, documented
at <https://github.com/FluxKeyboard/flux_api>, so existing apps that work with
that API work with Keyglass unchanged: editor and desktop plugins that switch
profiles and shortcut contexts, report the active application or input
language, add Mosaic modules, style keys or import configurations.

The published documentation is the reference for the routes, request and
response bodies and WebSocket messages. This page describes how `openfluxd`
provides the API: how apps find it, how they are approved, what the user
controls, and where Keyglass adds to it.

The implementation is `host/daemon/src/api/third_party.rs`. Its contract
tests, `host/daemon/tests/third_party_api.rs`, pin every part of the wire
format that apps depend on.

## Discovery

The API is served by `openfluxd` over HTTPS and secure WebSockets on the
loopback interface, on port 52323. If that port is taken, the daemon uses a
free port and announces it in the discovery file.

The discovery file is `keys.json`, written where apps look for it:

| Platform | Location |
|---|---|
| macOS | `~/Documents/flux/config/keys.json` |
| Windows | `%USERPROFILE%\Documents\flux\config\keys.json` |
| Linux | `~/flux/config/keys.json`, `$XDG_DATA_HOME/flux/config/keys.json` and `~/.local/share/flux/config/keys.json` |

It contains:

| Field | |
|---|---|
| `apiActive` | `true` while the daemon is running and the API is turned on; set to `false` when the daemon stops |
| `openPort` | the port the API listens on |
| `publicKeyLocation` | a directory holding the API's certificate as `cert.pem` |
| `certHash` | SHA-256 of the certificate (DER), as hex |
| `saveToKeyboard`, `import`, `profileChange`, `shortcutChange`, `mosaicChange` | the user's permission settings, below |
| `banned` | apps the user has denied |

The certificate is self-signed for `localhost`, `127.0.0.1` and `::1`. Apps
trust it by loading `cert.pem` from `publicKeyLocation`; the daemon also
places copies in the other certificate locations the published API names.
Fields of an existing `keys.json` that Keyglass does not manage are kept as
they are. `keys.json` can be read only by the user, because the tokens of
apps may be stored in it.

## Enrollment and consent

An app asks for access with `POST /v1/authentication/register` and names
itself with its `User-Agent` header. The request stays open until the user
decides:

1. Keyglass Studio shows the request with the app's name.
2. If the user approves, the response is `200` with `{"message": "<token>"}`.
3. If the user denies, the response is `403` with
   `{"message": "User denied the request"}`. The app is remembered as
   denied and further requests are refused at once, until the user removes
   it from the denied list in Studio.
4. If nobody answers within two minutes, the response is `408`. The app can
   ask again.

Several requests from the same app while one is pending wait for the same
decision.

Apps send the token as `Authorization: Bearer <token>`, or as a `token`
query parameter where a header can't be set (WebSocket clients). An app whose
token is refused registers again.

Requests that carry an `Origin` header are accepted only from
`http(s)://localhost`, `127.0.0.1` or `[::1]` (any port) and Studio's own
origin, so pages served from other origins can't use the API.

## Permissions

In Studio's settings for third-party apps, the user can turn the API off
entirely, and can allow or refuse each capability:

| Setting | Governs |
|---|---|
| `saveToKeyboard` | `POST /v1/config/save` |
| `import` | `POST /v1/config/import` |
| `mosaicChange` | `POST /v1/config/addGenericTile`, `PUT /v1/config/updateGenericTile` |
| `profileChange` | WebSocket `changeProfile` |
| `shortcutChange` | WebSocket `changeContext` |

All are allowed by default. A refused HTTP request is answered `403` with
`{"message": "User denied the request"}`; a refused WebSocket request with
status `403` and `"User denied request"`. While the API is off, requests are
answered `503`.

Studio lists the approved apps. Revoking an app removes its tokens, closes
its open WebSocket connections with close code `1008`, and optionally adds
it to the denied list. Turning the API off or denying an app closes its
connections the same way.

## Keyglass behavior

- **Scope of third-party tokens.** A token issued to a third-party app can
  use only the routes of this API. The rest of the daemon's API, which
  Keyglass Studio uses, refuses it.
- **Network.** The API is reachable only from the same computer. The
  `apiNetworkAccess` and `domain` settings of the published API are kept in
  `keys.json` but have no effect.
- **Key codes.** Apps see Keyglass's own key codes under the names of the
  published API (`KC_FF_*`); Keyglass calls them `KC_OF_*`. Binary values are
  identical, and names are translated in both directions.
- **Active application and input language.** On every platform, an app can
  report the active process (`POST /v1/config/updateActiveProcess`) and the
  input language (`POST /v1/config/updateInputLanguage`). Reports from apps
  take precedence over the daemon's own detection for as long as they keep
  arriving. A process report switches to the profile whose application rule
  matches it.
- **Profile switching.** `changeProfile` accepts a profile's name (case
  insensitive) or its ID. It answers `503` while the keyboard is not
  connected and `409` when the profile has not been saved to the keyboard
  yet.
- **Import.** `POST /v1/config/import` accepts `.flux` files as described in
  the published API, and Keyglass bundles (`.openflux`). The content must
  match the file extension.
- **Events.** WebSocket clients receive `{"event": ..., "data": ...}` messages
  when the configuration, the active profile, the shortcut context, the
  input language or a generic Mosaic module changes.
- **Existing settings.** When the daemon starts, it adopts the tokens,
  denied apps and permission settings it finds in existing `keys.json` files,
  so apps approved before keep working.
- **Discovery.** `GET /` answers with a short text naming the API, and
  `GET /v1/docs` with the link to the published documentation. Neither needs
  a token.
