//! Commands for the daemon's update, conversion, job, lifecycle, keymap recovery, press
//! refinement and third-party app routes.
//!
//! These commands report failures as a [`CommandError`] object rather than a string, so the
//! interface can tell a refusal (with the daemon's error code and recovery hint) from a daemon
//! that isn't running or a request whose outcome is unknown.

use crate::{ConnectError, DaemonErr, DesktopState};
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::State;

/// What kind of failure a command hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    /// No daemon is running.
    NotRunning,
    /// The daemon couldn't be reached or enrolled with.
    Unreachable,
    /// The request was sent but the connection failed before an answer; it may have run.
    Indeterminate,
    /// The daemon answered with an error status.
    Daemon,
    /// Studio itself failed (files, updater, operating system).
    Local,
}

/// Error object returned to the interface by the commands in this module and its siblings.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub kind: ErrorKind,
    /// HTTP status, for [`ErrorKind::Daemon`].
    pub status: Option<u16>,
    /// The daemon's error code (`busy`, `keyboardDisconnected`, ...), when it sent one.
    pub code: Option<String>,
    pub message: String,
    /// What the user can do about it, when the daemon said.
    pub recovery: Option<String>,
    /// The raw request and response summary, for logs and support.
    pub detail: Option<String>,
}

impl CommandError {
    pub fn local(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Local,
            status: None,
            code: None,
            message: message.into(),
            recovery: None,
            detail: None,
        }
    }

    fn plain(kind: ErrorKind, message: String) -> Self {
        Self {
            kind,
            status: None,
            code: None,
            message,
            recovery: None,
            detail: None,
        }
    }

    /// Builds the error for a non-success response. The daemon's update routes answer
    /// `{"error": {code, message, recovery?}}`; the other routes answer `{"message": ...}`.
    pub fn from_response(status: u16, body: &str, summary: &str) -> Self {
        let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let err = parsed.get("error").filter(|e| e.is_object());
        let text = |v: Option<&Value>, k: &str| {
            v.and_then(|o| o.get(k))
                .and_then(|x| x.as_str())
                .map(str::to_string)
        };
        let code = text(err, "code");
        let message = text(err, "message")
            .or_else(|| text(Some(&parsed), "message"))
            .or_else(|| {
                parsed
                    .get("error")
                    .and_then(|e| e.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| {
                let t = body.trim();
                if t.is_empty() {
                    format!("The daemon answered with status {status}")
                } else {
                    t.to_string()
                }
            });
        Self {
            kind: ErrorKind::Daemon,
            status: Some(status),
            code,
            message,
            recovery: text(err, "recovery"),
            detail: Some(summary.to_string()),
        }
    }
}

impl From<ConnectError> for CommandError {
    fn from(e: ConnectError) -> Self {
        match e {
            ConnectError::NotRunning(s) => Self::plain(ErrorKind::NotRunning, s),
            ConnectError::Failed(s) => Self::plain(ErrorKind::Unreachable, s),
        }
    }
}

impl From<DaemonErr> for CommandError {
    fn from(e: DaemonErr) -> Self {
        match e {
            // Whether a daemon is still there is decided by `daemon_presence`, not by one
            // failed connection.
            DaemonErr::PreSendConnect(s) => Self::plain(ErrorKind::Unreachable, s),
            DaemonErr::Unauthorized(s) => Self::plain(ErrorKind::Unreachable, s),
            DaemonErr::AmbiguousTransport(s) => Self::plain(ErrorKind::Indeterminate, s),
            DaemonErr::Api(f) => Self::from_response(f.status, &f.body, &f.summary),
        }
    }
}

impl DesktopState {
    /// GET with one retry on a fresh connection, like [`DesktopState::get_json`].
    pub(crate) async fn get_api(&self, path: &str) -> Result<Value, CommandError> {
        let c = self.ensure_conn_checked().await?;
        match crate::daemon_get_json(&c, path).await {
            Ok(v) => Ok(v),
            Err(DaemonErr::Api(f)) => Err(DaemonErr::Api(f).into()),
            Err(_) => {
                self.invalidate_conn(&c).await;
                let c2 = self.ensure_conn_checked().await?;
                Ok(crate::daemon_get_json(&c2, path).await?)
            }
        }
    }

    /// A request that is replayed only when it can't have run, like
    /// [`DesktopState::req_json_with_timeout`].
    pub(crate) async fn send_api(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Value,
        timeout: Duration,
    ) -> Result<Value, CommandError> {
        let c = self.ensure_conn_checked().await?;
        match crate::daemon_req_json_with_timeout(&c, method.clone(), path, body.clone(), timeout)
            .await
        {
            Ok(v) => Ok(v),
            Err(DaemonErr::PreSendConnect(_)) | Err(DaemonErr::Unauthorized(_)) => {
                self.invalidate_conn(&c).await;
                let c2 = self.ensure_conn_checked().await?;
                Ok(crate::daemon_req_json_with_timeout(&c2, method, path, body, timeout).await?)
            }
            Err(e @ DaemonErr::AmbiguousTransport(_)) => {
                self.invalidate_conn(&c).await;
                Err(e.into())
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn post_api(&self, path: &str, body: Value) -> Result<Value, CommandError> {
        self.send_api(reqwest::Method::POST, path, body, Duration::from_secs(15))
            .await
    }

    /// `POST /v1/daemon/lifecycle`.
    ///
    /// When the request fails, the answer is `{"outcome": "not-running"}` only if the daemon is
    /// known to be gone (no discovery file or a refused port, and a free instance lock): during
    /// a host update that is what allows the install. A daemon that may still run is an error
    /// with code `daemonStateUnknown` (or the request's own error while something listens).
    pub(crate) async fn lifecycle(&self, action: &str) -> Result<Value, CommandError> {
        let body = json!({ "action": action });
        let err = match self
            .send_api(
                reqwest::Method::POST,
                "/v1/daemon/lifecycle",
                body,
                Duration::from_secs(10),
            )
            .await
        {
            Ok(v) => return Ok(v),
            // The daemon answered with an error status, so it is running.
            Err(e) if e.kind == ErrorKind::Daemon => return Err(e),
            Err(e) => e,
        };
        self.drop_conn().await;
        let dir = self.data_dir.clone();
        let presence = tokio::task::spawn_blocking(move || {
            crate::daemon_presence::probe(&dir, crate::daemon_presence::DEFAULT_PROBE_TIMEOUT)
        })
        .await
        .unwrap_or_else(|e| crate::daemon_presence::Presence::Unknown {
            reason: e.to_string(),
        });
        lifecycle_failure(err, presence)
    }

    /// Forgets the cached connection, so the next request discovers the daemon again.
    pub(crate) async fn drop_conn(&self) {
        *self.conn.lock().await = None;
    }
}

/// Ids are put into URL paths, so only letters, digits, `-` and `_` are accepted.
fn is_path_segment(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The answer to a lifecycle request that failed, given what is known about the daemon.
fn lifecycle_failure(
    err: CommandError,
    presence: crate::daemon_presence::Presence,
) -> Result<Value, CommandError> {
    use crate::daemon_presence::Presence;
    match presence {
        Presence::Exited { evidence } => Ok(json!({ "outcome": "not-running", "evidence": evidence })),
        Presence::Listening => Err(err),
        Presence::Unknown { reason } => Err(CommandError {
            kind: ErrorKind::Unreachable,
            status: None,
            code: Some("daemonStateUnknown".into()),
            message: format!(
                "OpenFlux couldn't confirm whether the background service is still running: {reason}"
            ),
            recovery: Some(
                "Wait a moment and try again. If this keeps happening, restart the computer.".into(),
            ),
            detail: Some(err.message),
        }),
    }
}

fn job_path(id: &str) -> Result<String, CommandError> {
    if !is_path_segment(id) {
        return Err(CommandError::local(format!("Invalid job id {id:?}")));
    }
    Ok(format!("/v1/jobs/{id}"))
}

#[tauri::command]
pub async fn get_updates(st: State<'_, DesktopState>) -> Result<Value, CommandError> {
    st.get_api("/v1/updates").await
}

#[tauri::command]
pub async fn check_updates(st: State<'_, DesktopState>) -> Result<Value, CommandError> {
    st.post_api("/v1/updates/check", json!({})).await
}

#[tauri::command]
pub async fn put_update_settings(
    st: State<'_, DesktopState>,
    settings: Value,
) -> Result<Value, CommandError> {
    st.send_api(
        reqwest::Method::PUT,
        "/v1/updates/settings",
        settings,
        Duration::from_secs(10),
    )
    .await
}

#[tauri::command]
pub async fn plan_updates(
    st: State<'_, DesktopState>,
    components: Option<Vec<String>>,
) -> Result<Value, CommandError> {
    let body = match components {
        Some(c) => json!({ "components": c }),
        None => json!({}),
    };
    st.post_api("/v1/updates/plan", body).await
}

#[tauri::command]
pub async fn apply_updates(
    st: State<'_, DesktopState>,
    components: Option<Vec<String>>,
    expected_steps: Option<Vec<Value>>,
) -> Result<Value, CommandError> {
    let mut obj = serde_json::Map::new();
    if let Some(c) = components {
        obj.insert("components".into(), json!(c));
    }
    if let Some(steps) = expected_steps {
        obj.insert("expectedSteps".into(), Value::Array(steps));
    }
    st.post_api("/v1/updates/apply", Value::Object(obj)).await
}

#[tauri::command]
pub async fn install_local_updates(
    st: State<'_, DesktopState>,
    components: Vec<String>,
) -> Result<Value, CommandError> {
    st.post_api(
        "/v1/updates/install-local",
        json!({ "components": components, "confirm": true }),
    )
    .await
}

#[tauri::command]
pub async fn get_conversion(st: State<'_, DesktopState>) -> Result<Value, CommandError> {
    st.get_api("/v1/conversion").await
}

#[tauri::command]
pub async fn start_conversion(
    st: State<'_, DesktopState>,
    step: String,
) -> Result<Value, CommandError> {
    let (path, body) = match step.as_str() {
        "convert" => ("/v1/conversion/convert", json!({ "confirm": true })),
        "restore" => ("/v1/conversion/restore", json!({ "confirm": true })),
        other => {
            return Err(CommandError::local(format!(
                "Unknown conversion step {other:?}"
            )))
        }
    };
    st.post_api(path, body).await
}

#[tauri::command]
pub async fn get_job(st: State<'_, DesktopState>, id: String) -> Result<Value, CommandError> {
    st.get_api(&job_path(&id)?).await
}

/// `POST /v1/keymap/adopt-firmware`: rebuilds the keymap file from the keyboard's firmware
/// keymap and writes it to the keyboard. Answers with the new config.
#[tauri::command]
pub async fn adopt_firmware_keymap(st: State<'_, DesktopState>) -> Result<Value, CommandError> {
    let r = st
        .send_api(
            reqwest::Method::POST,
            "/v1/keymap/adopt-firmware",
            json!({}),
            Duration::from_secs(90),
        )
        .await;
    // Also after a failure: a request that was sent may have changed the config.
    st.config_gen
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    r
}

/// `GET /v1/keyboard/press-refinement`: whether the keyboard has its press-refinement data.
#[tauri::command]
pub async fn get_press_refinement(st: State<'_, DesktopState>) -> Result<Value, CommandError> {
    st.get_api("/v1/keyboard/press-refinement").await
}

/// `POST /v1/keyboard/press-refinement/install`: verifies and installs the keyboard's
/// press-refinement data, downloaded or, with `path`, read from a file the user chose. A server
/// without range requests means downloading the whole published package, so the request may take
/// minutes.
#[tauri::command]
pub async fn install_press_refinement(
    st: State<'_, DesktopState>,
    path: Option<String>,
) -> Result<Value, CommandError> {
    let body = match path {
        Some(p) => json!({ "path": p }),
        None => json!({}),
    };
    st.send_api(
        reqwest::Method::POST,
        "/v1/keyboard/press-refinement/install",
        body,
        Duration::from_secs(960),
    )
    .await
}

#[tauri::command]
pub async fn daemon_lifecycle(
    st: State<'_, DesktopState>,
    action: String,
) -> Result<Value, CommandError> {
    if !matches!(action.as_str(), "status" | "stop" | "restart") {
        return Err(CommandError::local(format!(
            "Unknown lifecycle action {action:?}"
        )));
    }
    st.lifecycle(&action).await
}

#[tauri::command]
pub async fn get_third_party_registrations(
    st: State<'_, DesktopState>,
) -> Result<Value, CommandError> {
    st.get_api("/v1/third-party/registrations").await
}

#[tauri::command]
pub async fn decide_third_party_registration(
    st: State<'_, DesktopState>,
    id: String,
    decision: String,
) -> Result<Value, CommandError> {
    if !is_path_segment(&id) {
        return Err(CommandError::local(format!(
            "Invalid registration id {id:?}"
        )));
    }
    st.post_api(
        &format!("/v1/third-party/registrations/{id}"),
        json!({ "decision": decision }),
    )
    .await
}

#[tauri::command]
pub async fn revoke_third_party_app(
    st: State<'_, DesktopState>,
    app: String,
    deny: bool,
) -> Result<Value, CommandError> {
    st.post_api(
        "/v1/third-party/apps/revoke",
        json!({ "app": app, "deny": deny }),
    )
    .await
}

#[tauri::command]
pub async fn forget_third_party_denied(
    st: State<'_, DesktopState>,
    app: String,
) -> Result<Value, CommandError> {
    st.post_api("/v1/third-party/denied/forget", json!({ "app": app }))
        .await
}

#[tauri::command]
pub async fn get_third_party_settings(st: State<'_, DesktopState>) -> Result<Value, CommandError> {
    st.get_api("/v1/third-party/settings").await
}

#[tauri::command]
pub async fn put_third_party_settings(
    st: State<'_, DesktopState>,
    patch: Value,
) -> Result<Value, CommandError> {
    st.send_api(
        reqwest::Method::PUT,
        "/v1/third-party/settings",
        patch,
        Duration::from_secs(10),
    )
    .await
}

/// Generic OpenFlux v2 daemon proxy for `/v2/*` routes (bus, modules, disclosure, lists, presets, GPU preview).
#[tauri::command]
pub async fn v2_request(
    st: State<'_, DesktopState>,
    method: String,
    path: String,
    body: Option<Value>,
) -> Result<Value, CommandError> {
    if !path.starts_with("/v2/") {
        return Err(CommandError::local("v2_request path must start with /v2/"));
    }
    let m = reqwest::Method::from_bytes(method.trim().to_ascii_uppercase().as_bytes())
        .map_err(|e| CommandError::local(format!("invalid HTTP method: {e}")))?;
    if m == reqwest::Method::GET {
        st.get_api(&path).await
    } else {
        let timeout = if path.starts_with("/v2/modules/") && path.ends_with("/binding-layout") {
            Duration::from_secs(120)
        } else {
            Duration::from_secs(20)
        };
        st.send_api(m, &path, body.unwrap_or_else(|| json!({})), timeout).await
    }
}

/// Returns an authenticated loopback media streaming URL (`http://127.0.0.1:52324/v2/media/file`)
/// so KeyGlass's WebGL2 optical glass preview can play hardware-decoded 60 FPS `<video>` cleanly!
#[tauri::command]
pub async fn v2_media_url(st: State<'_, DesktopState>, name: String) -> Result<String, CommandError> {
    let fname = name.trim();
    if fname.is_empty() || fname.contains('/') || fname.contains('\\') || fname.contains("..") {
        return Err(CommandError::local("invalid media filename"));
    }
    let tok_path = st.data_dir.join("config/desktop_token.txt");
    let tok = std::fs::read_to_string(&tok_path)
        .map_err(|e| CommandError::local(format!("failed to read daemon token: {e}")))?;
    let url = reqwest::Url::parse_with_params(
        "http://127.0.0.1:52324/v2/media/file",
        &[("name", fname), ("token", tok.trim())],
    )
    .map_err(|e| CommandError::local(e.to_string()))?;
    Ok(url.to_string())
}

/// Returns an authenticated Property Inspector URL for an imported `.sdPlugin` HTML settings page.
#[tauri::command]
pub async fn v2_streamdeck_pi_url(
    st: State<'_, DesktopState>,
    plugin_id: String,
    rel_path: String,
    ws_port: u16,
    btn_id: String,
    action_uuid: String,
) -> Result<String, CommandError> {
    if plugin_id.contains("..") || rel_path.contains("..") || plugin_id.contains('/') {
        return Err(CommandError::local("invalid plugin path"));
    }
    let tok_path = st.data_dir.join("config/streamdeck_asset_token.txt");
    let tok = std::fs::read_to_string(&tok_path)
        .map_err(|e| CommandError::local(format!("failed to read scoped streamdeck token: {e}")))?;
    let base = format!(
        "http://127.0.0.1:52324/v2/streamdeck/plugin-asset/{}/{}",
        plugin_id.trim(),
        rel_path.trim_start_matches('/')
    );
    let url = reqwest::Url::parse_with_params(
        &base,
        &[
            ("token", tok.trim()),
            ("pi_boot", "1"),
            ("ws_port", &ws_port.to_string()),
            ("btn_id", btn_id.as_str()),
            ("action_uuid", action_uuid.as_str()),
        ],
    )
    .map_err(|e| CommandError::local(e.to_string()))?;
    Ok(url.to_string())
}

/// Returns an authenticated HTML `.sdPlugin` runner URL (`plugin_boot=1`) so KeyGlass can run
/// HTML/JS Stream Deck plugins (like Task Timer's `index.html`) inside a hidden WebView iframe!
#[tauri::command]
pub async fn v2_streamdeck_plugin_url(
    st: State<'_, DesktopState>,
    plugin_id: String,
    rel_path: String,
    ws_port: u16,
) -> Result<String, CommandError> {
    if plugin_id.contains("..") || rel_path.contains("..") || plugin_id.contains('/') {
        return Err(CommandError::local("invalid plugin path"));
    }
    let tok_path = st.data_dir.join("config/streamdeck_asset_token.txt");
    let tok = std::fs::read_to_string(&tok_path)
        .map_err(|e| CommandError::local(format!("failed to read scoped streamdeck token: {e}")))?;
    let base = format!(
        "http://127.0.0.1:52324/v2/streamdeck/plugin-asset/{}/{}",
        plugin_id.trim(),
        rel_path.trim_start_matches('/')
    );
    let url = reqwest::Url::parse_with_params(
        &base,
        &[
            ("token", tok.trim()),
            ("plugin_boot", "1"),
            ("ws_port", &ws_port.to_string()),
        ],
    )
    .map_err(|e| CommandError::local(e.to_string()))?;
    Ok(url.to_string())
}

/// The link `open_url` hands to the browser: a well-formed `https` URL with a host, in its
/// normalized form, so the opener receives no spaces, quotes or other raw text.
fn browser_link(url: &str) -> Result<String, CommandError> {
    match reqwest::Url::parse(url.trim()) {
        Ok(u) if u.scheme() == "https" && u.host_str().is_some_and(|h| !h.is_empty()) => {
            Ok(u.to_string())
        }
        _ => Err(CommandError::local("Only https links can be opened")),
    }
}

/// Opens a web page (release notes) in the default browser. Only `https` links are opened.
#[tauri::command]
pub fn open_url(url: String) -> Result<(), CommandError> {
    let link = browser_link(&url)?;
    let mut cmd = if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else if cfg!(windows) {
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    } else {
        std::process::Command::new("xdg-open")
    };
    let mut child = cmd
        .arg(&link)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| CommandError::local(format!("Couldn't open the browser: {e}")))?;
    // The opener hands the link over and exits; reap it so it doesn't linger.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_well_formed_https_links_reach_the_browser() {
        assert_eq!(
            browser_link("https://example.org/notes?v=1 ").unwrap(),
            "https://example.org/notes?v=1"
        );
        assert_eq!(
            browser_link("https://example.org/a b\"c").unwrap(),
            "https://example.org/a%20b%22c"
        );
        for bad in [
            "http://example.org",
            "https://",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "example.org",
            "",
        ] {
            assert!(browser_link(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn update_route_errors_keep_code_message_and_recovery() {
        let e = CommandError::from_response(
            409,
            r#"{"error":{"code":"busy","message":"Another job is running","recovery":"Wait for it to finish"}}"#,
            "POST /v1/updates/apply failed",
        );
        assert_eq!(e.kind, ErrorKind::Daemon);
        assert_eq!(e.status, Some(409));
        assert_eq!(e.code.as_deref(), Some("busy"));
        assert_eq!(e.message, "Another job is running");
        assert_eq!(e.recovery.as_deref(), Some("Wait for it to finish"));
    }

    #[test]
    fn message_only_errors_and_plain_bodies_become_the_message() {
        let e = CommandError::from_response(404, r#"{"message":"That app is not denied"}"#, "s");
        assert_eq!(e.message, "That app is not denied");
        assert_eq!(e.code, None);
        let e = CommandError::from_response(500, "boom", "s");
        assert_eq!(e.message, "boom");
        let e = CommandError::from_response(502, "", "s");
        assert!(e.message.contains("502"));
    }

    use crate::daemon_presence::tests::{data_dir, hold_lock, refused_port, write_keys};

    const TEST_CERT: &str = "-----BEGIN CERTIFICATE-----
MIIBlDCCATugAwIBAgIUaL3Y9Qzgr08MKanL3SLb9oMKNogwCgYIKoZIzj0EAwIw
FDESMBAGA1UEAwwJbG9jYWxob3N0MCAXDTI2MTEyMDAyMTkxMloYDzIxMjYxMDI3
MDIxOTEyWjAUMRIwEAYDVQQDDAlsb2NhbGhvc3QwWTATBgcqhkjOPQIBBggqhkjO
PQMBBwNCAAT9h5ZnhBotCfPQ5KueiydsNMx+ZKo5QHxct7UEyQhFJvvRq/y2R2ke
P+yK4Nv2piBWjtN80NcvPtnFRQAerJEvo2kwZzAdBgNVHQ4EFgQUEAQIjgp6zhDi
uFOm9HpmgCgWim8wHwYDVR0jBBgwFoAUEAQIjgp6zhDiuFOm9HpmgCgWim8wDwYD
VR0TAQH/BAUwAwEB/zAUBgNVHREEDTALgglsb2NhbGhvc3QwCgYIKoZIzj0EAwID
RwAwRAIgUH+AeadnNmoWXto81qgrvTFFHXMDOkzDlJmaF2GYo2ECIGMLud63bzFX
sXJiKrPC+MOlUBLYQEqaFsvPIoxj3j1z
-----END CERTIFICATE-----
";

    fn state_for(dir: &std::path::Path) -> DesktopState {
        DesktopState {
            data_dir: dir.to_path_buf(),
            conn: tokio::sync::Mutex::new(None),
            closing: std::sync::atomic::AtomicBool::new(false),
            config_gen: std::sync::atomic::AtomicU64::new(0),
            live_cache: crate::LiveDataCache::new(),
        }
    }

    fn assert_blocked(r: Result<Value, CommandError>, why: &str) {
        match r {
            Ok(v) => panic!("{why}: expected the install to be blocked, got {v}"),
            Err(e) => assert_ne!(e.kind, ErrorKind::NotRunning, "{why}: {e:?}"),
        }
    }

    #[tokio::test]
    async fn stop_is_unconfirmed_while_a_process_holds_the_lock_without_a_discovery_file() {
        let d = data_dir("api_held");
        let _lock = hold_lock(&d);
        let r = state_for(&d).lifecycle("status").await;
        let e = r.as_ref().unwrap_err();
        assert_eq!(e.code.as_deref(), Some("daemonStateUnknown"));
        assert!(e.message.contains("instance lock"), "{}", e.message);
        assert_blocked(r, "held lock");
    }

    #[tokio::test]
    async fn stop_is_unconfirmed_when_the_discovery_file_is_unreadable() {
        let d = data_dir("api_unreadable");
        std::fs::create_dir_all(d.join("config/keys.json")).unwrap();
        let r = state_for(&d).lifecycle("status").await;
        assert_eq!(
            r.as_ref().unwrap_err().code.as_deref(),
            Some("daemonStateUnknown")
        );
        assert_blocked(r, "unreadable discovery file");
    }

    #[tokio::test]
    async fn a_daemon_failing_tls_is_running_not_gone() {
        use tokio::io::AsyncWriteExt;
        let d = data_dir("api_tls");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let _ = sock.write_all(b"not a TLS server\r\n").await;
            }
        });
        let cert = d.join("cert.pem");
        std::fs::write(&cert, TEST_CERT).unwrap();
        std::fs::write(
            d.join("config/keys.json"),
            serde_json::json!({ "openPort": port, "publicKeyLocation": cert }).to_string(),
        )
        .unwrap();
        std::fs::write(d.join("config/enrollment.secret"), "s").unwrap();
        let st = state_for(&d);
        assert!(matches!(
            st.ensure_conn_checked().await,
            Err(crate::ConnectError::Failed(_))
        ));
        assert_blocked(st.lifecycle("stop").await, "TLS failure");
    }

    #[tokio::test]
    async fn a_refused_port_with_a_free_lock_is_not_running() {
        let d = data_dir("api_refused");
        write_keys(&d, refused_port());
        std::fs::write(d.join("config/openfluxd.lock"), "").unwrap();
        let st = state_for(&d);
        let v = st.lifecycle("status").await.unwrap();
        assert_eq!(v["outcome"], "not-running");
        assert!(v["evidence"]
            .as_str()
            .unwrap()
            .contains("refuses connections"));
        assert!(matches!(
            st.ensure_conn_checked().await,
            Err(crate::ConnectError::NotRunning(_))
        ));
        // The same port with the lock held is not proof.
        let _lock = hold_lock(&d);
        assert_blocked(st.lifecycle("status").await, "refused port, held lock");
    }

    #[test]
    fn job_ids_are_restricted_to_path_safe_characters() {
        assert_eq!(job_path("j-12_a").unwrap(), "/v1/jobs/j-12_a");
        assert!(job_path("../config").is_err());
        assert!(job_path("").is_err());
    }
}
