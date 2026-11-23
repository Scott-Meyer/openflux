//! The keyboard's third-party app API.
//!
//! Existing third-party apps (editor plugins, desktop-environment helpers and similar) talk to the
//! keyboard's desktop service through this API, the Flux keyboard API published at
//! <https://github.com/FluxKeyboard/flux_api> (OpenFlux notes: `docs/third-party-api.md`). Its wire
//! format is a fixed external contract: route paths, JSON field names, error messages, WebSocket
//! messages, the discovery files and the enrollment flow stay exactly as these apps expect.
//!
//! This module is the only place that uses the external names. Anything that crosses the boundary
//! is translated with [`KEYCODE_NAMES`] (the factory keycode names these apps use); the rest of
//! the daemon uses OpenFlux names only.
//!
//! Tokens issued to third-party apps can call only the routes in this module.

use crate::api::state::{
    ApiState, GenericTileConfig, PendingRegistration, RegistrationDecision, ShortcutProfileState,
    TokenKind,
};
use crate::bridges::layout_to_language_code;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

/// The published documentation of the API this module implements, served by `GET /v1/docs`.
pub const DOCS_URL: &str = "https://github.com/FluxKeyboard/flux_api";
/// Served by `GET /`.
pub const ROOT_BANNER: &str =
    "OpenFlux API, compatible with the Flux keyboard API: https://github.com/FluxKeyboard/flux_api";

/// Error message for a denied or previously denied registration.
pub const DENIED_MESSAGE: &str = "User denied the request";
/// App name used when a registering app sends no User-Agent.
pub const UNNAMED_APP: &str = "Unknown application";
/// Error message while the user has turned the third-party app API off.
pub const API_OFF_MESSAGE: &str = "The third-party app API is turned off";

// ---------------------------------------------------------------------------------------------
// Translation table
// ---------------------------------------------------------------------------------------------

/// Keycodes whose OpenFlux name differs from the name third-party apps use:
/// `(OpenFlux name, third-party name)`. Binary values are the same.
pub use crate::codecs::keycode_names::KEYCODE_NAMES;

/// Rewrites OpenFlux keycode names in `v` to the names third-party apps use.
pub fn keycodes_to_external(v: &mut serde_json::Value) {
    crate::codecs::keycode_names::to_factory(v);
}

/// Rewrites third-party keycode names in `v` to OpenFlux names.
pub fn keycodes_to_internal(v: &mut serde_json::Value) {
    crate::codecs::keycode_names::to_openflux(v);
}

// ---------------------------------------------------------------------------------------------
// Discovery files
// ---------------------------------------------------------------------------------------------

/// Where apps look for the API. Each `keys.json` holds `apiActive`, `openPort` and
/// `publicKeyLocation` (a directory containing `cert.pem`); some apps read `cert.pem` from a
/// fixed directory instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryLocations {
    pub keys_files: Vec<PathBuf>,
    /// Directories that receive a copy of the API certificate as `cert.pem`. The first one is
    /// what `publicKeyLocation` names.
    pub cert_dirs: Vec<PathBuf>,
}

/// Environment the discovery paths are derived from.
#[derive(Debug, Clone, Default)]
pub struct DiscoveryEnv {
    pub home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
    /// `%LOCALAPPDATA%` on Windows, where the shared token store lives.
    pub local_appdata: Option<PathBuf>,
}

impl DiscoveryEnv {
    /// The user's real locations, from `HOME` (`USERPROFILE` on Windows), `XDG_DATA_HOME`,
    /// `APPDATA` and `LOCALAPPDATA`.
    pub fn from_process() -> Self {
        let var = |k: &str| {
            std::env::var_os(k)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        #[cfg(windows)]
        let home = var("USERPROFILE").or_else(|| var("HOME"));
        #[cfg(not(windows))]
        let home = var("HOME");
        Self {
            home,
            xdg_data_home: var("XDG_DATA_HOME"),
            appdata: var("APPDATA"),
            local_appdata: var("LOCALAPPDATA"),
        }
    }

    /// Every location rooted at `root`, as if it were the user's home directory.
    pub fn rooted_at(root: PathBuf) -> Self {
        Self {
            appdata: Some(root.join("AppData").join("Roaming")),
            local_appdata: Some(root.join("AppData").join("Local")),
            xdg_data_home: None,
            home: Some(root),
        }
    }

    /// The locations the daemon uses for third-party discovery and for adopting earlier state.
    ///
    /// `OPENFLUX_HOME_OVERRIDE` roots every location in another directory (tests and development).
    /// Without it, a daemon given its own `OPENFLUX_DATA_DIR` is not the user's installed daemon,
    /// so it touches none of the user's locations: `None`. Otherwise the user's real locations.
    pub fn for_daemon() -> Option<Self> {
        if let Some(root) = std::env::var_os("OPENFLUX_HOME_OVERRIDE").filter(|v| !v.is_empty()) {
            return Some(Self::rooted_at(PathBuf::from(root)));
        }
        if std::env::var_os("OPENFLUX_DATA_DIR").is_some_and(|v| !v.is_empty()) {
            return None;
        }
        Some(Self::from_process())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryPlatform {
    Linux,
    MacOs,
    Windows,
}

impl DiscoveryPlatform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            DiscoveryPlatform::MacOs
        } else if cfg!(windows) {
            DiscoveryPlatform::Windows
        } else {
            DiscoveryPlatform::Linux
        }
    }
}

pub fn discovery_locations(platform: DiscoveryPlatform, env: &DiscoveryEnv) -> DiscoveryLocations {
    let mut keys_files = Vec::new();
    let mut cert_dirs = Vec::new();
    let Some(home) = env.home.clone() else {
        return DiscoveryLocations {
            keys_files,
            cert_dirs,
        };
    };
    let xdg = env.xdg_data_home.clone().filter(|p| p.is_absolute());
    match platform {
        DiscoveryPlatform::Linux => {
            keys_files.push(home.join("flux/config/keys.json"));
            if let Some(ref x) = xdg {
                keys_files.push(x.join("flux/config/keys.json"));
            }
            keys_files.push(home.join(".local/share/flux/config/keys.json"));
            cert_dirs.push(home.join(".local/share/com.fluxkeyboard.polymath/tls"));
            if let Some(ref x) = xdg {
                cert_dirs.push(x.join("com.fluxkeyboard.polymath/tls"));
            }
        }
        DiscoveryPlatform::MacOs => {
            keys_files.push(home.join("Documents/flux/config/keys.json"));
            cert_dirs.push(home.join("Library/Application Support/com.flux.polymath/tls"));
        }
        DiscoveryPlatform::Windows => {
            keys_files.push(
                home.join("Documents")
                    .join("flux")
                    .join("config")
                    .join("keys.json"),
            );
            let appdata = env
                .appdata
                .clone()
                .unwrap_or_else(|| home.join("AppData").join("Roaming"));
            cert_dirs.push(
                appdata
                    .join("com.fluxkeyboard")
                    .join("polymath")
                    .join("tls"),
            );
        }
    }
    keys_files.dedup();
    cert_dirs.dedup();
    DiscoveryLocations {
        keys_files,
        cert_dirs,
    }
}

/// The shared token store, `flux/config/api_tokens.json` under the local data directory. Tokens in
/// it were issued to third-party apps.
pub fn shared_token_store(env: &DiscoveryEnv) -> Option<PathBuf> {
    if cfg!(windows) {
        if let Some(l) = &env.local_appdata {
            return Some(l.join("flux").join("config").join("api_tokens.json"));
        }
    }
    env.home
        .as_ref()
        .map(|h| h.join(".local/share/flux/config/api_tokens.json"))
}

/// Reads the shared token store: a JSON array of token strings.
pub fn read_shared_tokens(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// Settings and migration of an existing keys.json
// ---------------------------------------------------------------------------------------------

/// The user's settings for the third-party app API. Field names match `keys.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThirdPartySettings {
    /// Whether third-party apps may use the API at all.
    pub api_active: bool,
    /// Kept for `keys.json`; the daemon only listens on the loopback interface.
    pub api_network_access: bool,
    /// `POST /config/save`.
    pub save_to_keyboard: bool,
    /// `POST /config/import`.
    pub import: bool,
    /// WebSocket `changeProfile`.
    pub profile_change: bool,
    /// WebSocket `changeContext`.
    pub shortcut_change: bool,
    /// `POST /config/addGenericTile` and `PUT /config/updateGenericTile`.
    pub mosaic_change: bool,
    /// Kept for `keys.json`; the certificate always covers localhost only.
    pub domain: serde_json::Value,
}

impl Default for ThirdPartySettings {
    fn default() -> Self {
        Self {
            api_active: true,
            api_network_access: false,
            save_to_keyboard: true,
            import: true,
            profile_change: true,
            shortcut_change: true,
            mosaic_change: true,
            domain: serde_json::Value::Null,
        }
    }
}

/// A capability third-party tokens need the user's permission for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    SaveToKeyboard,
    Import,
    ProfileChange,
    ShortcutChange,
    MosaicChange,
}

impl ThirdPartySettings {
    pub fn allows(&self, p: Permission) -> bool {
        match p {
            Permission::SaveToKeyboard => self.save_to_keyboard,
            Permission::Import => self.import,
            Permission::ProfileChange => self.profile_change,
            Permission::ShortcutChange => self.shortcut_change,
            Permission::MosaicChange => self.mosaic_change,
        }
    }
}

