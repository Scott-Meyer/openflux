#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use reqwest::{Certificate, Client, StatusCode};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{Manager, State, WindowEvent};

mod daemon_api;
mod daemon_presence;
// Each platform uses only the parts for its own service manager.
#[allow(dead_code)]
mod daemon_service;
mod host_update;
mod navigation;

struct DaemonConn {
    base_url: String,
    token: String,
    http: Client,
}

/// The daemon's data directory, where it publishes `config/keys.json` for discovery.
/// `OPENFLUX_DATA_DIR` overrides the platform default.
fn default_data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("OPENFLUX_DATA_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    #[cfg(windows)]
    {
        if let Ok(d) = std::env::var("LOCALAPPDATA").or_else(|_| std::env::var("APPDATA")) {
            if !d.is_empty() {
                return PathBuf::from(d).join("openflux");
            }
        }
        if let Ok(d) = std::env::var("USERPROFILE") {
            if !d.is_empty() {
                return PathBuf::from(d)
                    .join("AppData")
                    .join("Local")
                    .join("openflux");
            }
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".local/share/openflux")
}

/// Connects to the daemon whose data directory is `data_dir`, enrolling if needed.
///
/// A failure is reported as [`ConnectError::NotRunning`] only when the daemon is known to be
/// gone (see [`daemon_presence`]); everything else is [`ConnectError::Failed`].
async fn connect_to_daemon_in(data_dir: &std::path::Path) -> Result<DaemonConn, ConnectError> {
    let msg = match try_connect(data_dir).await {
        Ok(c) => return Ok(c),
        Err(e) => String::from(e),
    };
    let dir = data_dir.to_path_buf();
    let presence = tokio::task::spawn_blocking(move || {
        daemon_presence::probe(&dir, daemon_presence::DEFAULT_PROBE_TIMEOUT)
    })
    .await
    .unwrap_or_else(|e| daemon_presence::Presence::Unknown {
        reason: e.to_string(),
    });
    Err(match presence {
        daemon_presence::Presence::Exited { evidence } => {
            ConnectError::NotRunning(format!("{msg} ({evidence})"))
        }
        daemon_presence::Presence::Listening => ConnectError::Failed(msg),
        daemon_presence::Presence::Unknown { reason } => {
            ConnectError::Failed(format!("{msg} ({reason})"))
        }
    })
}

async fn try_connect(data_dir: &std::path::Path) -> Result<DaemonConn, ConnectError> {
    let cfg_dir = data_dir.join("config");
    let keys_path = cfg_dir.join("keys.json");
    let keys_raw = std::fs::read_to_string(&keys_path).map_err(|e| {
        ConnectError::NotRunning(format!(
            "OpenFlux daemon discovery file {} unavailable: {e}",
            keys_path.display()
        ))
    })?;
    let keys: Value =
        serde_json::from_str(&keys_raw).map_err(|e| format!("Invalid keys.json: {e}"))?;

    let port = keys
        .get("openPort")
        .and_then(|v| v.as_u64())
        .and_then(|p| u16::try_from(p).ok())
        .ok_or_else(|| "Missing openPort in keys.json".to_string())?;

    let client = Client::builder()
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(10));
    // A daemon started with `--plaintext` serves plain HTTP on the address it names and has no
    // certificate. Otherwise the API is HTTPS with the daemon's own certificate, which names
    // `localhost`.
    let (http, base_url) = if keys.get("scheme").and_then(|v| v.as_str()) == Some("http") {
        let host: std::net::IpAddr = keys
            .get("host")
            .and_then(|v| v.as_str())
            .and_then(|h| h.parse().ok())
            .unwrap_or(std::net::IpAddr::from([127, 0, 0, 1]));
        let http = client.build().map_err(|e| format!("reqwest client: {e}"))?;
        (
            http,
            format!("http://{}", std::net::SocketAddr::new(host, port)),
        )
    } else {
        let cert_path = keys
            .get("publicKeyLocation")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing publicKeyLocation in keys.json".to_string())?;
        let cert_pem =
            std::fs::read(cert_path).map_err(|e| format!("Read cert {cert_path}: {e}"))?;
        let cert = Certificate::from_pem(&cert_pem).map_err(|e| format!("Parse cert PEM: {e}"))?;
        let http = client
            .add_root_certificate(cert)
            .use_rustls_tls()
            .build()
            .map_err(|e| format!("reqwest client: {e}"))?;
        (http, format!("https://localhost:{port}"))
    };

    let token_path = cfg_dir.join("desktop_token.txt");
    if let Ok(tok) = std::fs::read_to_string(&token_path) {
        let tok = tok.trim().to_string();
        if !tok.is_empty()
            && http
                .get(format!("{base_url}/v1/authentication/check"))
                .bearer_auth(&tok)
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false)
        {
            return Ok(DaemonConn {
                base_url,
                token: tok,
                http,
            });
        }
    }

    let secret = std::fs::read_to_string(cfg_dir.join("enrollment.secret"))
        .map_err(|e| format!("Read enrollment.secret: {e}"))?
        .trim()
        .to_string();
    let reg_res = http
        .post(format!("{base_url}/v1/authentication/register"))
        .json(&serde_json::json!({ "clientName": "openflux-studio", "enrollmentSecret": secret }))
        .send()
        .await
        .map_err(|e| {
            let msg = format!("Enrollment request failed: {e}");
            if e.is_connect() {
                ConnectError::NotRunning(msg)
            } else {
                ConnectError::Failed(msg)
            }
        })?;
    if !reg_res.status().is_success() {
        return Err(format!("Enrollment rejected ({})", reg_res.status()).into());
    }
    let reg_json: Value = reg_res.json().await.map_err(|e| e.to_string())?;
    let token = reg_json
        .get("message")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing token in enrollment response".to_string())?
        .to_string();
    // Saving the token only spares the next launch an enrollment, so a failure here is not an
    // error.
    let _ = write_token_file(&token_path, &token);
    Ok(DaemonConn {
        base_url,
        token,
        http,
    })
}