/// Routes governed by a permission (paths below `/v1`).
pub const ROUTE_PERMISSIONS: [(&str, Permission); 4] = [
    ("/config/save", Permission::SaveToKeyboard),
    ("/config/import", Permission::Import),
    ("/config/addGenericTile", Permission::MosaicChange),
    ("/config/updateGenericTile", Permission::MosaicChange),
];

/// WebSocket routes governed by a permission.
pub const WS_PERMISSIONS: [(&str, Permission); 2] = [
    ("changeProfile", Permission::ProfileChange),
    ("changeContext", Permission::ShortcutChange),
];

/// Fields of `keys.json` that the daemon writes; every other field is carried over unchanged.
const OWNED_FIELDS: [&str; 12] = [
    "apiActive",
    "openPort",
    "publicKeyLocation",
    "certHash",
    "apiNetworkAccess",
    "saveToKeyboard",
    "import",
    "profileChange",
    "shortcutChange",
    "mosaicChange",
    "domain",
    "banned",
];

/// What an existing `keys.json` holds for the daemon to adopt.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExistingDiscovery {
    /// Fields of the existing documents, merged with earlier locations taking precedence.
    pub document: serde_json::Map<String, serde_json::Value>,
    /// Tokens in `apiKeys`, with the app each was issued to when the file says.
    pub tokens: Vec<(String, String)>,
    /// Apps in `rejected` and `banned`.
    pub denied_apps: Vec<String>,
    /// Permission flags, if any document had them.
    pub settings: Option<ThirdPartySettings>,
}

/// Every place an existing `keys.json` may be, on any platform.
pub fn existing_keys_files(env: &DiscoveryEnv) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for platform in [
        DiscoveryPlatform::current(),
        DiscoveryPlatform::Linux,
        DiscoveryPlatform::MacOs,
    ] {
        for p in discovery_locations(platform, env).keys_files {
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

fn app_name_of(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(o) => ["name", "app", "appName", "userAgent", "clientName"]
            .iter()
            .find_map(|k| o.get(*k).and_then(|x| x.as_str()).map(|s| s.to_string())),
        _ => None,
    }
}

fn token_of(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Object(o) => ["key", "token", "apiKey"]
            .iter()
            .find_map(|k| o.get(*k).and_then(|x| x.as_str()).map(|s| s.to_string())),
        _ => None,
    }
}

/// Reads the existing `keys.json` files.
///
/// `apiKeys` is an object; an entry whose value is a string or an object with `key`/`token`/
/// `apiKey` is read as app name to token, any other entry as a token key. `rejected` (object keys or
/// entries) and `banned` (array of names or objects with a name) are denied apps.
pub fn read_existing_discovery(paths: &[PathBuf]) -> ExistingDiscovery {
    let mut out = ExistingDiscovery::default();
    for p in paths {
        let Some(doc) = std::fs::read_to_string(p)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|v| v.as_object().cloned())
        else {
            continue;
        };
        if let Some(keys) = doc.get("apiKeys").and_then(|v| v.as_object()) {
            for (k, v) in keys {
                let entry = match token_of(v) {
                    Some(tok) => (tok, k.clone()),
                    None => (
                        k.clone(),
                        app_name_of(v).unwrap_or_else(|| "Previously registered app".into()),
                    ),
                };
                if !out.tokens.iter().any(|(t, _)| *t == entry.0) {
                    out.tokens.push(entry);
                }
            }
        }
        if let Some(rej) = doc.get("rejected") {
            let names: Vec<String> = match rej {
                serde_json::Value::Object(o) => o.keys().cloned().collect(),
                serde_json::Value::Array(a) => a.iter().filter_map(app_name_of).collect(),
                _ => Vec::new(),
            };
            out.denied_apps.extend(names);
        }
        if let Some(b) = doc.get("banned").and_then(|v| v.as_array()) {
            out.denied_apps.extend(b.iter().filter_map(app_name_of));
        }
        if out.settings.is_none() && doc.contains_key("apiActive") {
            let mut flags = serde_json::Map::new();
            for k in OWNED_FIELDS {
                if let Some(v) = doc.get(k) {
                    flags.insert(k.into(), v.clone());
                }
            }
            out.settings = serde_json::from_value(serde_json::Value::Object(flags)).ok();
        }
        for (k, v) in doc {
            out.document.entry(k).or_insert(v);
        }
    }
    out.denied_apps.sort();
    out.denied_apps.dedup();
    out
}

/// Third-party access as the daemon's stores hold it, for writing `keys.json`.
#[derive(Debug, Clone, Default)]
pub struct Grants {
    /// Tokens issued to third-party apps that are still valid.
    pub tokens: std::collections::BTreeSet<String>,
    /// Apps that are refused when they register.
    pub denied: std::collections::BTreeSet<String>,
}

/// Everything needed to (re)write the discovery files.
#[derive(Debug, Clone)]
pub struct Discovery {
    pub locations: DiscoveryLocations,
    pub port: u16,
    pub cert_pem: String,
    pub cert_sha256: String,
    /// Fields of the existing `keys.json` carried over unchanged.
    pub preserved: serde_json::Map<String, serde_json::Value>,
}

impl Discovery {
    /// The `keys.json` document for the given settings and grants. The grant and denial fields
    /// carried over from an existing file (`apiKeys`, `rejected`, `banned`) keep only the entries the
    /// daemon's stores still hold.
    pub fn document(&self, settings: &ThirdPartySettings, grants: &Grants) -> serde_json::Value {
        let denied = &grants.denied;
        let mut doc = self.preserved.clone();
        if let Some(serde_json::Value::Object(keys)) = doc.get_mut("apiKeys") {
            keys.retain(|k, v| {
                grants
                    .tokens
                    .contains(&token_of(v).unwrap_or_else(|| k.clone()))
            });
        }
        match doc.get_mut("rejected") {
            Some(serde_json::Value::Object(rej)) => rej.retain(|k, _| denied.contains(k)),
            Some(serde_json::Value::Array(rej)) => {
                rej.retain(|e| app_name_of(e).is_some_and(|n| denied.contains(&n)))
            }
            _ => {}
        }
        let mut banned: Vec<serde_json::Value> = self
            .preserved
            .get("banned")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter(|e| app_name_of(e).map(|n| denied.contains(&n)).unwrap_or(false))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let listed: std::collections::BTreeSet<String> = banned
            .iter()
            .filter_map(app_name_of)
            .chain(match doc.get("rejected") {
                Some(serde_json::Value::Object(o)) => o.keys().cloned().collect::<Vec<_>>(),
                Some(serde_json::Value::Array(a)) => a.iter().filter_map(app_name_of).collect(),
                _ => Vec::new(),
            })
            .collect();
        for app in denied.iter().filter(|a| !listed.contains(*a)) {
            banned.push(serde_json::Value::String(app.clone()));
        }
        let flags = serde_json::to_value(settings).unwrap_or_default();
        if let Some(f) = flags.as_object() {
            for (k, v) in f {
                doc.insert(k.clone(), v.clone());
            }
        }
        doc.insert("banned".into(), serde_json::Value::Array(banned));
        doc.insert("openPort".into(), serde_json::json!(self.port));
        doc.insert("certHash".into(), serde_json::json!(self.cert_sha256));
        doc.insert(
            "publicKeyLocation".into(),
            serde_json::json!(self
                .locations
                .cert_dirs
                .first()
                .map(|d| d.to_string_lossy().to_string())
                .unwrap_or_default()),
        );
        serde_json::Value::Object(doc)
    }

    /// Writes the certificate copies and every `keys.json`. Returns the paths that failed.
    pub fn write(&self, settings: &ThirdPartySettings, grants: &Grants) -> Vec<(PathBuf, String)> {
        let mut failures = Vec::new();
        for dir in &self.locations.cert_dirs {
            let p = dir.join("cert.pem");
            if let Err(e) = write_file(&p, self.cert_pem.as_bytes(), false) {
                failures.push((p, e));
            }
        }
        let txt =
            serde_json::to_string_pretty(&self.document(settings, grants)).unwrap_or_default();
        for p in &self.locations.keys_files {
            // keys.json can hold tokens, so only the user may read it.
            if let Err(e) = write_file(p, txt.as_bytes(), true) {
                failures.push((p.clone(), e));
            }
        }
        failures
    }
}

fn write_file(path: &Path, content: &[u8], owner_only: bool) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    crate::api::tls::write_owner_only_file(path, content, owner_only)
}

const SETTINGS_FILE: &str = "third_party_settings.json";
/// Written once existing third-party state has been adopted and saved. Its presence means the
/// daemon's own stores are the only source of grants, denials and settings.
const MIGRATED_FILE: &str = "third_party_migrated.json";

/// Adopts existing third-party state once and writes the discovery files. Called once the API is
/// listening.
///
/// On the first start, tokens and denials found in an existing `keys.json` or in the shared token
/// store are added to the daemon's own stores, and the permission flags (including
/// `apiActive`) are adopted unless the user already has OpenFlux settings. When everything is
/// saved, a marker records that the import happened; later starts read only the daemon's stores,
/// so a revoked token or a forgotten denial stays that way. Existing files are read before
/// anything is written.
pub fn start(
    state: &ApiState,
    env: &DiscoveryEnv,
    platform: DiscoveryPlatform,
    port: u16,
    cert_pem: &str,
    cert_sha256: &str,
) -> Vec<(PathBuf, String)> {
    let existing = read_existing_discovery(&existing_keys_files(env));
    let config_dir = state.config_dir();
    let migrated = config_dir
        .as_ref()
        .is_some_and(|d| d.join(MIGRATED_FILE).is_file());
    let settings_path = config_dir.as_ref().map(|d| d.join(SETTINGS_FILE));
    let saved: Option<ThirdPartySettings> = settings_path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok());
    if migrated {
        state.inner.lock().unwrap().third_party = saved.clone().unwrap_or_default();
        if saved.is_none() {
            let _ = persist_settings(state);
        }
    } else {
        let mut tokens: Vec<(String, String)> = existing.tokens.clone();
        if let Some(shared) = shared_token_store(env) {
            for t in read_shared_tokens(&shared) {
                if !tokens.iter().any(|(x, _)| *x == t) {
                    tokens.push((t, "Previously registered app".into()));
                }
            }
        }
        let imported_tokens = state.import_third_party_tokens(tokens);
        {
            let mut g = state.inner.lock().unwrap();
            g.denied_apps.extend(existing.denied_apps.iter().cloned());
            g.third_party = saved
                .clone()
                .or_else(|| existing.settings.clone())
                .unwrap_or_default();
        }
        let persisted = imported_tokens
            .map(|_| ())
            .and_then(|_| state.persist_tokens())
            .and_then(|_| state.persist_denied_apps())
            .and_then(|_| {
                if saved.is_none() {
                    persist_settings(state)
                } else {
                    Ok(())
                }
            });
        if persisted.is_ok() {
            if let Some(dir) = config_dir.as_ref() {
                let marker = serde_json::json!({ "version": 1 }).to_string();
                let _ = write_file(&dir.join(MIGRATED_FILE), marker.as_bytes(), false);
            }
        }
    }
    let discovery = Discovery {
        locations: discovery_locations(platform, env),
        port,
        cert_pem: cert_pem.to_string(),
        cert_sha256: cert_sha256.to_string(),
        preserved: existing.document,
    };
    *state.third_party_discovery.lock().unwrap() = Some(discovery);
    rewrite_discovery(state)
}

/// Loads the third-party settings without discovery files or adopting earlier state, for a
/// daemon that must not touch the user's locations (see [`DiscoveryEnv::for_daemon`]).
pub fn start_without_discovery(state: &ApiState) {
    let saved: Option<ThirdPartySettings> = state
        .config_dir()
        .and_then(|d| std::fs::read_to_string(d.join(SETTINGS_FILE)).ok())
        .and_then(|raw| serde_json::from_str(&raw).ok());
    state.inner.lock().unwrap().third_party = saved.unwrap_or_default();
}

/// Rewrites the discovery files from the current settings.
pub fn rewrite_discovery(state: &ApiState) -> Vec<(PathBuf, String)> {
    let Some(d) = state.third_party_discovery.lock().unwrap().clone() else {
        return Vec::new();
    };
    let (settings, grants) = current_grants(state);
    d.write(&settings, &grants)
}

/// The settings and the third-party grants and denials as the daemon's stores hold them.
fn current_grants(state: &ApiState) -> (ThirdPartySettings, Grants) {
    let g = state.inner.lock().unwrap();
    let tokens = g
        .tokens
        .iter()
        .filter(|(_, k)| matches!(k, TokenKind::ThirdParty { .. }))
        .map(|(t, _)| t.clone())
        .collect();
    (
        g.third_party.clone(),
        Grants {
            tokens,
            denied: g.denied_apps.clone(),
        },
    )
}

/// Marks the discovery files inactive when the daemon stops.
pub fn stop(state: &ApiState) {
    let Some(d) = state.third_party_discovery.lock().unwrap().clone() else {
        return;
    };
    let (mut settings, grants) = current_grants(state);
    settings.api_active = false;
    let _ = d.write(&settings, &grants);
}

fn persist_settings(state: &ApiState) -> Result<(), String> {
    let Some(dir) = state.config_dir() else {
        return Ok(());
    };
    let settings = state.inner.lock().unwrap().third_party.clone();
    let raw = serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?;
    write_file(&dir.join(SETTINGS_FILE), &raw, false)
}

// ---------------------------------------------------------------------------------------------
// Router and authentication
// ---------------------------------------------------------------------------------------------

pub(crate) fn err_msg(code: StatusCode, message: &str) -> (StatusCode, Json<serde_json::Value>) {
    (code, Json(serde_json::json!({ "message": message })))
}

fn bearer_from(req: &Request) -> Option<String> {
    let hdr = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    hdr.or_else(|| {
        req.uri().query().and_then(|q| {
            q.split('&')
                .filter_map(|kv| kv.split_once('='))
                .find(|(k, _)| *k == "token")
                .map(|(_, v)| format!("Bearer {v}"))
        })
    })
}

fn origin_of(req: &Request) -> Option<&str> {
    req.headers()
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
}

/// Accepts any valid token, first-party or third-party.
async fn any_token(
    State(state): State<Arc<ApiState>>,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    if !ApiState::is_trusted_origin(origin_of(&req)) {
        return Err(err_msg(StatusCode::FORBIDDEN, "Untrusted Origin"));
    }
    match state.verify_bearer(bearer_from(&req).as_deref()) {
        Ok(TokenKind::FirstParty) => Ok(next.run(req).await),
        Ok(TokenKind::ThirdParty { app }) => {
            let (settings, denied) = {
                let g = state.inner.lock().unwrap();
                (g.third_party.clone(), g.denied_apps.contains(&app))
            };
            if !settings.api_active {
                return Err(err_msg(StatusCode::SERVICE_UNAVAILABLE, API_OFF_MESSAGE));
            }
            if denied {
                return Err(err_msg(StatusCode::FORBIDDEN, DENIED_MESSAGE));
            }
            let path = req.uri().path();
            let path = path.strip_prefix("/v1").unwrap_or(path);
            if let Some((_, perm)) = ROUTE_PERMISSIONS.iter().find(|(p, _)| *p == path) {
                if !settings.allows(*perm) {
                    return Err(err_msg(StatusCode::FORBIDDEN, DENIED_MESSAGE));
                }
            }
            Ok(next.run(req).await)
        }
        Err((code, msg)) => Err(err_msg(
            StatusCode::from_u16(code).unwrap_or(StatusCode::UNAUTHORIZED),
            msg,
        )),
    }
}

/// Accepts only tokens issued to OpenFlux's own clients.
pub(crate) async fn first_party_token(
    State(state): State<Arc<ApiState>>,
    req: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    if !ApiState::is_trusted_origin(origin_of(&req)) {
        return Err(err_msg(StatusCode::FORBIDDEN, "Untrusted Origin"));
    }
    match state.verify_bearer(bearer_from(&req).as_deref()) {
        Ok(TokenKind::FirstParty) => Ok(next.run(req).await),
        Ok(TokenKind::ThirdParty { .. }) => Err(err_msg(
            StatusCode::FORBIDDEN,
            "This token may only use the third-party app API",
        )),
        Err((code, msg)) => Err(err_msg(
            StatusCode::from_u16(code).unwrap_or(StatusCode::UNAUTHORIZED),
            msg,
        )),
    }
}

/// Every third-party route, plus the first-party routes that manage third-party apps.
pub fn router(state: Arc<ApiState>) -> Router {
    let protected = Router::new()
        .route("/v1/authentication/check", get(auth_check))
        .route("/v1/config/addIcon", post(crate::api::routes::add_icon))
        .route("/v1/config/addKeystyleToAppearance", put(add_keystyle))
        .route("/v1/config/import", post(import_file))
        .route("/v1/config/addGenericTile", post(add_generic_tile))
        .route("/v1/config/updateGenericTile", put(update_generic_tile))
        .route("/v1/config/keymapData", get(get_keymap_data))
        .route(
            "/v1/config/save",
            post(crate::api::routes::save_to_keyboard),
        )
        .route(
            "/v1/config/updateActiveProcess",
            post(update_active_process),
        )
        .route(
            "/v1/config/updateInputLanguage",
            post(update_input_language),
        )
        .route("/v1/config/updateProcesses", post(update_processes))
        .layer(middleware::from_fn_with_state(state.clone(), any_token));

    let management = Router::new()
        .route("/v1/third-party/registrations", get(list_registrations))
        .route(
            "/v1/third-party/registrations/:id",
            post(decide_registration),
        )
        .route("/v1/third-party/denied/forget", post(forget_denied_app))
        .route("/v1/third-party/apps/revoke", post(revoke_app))
        .route(
            "/v1/third-party/settings",
            get(get_settings).put(put_settings),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            first_party_token,
        ));

    Router::new()
        .route("/", get(root_banner))
        .route("/v1/docs", get(docs_link))
        .route("/v1/authentication/register", post(register))
        .route("/v1/configChange", get(ws_handler))
        .merge(protected)
        .merge(management)
        .with_state(state)
}

async fn root_banner() -> &'static str {
    ROOT_BANNER
}

async fn docs_link() -> &'static str {
    DOCS_URL
}

async fn auth_check() -> impl IntoResponse {
    StatusCode::OK
}

// ---------------------------------------------------------------------------------------------
// Enrollment
// ---------------------------------------------------------------------------------------------