/// Writes the bearer token so only the current user can read it. On Unix the file is created
/// with mode 0600, and an existing file is set to 0600 before the token is written to it.
fn write_token_file(path: &std::path::Path, token: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(format!("{token}\n").as_bytes())
}

/// Why Studio has no connection to the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConnectError {
    /// No daemon is listening: its discovery file is missing or nothing accepts the connection.
    NotRunning(String),
    /// A daemon may be running, but connecting or enrolling failed.
    Failed(String),
}

impl From<String> for ConnectError {
    fn from(s: String) -> Self {
        ConnectError::Failed(s)
    }
}

impl From<ConnectError> for String {
    fn from(e: ConnectError) -> Self {
        match e {
            ConnectError::NotRunning(s) | ConnectError::Failed(s) => s,
        }
    }
}

/// A response from the daemon with a non-success status.
#[derive(Debug, Clone)]
pub(crate) struct ApiFailure {
    pub status: u16,
    pub body: String,
    /// One line naming the request and the status, used where a plain string error is returned.
    pub summary: String,
}

#[derive(Debug)]
pub(crate) enum DaemonErr {
    PreSendConnect(String),
    Unauthorized(String),
    AmbiguousTransport(String),
    Api(ApiFailure),
}

impl From<DaemonErr> for String {
    fn from(e: DaemonErr) -> Self {
        match e {
            DaemonErr::PreSendConnect(s)
            | DaemonErr::Unauthorized(s)
            | DaemonErr::AmbiguousTransport(s) => s,
            DaemonErr::Api(f) => f.summary,
        }
    }
}

async fn daemon_get_json(conn: &DaemonConn, path: &str) -> Result<Value, DaemonErr> {
    let res = conn
        .http
        .get(format!("{}{path}", conn.base_url))
        .bearer_auth(&conn.token)
        .send()
        .await
        .map_err(|e| {
            if e.is_connect() {
                DaemonErr::PreSendConnect(e.to_string())
            } else {
                DaemonErr::AmbiguousTransport(e.to_string())
            }
        })?;
    if res.status() == StatusCode::UNAUTHORIZED {
        return Err(DaemonErr::Unauthorized(format!("GET {path} -> 401")));
    }
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(DaemonErr::Api(ApiFailure {
            status: status.as_u16(),
            body,
            summary: format!("GET {path} -> {status}"),
        }));
    }
    res.json()
        .await
        .map_err(|e| DaemonErr::AmbiguousTransport(e.to_string()))
}