/// `POST /authentication/register`.
///
/// OpenFlux's own clients send `enrollmentSecret` (the owner-only `enrollment.secret` file) and
/// get a first-party token at once. Third-party apps send any body and name themselves with
/// `User-Agent`; the request waits until the user approves or denies the app. Approval answers
/// 200 `{"message": token}`, denial 403, and no decision within the timeout 408. A denied app is
/// remembered and refused immediately until the user forgets the denial.
async fn register(
    State(state): State<Arc<ApiState>>,
    req_hdrs: axum::http::HeaderMap,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    let origin = req_hdrs
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    if !ApiState::is_trusted_origin(origin) {
        return err_msg(StatusCode::FORBIDDEN, "Untrusted Origin").into_response();
    }

    if let Some(secret) = body
        .as_ref()
        .and_then(|b| b.get("enrollmentSecret"))
        .and_then(|v| v.as_str())
    {
        let ok = state.inner.lock().unwrap().registration_secret.as_deref() == Some(secret);
        if !ok {
            return err_msg(StatusCode::FORBIDDEN, DENIED_MESSAGE).into_response();
        }
        return match state.issue_token(TokenKind::FirstParty) {
            Ok(tok) => {
                (StatusCode::OK, Json(serde_json::json!({ "message": tok }))).into_response()
            }
            Err(e) => err_msg(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Token persist failed: {e}"),
            )
            .into_response(),
        };
    }

    let app = req_hdrs
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| UNNAMED_APP.to_string());

    let rx = {
        let mut g = state.inner.lock().unwrap();
        if !g.third_party.api_active {
            return err_msg(StatusCode::SERVICE_UNAVAILABLE, API_OFF_MESSAGE).into_response();
        }
        if g.denied_apps.contains(&app) {
            return err_msg(StatusCode::FORBIDDEN, DENIED_MESSAGE).into_response();
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let existing = g
            .pending_registrations
            .iter()
            .find(|(_, p)| p.app == app)
            .map(|(id, _)| id.clone());
        let id = match existing {
            Some(id) => id,
            None => {
                // The id names the request when the user decides it, so it must not be guessable
                // or repeat.
                let Ok(token) = ApiState::generate_csprng_token() else {
                    return err_msg(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "The request couldn't be recorded: no random source is available",
                    )
                    .into_response();
                };
                let id = token.chars().rev().take(16).collect::<String>();
                g.pending_registrations.insert(
                    id.clone(),
                    PendingRegistration {
                        app: app.clone(),
                        requested_at_unix: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0),
                        waiters: Vec::new(),
                    },
                );
                id
            }
        };
        if let Some(p) = g.pending_registrations.get_mut(&id) {
            p.waiters.push(tx);
        }
        drop(g);
        state.emit(
            "registrationPending",
            serde_json::json!({ "id": id, "app": app }),
        );
        rx
    };

    match tokio::time::timeout(state.registration_timeout(), rx).await {
        Ok(Ok(RegistrationDecision::Approve)) => {
            match state.issue_token(TokenKind::ThirdParty { app }) {
                Ok(tok) => {
                    (StatusCode::OK, Json(serde_json::json!({ "message": tok }))).into_response()
                }
                Err(e) => err_msg(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Token persist failed: {e}"),
                )
                .into_response(),
            }
        }
        Ok(Ok(RegistrationDecision::Deny)) => {
            err_msg(StatusCode::FORBIDDEN, DENIED_MESSAGE).into_response()
        }
        Ok(Err(_)) => err_msg(
            StatusCode::SERVICE_UNAVAILABLE,
            "Registration was cancelled",
        )
        .into_response(),
        Err(_) => {
            prune_pending(&state);
            err_msg(
                StatusCode::REQUEST_TIMEOUT,
                "The registration was not approved in time",
            )
            .into_response()
        }
    }
}

/// Drops waiters whose requests ended and registrations nobody waits for any more.
fn prune_pending(state: &ApiState) {
    let mut g = state.inner.lock().unwrap();
    for p in g.pending_registrations.values_mut() {
        p.waiters.retain(|w| !w.is_closed());
    }
    g.pending_registrations.retain(|_, p| !p.waiters.is_empty());
}

/// `GET /v1/third-party/registrations` (first-party): pending and denied apps.
async fn list_registrations(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    prune_pending(&state);
    let g = state.inner.lock().unwrap();
    let pending: Vec<serde_json::Value> = g
        .pending_registrations
        .iter()
        .map(|(id, p)| serde_json::json!({ "id": id, "app": p.app, "requestedAt": p.requested_at_unix }))
        .collect();
    let denied: Vec<serde_json::Value> = g
        .denied_apps
        .iter()
        .map(|a| serde_json::json!({ "app": a }))
        .collect();
    let mut approved: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for kind in g.tokens.values() {
        if let TokenKind::ThirdParty { app } = kind {
            *approved.entry(app.as_str()).or_default() += 1;
        }
    }
    let approved: Vec<serde_json::Value> = approved
        .into_iter()
        .map(|(app, tokens)| serde_json::json!({ "app": app, "tokens": tokens }))
        .collect();
    (
        StatusCode::OK,
        Json(serde_json::json!({ "pending": pending, "approved": approved, "denied": denied })),
    )
}

/// `POST /v1/third-party/registrations/{id}` (first-party) with `{"decision": "approve" | "deny"}`.
async fn decide_registration(
    State(state): State<Arc<ApiState>>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let decision = match body.get("decision").and_then(|v| v.as_str()) {
        Some("approve") => RegistrationDecision::Approve,
        Some("deny") => RegistrationDecision::Deny,
        _ => {
            return err_msg(
                StatusCode::BAD_REQUEST,
                "decision must be \"approve\" or \"deny\"",
            )
            .into_response()
        }
    };
    prune_pending(&state);
    let Some(pending) = state
        .inner
        .lock()
        .unwrap()
        .pending_registrations
        .remove(&id)
    else {
        return err_msg(
            StatusCode::NOT_FOUND,
            "No pending registration with that id",
        )
        .into_response();
    };
    if decision == RegistrationDecision::Deny {
        state
            .inner
            .lock()
            .unwrap()
            .denied_apps
            .insert(pending.app.clone());
        state.third_party_access_changed();
        if let Err(e) = state.persist_denied_apps() {
            return err_msg(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to record the denial: {e}"),
            )
            .into_response();
        }
        let _ = rewrite_discovery(&state);
    }
    let mut delivered = 0;
    for w in pending.waiters {
        if w.send(decision).is_ok() {
            delivered += 1;
        }
    }
    let verb = if decision == RegistrationDecision::Approve {
        "approved"
    } else {
        "denied"
    };
    (
        StatusCode::OK,
        Json(serde_json::json!({ "message": format!("{} {verb}", pending.app), "app": pending.app, "delivered": delivered })),
    )
        .into_response()
}

/// `POST /v1/third-party/apps/revoke` (first-party) with `{"app": name, "deny": bool}`: removes
/// every token issued to the app and closes its connections. With `deny`, the app is also
/// refused if it registers again.
async fn revoke_app(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let Some(app) = body
        .get("app")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
    else {
        return err_msg(StatusCode::BAD_REQUEST, "app is required").into_response();
    };
    let deny = body.get("deny").and_then(|v| v.as_bool()).unwrap_or(false);
    let revoked = {
        let mut g = state.inner.lock().unwrap();
        let before = g.tokens.len();
        g.tokens
            .retain(|_, k| !matches!(k, TokenKind::ThirdParty { app: a } if *a == app));
        if deny {
            g.denied_apps.insert(app.clone());
        }
        before - g.tokens.len()
    };
    state.third_party_access_changed();
    if let Err(e) = state
        .persist_tokens()
        .and_then(|_| state.persist_denied_apps())
    {
        return err_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to save the change: {e}"),
        )
        .into_response();
    }
    let _ = rewrite_discovery(&state);
    (
        StatusCode::OK,
        Json(serde_json::json!({ "message": format!("Revoked {revoked} token(s) for {app}"), "revoked": revoked, "denied": deny })),
    )
        .into_response()
}

/// `POST /v1/third-party/denied/forget` (first-party) with `{"app": name}`: the app may ask again.
async fn forget_denied_app(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let Some(app) = body.get("app").and_then(|v| v.as_str()) else {
        return err_msg(StatusCode::BAD_REQUEST, "app is required").into_response();
    };
    let removed = state.inner.lock().unwrap().denied_apps.remove(app);
    if let Err(e) = state.persist_denied_apps() {
        return err_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to update denied apps: {e}"),
        )
        .into_response();
    }
    if !removed {
        return err_msg(StatusCode::NOT_FOUND, "That app is not denied").into_response();
    }
    let _ = rewrite_discovery(&state);
    (
        StatusCode::OK,
        Json(serde_json::json!({ "message": format!("{app} may register again") })),
    )
        .into_response()
}

/// `GET /v1/third-party/settings` (first-party).
async fn get_settings(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let settings = state.inner.lock().unwrap().third_party.clone();
    (StatusCode::OK, Json(settings))
}

/// `PUT /v1/third-party/settings` (first-party) with any subset of the settings fields.
async fn put_settings(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let Some(patch) = body.as_object() else {
        return err_msg(
            StatusCode::BAD_REQUEST,
            "Request body must be a JSON object",
        )
        .into_response();
    };
    let current = state.inner.lock().unwrap().third_party.clone();
    let mut merged = serde_json::to_value(&current).unwrap_or_default();
    for (k, v) in patch {
        if merged.get(k).is_none() {
            return err_msg(StatusCode::BAD_REQUEST, &format!("Unknown setting {k}"))
                .into_response();
        }
        merged[k] = v.clone();
    }
    let next: ThirdPartySettings = match serde_json::from_value(merged) {
        Ok(s) => s,
        Err(e) => {
            return err_msg(StatusCode::BAD_REQUEST, &format!("Invalid settings: {e}"))
                .into_response()
        }
    };
    state.inner.lock().unwrap().third_party = next.clone();
    state.third_party_access_changed();
    if let Err(e) = persist_settings(&state) {
        return err_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Failed to save settings: {e}"),
        )
        .into_response();
    }
    let failures = rewrite_discovery(&state);
    let failed: Vec<String> = failures
        .iter()
        .map(|(p, e)| format!("{}: {e}", p.display()))
        .collect();
    (
        StatusCode::OK,
        Json(serde_json::json!({ "settings": next, "discoveryWriteFailures": failed })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------------------------
// /config routes
// ---------------------------------------------------------------------------------------------

async fn add_generic_tile(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(r) = state.check_keyboard_write(&[crate::updates::Area::Config]) {
        return err_msg(StatusCode::CONFLICT, &r.message);
    }
    let Some(title) = body.get("title").and_then(|v| v.as_str()) else {
        if body.get("title").is_none() {
            return err_msg(
                StatusCode::BAD_REQUEST,
                "Request body must contain a \"title\" key",
            );
        }
        return err_msg(StatusCode::BAD_REQUEST, "title must be a string");
    };
    let Some(ttype) = body.get("type").and_then(|v| v.as_i64()) else {
        if body.get("type").is_none() {
            return err_msg(
                StatusCode::BAD_REQUEST,
                "Request body must contain a \"type\" key",
            );
        }
        return err_msg(StatusCode::BAD_REQUEST, "type must be an int");
    };
    if !(0..=1).contains(&ttype) {
        return err_msg(StatusCode::BAD_REQUEST, "type must be between 0 and 1");
    }
    let Some(index) = body.get("index").and_then(|v| v.as_i64()) else {
        if body.get("index").is_none() {
            return err_msg(
                StatusCode::BAD_REQUEST,
                "Request body must contain a \"index\" key",
            );
        }
        return err_msg(StatusCode::BAD_REQUEST, "index must be an int");
    };
    if !(0..=3).contains(&index) {
        return err_msg(StatusCode::BAD_REQUEST, "index must be between 0 and 3");
    }
    let length = if ttype == 0 {
        let Some(l) = body.get("length").and_then(|v| v.as_i64()) else {
            if body.get("length").is_none() {
                return err_msg(
                    StatusCode::BAD_REQUEST,
                    "Request body must contain a \"length\" key",
                );
            }
            return err_msg(StatusCode::BAD_REQUEST, "length must be an int");
        };
        if !(1..100000).contains(&l) {
            return err_msg(
                StatusCode::BAD_REQUEST,
                "length must be less than 100000 and greater than 0",
            );
        }
        l as u32
    } else {
        body.get("length").and_then(|v| v.as_i64()).unwrap_or(100) as u32
    };

    let tile_id = title.to_string();
    let tile = GenericTileConfig {
        id: tile_id.clone(),
        index: index as usize,
        title: title.to_string(),
        tile_type: ttype as u8,
        length,
        sub_title: String::new(),
        unit: String::new(),
        secondary_subtitle: String::new(),
        time_unit: String::new(),
        color: "Legend".to_string(),
        leading: String::new(),
        ending: String::new(),
        current_value: "0".to_string(),
    };

    let mut g = state.inner.lock().unwrap();
    g.generic_tiles[index as usize] = Some(tile);
    if let Some(arr) = g.mosaic.get_mut("tileTypes").and_then(|v| v.as_array_mut()) {
        while arr.len() <= index as usize {
            arr.push(serde_json::json!(0));
        }
        arr[index as usize] = serde_json::json!(8u64);
    }
    if let Some(cfgs) = g
        .mosaic
        .get_mut("tileConfigs")
        .and_then(|v| v.as_array_mut())
    {
        while cfgs.len() <= index as usize {
            cfgs.push(serde_json::json!({}));
        }
        cfgs[index as usize] = serde_json::json!({
            "title": tile_id,
            "subtitle": "",
            "value": "0",
            "progress": 0.0
        });
    }
    g.mosaic_dirty = true;
    g.mosaic_rev = g.mosaic_rev.wrapping_add(1);
    drop(g);
    state.emit(
        "configChange",
        serde_json::json!({ "kind": "addGenericTile", "id": tile_id, "index": index }),
    );
    (
        StatusCode::OK,
        Json(serde_json::json!({ "message": tile_id })),
    )
}

async fn update_generic_tile(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(r) = state.check_keyboard_write(&[crate::updates::Area::Config]) {
        return err_msg(StatusCode::CONFLICT, &r.message);
    }
    let req = body
        .get("GenericGraphUpdateRequest")
        .or_else(|| body.get("genericGraphUpdateRequest"))
        .unwrap_or(&body);

    let id_opt = req
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let idx_opt = req
        .get("index")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize);

    let live_payload;
    {
        let mut g = state.inner.lock().unwrap();
        let slot_idx = if let Some(ref tid) = id_opt {
            g.generic_tiles.iter().position(|t| {
                t.as_ref()
                    .map(|x| x.id == *tid || x.title == *tid)
                    .unwrap_or(false)
            })
        } else {
            idx_opt.filter(|&i| i < 4 && g.generic_tiles[i].is_some())
        };
        let Some(si) = slot_idx else {
            return err_msg(
                StatusCode::NOT_FOUND,
                "Module ID does not exist. Have you added it yet?",
            );
        };
        // The request is validated whole before the tile changes. The message is the published
        // API's.
        let val_str = if let Some(sv) = req.get("currentValue").and_then(|v| v.as_str()) {
            sv.to_string()
        } else if let Some(fv) = req.get("currentValue").and_then(|v| v.as_f64()) {
            format!("{fv:.2}")
        } else {
            return err_msg(StatusCode::BAD_REQUEST, "currentValue must be an double");
        };
        let slot = g.generic_tiles[si].as_mut().unwrap();
        if let Some(st) = req.get("subTitle").and_then(|v| v.as_str()) {
            slot.sub_title = st.to_string();
        }
        if let Some(u) = req.get("unit").and_then(|v| v.as_str()) {
            slot.unit = u.to_string();
        }
        if let Some(ss) = req.get("secondarySubtitle").and_then(|v| v.as_str()) {
            slot.secondary_subtitle = ss.to_string();
        }
        if let Some(tu) = req.get("timeUnit").and_then(|v| v.as_str()) {
            slot.time_unit = tu.to_string();
        }
        if let Some(c) = req.get("color").and_then(|v| v.as_str()) {
            slot.color = c.to_string();
        }
        if let Some(l) = req.get("leading").and_then(|v| v.as_str()) {
            slot.leading = l.to_string();
        }
        if let Some(e) = req.get("ending").and_then(|v| v.as_str()) {
            slot.ending = e.to_string();
        }
        slot.current_value = val_str.clone();
        let prog = val_str.parse::<f64>().unwrap_or(0.0) / 100.0;
        live_payload = serde_json::json!({
            "customGraphData": {
                "index": si,
                "id": slot.id,
                "type": slot.tile_type,
                "title": slot.title,
                "subtitle": slot.sub_title,
                "value": format!("{}{}{}{}", slot.leading, slot.current_value, slot.unit, slot.ending),
                "progress": prog.clamp(0.0, 1.0),
            }
        });
    }

    let has_bridge = state.inner.lock().unwrap().cached_connected.0;
    if has_bridge {
        let res = if let Some(w) = state.get_worker() {
            w.send_live_json(live_payload.clone()).await
        } else {
            state.device.send_live_json(&live_payload)
        };
        if let Err(e) = res {
            return err_msg(
                StatusCode::BAD_GATEWAY,
                &format!("USB live update failed: {e}"),
            );
        }
    }
    state.emit("genericTileUpdate", live_payload);
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "message": "Module updated successfully",
            "appliedToHardware": has_bridge
        })),
    )
}

/// Maps a `KeyModel` (`frameLayoutType`, `row`, `index`) to the flat physical key index used by
/// keymaps and stylemaps.
pub fn canonical_key_index(layout: &str, row: u64, index: u64) -> Result<u64, &'static str> {
    let row_counts: [u64; 6] = if layout.eq_ignore_ascii_case("iso") {
        crate::defaults::ISO_ROWS.map(|n| n as u64)
    } else if layout.eq_ignore_ascii_case("ansi") {
        crate::defaults::ANSI_ROWS.map(|n| n as u64)
    } else {
        return Err("Unsupported frameLayoutType (expected ansi or iso)");
    };
    let r = row as usize;
    if r >= row_counts.len() {
        return Err("model.row out of range (0..=5)");
    }
    if index >= row_counts[r] {
        return Err("model.index out of range for row");
    }
    let base: u64 = row_counts[..r].iter().sum();
    Ok(base + index)
}