async fn daemon_req_json_with_timeout(
    conn: &DaemonConn,
    method: reqwest::Method,
    path: &str,
    body: Value,
    timeout: Duration,
) -> Result<Value, DaemonErr> {
    let res = conn
        .http
        .request(method.clone(), format!("{}{path}", conn.base_url))
        .bearer_auth(&conn.token)
        .timeout(timeout)
        .json(&body)
        .send()
        .await
        .map_err(|e| {
            if e.is_connect() {
                DaemonErr::PreSendConnect(e.to_string())
            } else {
                DaemonErr::AmbiguousTransport(format!(
                    "The connection to Keyglass was lost before it answered {method} {path}. \
                     The request may have run, so it was not replayed: {e}"
                ))
            }
        })?;
    if res.status() == StatusCode::UNAUTHORIZED {
        return Err(DaemonErr::Unauthorized(format!("{method} {path} -> 401")));
    }
    if !res.status().is_success() {
        let status = res.status();
        let txt = res.text().await.unwrap_or_default();
        return Err(DaemonErr::Api(ApiFailure {
            status: status.as_u16(),
            summary: format!("{method} {path} failed: {txt}"),
            body: txt,
        }));
    }
    res.json().await.map_err(|e| {
        DaemonErr::AmbiguousTransport(format!(
            "Keyglass's answer to {method} {path} couldn't be read. \
             The request may have run, so it was not replayed: {e}"
        ))
    })
}

struct LiveDataCache {
    cached: tokio::sync::Mutex<Option<Value>>,
    inflight: AtomicBool,
    next_at: tokio::sync::Mutex<Option<Instant>>,
}

impl LiveDataCache {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            cached: tokio::sync::Mutex::new(None),
            inflight: AtomicBool::new(false),
            next_at: tokio::sync::Mutex::new(None),
        })
    }
}

struct DesktopState {
    /// The daemon's data directory, where it publishes its discovery file and instance lock.
    data_dir: PathBuf,
    conn: tokio::sync::Mutex<Option<Arc<DaemonConn>>>,
    closing: AtomicBool,
    /// Advances each time Studio changes the daemon's config, which retires the preview's cached
    /// copy, including one a render read before the change and stores after it.
    config_gen: AtomicU64,
    live_cache: Arc<LiveDataCache>,
}

impl DesktopState {
    async fn ensure_conn(&self) -> Result<Arc<DaemonConn>, String> {
        self.ensure_conn_checked().await.map_err(String::from)
    }

    async fn ensure_conn_checked(&self) -> Result<Arc<DaemonConn>, ConnectError> {
        {
            let g = self.conn.lock().await;
            if let Some(ref c) = *g {
                return Ok(c.clone());
            }
        }
        let c = Arc::new(connect_to_daemon_in(&self.data_dir).await?);
        let mut g = self.conn.lock().await;
        if let Some(ref existing) = *g {
            return Ok(existing.clone());
        }
        *g = Some(c.clone());
        Ok(c)
    }

    async fn invalidate_conn(&self, failed: &Arc<DaemonConn>) {
        let mut g = self.conn.lock().await;
        if let Some(ref cur) = *g {
            if Arc::ptr_eq(cur, failed) {
                *g = None;
            }
        }
    }

    async fn get_json(&self, path: &str) -> Result<Value, String> {
        let c = self.ensure_conn().await?;
        match daemon_get_json(&c, path).await {
            Ok(v) => Ok(v),
            Err(DaemonErr::PreSendConnect(_))
            | Err(DaemonErr::Unauthorized(_))
            | Err(DaemonErr::AmbiguousTransport(_)) => {
                // GET is idempotent, so retry once on a fresh connection (which may re-enroll).
                self.invalidate_conn(&c).await;
                let c2 = self.ensure_conn().await?;
                daemon_get_json(&c2, path).await.map_err(String::from)
            }
            Err(e @ DaemonErr::Api(_)) => Err(e.into()),
        }
    }

    async fn req_json_with_timeout(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        let c = self.ensure_conn().await?;
        match daemon_req_json_with_timeout(&c, method.clone(), path, body.clone(), timeout).await {
            Ok(v) => Ok(v),
            Err(DaemonErr::PreSendConnect(_)) | Err(DaemonErr::Unauthorized(_)) => {
                // The request never ran: either the connection failed before sending or the
                // daemon rejected the token. Replaying on a fresh connection is safe.
                self.invalidate_conn(&c).await;
                let c2 = self.ensure_conn().await?;
                daemon_req_json_with_timeout(&c2, method, path, body, timeout)
                    .await
                    .map_err(String::from)
            }
            Err(DaemonErr::AmbiguousTransport(e)) => {
                self.invalidate_conn(&c).await;
                Err(e)
            }
            Err(e @ DaemonErr::Api(_)) => Err(e.into()),
        }
    }

    async fn req_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Value,
    ) -> Result<Value, String> {
        self.req_json_with_timeout(method, path, body, Duration::from_secs(10))
            .await
    }
}

#[tauri::command]
async fn get_status(st: State<'_, DesktopState>) -> Result<Value, String> {
    st.get_json("/v1/device/status").await
}

/// `PUT /v1/profiles/autoLock`: locks or unlocks automatic profile switching. Answers with the new
/// `autoProfileSwitch` of the device status.
#[tauri::command]
async fn set_auto_profile_lock(st: State<'_, DesktopState>, locked: bool) -> Result<Value, String> {
    st.req_json(
        reqwest::Method::PUT,
        "/v1/profiles/autoLock",
        serde_json::json!({ "locked": locked }),
    )
    .await
}

#[tauri::command]
async fn get_config(st: State<'_, DesktopState>) -> Result<Value, String> {
    st.get_json("/v1/config").await
}

#[tauri::command]
async fn get_default_config(st: State<'_, DesktopState>) -> Result<Value, String> {
    st.get_json("/v1/config/defaults").await
}

#[tauri::command]
async fn update_config(st: State<'_, DesktopState>, patch: Value) -> Result<(), String> {
    let r = st.req_json(reqwest::Method::PUT, "/v1/config", patch).await;
    // Also after a failure: a request that was sent may have changed the config.
    st.config_gen.fetch_add(1, Ordering::SeqCst);
    r.map(|_| ())
}

#[tauri::command]
async fn save_to_keyboard(st: State<'_, DesktopState>) -> Result<Value, String> {
    st.req_json_with_timeout(
        reqwest::Method::POST,
        "/v1/config/save",
        serde_json::json!({}),
        Duration::from_secs(90),
    )
    .await
}

#[tauri::command]
async fn import_bundle(st: State<'_, DesktopState>, file_path: String) -> Result<Value, String> {
    let r = st
        .req_json(
            reqwest::Method::POST,
            "/v1/config/import",
            serde_json::json!({ "file": file_path }),
        )
        .await;
    st.config_gen.fetch_add(1, Ordering::SeqCst);
    r
}

#[tauri::command]
async fn add_icon(st: State<'_, DesktopState>, file_path: String) -> Result<Value, String> {
    st.req_json(
        reqwest::Method::POST,
        "/v1/config/addIcon",
        serde_json::json!({ "icons": [{ "file": file_path }] }),
    )
    .await
}

#[tauri::command]
async fn preview_rects(_layout: Option<String>) -> Result<Value, String> {
    Ok(Value::Array(Vec::new()))
}

#[tauri::command]
async fn preview_render_rgba(_opts: Value) -> Result<tauri::ipc::Response, String> {
    Err("v1 preview removed; use OpenFlux v2 /v2/preview/frame.png".into())
}

#[tauri::command]
async fn preview_close() -> Result<(), String> {
    Ok(())
}

#[tauri::command]
async fn backup_device(st: State<'_, DesktopState>, out_dir: String) -> Result<Value, String> {
    st.req_json(
        reqwest::Method::POST,
        "/v1/maintenance/backup",
        serde_json::json!({ "outDir": out_dir }),
    )
    .await
}