async fn add_keystyle(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(r) = state.check_keyboard_write(&[crate::updates::Area::Config]) {
        return err_msg(StatusCode::CONFLICT, &r.message);
    }
    let Some(styles) = body.get("styles").and_then(|v| v.as_array()) else {
        return err_msg(
            StatusCode::BAD_REQUEST,
            "Request body must contain a \"styles\" key",
        );
    };
    let mut g = state.inner.lock().unwrap();

    // Every entry is validated before any theme changes.
    struct ValidStyleOp {
        theme_id: String,
        layer: String,
        flat_key: String,
        new_style: serde_json::Value,
    }
    let mut ops = Vec::with_capacity(styles.len());

    for entry in styles {
        let Some(prof_name) = entry.get("profileName").and_then(|v| v.as_str()) else {
            return err_msg(
                StatusCode::BAD_REQUEST,
                "Each style item must contain \"profileName\"",
            );
        };
        let Some(prof) = g
            .profiles
            .iter()
            .find(|p| p.name == prof_name || p.id == prof_name)
        else {
            return err_msg(StatusCode::NOT_FOUND, "Profile doesn't exist");
        };
        let theme_id = prof.theme_id.clone();

        let Some(model) = entry.get("model") else {
            return err_msg(
                StatusCode::BAD_REQUEST,
                "Each style item must contain \"model\"",
            );
        };
        let layout = model
            .get("frameLayoutType")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let Some(row) = model.get("row").and_then(|v| v.as_u64()) else {
            return err_msg(
                StatusCode::BAD_REQUEST,
                "model.row must be a non-negative integer",
            );
        };
        let Some(idx) = model.get("index").and_then(|v| v.as_u64()) else {
            return err_msg(
                StatusCode::BAD_REQUEST,
                "model.index must be a non-negative integer",
            );
        };
        let flat = match canonical_key_index(layout, row, idx) {
            Ok(f) => f,
            Err(msg) => return err_msg(StatusCode::BAD_REQUEST, msg),
        };

        let Some(new_style) = entry.get("newStyle") else {
            return err_msg(
                StatusCode::BAD_REQUEST,
                "Each style item must contain \"newStyle\"",
            );
        };
        if !new_style.is_object() {
            return err_msg(StatusCode::BAD_REQUEST, "newStyle must be a JSON object");
        }
        if let Some(iap) = new_style.get("iconAssetPath").and_then(|v| v.as_str()) {
            let name = Path::new(iap)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(iap);
            if !iap.is_empty() && !g.imported_icons.iter().any(|x| x == name || x == iap) {
                return err_msg(
                    StatusCode::BAD_REQUEST,
                    "Icon doesn't exist, please call /addIcon to add the icon",
                );
            }
        }
        let layer = entry
            .get("layer")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .to_string();
        ops.push(ValidStyleOp {
            theme_id,
            layer,
            flat_key: flat.to_string(),
            new_style: new_style.clone(),
        });
    }

    let Some(app_obj) = g.appearance.as_object_mut() else {
        return err_msg(StatusCode::BAD_REQUEST, "Appearance state is not an object");
    };

    for op in ops {
        let theme_val = if op.theme_id == "default" {
            app_obj
                .entry("defaultTheme")
                .or_insert_with(|| serde_json::json!({}))
        } else {
            let custom = app_obj
                .entry("customThemes")
                .or_insert_with(|| serde_json::json!([]));
            let Some(arr) = custom.as_array_mut() else {
                return err_msg(StatusCode::BAD_REQUEST, "customThemes is not an array");
            };
            if let Some(pos) = arr
                .iter()
                .position(|t| t.get("id").and_then(|x| x.as_str()) == Some(op.theme_id.as_str()))
            {
                &mut arr[pos]
            } else {
                arr.push(
                    serde_json::json!({ "id": op.theme_id, "name": op.theme_id, "stylemaps": {} }),
                );
                let last = arr.len() - 1;
                &mut arr[last]
            }
        };

        let Some(t) = theme_val.as_object_mut() else {
            return err_msg(StatusCode::BAD_REQUEST, "Target theme is not a JSON object");
        };
        let sm_val = t
            .entry("stylemaps")
            .or_insert_with(|| serde_json::json!({}));
        let Some(sm) = sm_val.as_object_mut() else {
            return err_msg(StatusCode::BAD_REQUEST, "stylemaps is not a JSON object");
        };
        let lval = sm
            .entry(op.layer)
            .or_insert_with(|| serde_json::json!({"styles": {}}));
        let Some(lobj) = lval.as_object_mut() else {
            return err_msg(
                StatusCode::BAD_REQUEST,
                "stylemap layer entry is not a JSON object",
            );
        };
        let styles_val = lobj
            .entry("styles")
            .or_insert_with(|| serde_json::json!({}));
        let Some(kmap) = styles_val.as_object_mut() else {
            return err_msg(
                StatusCode::BAD_REQUEST,
                "stylemap layer styles is not a JSON object",
            );
        };
        kmap.insert(op.flat_key, op.new_style);
    }

    g.appearance_dirty = true;
    g.appearance_rev = g.appearance_rev.wrapping_add(1);
    drop(g);
    state.emit(
        "configChange",
        serde_json::json!({ "kind": "addKeystyleToAppearance" }),
    );
    (
        StatusCode::OK,
        Json(serde_json::json!({ "message": "Key style added successfully" })),
    )
}

async fn get_keymap_data(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let g = state.inner.lock().unwrap();
    let msg = keymap_data_response(&g.keymap);
    (StatusCode::OK, Json(serde_json::json!({ "message": msg })))
}

/// `GET /config/keymapData`: per layer, every key's `KeyModel` location and action, with keycodes
/// in their external names.
pub fn keymap_data_response(keymap_doc: &serde_json::Value) -> serde_json::Value {
    let layout = keymap_doc
        .get("selectedFrameLayoutType")
        .and_then(|v| v.as_str())
        .unwrap_or("ansi");
    let row_counts = if layout.eq_ignore_ascii_case("iso") {
        crate::defaults::ISO_ROWS
    } else {
        crate::defaults::ANSI_ROWS
    };
    let mut out = serde_json::Map::new();
    if let Some(layers) = keymap_doc
        .get("keymaps")
        .and_then(|k| k.get(layout))
        .and_then(|l| l.get("layers"))
        .and_then(|a| a.as_array())
    {
        for (lidx, lobj) in layers.iter().enumerate() {
            let mut entries = Vec::new();
            if let Some(actions) = lobj.get("keyActions").and_then(|a| a.as_array()) {
                let mut flat = 0usize;
                for (r, &rcount) in row_counts.iter().enumerate() {
                    for c in 0..rcount {
                        if let Some(action) = actions.get(flat) {
                            let mut action = action.clone();
                            keycodes_to_external(&mut action);
                            entries.push(serde_json::json!({
                                "location": {
                                    "width": 1.0,
                                    "frameLayoutType": layout,
                                    "row": r,
                                    "index": c
                                },
                                "action": action
                            }));
                        }
                        flat += 1;
                    }
                }
            }
            out.insert(lidx.to_string(), serde_json::Value::Array(entries));
        }
    }
    serde_json::Value::Object(out)
}