#[tauri::command]
async fn flash_uf2(st: State<'_, DesktopState>, uf2_path: String) -> Result<Value, String> {
    st.req_json(
        reqwest::Method::POST,
        "/v1/maintenance/flash-uf2",
        serde_json::json!({ "uf2Path": uf2_path }),
    )
    .await
}

#[tauri::command]
async fn sideload_ipk(st: State<'_, DesktopState>, ipk_path: String) -> Result<Value, String> {
    st.req_json(
        reqwest::Method::POST,
        "/v1/maintenance/sideload-ipk",
        serde_json::json!({ "ipkPath": ipk_path }),
    )
    .await
}

#[tauri::command]
async fn get_assets_catalog(st: State<'_, DesktopState>) -> Result<Value, String> {
    st.get_json("/v1/assets").await
}

#[tauri::command]
async fn get_shortcut_pack(st: State<'_, DesktopState>, pack: String) -> Result<Value, String> {
    let c = st.ensure_conn().await?;
    let res = c
        .http
        .get(format!("{}/v1/shortcuts", c.base_url))
        .query(&[("pack", &pack)])
        .bearer_auth(&c.token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("GET /v1/shortcuts -> {}", res.status()));
    }
    res.json().await.map_err(|e| e.to_string())
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = *chunk.get(1).unwrap_or(&0) as usize;
        let b2 = *chunk.get(2).unwrap_or(&0) as usize;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18) & 63] as char);
        out.push(TABLE[(n >> 12) & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n & 63] as char
        } else {
            '='
        });
    }
    out
}

#[tauri::command]
async fn read_asset_data_url(
    st: State<'_, DesktopState>,
    cat: String,
    name: String,
) -> Result<String, String> {
    let c = st.ensure_conn().await?;
    let res = c
        .http
        .get(format!("{}/v1/asset-file", c.base_url))
        .query(&[("cat", &cat), ("name", &name)])
        .bearer_auth(&c.token)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("Asset fetch -> {}", res.status()));
    }
    let ctype = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    let bytes = res.bytes().await.map_err(|e| e.to_string())?;
    Ok(format!("data:{ctype};base64,{}", encode_base64(&bytes)))
}

#[tauri::command]
async fn get_installed_apps(st: State<'_, DesktopState>) -> Result<Value, String> {
    st.get_json("/v1/apps").await
}

#[tauri::command]
async fn import_media(
    st: State<'_, DesktopState>,
    file_path: String,
    loop_reverse: Option<bool>,
) -> Result<Value, daemon_api::CommandError> {
    st.send_api(
        reqwest::Method::POST,
        "/v1/media/import",
        serde_json::json!({ "file": file_path, "loopReverse": loop_reverse.unwrap_or(false) }),
        Duration::from_secs(420),
    )
    .await
}

#[tauri::command]
async fn export_bundle(st: State<'_, DesktopState>, out_path: String) -> Result<Value, String> {
    st.req_json_with_timeout(
        reqwest::Method::POST,
        "/v1/config/export",
        serde_json::json!({ "outPath": out_path }),
        Duration::from_secs(60),
    )
    .await
}

#[tauri::command]
async fn get_aerial_videos(st: State<'_, DesktopState>) -> Result<Value, String> {
    st.req_json_with_timeout(
        reqwest::Method::GET,
        "/v1/media/aerials",
        Value::Null,
        Duration::from_secs(20),
    )
    .await
}

#[tauri::command]
async fn import_aerial_video(
    st: State<'_, DesktopState>,
    url: String,
    title: Option<String>,
    loop_reverse: Option<bool>,
) -> Result<Value, daemon_api::CommandError> {
    st.send_api(
        reqwest::Method::POST,
        "/v1/media/aerial-import",
        serde_json::json!({ "url": url, "title": title, "loopReverse": loop_reverse.unwrap_or(false) }),
        Duration::from_secs(420),
    )
    .await
}

#[tauri::command]
async fn get_soc_stats(st: State<'_, DesktopState>) -> Result<Value, String> {
    st.req_json(reqwest::Method::GET, "/v1/device/soc-stats", Value::Null)
        .await
}

#[tauri::command]
async fn pomodoro_action(st: State<'_, DesktopState>, action: String) -> Result<Value, String> {
    let res = st
        .req_json(
            reqwest::Method::POST,
            "/v1/pomodoro",
            serde_json::json!({ "action": action }),
        )
        .await?;
    if let Some(pomo_state) = res.get("pomodoro").cloned() {
        let live_patch = serde_json::json!({ "pomodoro": pomo_state });
        {
            let mut lc = st.live_cache.cached.lock().await;
            if let Some(ref mut cur) = *lc {
                if let Some(obj) = cur.as_object_mut() {
                    obj.insert("pomodoro".into(), pomo_state.clone());
                }
            }
        }
    }
    Ok(res)
}

#[tauri::command]
async fn pick_file(
    title: Option<String>,
    extensions: Option<Vec<String>>,
) -> Result<Option<String>, String> {
    tokio::task::spawn_blocking(move || {
        let mut dlg = rfd::FileDialog::new();
        if let Some(ref t) = title {
            dlg = dlg.set_title(t);
        }
        if let Some(ref exts) = extensions {
            if !exts.is_empty() {
                let refs: Vec<&str> = exts.iter().map(|s| s.as_str()).collect();
                dlg = dlg.add_filter("Supported Files", &refs);
            }
        }
        dlg.pick_file().map(|p| p.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn pick_save_file(
    title: Option<String>,
    default_name: Option<String>,
    extensions: Option<Vec<String>>,
) -> Result<Option<String>, String> {
    tokio::task::spawn_blocking(move || {
        let mut dlg = rfd::FileDialog::new();
        if let Some(ref t) = title {
            dlg = dlg.set_title(t);
        }
        if let Some(ref dn) = default_name {
            dlg = dlg.set_file_name(dn);
        }
        if let Some(ref exts) = extensions {
            if !exts.is_empty() {
                let refs: Vec<&str> = exts.iter().map(|s| s.as_str()).collect();
                dlg = dlg.add_filter("Supported Files", &refs);
            }
        }
        dlg.save_file().map(|p| p.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
async fn pick_folder(title: Option<String>) -> Result<Option<String>, String> {
    tokio::task::spawn_blocking(move || {
        let mut dlg = rfd::FileDialog::new();
        if let Some(ref t) = title {
            dlg = dlg.set_title(t);
        }
        dlg.pick_folder().map(|p| p.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string())
}

async fn shutdown_preview_drain(st: &DesktopState, _max_wait: Duration) {
    st.closing.store(true, Ordering::SeqCst);
}

fn main() {
    let initial_page = navigation::page_from_args(std::env::args().skip(1));
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            navigation::on_second_launch(app, args);
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(navigation::NavigationState::with_initial(initial_page))
        .manage(host_update::HostUpdateState::default())
        .manage(DesktopState {
            data_dir: default_data_dir(),
            conn: tokio::sync::Mutex::new(None),
            closing: AtomicBool::new(false),
            config_gen: AtomicU64::new(0),
            live_cache: LiveDataCache::new(),
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            set_auto_profile_lock,
            get_config,
            get_default_config,
            update_config,
            save_to_keyboard,
            import_bundle,
            add_icon,
            preview_rects,
            preview_render_rgba,
            preview_close,
            backup_device,
            flash_uf2,
            sideload_ipk,
            get_assets_catalog,
            get_shortcut_pack,
            read_asset_data_url,
            get_installed_apps,
            import_media,
            export_bundle,
            get_soc_stats,
            get_aerial_videos,
            import_aerial_video,
            pomodoro_action,
            pick_file,
            pick_save_file,
            pick_folder,
            daemon_api::get_updates,
            daemon_api::check_updates,
            daemon_api::put_update_settings,
            daemon_api::plan_updates,
            daemon_api::apply_updates,
            daemon_api::install_local_updates,
            daemon_api::get_conversion,
            daemon_api::start_conversion,
            daemon_api::get_job,
            daemon_api::adopt_firmware_keymap,
            daemon_api::get_press_refinement,
            daemon_api::install_press_refinement,
            daemon_api::daemon_lifecycle,
            daemon_api::get_third_party_registrations,
            daemon_api::decide_third_party_registration,
            daemon_api::revoke_third_party_app,
            daemon_api::forget_third_party_denied,
            daemon_api::get_third_party_settings,
            daemon_api::v2_request,
            daemon_api::v2_media_url,
            daemon_api::v2_streamdeck_pi_url,
            daemon_api::v2_streamdeck_plugin_url,
            daemon_api::put_third_party_settings,
            daemon_api::open_url,
            daemon_service::daemon_service_status,
            daemon_service::daemon_service_start,
            daemon_service::open_login_items_settings,
            host_update::host_update_support,
            host_update::host_update_check,
            host_update::host_update_download,
            host_update::host_update_install,
            host_update::host_update_discard,
            host_update::relaunch_studio,
            navigation::take_initial_page,
            navigation::subscribe_navigation
        ])
        .on_window_event(|window, event| {
            // Hold the close until in-flight preview renders finish (bounded), drop the
            // preview, then exit.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle().clone();
                if let Some(st) = app.try_state::<DesktopState>() {
                    if !st.closing.swap(true, Ordering::SeqCst) {
                        api.prevent_close();
                        tauri::async_runtime::spawn(async move {
                            if let Some(st_inner) = app.try_state::<DesktopState>() {
                                shutdown_preview_drain(&st_inner, Duration::from_secs(3)).await;
                            }
                            app.exit(0);
                        });
                    } else {
                        api.prevent_close();
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running OpenFlux Studio");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn long_media_import_request_timeout_overrides_client_default() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                tokio::time::sleep(Duration::from_millis(250)).await;
                let body = r#"{"assetPath":"ok.mp4"}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            }
        });
        let http = Client::builder()
            .timeout(Duration::from_millis(80))
            .build()
            .unwrap();
        let conn = DaemonConn {
            base_url: format!("http://{addr}"),
            token: "t".into(),
            http,
        };
        let v = daemon_req_json_with_timeout(
            &conn,
            reqwest::Method::POST,
            "/v1/media/import",
            serde_json::json!({"file": "x.mov"}),
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(v["assetPath"], "ok.mp4");
    }

    #[cfg(unix)]
    #[test]
    fn the_token_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("openflux_studio_token_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("desktop_token.txt");
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;

        write_token_file(&path, "first").unwrap();
        assert_eq!(mode(&path), 0o600);
        // A file left readable by others is closed before the new token is written to it.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_token_file(&path, "second").unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn connects_to_a_plaintext_daemon_at_the_address_it_names() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let ok = String::from_utf8_lossy(&buf[..n])
                        .starts_with("GET /v1/authentication/check ");
                    let reply: &[u8] = if ok {
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
                    } else {
                        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    };
                    let _ = sock.write_all(reply).await;
                });
            }
        });
        let dir =
            std::env::temp_dir().join(format!("openflux_studio_plain_{}", std::process::id()));
        let cfg = dir.join("config");
        std::fs::create_dir_all(&cfg).unwrap();
        std::fs::write(
            cfg.join("keys.json"),
            serde_json::json!({ "openPort": addr.port(), "host": "127.0.0.1", "scheme": "http" })
                .to_string(),
        )
        .unwrap();
        std::fs::write(cfg.join("desktop_token.txt"), "tok\n").unwrap();
        let conn = try_connect(&dir).await.unwrap();
        assert_eq!(conn.base_url, format!("http://{addr}"));
        assert_eq!(conn.token, "tok");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn shutdown_drain_is_bounded_even_when_preview_stalled() {
        let st = Arc::new(DesktopState {
            data_dir: std::env::temp_dir().join("openflux_studio_unused"),
            conn: tokio::sync::Mutex::new(None),
            closing: AtomicBool::new(false),
            config_gen: AtomicU64::new(0),
            live_cache: LiveDataCache::new(),
        });
        // Holding these guards stands in for a render that never finishes.
        let _held = st.preview_inflight.read().await;
        let _held_lp = st.local_preview.lock().await;
        let t0 = std::time::Instant::now();
        shutdown_preview_drain(&st, Duration::from_millis(80)).await;
        assert!(t0.elapsed() < Duration::from_millis(350));
        assert!(st.closing.load(Ordering::SeqCst));
    }

    /// Kills and reaps the daemon when dropped, also when an assertion fails.
    struct DaemonGuard(std::process::Child);

    impl Drop for DaemonGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Size and modification time of every entry under `root`, two levels deep, so a test can
    /// show it changed nothing there.
    fn snapshot(root: &std::path::Path) -> Vec<(PathBuf, u64, Option<SystemTime>)> {
        fn walk(
            p: &std::path::Path,
            depth: u32,
            out: &mut Vec<(PathBuf, u64, Option<SystemTime>)>,
        ) {
            let Ok(meta) = std::fs::symlink_metadata(p) else {
                return;
            };
            out.push((p.to_path_buf(), meta.len(), meta.modified().ok()));
            if meta.is_dir() && depth < 2 {
                if let Ok(rd) = std::fs::read_dir(p) {
                    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
                    entries.sort();
                    for e in entries {
                        walk(&e, depth + 1, out);
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(root, 0, &mut out);
        out
    }

    /// The daemon runs from an unrelated working directory; Studio reaches it through the data
    /// directory alone.
    #[tokio::test]
    #[ignore = "needs a release build of the daemon: cargo build --release -p openfluxd"]
    async fn connects_from_any_working_directory_and_reaches_maintenance_endpoints() {
        let daemon_bin = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/release")
            .join(format!("openfluxd{}", std::env::consts::EXE_SUFFIX));
        assert!(
            daemon_bin.is_file(),
            "{} is missing; build it with cargo build --release -p openfluxd",
            daemon_bin.display()
        );
        let tmp = std::env::temp_dir().join(format!("openflux_studio_cwd_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let data = tmp.join("data");
        let home = tmp.join("home");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(home.join("AppData/Roaming")).unwrap();
        std::fs::create_dir_all(home.join("AppData/Local")).unwrap();

        // The daemon must not touch Studio's real data directory; the daemon's own tests cover
        // the rest of the home directory.
        let sentinels = [default_data_dir()];
        let before: Vec<_> = sentinels.iter().map(|p| snapshot(p)).collect();

        let child = std::process::Command::new(&daemon_bin)
            .args(["--no-tray", "--no-usb"])
            .current_dir(std::env::temp_dir())
            .env("OPENFLUX_DATA_DIR", &data)
            .env("OPENFLUX_HOME_OVERRIDE", &home)
            .env("HOME", &home)
            .env("USERPROFILE", &home)
            .env("APPDATA", home.join("AppData/Roaming"))
            .env("LOCALAPPDATA", home.join("AppData/Local"))
            .env_remove("XDG_DATA_HOME")
            .env_remove("OPENFLUX_ENROLLMENT_SECRET")
            .env("OPENFLUX_API_ADDR", "127.0.0.1:0")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let guard = DaemonGuard(child);

        let keys_path = data.join("config/keys.json");
        let secret_path = data.join("config/enrollment.secret");
        for _ in 0..80 {
            if keys_path.is_file() && secret_path.is_file() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(keys_path.is_file() && secret_path.is_file());

        let conn = connect_to_daemon_in(&data).await.unwrap();
        let st = daemon_get_json(&conn, "/v1/device/status").await.unwrap();
        assert!(st.get("connected").is_some());

        // With no keyboard attached the daemon reports a device error, which shows the
        // endpoint itself is served.
        let err = String::from(
            daemon_req_json_with_timeout(
                &conn,
                reqwest::Method::POST,
                "/v1/maintenance/backup",
                serde_json::json!({ "outDir": tmp.join("bkp").to_string_lossy() }),
                Duration::from_secs(10),
            )
            .await
            .unwrap_err(),
        );
        assert!(
            err.contains("libusb") || err.contains("Backup") || err.contains("No config pulled"),
            "{err}"
        );

        drop(guard);
        for (path, snap) in sentinels.iter().zip(&before) {
            assert_eq!(
                &snapshot(path),
                snap,
                "{} changed outside the test's home directory",
                path.display()
            );
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

}