/// `POST /config/import`. A `.flux` file is a JSON manifest (`appearance`, `profiles`, `media`,
/// `actuation` and `shortcuts`, each a list); a `.openflux` file is an OpenFlux bundle. The file's
/// content must match its extension.
pub(crate) async fn import_file(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(r) = state.check_keyboard_write(&[
        crate::updates::Area::Config,
        crate::updates::Area::Keymap,
        crate::updates::Area::ProfileSettings,
    ]) {
        return err_msg(StatusCode::CONFLICT, &r.message);
    }
    // The published API's message, word for word.
    const NOT_IMPORTABLE: &str = "The file must a .flux file and must exist";
    let Some(fp) = body
        .get("file")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    else {
        return err_msg(StatusCode::BAD_REQUEST, NOT_IMPORTABLE);
    };
    let is_bundle = fp.ends_with(crate::api::assets::BUNDLE_EXTENSION_DOTTED);
    if !fp.ends_with(".flux") && !is_bundle {
        return err_msg(StatusCode::BAD_REQUEST, NOT_IMPORTABLE);
    }
    let path = crate::api::assets::expand_home_path(fp);
    let Ok(meta) = std::fs::metadata(&path) else {
        return err_msg(StatusCode::BAD_REQUEST, NOT_IMPORTABLE);
    };
    if !meta.is_file() || meta.len() == 0 {
        return err_msg(StatusCode::BAD_REQUEST, NOT_IMPORTABLE);
    }
    let looks_like_zip = std::fs::File::open(&path)
        .and_then(|mut f| {
            let mut magic = [0u8; 4];
            std::io::Read::read_exact(&mut f, &mut magic).map(|_| magic == *b"PK\x03\x04")
        })
        .unwrap_or(false);
    if is_bundle {
        if !looks_like_zip {
            return err_msg(
                StatusCode::BAD_REQUEST,
                "This .openflux file is not an OpenFlux bundle",
            );
        }
        let state_clone = state.clone();
        let res = tokio::task::spawn_blocking(move || {
            crate::api::assets::import_bundle_archive(&state_clone, &path)
        })
        .await;
        let imported_names = match res {
            Ok(Ok(names)) => names,
            Ok(Err((code, msg))) => return err_msg(code, &msg),
            Err(e) => {
                return err_msg(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Bundle import task failed: {e}"),
                )
            }
        };
        state.emit(
            "configChange",
            serde_json::json!({ "kind": "import", "profiles": imported_names }),
        );
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "message": "The file imported successfully",
                "profiles": imported_names
            })),
        );
    }
    if looks_like_zip {
        return err_msg(
            StatusCode::BAD_REQUEST,
            "This .flux file is an archive; OpenFlux bundles use the .openflux extension",
        );
    }
    const MAX_MANIFEST_BYTES: u64 = 32 * 1024 * 1024;
    if meta.len() > MAX_MANIFEST_BYTES {
        return err_msg(StatusCode::BAD_REQUEST, NOT_IMPORTABLE);
    }
    let Ok(raw) = std::fs::read(&path) else {
        return err_msg(StatusCode::BAD_REQUEST, NOT_IMPORTABLE);
    };
    let Ok(mut parsed) = serde_json::from_slice::<serde_json::Value>(&raw) else {
        return err_msg(StatusCode::BAD_REQUEST, "Invalid .flux manifest JSON");
    };
    keycodes_to_internal(&mut parsed);
    let Some(obj) = parsed.as_object() else {
        return err_msg(
            StatusCode::BAD_REQUEST,
            "Invalid .flux manifest root object",
        );
    };

    for list_key in ["appearance", "profiles", "media", "actuation", "shortcuts"] {
        if let Some(v) = obj.get(list_key) {
            if !v.is_array() {
                return err_msg(
                    StatusCode::BAD_REQUEST,
                    "Invalid .flux field type (expected array)",
                );
            }
        }
    }

    // Profiles are validated before any state changes, then merged into the live list under the
    // state lock so concurrent edits are kept.
    let mut prevalidated_incoming: Option<(Vec<ShortcutProfileState>, Vec<String>)> = None;
    if let Some(profs) = obj.get("profiles").and_then(|v| v.as_array()) {
        let mut incoming = Vec::with_capacity(profs.len());
        let mut p_names = Vec::new();
        for pval in profs {
            let Some(name) = pval
                .get("name")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let id = pval
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or(name)
                .to_string();
            let theme_id = pval
                .get("themeId")
                .or_else(|| pval.get("theme_id"))
                .and_then(|x| x.as_str())
                .unwrap_or("default")
                .to_string();
            let default_layer = pval
                .get("default_layer")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let am_raw = pval
                .get("actuation_mode_index")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if default_layer > 31 || u32::try_from(am_raw).is_err() {
                return err_msg(
                    StatusCode::BAD_REQUEST,
                    "Imported profile default_layer or actuation_mode_index out of range",
                );
            }
            incoming.push(ShortcutProfileState {
                id,
                name: name.to_string(),
                theme_id,
                contexts: vec!["Main".into()],
                active_context: "Main".into(),
                actuation_mode_index: am_raw as usize,
                default_layer,
                shortcut_pack: pval
                    .get("shortcut_pack")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                app_matcher: pval
                    .get("app_matcher")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            });
            p_names.push(name.to_string());
        }
        prevalidated_incoming = Some((incoming, p_names));
    }

    let mut imported_names = Vec::new();
    let mut g = state.inner.lock().unwrap();
    let merged_to_apply: Option<(Vec<ShortcutProfileState>, Vec<String>)> =
        if let Some((incoming, p_names)) = prevalidated_incoming {
            let mut merged = g.profiles.clone();
            let mut imported_any = false;
            for np in incoming {
                if let Some(pos) = merged.iter().position(|ep| ep.id == np.id) {
                    merged[pos] = np;
                } else if let Some(pos) = merged.iter().position(|ep| ep.name == np.name) {
                    merged[pos] = np;
                } else {
                    if merged.len() >= crate::codecs::keymap::MAX_PROFILES {
                        return err_msg(
                            StatusCode::BAD_REQUEST,
                            &format!(
                                "Imported profiles exceed the keyboard's {} profile limit",
                                crate::codecs::keymap::MAX_PROFILES
                            ),
                        );
                    }
                    merged.push(np);
                }
                imported_any = true;
            }
            let mut seen_ids = std::collections::HashSet::new();
            if !merged.iter().all(|p| seen_ids.insert(p.id.as_str())) {
                return err_msg(
                    StatusCode::BAD_REQUEST,
                    "Imported profiles create duplicate stable profile IDs",
                );
            }
            if imported_any {
                Some((merged, p_names))
            } else {
                imported_names.extend(p_names);
                None
            }
        } else {
            None
        };
    if let Some(themes) = obj.get("appearance").and_then(|v| v.as_array()) {
        if !themes.is_empty() {
            if let Some(ct) = g
                .appearance
                .as_object_mut()
                .and_then(|a| a.get_mut("customThemes"))
                .and_then(|c| c.as_array_mut())
            {
                let mut added_theme = false;
                for th in themes {
                    if th.is_object() {
                        ct.push(th.clone());
                        added_theme = true;
                    }
                }
                if added_theme {
                    g.appearance_dirty = true;
                    g.appearance_rev = g.appearance_rev.wrapping_add(1);
                }
            }
        }
    }
    let mut had_prof_import = false;
    if let Some((merged, p_names)) = merged_to_apply {
        ApiState::replace_profiles_and_remap(&mut g, merged, true, true);
        imported_names.extend(p_names);
        had_prof_import = true;
    }
    drop(g);
    if had_prof_import {
        if let Err(pe) = state.persist_profiles() {
            return err_msg(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to persist imported profile store: {pe}"),
            );
        }
    }
    state.emit(
        "configChange",
        serde_json::json!({ "kind": "import", "profiles": imported_names }),
    );
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "message": "The file imported successfully",
            "profiles": imported_names
        })),
    )
}

async fn update_processes(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(procs) = body.get("processes").and_then(|v| v.as_array()) else {
        return err_msg(
            StatusCode::BAD_REQUEST,
            "Request body must contain a \"processes\" key",
        );
    };
    let list: Vec<String> = procs
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    let count = list.len();
    let mut g = state.inner.lock().unwrap();
    g.detection_override_processes = Some(list);
    g.last_client_activity = Some(Instant::now());
    (
        StatusCode::OK,
        Json(serde_json::json!({ "message": format!("Updated {count} active processes") })),
    )
}

async fn update_active_process(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(proc_name) = body.get("process").and_then(|v| v.as_str()) else {
        return err_msg(
            StatusCode::BAD_REQUEST,
            "Request body must contain a \"process\" key",
        );
    };
    let (midx, pid, hw_slot_opt, has_firmware, theme_rev_changed) = {
        let mut g = state.inner.lock().unwrap();
        g.detection_override_process = Some(proc_name.to_string());
        g.last_client_activity = Some(Instant::now());
        let foreground = crate::api::state::foreground_app(Some(proc_name), None);
        g.observe_foreground_app(foreground.as_deref());
        let midx = g
            .app_rules
            .iter()
            .find(|r| {
                let m = r.matcher.to_lowercase();
                !m.is_empty() && proc_name.to_lowercase().contains(&m)
            })
            .map(|r| r.profile_index as usize)
            .filter(|_| g.app_rules_switch_profiles());
        let pid = midx.and_then(|i| g.profiles.get(i).map(|p| p.id.clone()));
        let hw = midx.and_then(|i| ApiState::hardware_profile_slot(&g, i));
        let mut theme_rev_changed = false;
        if let Some(i) = midx {
            ApiState::apply_profile_theme(&mut g, i);
            theme_rev_changed = g.choose_profile_theme(i, false);
        }
        (midx, pid, hw, g.cached_connected.1, theme_rev_changed)
    };
    if theme_rev_changed {
        if let Err(e) = state.persist_profiles() {
            eprintln!("openfluxd: the theme revision couldn't be stored: {e}");
        }
    }
    if let (Some(pidx), Some(prof_id)) = (midx, pid) {
        if has_firmware {
            let res = if let Some(w) = state.get_worker() {
                w.set_profile(prof_id).await
            } else if let Some(hw_slot) = hw_slot_opt {
                let r = state.device.set_active_profile(hw_slot as u32);
                if r.is_ok() {
                    state
                        .inner
                        .lock()
                        .unwrap()
                        .record_applied_profile(Some(pidx), hw_slot as u32);
                }
                r
            } else {
                Ok(())
            };
            if let Err(e) = res {
                return err_msg(
                    StatusCode::BAD_GATEWAY,
                    &format!("Keyboard profile write failed: {e}"),
                );
            }
            state.emit(
                "profileChange",
                serde_json::json!({ "profileIndex": pidx, "process": proc_name }),
            );
        }
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({ "message": format!("Updated active process: {proc_name}") })),
    )
}

async fn update_input_language(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(layout) = body.get("layout").and_then(|v| v.as_str()) else {
        return err_msg(
            StatusCode::BAD_REQUEST,
            "Request body must contain a \"layout\" key",
        );
    };
    let code = if layout.is_empty() {
        None
    } else {
        let Some(c) = layout_to_language_code(layout) else {
            return err_msg(StatusCode::BAD_REQUEST, "Unsupported XKB layout identifier");
        };
        Some(c)
    };
    let has_firmware = {
        let mut g = state.inner.lock().unwrap();
        g.detection_override_layout = Some(layout.to_string());
        g.desired_language_code = code;
        g.last_client_activity = Some(Instant::now());
        g.cached_connected.1
    };
    if let Some(c) = code {
        if has_firmware {
            let res = if let Some(w) = state.get_worker() {
                w.set_language(c).await
            } else {
                let r = state.device.set_language(c);
                if r.is_ok() {
                    state.inner.lock().unwrap().applied_language_code = Some(c);
                }
                r
            };
            if let Err(e) = res {
                return err_msg(
                    StatusCode::BAD_GATEWAY,
                    &format!("Keyboard language write failed: {e}"),
                );
            }
            state.emit(
                "inputLanguageChange",
                serde_json::json!({ "layout": layout, "code": c }),
            );
        }
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "message": format!("Updated input language: {layout}"),
            "appliedToHardware": has_firmware && code.is_some()
        })),
    )
}

// ---------------------------------------------------------------------------------------------
// WebSocket /v1/configChange
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct WsQuery {
    token: Option<String>,
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<ApiState>>,
    Query(q): Query<WsQuery>,
    req: Request,
) -> Response {
    let origin = req
        .headers()
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    if !ApiState::is_trusted_origin(origin) {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(serde_json::json!({ "message": "Untrusted Origin" })),
        )
            .into_response();
    }

    let hdr_auth = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| q.token.map(|t| format!("Bearer {t}")));

    let kind = match state.verify_bearer(hdr_auth.as_deref()) {
        Ok(k) => k,
        Err((code, msg)) => {
            let st = StatusCode::from_u16(code).unwrap_or(StatusCode::UNAUTHORIZED);
            return (st, axum::Json(serde_json::json!({ "message": msg }))).into_response();
        }
    };
    if matches!(kind, TokenKind::ThirdParty { .. })
        && !state.inner.lock().unwrap().third_party.api_active
    {
        return err_msg(StatusCode::SERVICE_UNAVAILABLE, API_OFF_MESSAGE).into_response();
    }

    let Ok(ws_permit) = state.ws_sem.clone().try_acquire_owned() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({ "message": "Too many active WebSocket sessions" })),
        )
            .into_response();
    };

    let token = hdr_auth
        .as_deref()
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string())
        .unwrap_or_default();
    ws.on_upgrade(move |socket| handle_socket(socket, state, kind, token, ws_permit))
}

/// Close code sent to a third-party socket whose access ended (RFC 6455 policy violation).
pub const WS_ACCESS_ENDED_CODE: u16 = 1008;
/// Events only OpenFlux's own clients receive.
const FIRST_PARTY_EVENTS: [&str; 1] = ["registrationPending"];

/// Why a third-party connection may no longer be used, or `None` while it may.
fn third_party_access_ended(state: &ApiState, token: &str) -> Option<&'static str> {
    let g = state.inner.lock().unwrap();
    if !g.third_party.api_active {
        return Some(API_OFF_MESSAGE);
    }
    match g.tokens.get(token) {
        Some(TokenKind::ThirdParty { app }) if g.denied_apps.contains(app) => Some(DENIED_MESSAGE),
        Some(_) => None,
        None => Some("Invalid token"),
    }
}

async fn handle_socket(
    mut socket: WebSocket,
    state: Arc<ApiState>,
    kind: TokenKind,
    token: String,
    _ws_permit: tokio::sync::OwnedSemaphorePermit,
) {
    let third_party = matches!(kind, TokenKind::ThirdParty { .. });
    let mut rx = state.events.subscribe();
    let mut access = state.third_party_access.subscribe();
    let ended = |state: &ApiState| {
        if third_party {
            third_party_access_ended(state, &token)
        } else {
            None
        }
    };
    loop {
        if let Some(reason) = ended(&state) {
            let _ = socket
                .send(Message::Close(Some(axum::extract::ws::CloseFrame {
                    code: WS_ACCESS_ENDED_CODE,
                    reason: reason.into(),
                })))
                .await;
            break;
        }
        tokio::select! {
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(txt))) => {
                        if ended(&state).is_some() {
                            continue;
                        }
                        let reply = process_ws_message(&state, &kind, &txt).await;
                        if socket.send(Message::Text(reply.to_string())).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(p))) => {
                        if socket.send(Message::Pong(p)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
            ev = rx.recv() => {
                if let Ok(evt) = ev {
                    if third_party && (FIRST_PARTY_EVENTS.contains(&evt.event.as_str()) || ended(&state).is_some()) {
                        continue;
                    }
                    if let Ok(txt) = serde_json::to_string(&evt) {
                        if socket.send(Message::Text(txt)).await.is_err() {
                            break;
                        }
                    }
                }
            }
            changed = access.changed() => {
                if changed.is_err() {
                    break;
                }
            }
        }
    }
}

async fn process_ws_message(
    state: &Arc<ApiState>,
    kind: &TokenKind,
    txt: &str,
) -> serde_json::Value {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(txt) else {
        return serde_json::json!({ "status": 400, "message": "Invalid JSON" });
    };
    let route = v.get("route").and_then(|r| r.as_str()).unwrap_or("");
    let msg_str = v.get("message").and_then(|m| m.as_str());
    if matches!(kind, TokenKind::ThirdParty { .. }) {
        if !state.inner.lock().unwrap().third_party.api_active {
            return serde_json::json!({ "route": route, "status": 503, "message": API_OFF_MESSAGE });
        }
        if let Some((_, perm)) = WS_PERMISSIONS.iter().find(|(r, _)| *r == route) {
            if !state.inner.lock().unwrap().third_party.allows(*perm) {
                return serde_json::json!({ "route": route, "status": 403, "message": "User denied request" });
            }
        }
    }

    match route {
        "currentProfile" => {
            let g = state.inner.lock().unwrap();
            let idx = g.applied_profile_index.unwrap_or(g.desired_profile_index);
            let name = g
                .profiles
                .get(idx)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            serde_json::json!({
                "route": "currentProfile",
                "status": 200,
                "message": name,
            })
        }
        "changeProfile" => {
            let Some(name) = msg_str else {
                return serde_json::json!({ "route": "changeProfile", "status": 400, "message": "Profile error" });
            };
            let (target_idx, prof_name, has_firmware) = {
                let g = state.inner.lock().unwrap();
                let idx = g
                    .profiles
                    .iter()
                    .position(|p| p.name.eq_ignore_ascii_case(name) || p.id == name);
                let pname = idx.map(|i| g.profiles[i].name.clone()).unwrap_or_default();
                (idx, pname, g.cached_connected.1)
            };
            let Some(idx) = target_idx else {
                return serde_json::json!({ "route": "changeProfile", "status": 404, "message": "Profile error" });
            };

            let (prof_id, hw_slot_opt) = {
                let mut g = state.inner.lock().unwrap();
                crate::api::state::ApiState::apply_profile_selection(&mut g, idx);
                g.choose_profile_theme(idx, true);
                let pid = g
                    .profiles
                    .get(idx)
                    .map(|p| p.id.clone())
                    .unwrap_or_default();
                let slot = crate::api::state::ApiState::hardware_profile_slot(&g, idx);
                (pid, slot)
            };
            if let Err(e) = state.persist_profiles() {
                eprintln!("openfluxd: the theme revision couldn't be stored: {e}");
            }
            if has_firmware {
                let res = if let Some(w) = state.get_worker() {
                    w.set_profile(prof_id).await
                } else {
                    let Some(hw_slot) = hw_slot_opt else {
                        return serde_json::json!({
                            "route": "changeProfile",
                            "status": 409,
                            "message": "Profile not yet saved to keyboard"
                        });
                    };
                    let r = state.device.set_active_profile(hw_slot as u32);
                    if r.is_ok() {
                        state
                            .inner
                            .lock()
                            .unwrap()
                            .record_applied_profile(Some(idx), hw_slot as u32);
                    }
                    r
                };
                if let Err(e) = res {
                    return serde_json::json!({
                        "route": "changeProfile",
                        "status": 502,
                        "message": format!("Keyboard profile switch failed: {e}")
                    });
                }
                state.emit(
                    "profileChange",
                    serde_json::json!({ "profile": prof_name, "index": idx }),
                );
                serde_json::json!({ "route": "changeProfile", "status": 200, "message": prof_name })
            } else {
                serde_json::json!({
                    "route": "changeProfile",
                    "status": 503,
                    "message": "Keyboard not connected"
                })
            }
        }
        "currentContext" => {
            let g = state.inner.lock().unwrap();
            let idx = g.applied_profile_index.unwrap_or(g.desired_profile_index);
            let ctx = g
                .profiles
                .get(idx)
                .map(|p| p.active_context.clone())
                .unwrap_or_default();
            serde_json::json!({ "route": "currentContext", "status": 200, "message": ctx })
        }
        "changeContext" => {
            let Some(ctx_name) = msg_str else {
                return serde_json::json!({ "route": "changeContext", "status": 400, "message": "Shortcut error" });
            };
            let mut g = state.inner.lock().unwrap();
            let idx = g.applied_profile_index.unwrap_or(g.desired_profile_index);
            let Some(prof) = g.profiles.get_mut(idx) else {
                return serde_json::json!({ "route": "changeContext", "status": 404, "message": "There is no currently active profile" });
            };
            if !prof.contexts.iter().any(|c| c == ctx_name) {
                return serde_json::json!({
                    "route": "changeContext",
                    "status": 404,
                    "message": "Context does not exist in shortcut collection attached to profile"
                });
            }
            prof.active_context = ctx_name.to_string();
            let ctx_out = ctx_name.to_string();
            drop(g);
            state.emit(
                "shortcutContext",
                serde_json::json!({ "shortcutContext": ctx_out }),
            );
            serde_json::json!({ "route": "changeContext", "status": 200, "message": "Context updated successfully" })
        }
        _ => serde_json::json!({ "status": 404, "message": "Route not found" }),
    }
}
