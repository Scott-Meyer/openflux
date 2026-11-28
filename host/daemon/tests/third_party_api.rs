//! Contract tests for the keyboard's third-party app API (the published Flux keyboard API,
//! https://github.com/FluxKeyboard/flux_api).
//!
//! Existing third-party apps depend on everything pinned here: discovery files and certificate
//! locations, the enrollment flow, route paths, request and response JSON, error messages,
//! WebSocket messages and keycode names. The example data uses the names those apps use, such as
//! `KC_FF_*` keycodes. Every token, app name and port in these tests is synthetic.

use futures_util::{SinkExt, StreamExt};
use keyglassd::api::build_router;
use keyglassd::api::lifecycle_client::{request as https_request, HttpResponse};
use keyglassd::api::state::{ApiState, DeviceSink, NoopDeviceSink};
use keyglassd::api::third_party::{self, DiscoveryEnv, DiscoveryPlatform, DOCS_URL, ROOT_BANNER};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

const FIRST_PARTY: &str = "first_party_test_token";
const SECRET: &str = "enrollment_test_secret";

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "keyglassd_tp_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A daemon state with persistent stores in `dir/data`, as the daemon runs in production: no
/// automatic approval, a first-party token and an enrollment secret.
fn persistent_state(dir: &Path) -> Arc<ApiState> {
    persistent_state_with(dir, Arc::new(NoopDeviceSink))
}

fn persistent_state_with(dir: &Path, device: Arc<dyn DeviceSink>) -> Arc<ApiState> {
    let data = dir.join("data");
    std::fs::create_dir_all(data.join("config")).unwrap();
    ApiState::with_persistent_stores(
        device,
        Some(FIRST_PARTY.into()),
        Duration::from_secs(20),
        data.join("assets/key_icons"),
        data.join("config/api_tokens.json"),
        SECRET.into(),
    )
}

async fn serve(state: Arc<ApiState>) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state);
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn http(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let mut raw = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    for (k, v) in headers {
        raw.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        raw.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ));
    } else {
        raw.push_str("\r\n");
    }
    stream.write_all(raw.as_bytes()).await.unwrap();
    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).await.unwrap();
    let txt = String::from_utf8_lossy(&resp).to_string();
    let (head, body) = txt.split_once("\r\n\r\n").unwrap_or((&txt, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, body.to_string())
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

fn json(body: &str) -> serde_json::Value {
    serde_json::from_str(body).unwrap_or(serde_json::Value::Null)
}

async fn pending_ids(addr: SocketAddr, app: &str) -> Vec<String> {
    let auth = bearer(FIRST_PARTY);
    let (code, body) = http(
        addr,
        "GET",
        "/v1/third-party/registrations",
        &[("Authorization", &auth)],
        None,
    )
    .await;
    assert_eq!(code, 200, "{body}");
    json(&body)["pending"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["app"] == app)
        .map(|p| p["id"].as_str().unwrap().to_string())
        .collect()
}

async fn wait_pending(addr: SocketAddr, app: &str) -> String {
    for _ in 0..200 {
        if let Some(id) = pending_ids(addr, app).await.into_iter().next() {
            return id;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no pending registration for {app}");
}

async fn decide(addr: SocketAddr, id: &str, decision: &str) -> (u16, String) {
    let auth = bearer(FIRST_PARTY);
    http(
        addr,
        "POST",
        &format!("/v1/third-party/registrations/{id}"),
        &[("Authorization", &auth)],
        Some(&serde_json::json!({ "decision": decision }).to_string()),
    )
    .await
}

/// Registers like a documented client (`{}` body, app named by User-Agent) and approves it.
async fn enroll_app(addr: SocketAddr, app: &'static str) -> String {
    let reg = tokio::spawn(async move {
        http(
            addr,
            "POST",
            "/v1/authentication/register",
            &[("User-Agent", app)],
            Some("{}"),
        )
        .await
    });
    let id = wait_pending(addr, app).await;
    assert_eq!(decide(addr, &id, "approve").await.0, 200);
    let (code, body) = reg.await.unwrap();
    assert_eq!(code, 200, "{body}");
    json(&body)["message"].as_str().unwrap().to_string()
}

async fn put_settings(addr: SocketAddr, patch: serde_json::Value) {
    let auth = bearer(FIRST_PARTY);
    let (code, body) = http(
        addr,
        "PUT",
        "/v1/third-party/settings",
        &[("Authorization", &auth)],
        Some(&patch.to_string()),
    )
    .await;
    assert_eq!(code, 200, "{body}");
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

async fn ws_connect(addr: SocketAddr, token: &str) -> Ws {
    let mut req = format!("ws://{addr}/v1/configChange")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("Authorization", bearer(token).parse().unwrap());
    tokio_tungstenite::connect_async(req).await.unwrap().0
}

/// Sends a WebSocket request and returns the first reply that answers a route (events are skipped).
async fn ws_call(ws: &mut Ws, msg: serde_json::Value) -> serde_json::Value {
    ws_call_with_events(ws, msg).await.0
}

/// [`ws_call`], also returning the events that arrived before the reply.
async fn ws_call_with_events(
    ws: &mut Ws,
    msg: serde_json::Value,
) -> (serde_json::Value, Vec<serde_json::Value>) {
    ws.send(Message::Text(msg.to_string())).await.unwrap();
    let mut events = Vec::new();
    loop {
        let m = tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .expect("ws reply")
            .unwrap()
            .unwrap();
        if let Message::Text(t) = m {
            let v = json(&t);
            if v.get("event").is_none() {
                return (v, events);
            }
            events.push(v);
        }
    }
}

/// Waits for the server to close the socket; returns the close code, or `None` if it stayed open.
async fn ws_closed(ws: &mut Ws, within: Duration) -> Option<u16> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        match tokio::time::timeout_at(deadline, ws.next()).await {
            Err(_) => return None,
            Ok(None) => return Some(1006),
            Ok(Some(Ok(Message::Close(frame)))) => {
                return Some(frame.map(|f| u16::from(f.code)).unwrap_or(1005))
            }
            Ok(Some(Err(_))) => return Some(1006),
            Ok(Some(Ok(_))) => continue,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------------------------

/// What a documented client does: read keys.json, check apiActive, trust
/// `$publicKeyLocation/cert.pem`, and ping `GET /v1/docs` over TLS.
fn client_reaches_docs(keys_file: &Path) -> HttpResponse {
    let keys: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(keys_file).unwrap()).unwrap();
    assert_eq!(keys["apiActive"], true, "{}", keys_file.display());
    let port = keys["openPort"].as_u64().unwrap() as u16;
    let cert = std::fs::read_to_string(
        Path::new(keys["publicKeyLocation"].as_str().unwrap()).join("cert.pem"),
    )
    .unwrap();
    https_request(
        SocketAddr::from(([127, 0, 0, 1], port)),
        Some(&cert),
        "GET",
        "/v1/docs",
        &[],
        None,
        Duration::from_secs(5),
    )
    .unwrap()
}

fn cert_reaches_docs(cert_file: &Path, port: u16) -> HttpResponse {
    let cert = std::fs::read_to_string(cert_file).unwrap();
    https_request(
        SocketAddr::from(([127, 0, 0, 1], port)),
        Some(&cert),
        "GET",
        "/v1/docs",
        &[],
        None,
        Duration::from_secs(5),
    )
    .unwrap()
}

#[tokio::test]
async fn discovery_files_lead_clients_to_the_tls_api_for_every_layout() {
    let tmp = temp_dir("discovery");
    let bundle = keyglassd::api::tls::ensure_tls_bundle(&tmp.join("tls")).unwrap();
    let state = persistent_state(&tmp);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let running = Arc::new(AtomicBool::new(true));
    let app = build_router(state.clone());
    let acc = bundle.acceptor.clone();
    let r = running.clone();
    tokio::spawn(async move { keyglassd::api::tls::serve_tls_router(listener, acc, app, r).await });

    // (name, platform, XDG_DATA_HOME ("ABS" for an absolute one under home), keys.json files, cert files)
    type Case = (
        &'static str,
        DiscoveryPlatform,
        Option<&'static str>,
        Vec<&'static str>,
        Vec<&'static str>,
    );
    let cases: Vec<Case> = vec![
        (
            "linux_xdg_absolute",
            DiscoveryPlatform::Linux,
            Some("ABS"),
            vec![
                "flux/config/keys.json",
                "xdg/flux/config/keys.json",
                ".local/share/flux/config/keys.json",
            ],
            vec![
                ".local/share/com.fluxkeyboard.polymath/tls/cert.pem",
                "xdg/com.fluxkeyboard.polymath/tls/cert.pem",
            ],
        ),
        (
            "linux_xdg_relative",
            DiscoveryPlatform::Linux,
            Some("relative/xdg"),
            vec![
                "flux/config/keys.json",
                ".local/share/flux/config/keys.json",
            ],
            vec![".local/share/com.fluxkeyboard.polymath/tls/cert.pem"],
        ),
        (
            "linux_xdg_unset",
            DiscoveryPlatform::Linux,
            None,
            vec![
                "flux/config/keys.json",
                ".local/share/flux/config/keys.json",
            ],
            vec![".local/share/com.fluxkeyboard.polymath/tls/cert.pem"],
        ),
        (
            "macos",
            DiscoveryPlatform::MacOs,
            None,
            vec!["Documents/flux/config/keys.json"],
            vec!["Library/Application Support/com.flux.polymath/tls/cert.pem"],
        ),
        (
            "windows",
            DiscoveryPlatform::Windows,
            None,
            vec!["Documents/flux/config/keys.json"],
            vec!["AppData/Roaming/com.fluxkeyboard/polymath/tls/cert.pem"],
        ),
    ];

    for (name, platform, xdg, keys_rel, certs_rel) in cases {
        let home = tmp.join(name);
        let env = DiscoveryEnv {
            home: Some(home.clone()),
            xdg_data_home: xdg.map(|x| {
                if x == "ABS" {
                    home.join("xdg")
                } else {
                    PathBuf::from(x)
                }
            }),
            appdata: None,
            local_appdata: None,
        };
        let failures = third_party::start(
            &state,
            &env,
            platform,
            port,
            &bundle.cert_pem,
            &bundle.cert_sha256,
        );
        assert!(failures.is_empty(), "{name}: {failures:?}");
        let locations = third_party::discovery_locations(platform, &env);
        let expected_keys: Vec<PathBuf> = keys_rel.iter().map(|r| home.join(r)).collect();
        assert_eq!(locations.keys_files, expected_keys, "{name}");

        let mut contents = Vec::new();
        for k in &expected_keys {
            contents.push(std::fs::read_to_string(k).unwrap());
            let kf = k.clone();
            let resp = tokio::task::spawn_blocking(move || client_reaches_docs(&kf))
                .await
                .unwrap();
            assert_eq!(resp.status, 200, "{name} {}", k.display());
            assert_eq!(String::from_utf8_lossy(&resp.body), DOCS_URL);
        }
        assert!(
            contents.windows(2).all(|w| w[0] == w[1]),
            "{name}: every keys.json is identical"
        );
        let doc = json(&contents[0]);
        assert_eq!(doc["openPort"], port);
        assert_eq!(doc["certHash"], bundle.cert_sha256);
        for c in certs_rel {
            let cf = home.join(c);
            let resp = tokio::task::spawn_blocking(move || cert_reaches_docs(&cf, port))
                .await
                .unwrap();
            assert_eq!(resp.status, 200, "{name} {c}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&expected_keys[0])
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(
                mode, 0o600,
                "keys.json can hold tokens, so only the user may read it"
            );
        }
    }

    running.store(false, Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn docs_and_banner_answer_without_authentication() {
    let tmp = temp_dir("docs");
    let addr = serve(persistent_state(&tmp)).await;
    assert_eq!(
        http(addr, "GET", "/v1/docs", &[], None).await,
        (200, DOCS_URL.to_string())
    );
    assert_eq!(
        http(addr, "GET", "/", &[], None).await,
        (200, ROOT_BANNER.to_string())
    );
    assert_eq!(DOCS_URL, "https://github.com/FluxKeyboard/flux_api");
    assert_eq!(ROOT_BANNER, "Keyglass API, compatible with the Flux keyboard API: https://github.com/FluxKeyboard/flux_api");
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------------------------
// Enrollment
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_registering_app_waits_for_the_user_and_gets_a_working_token_on_approval() {
    let tmp = temp_dir("approve");
    let addr = serve(persistent_state(&tmp)).await;
    let reg = tokio::spawn(async move {
        http(
            addr,
            "POST",
            "/v1/authentication/register",
            &[("User-Agent", "Example Plugin")],
            Some("{}"),
        )
        .await
    });
    let id = wait_pending(addr, "Example Plugin").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !reg.is_finished(),
        "registration waits for the user's decision"
    );

    let (code, body) = decide(addr, &id, "approve").await;
    assert_eq!(code, 200, "{body}");
    let (code, body) = reg.await.unwrap();
    assert_eq!(code, 200);
    let token = json(&body)["message"].as_str().unwrap().to_string();
    assert!(!token.is_empty());

    let auth = bearer(&token);
    assert_eq!(
        http(
            addr,
            "GET",
            "/v1/authentication/check",
            &[("Authorization", &auth)],
            None
        )
        .await,
        (200, String::new())
    );
    let (code, body) = http(
        addr,
        "GET",
        "/v1/config/keymapData",
        &[("Authorization", &auth)],
        None,
    )
    .await;
    assert_eq!(code, 200, "{body}");
    assert!(json(&body)["message"]["0"].is_array());
    assert!(pending_ids(addr, "Example Plugin").await.is_empty());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn a_denied_app_is_refused_now_on_retry_and_after_a_restart() {
    let tmp = temp_dir("deny");
    let addr = serve(persistent_state(&tmp)).await;
    let reg = tokio::spawn(async move {
        http(
            addr,
            "POST",
            "/v1/authentication/register",
            &[("User-Agent", "Unwanted App")],
            Some("{}"),
        )
        .await
    });
    let id = wait_pending(addr, "Unwanted App").await;
    assert_eq!(decide(addr, &id, "deny").await.0, 200);
    let denied = (403, r#"{"message":"User denied the request"}"#.to_string());
    assert_eq!(reg.await.unwrap(), denied);
    let retry = tokio::time::timeout(
        Duration::from_secs(1),
        http(
            addr,
            "POST",
            "/v1/authentication/register",
            &[("User-Agent", "Unwanted App")],
            Some("{}"),
        ),
    )
    .await
    .expect("a denied app is refused immediately");
    assert_eq!(retry, denied);

    let addr2 = serve(persistent_state(&tmp)).await;
    let after_restart = tokio::time::timeout(
        Duration::from_secs(1),
        http(
            addr2,
            "POST",
            "/v1/authentication/register",
            &[("User-Agent", "Unwanted App")],
            Some("{}"),
        ),
    )
    .await
    .expect("the denial survives a restart");
    assert_eq!(after_restart, denied);

    // Forgetting the denial lets the app ask again.
    let auth = bearer(FIRST_PARTY);
    let (code, _) = http(
        addr2,
        "POST",
        "/v1/third-party/denied/forget",
        &[("Authorization", &auth)],
        Some(r#"{"app":"Unwanted App"}"#),
    )
    .await;
    assert_eq!(code, 200);
    let again = tokio::spawn(async move {
        http(
            addr2,
            "POST",
            "/v1/authentication/register",
            &[("User-Agent", "Unwanted App")],
            Some("{}"),
        )
        .await
    });
    let id = wait_pending(addr2, "Unwanted App").await;
    assert_eq!(decide(addr2, &id, "approve").await.0, 200);
    assert_eq!(again.await.unwrap().0, 200);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn an_unanswered_registration_times_out_with_408() {
    let tmp = temp_dir("timeout");
    let state = persistent_state(&tmp);
    state.set_registration_timeout(Duration::from_millis(200));
    let addr = serve(state).await;
    let (code, body) = http(
        addr,
        "POST",
        "/v1/authentication/register",
        &[("User-Agent", "Slow User App")],
        Some("{}"),
    )
    .await;
    assert_eq!(code, 408);
    assert_eq!(
        json(&body),
        serde_json::json!({"message": "The registration was not approved in time"})
    );
    assert!(
        pending_ids(addr, "Slow User App").await.is_empty(),
        "nobody waits for it any more"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn a_rejected_token_leads_the_app_to_register_again() {
    let tmp = temp_dir("reregister");
    let addr = serve(persistent_state(&tmp)).await;
    let (code, body) = http(
        addr,
        "GET",
        "/v1/authentication/check",
        &[("Authorization", "Bearer revoked_or_unknown")],
        None,
    )
    .await;
    assert_eq!(
        (code, json(&body)),
        (401, serde_json::json!({"message": "Invalid token"}))
    );
    let (code, body) = http(addr, "GET", "/v1/authentication/check", &[], None).await;
    assert_eq!(
        (code, json(&body)),
        (
            401,
            serde_json::json!({"message": "Missing or invalid Authorization header"})
        )
    );
    let token = enroll_app(addr, "Returning App").await;
    assert_eq!(
        http(
            addr,
            "GET",
            "/v1/authentication/check",
            &[("Authorization", &bearer(&token))],
            None
        )
        .await
        .0,
        200
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn registration_from_an_untrusted_origin_is_refused() {
    let tmp = temp_dir("origin");
    let addr = serve(persistent_state(&tmp)).await;
    let (code, body) = http(
        addr,
        "POST",
        "/v1/authentication/register",
        &[
            ("User-Agent", "Web Page"),
            ("Origin", "https://evil.example"),
        ],
        Some("{}"),
    )
    .await;
    assert_eq!(
        (code, json(&body)),
        (403, serde_json::json!({"message": "Untrusted Origin"}))
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn tokens_from_the_shared_token_store_keep_working() {
    let tmp = temp_dir("shared_store");
    let home = tmp.join("home");
    let shared = home.join(".local/share/flux/config");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(
        shared.join("api_tokens.json"),
        r#"["shared_store_test_token"]"#,
    )
    .unwrap();
    let state = persistent_state(&tmp);
    let env = DiscoveryEnv {
        home: Some(home),
        xdg_data_home: None,
        appdata: None,
        local_appdata: None,
    };
    third_party::start(&state, &env, DiscoveryPlatform::Linux, 1, "cert", "hash");
    let addr = serve(state).await;
    let auth = bearer("shared_store_test_token");
    assert_eq!(
        http(
            addr,
            "GET",
            "/v1/authentication/check",
            &[("Authorization", &auth)],
            None
        )
        .await
        .0,
        200
    );

    // It is a third-party token and survives a restart.
    assert_eq!(
        http(
            addr,
            "GET",
            "/v1/device/status",
            &[("Authorization", &auth)],
            None
        )
        .await
        .0,
        403
    );
    let addr2 = serve(persistent_state(&tmp)).await;
    assert_eq!(
        http(
            addr2,
            "GET",
            "/v1/authentication/check",
            &[("Authorization", &auth)],
            None
        )
        .await
        .0,
        200
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------------------------
// Migration of an existing keys.json
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn an_existing_keys_json_is_adopted_and_its_unknown_fields_survive() {
    let tmp = temp_dir("migrate");
    let home = tmp.join("home");
    let keys = home.join("flux/config/keys.json");
    std::fs::create_dir_all(keys.parent().unwrap()).unwrap();
    std::fs::write(
        &keys,
        serde_json::json!({
            "apiKeys": { "Legacy Test App": "legacy_test_token" },
            "rejected": { "Rejected Test App": { "at": 1 } },
            "banned": ["Banned Test App"],
            "openPort": 1,
            "apiActive": true,
            "apiNetworkAccess": false,
            "saveToKeyboard": true,
            "import": true,
            "profileChange": false,
            "shortcutChange": true,
            "mosaicChange": true,
            "domain": "",
            "certHash": "old",
            "publicKeyLocation": "/old/tls",
            "futureField": { "keep": [1, 2] }
        })
        .to_string(),
    )
    .unwrap();
    let state = persistent_state(&tmp);
    let env = DiscoveryEnv {
        home: Some(home.clone()),
        xdg_data_home: None,
        appdata: None,
        local_appdata: None,
    };
    let failures = third_party::start(
        &state,
        &env,
        DiscoveryPlatform::Linux,
        4321,
        "CERT",
        "newhash",
    );
    assert!(failures.is_empty(), "{failures:?}");
    let addr = serve(state).await;

    let auth = bearer("legacy_test_token");
    assert_eq!(
        http(
            addr,
            "GET",
            "/v1/authentication/check",
            &[("Authorization", &auth)],
            None
        )
        .await
        .0,
        200
    );
    for app in ["Banned Test App", "Rejected Test App"] {
        let r = tokio::time::timeout(
            Duration::from_secs(1),
            http(
                addr,
                "POST",
                "/v1/authentication/register",
                &[("User-Agent", app)],
                Some("{}"),
            ),
        )
        .await
        .expect("refused immediately");
        assert_eq!(r.0, 403, "{app}");
    }

    let mut ws = ws_connect(addr, "legacy_test_token").await;
    assert_eq!(
        ws_call(
            &mut ws,
            serde_json::json!({"route": "changeProfile", "message": "Default"})
        )
        .await,
        serde_json::json!({"route": "changeProfile", "status": 403, "message": "User denied request"}),
        "profileChange=false is honoured"
    );

    let rewritten = json(&std::fs::read_to_string(&keys).unwrap());
    assert_eq!(
        rewritten["futureField"],
        serde_json::json!({ "keep": [1, 2] })
    );
    assert_eq!(
        rewritten["apiKeys"],
        serde_json::json!({ "Legacy Test App": "legacy_test_token" })
    );
    assert_eq!(
        rewritten["rejected"],
        serde_json::json!({ "Rejected Test App": { "at": 1 } })
    );
    assert_eq!(rewritten["banned"], serde_json::json!(["Banned Test App"]));
    assert_eq!(rewritten["openPort"], 4321);
    assert_eq!(rewritten["certHash"], "newhash");
    assert_eq!(rewritten["apiActive"], true);
    assert_eq!(rewritten["profileChange"], false);
    assert_eq!(
        rewritten["publicKeyLocation"],
        home.join(".local/share/com.fluxkeyboard.polymath/tls")
            .to_string_lossy()
            .to_string()
    );
    let fp = bearer(FIRST_PARTY);
    let (_, settings) = http(
        addr,
        "GET",
        "/v1/third-party/settings",
        &[("Authorization", &fp)],
        None,
    )
    .await;
    assert_eq!(json(&settings)["profileChange"], false);
    let _ = std::fs::remove_dir_all(&tmp);
}

fn write_legacy_keys(home: &Path, doc: serde_json::Value) {
    let keys = home.join("flux/config/keys.json");
    std::fs::create_dir_all(keys.parent().unwrap()).unwrap();
    std::fs::write(&keys, doc.to_string()).unwrap();
}

fn start_third_party(state: &Arc<ApiState>, home: &Path) {
    let env = DiscoveryEnv {
        home: Some(home.to_path_buf()),
        xdg_data_home: None,
        appdata: None,
        local_appdata: None,
    };
    let failures = third_party::start(state, &env, DiscoveryPlatform::Linux, 4321, "CERT", "hash");
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn a_revoked_imported_token_stays_revoked_after_a_restart() {
    let tmp = temp_dir("revoke_restart");
    let home = tmp.join("home");
    write_legacy_keys(
        &home,
        serde_json::json!({"apiKeys": {"Imported App": "imported_token"}, "apiActive": true}),
    );
    let state = persistent_state(&tmp);
    start_third_party(&state, &home);
    let addr = serve(state).await;
    let auth = bearer("imported_token");
    assert_eq!(
        http(
            addr,
            "GET",
            "/v1/authentication/check",
            &[("Authorization", &auth)],
            None
        )
        .await
        .0,
        200
    );
    let fp = bearer(FIRST_PARTY);
    let (code, _) = http(
        addr,
        "POST",
        "/v1/third-party/apps/revoke",
        &[("Authorization", &fp)],
        Some(r#"{"app":"Imported App"}"#),
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(
        http(
            addr,
            "GET",
            "/v1/authentication/check",
            &[("Authorization", &auth)],
            None
        )
        .await
        .0,
        401
    );
    let keys = json(&std::fs::read_to_string(home.join("flux/config/keys.json")).unwrap());
    assert_eq!(
        keys["apiKeys"],
        serde_json::json!({}),
        "keys.json no longer lists the revoked token"
    );

    // Restart: the legacy file is read again, but the import already happened.
    write_legacy_keys(
        &home,
        serde_json::json!({"apiKeys": {"Imported App": "imported_token"}, "apiActive": true}),
    );
    let state = persistent_state(&tmp);
    start_third_party(&state, &home);
    let addr = serve(state).await;
    assert_eq!(
        http(
            addr,
            "GET",
            "/v1/authentication/check",
            &[("Authorization", &auth)],
            None
        )
        .await
        .0,
        401
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn a_forgotten_imported_denial_stays_forgotten_after_a_restart() {
    // `rejected` is a map of apps or a list of them.
    for rejected in [
        serde_json::json!({"Denied App": {"at": 1}}),
        serde_json::json!(["Denied App"]),
    ] {
        forget_an_imported_denial(rejected).await;
    }
}

async fn forget_an_imported_denial(rejected: serde_json::Value) {
    let empty = if rejected.is_array() {
        serde_json::json!([])
    } else {
        serde_json::json!({})
    };
    let tmp = temp_dir("forget_restart");
    let home = tmp.join("home");
    write_legacy_keys(
        &home,
        serde_json::json!({"banned": ["Denied App"], "rejected": rejected, "apiActive": true}),
    );
    let state = persistent_state(&tmp);
    start_third_party(&state, &home);
    let addr = serve(state.clone()).await;
    let fp = bearer(FIRST_PARTY);
    let (code, _) = http(
        addr,
        "POST",
        "/v1/third-party/denied/forget",
        &[("Authorization", &fp)],
        Some(r#"{"app":"Denied App"}"#),
    )
    .await;
    assert_eq!(code, 200);
    let keys = json(&std::fs::read_to_string(home.join("flux/config/keys.json")).unwrap());
    assert_eq!(keys["banned"], serde_json::json!([]));
    assert_eq!(keys["rejected"], empty);

    let state = persistent_state(&tmp);
    start_third_party(&state, &home);
    assert!(
        !state
            .inner
            .lock()
            .unwrap()
            .denied_apps
            .contains("Denied App"),
        "the denial is not imported again"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn api_active_keeps_the_imported_value_and_starts_on_for_a_fresh_install() {
    let tmp = temp_dir("api_active_import");
    let home = tmp.join("home");
    write_legacy_keys(
        &home,
        serde_json::json!({"apiActive": false, "profileChange": true}),
    );
    let state = persistent_state(&tmp);
    start_third_party(&state, &home);
    assert!(
        !state.inner.lock().unwrap().third_party.api_active,
        "the user's previous choice is kept"
    );
    let keys = json(&std::fs::read_to_string(home.join("flux/config/keys.json")).unwrap());
    assert_eq!(keys["apiActive"], false);

    let fresh = temp_dir("api_active_fresh");
    let state = persistent_state(&fresh);
    start_third_party(&state, &fresh.join("home"));
    assert!(
        state.inner.lock().unwrap().third_party.api_active,
        "a fresh install starts with the API on"
    );
    let _ = std::fs::remove_dir_all(&tmp);
    let _ = std::fs::remove_dir_all(&fresh);
}

// ---------------------------------------------------------------------------------------------
// Token kinds and permissions
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn third_party_tokens_cannot_use_keyglass_routes() {
    let tmp = temp_dir("kinds");
    let addr = serve(persistent_state(&tmp)).await;
    let token = enroll_app(addr, "Scoped App").await;
    let auth = bearer(&token);
    let refused = serde_json::json!({"message": "This token may only use the third-party app API"});
    for (method, path, body) in [
        ("GET", "/v1/config", None),
        ("PUT", "/v1/config", Some("{}")),
        ("GET", "/v1/config/defaults", None),
        ("POST", "/v1/config/pull", Some("{}")),
        ("POST", "/v1/config/export", Some(r#"{"outPath":"/tmp/x"}"#)),
        ("GET", "/v1/assets", None),
        ("GET", "/v1/shortcuts?pack=a.csv", None),
        ("GET", "/v1/asset-file?cat=fonts&name=a.ttf", None),
        ("GET", "/v1/apps", None),
        ("POST", "/v1/media/import", Some(r#"{"file":"/tmp/x.mp4"}"#)),
        ("GET", "/v1/media/aerials", None),
        (
            "POST",
            "/v1/media/aerial-import",
            Some(r#"{"url":"https://sylvan.apple.com/a.mov"}"#),
        ),
        (
            "POST",
            "/v1/maintenance/backup",
            Some(r#"{"outDir":"/tmp/x"}"#),
        ),
        (
            "POST",
            "/v1/maintenance/flash-uf2",
            Some(r#"{"uf2Path":"/tmp/x.uf2"}"#),
        ),
        (
            "POST",
            "/v1/maintenance/sideload-ipk",
            Some(r#"{"ipkPath":"/tmp/x.ipk"}"#),
        ),
        ("GET", "/v1/device/status", None),
        ("GET", "/v1/live-data", None),
        ("GET", "/v1/device/soc-stats", None),
        ("POST", "/v1/pomodoro", Some(r#"{"action":"toggle"}"#)),
        ("GET", "/v1/preview/rects", None),
        ("POST", "/v1/preview/render", Some("{}")),
        ("POST", "/v1/preview/close", Some("{}")),
        ("GET", "/v1/third-party/registrations", None),
        (
            "POST",
            "/v1/third-party/registrations/x",
            Some(r#"{"decision":"approve"}"#),
        ),
        (
            "POST",
            "/v1/third-party/denied/forget",
            Some(r#"{"app":"x"}"#),
        ),
        (
            "POST",
            "/v1/third-party/apps/revoke",
            Some(r#"{"app":"x"}"#),
        ),
        ("GET", "/v1/third-party/settings", None),
        ("PUT", "/v1/third-party/settings", Some("{}")),
    ] {
        let (code, resp) = http(addr, method, path, &[("Authorization", &auth)], body).await;
        assert_eq!(
            (code, json(&resp)),
            (403, refused.clone()),
            "{method} {path}"
        );
    }
    let (code, _) = http(
        addr,
        "POST",
        "/v1/daemon/lifecycle",
        &[("Authorization", &auth)],
        Some(r#"{"action":"stop"}"#),
    )
    .await;
    assert_eq!(code, 401, "a third-party token can't stop the daemon");

    // First-party tokens may use the third-party routes too.
    let fp = bearer(FIRST_PARTY);
    assert_eq!(
        http(
            addr,
            "GET",
            "/v1/config/keymapData",
            &[("Authorization", &fp)],
            None
        )
        .await
        .0,
        200
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn permission_flags_govern_their_routes_for_third_party_tokens_only() {
    let tmp = temp_dir("perms");
    let addr = serve(persistent_state(&tmp)).await;
    let token = enroll_app(addr, "Permission App").await;
    let tp = bearer(&token);
    let fp = bearer(FIRST_PARTY);
    let denied = (403, r#"{"message":"User denied the request"}"#.to_string());
    put_settings(
        addr,
        serde_json::json!({"saveToKeyboard": false, "import": false, "mosaicChange": false}),
    )
    .await;

    assert_eq!(
        http(
            addr,
            "POST",
            "/v1/config/save",
            &[("Authorization", &tp)],
            Some("{}")
        )
        .await,
        denied
    );
    assert_eq!(
        http(
            addr,
            "POST",
            "/v1/config/import",
            &[("Authorization", &tp)],
            Some(r#"{"file":"/tmp/a.flux"}"#)
        )
        .await,
        denied
    );
    assert_eq!(
        http(
            addr,
            "POST",
            "/v1/config/addGenericTile",
            &[("Authorization", &tp)],
            Some(r#"{"title":"T","type":0,"index":0,"length":5}"#)
        )
        .await,
        denied
    );
    assert_eq!(
        http(
            addr,
            "PUT",
            "/v1/config/updateGenericTile",
            &[("Authorization", &tp)],
            Some(r#"{"id":"T","currentValue":"1"}"#)
        )
        .await,
        denied
    );
    // Ungoverned routes still work.
    assert_eq!(
        http(
            addr,
            "GET",
            "/v1/config/keymapData",
            &[("Authorization", &tp)],
            None
        )
        .await
        .0,
        200
    );
    // Keyglass's own clients aren't limited by the flags.
    assert_ne!(
        http(
            addr,
            "POST",
            "/v1/config/save",
            &[("Authorization", &fp)],
            Some("{}")
        )
        .await
        .0,
        403
    );

    // With the API off, third-party tokens and registrations get 503; first-party tokens don't.
    put_settings(addr, serde_json::json!({"apiActive": false})).await;
    let off = (
        503,
        r#"{"message":"The third-party app API is turned off"}"#.to_string(),
    );
    assert_eq!(
        http(
            addr,
            "GET",
            "/v1/authentication/check",
            &[("Authorization", &tp)],
            None
        )
        .await,
        off
    );
    assert_eq!(
        http(
            addr,
            "POST",
            "/v1/authentication/register",
            &[("User-Agent", "Late App")],
            Some("{}")
        )
        .await,
        off
    );
    assert_eq!(
        http(
            addr,
            "GET",
            "/v1/authentication/check",
            &[("Authorization", &fp)],
            None
        )
        .await
        .0,
        200
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------------------------
// Routes and payloads
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn config_routes_answer_with_the_documented_payloads() {
    let tmp = temp_dir("payloads");
    let state = persistent_state(&tmp);
    let addr = serve(state.clone()).await;
    let token = enroll_app(addr, "Payload App").await;
    let auth = bearer(&token);
    let h = [("Authorization", auth.as_str())];
    let call = |method: &'static str, path: &'static str, body: Option<String>| {
        let auth = auth.clone();
        async move {
            http(
                addr,
                method,
                path,
                &[("Authorization", &auth)],
                body.as_deref(),
            )
            .await
        }
    };

    assert_eq!(
        call("POST", "/v1/config/save", Some("{}".into())).await,
        (503, r#"{"message":"Keyboard is not connected"}"#.into())
    );

    // Generic tiles.
    assert_eq!(
        call(
            "POST",
            "/v1/config/addGenericTile",
            Some(r#"{"type":0,"index":0}"#.into())
        )
        .await,
        (
            400,
            r#"{"message":"Request body must contain a \"title\" key"}"#.into()
        )
    );
    assert_eq!(
        call(
            "POST",
            "/v1/config/addGenericTile",
            Some(r#"{"title":"FPS","type":0,"index":4,"length":10}"#.into())
        )
        .await,
        (400, r#"{"message":"index must be between 0 and 3"}"#.into())
    );
    assert_eq!(
        call(
            "POST",
            "/v1/config/addGenericTile",
            Some(r#"{"title":"FPS","type":0,"index":1,"length":100}"#.into())
        )
        .await,
        (200, r#"{"message":"FPS"}"#.into())
    );
    assert_eq!(
        call(
            "PUT",
            "/v1/config/updateGenericTile",
            Some(r#"{"id":"Nope","currentValue":"1"}"#.into())
        )
        .await,
        (
            404,
            r#"{"message":"Module ID does not exist. Have you added it yet?"}"#.into()
        )
    );
    let (code, body) = call(
        "PUT",
        "/v1/config/updateGenericTile",
        Some(
            r#"{"GenericGraphUpdateRequest":{"id":"FPS","currentValue":"60","unit":"fps"}}"#.into(),
        ),
    )
    .await;
    assert_eq!(
        (code, json(&body)),
        (
            200,
            serde_json::json!({"message": "Module updated successfully", "appliedToHardware": false})
        )
    );
    // A request with an invalid value changes nothing.
    assert_eq!(
        call(
            "PUT",
            "/v1/config/updateGenericTile",
            Some(r#"{"id":"FPS","unit":"ms","currentValue":true}"#.into())
        )
        .await,
        (
            400,
            r#"{"message":"currentValue must be an double"}"#.into()
        )
    );
    {
        let g = state.inner.lock().unwrap();
        let tile = g.generic_tiles[1].as_ref().unwrap();
        assert_eq!(
            (tile.unit.as_str(), tile.current_value.as_str()),
            ("fps", "60")
        );
    }

    // Icons and key styles.
    let icon = tmp.join("icon.png");
    std::fs::write(&icon, b"\x89PNG\r\n\x1a\nICON").unwrap();
    let (code, body) = call(
        "POST",
        "/v1/config/addIcon",
        Some(serde_json::json!({"icons":[{"file": icon}]}).to_string()),
    )
    .await;
    assert_eq!(code, 200);
    let icon_json = json(&body);
    assert_eq!(icon_json["message"], "1 Icons imported successfully");
    let icon_id = icon_json["iconPaths"][0].as_str().unwrap().to_string();
    assert_eq!(
        call(
            "POST",
            "/v1/config/addIcon",
            Some(r#"{"icons":"x"}"#.into())
        )
        .await,
        (400, r#"{"message":"icons must be a list"}"#.into())
    );
    let style = |profile: &str, icon: &str| {
        serde_json::json!({"styles":[{"profileName": profile, "layer": 0,
            "model": {"width": 1, "frameLayoutType": "ansi", "row": 0, "index": 0},
            "newStyle": {"style": 2, "label": "L", "iconAssetPath": icon}}]})
        .to_string()
    };
    assert_eq!(
        call(
            "PUT",
            "/v1/config/addKeystyleToAppearance",
            Some(style("Default", &icon_id))
        )
        .await,
        (200, r#"{"message":"Key style added successfully"}"#.into())
    );
    assert_eq!(
        call(
            "PUT",
            "/v1/config/addKeystyleToAppearance",
            Some(style("Nobody", &icon_id))
        )
        .await,
        (404, r#"{"message":"Profile doesn't exist"}"#.into())
    );
    assert_eq!(
        call(
            "PUT",
            "/v1/config/addKeystyleToAppearance",
            Some(style("Default", "missing.png"))
        )
        .await,
        (
            400,
            r#"{"message":"Icon doesn't exist, please call /addIcon to add the icon"}"#.into()
        )
    );

    // Detection overrides.
    assert_eq!(
        call(
            "POST",
            "/v1/config/updateProcesses",
            Some(r#"{"processes":["a","b"]}"#.into())
        )
        .await,
        (200, r#"{"message":"Updated 2 active processes"}"#.into())
    );
    assert_eq!(
        call(
            "POST",
            "/v1/config/updateActiveProcess",
            Some(r#"{"process":"code"}"#.into())
        )
        .await,
        (200, r#"{"message":"Updated active process: code"}"#.into())
    );
    let (code, body) = call(
        "POST",
        "/v1/config/updateInputLanguage",
        Some(r#"{"layout":"de"}"#.into()),
    )
    .await;
    assert_eq!(
        (code, json(&body)),
        (
            200,
            serde_json::json!({"message": "Updated input language: de", "appliedToHardware": false})
        )
    );
    assert_eq!(
        call(
            "POST",
            "/v1/config/updateInputLanguage",
            Some(r#"{"layout":"xx-unknown"}"#.into())
        )
        .await,
        (
            400,
            r#"{"message":"Unsupported XKB layout identifier"}"#.into()
        )
    );

    // Import errors.
    assert_eq!(
        call(
            "POST",
            "/v1/config/import",
            Some(r#"{"file":"/tmp/missing.txt"}"#.into())
        )
        .await,
        (
            400,
            r#"{"message":"The file must a .flux file and must exist"}"#.into()
        )
    );
    assert_eq!(
        http(addr, "GET", "/v1/authentication/check", &h, None).await,
        (200, String::new())
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn keycodes_cross_the_boundary_under_their_external_names() {
    let tmp = temp_dir("keycodes");
    let state = persistent_state(&tmp);
    let addr = serve(state.clone()).await;
    let token = enroll_app(addr, "Keycode App").await;
    let auth = bearer(&token);

    // Internally the default keymap uses KC_OF_*; keymapData reports KC_FF_*.
    let (code, body) = http(
        addr,
        "GET",
        "/v1/config/keymapData",
        &[("Authorization", &auth)],
        None,
    )
    .await;
    assert_eq!(code, 200);
    assert!(!body.contains("KC_OF_"), "{body}");
    let v = json(&body);
    let fn_layer = v["message"]["1"].as_array().unwrap();
    let entry = fn_layer
        .iter()
        .find(|e| e["action"]["keyCode"] == "KC_FF_PROFILE_CYCLE_NEXT")
        .expect("KC_FF_PROFILE_CYCLE_NEXT in the FN layer");
    assert_eq!(
        entry["location"],
        serde_json::json!({"width": 1.0, "frameLayoutType": "ansi", "row": 1, "index": 4})
    );
    assert_eq!(
        fn_layer[0],
        serde_json::json!({
            "location": {"width": 1.0, "frameLayoutType": "ansi", "row": 0, "index": 0},
            "action": {"keyCode": "KC_TRANSPARENT", "modifiers": null, "type": 0}
        })
    );

    // A .flux manifest is read with external names and stored with Keyglass names.
    let manifest = tmp.join("pack.flux");
    std::fs::write(
        &manifest,
        serde_json::json!({
            "appearance": [{"id": "pack_theme", "name": "Pack", "hint": {"keyCode": "KC_FF_THEME_CYCLE_NEXT"}}],
            "profiles": [{"id": "pack", "name": "Pack Profile", "themeId": "pack_theme"}],
            "media": [], "actuation": [], "shortcuts": []
        })
        .to_string(),
    )
    .unwrap();
    let (code, body) = http(
        addr,
        "POST",
        "/v1/config/import",
        &[("Authorization", &auth)],
        Some(&serde_json::json!({"file": manifest}).to_string()),
    )
    .await;
    assert_eq!(
        (code, json(&body)),
        (
            200,
            serde_json::json!({"message": "The file imported successfully", "profiles": ["Pack Profile"]})
        )
    );
    let stored = state.inner.lock().unwrap().appearance["customThemes"].to_string();
    assert!(
        stored.contains("KC_OF_THEME_CYCLE_NEXT") && !stored.contains("KC_FF_"),
        "{stored}"
    );

    // Every renamed keycode has exactly one external name.
    for (internal, external) in third_party::KEYCODE_NAMES {
        if let Some(function) = internal.strip_prefix("KC_OF_") {
            assert_eq!(Some(function), external.strip_prefix("KC_FF_"));
        }
        let mut v = serde_json::json!({"k": internal});
        third_party::keycodes_to_external(&mut v);
        assert_eq!(v["k"], external);
        third_party::keycodes_to_internal(&mut v);
        assert_eq!(v["k"], internal);
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn import_checks_that_the_content_matches_the_extension() {
    let tmp = temp_dir("import_kind");
    let addr = serve(persistent_state(&tmp)).await;
    let fp = bearer(FIRST_PARTY);
    let zip_as_flux = tmp.join("archive.flux");
    {
        let mut zw = zip::ZipWriter::new(std::fs::File::create(&zip_as_flux).unwrap());
        zw.start_file("settings.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut zw, b"{}").unwrap();
        zw.finish().unwrap();
    }
    let json_as_bundle = tmp.join("manifest.keyglass");
    std::fs::write(&json_as_bundle, r#"{"profiles":[]}"#).unwrap();
    let (code, body) = http(
        addr,
        "POST",
        "/v1/config/import",
        &[("Authorization", &fp)],
        Some(&serde_json::json!({"file": zip_as_flux}).to_string()),
    )
    .await;
    assert_eq!(
        (code, json(&body)),
        (
            400,
            serde_json::json!({"message": "This .flux file is an archive; Keyglass bundles use the .keyglass extension"})
        )
    );
    let (code, body) = http(
        addr,
        "POST",
        "/v1/config/import",
        &[("Authorization", &fp)],
        Some(&serde_json::json!({"file": json_as_bundle}).to_string()),
    )
    .await;
    assert_eq!(
        (code, json(&body)),
        (
            400,
            serde_json::json!({"message": "This .keyglass file is not an Keyglass bundle"})
        )
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------------------------
// WebSocket /v1/configChange
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn websocket_routes_answer_with_the_documented_messages() {
    let tmp = temp_dir("ws");
    let addr = serve(persistent_state(&tmp)).await;
    let token = enroll_app(addr, "Socket App").await;
    let mut ws = ws_connect(addr, &token).await;
    let cases = [
        (
            serde_json::json!({"route": "currentProfile"}),
            serde_json::json!({"route": "currentProfile", "status": 200, "message": "Default"}),
        ),
        (
            serde_json::json!({"route": "changeProfile", "message": "Nobody"}),
            serde_json::json!({"route": "changeProfile", "status": 404, "message": "Profile error"}),
        ),
        (
            serde_json::json!({"route": "changeProfile"}),
            serde_json::json!({"route": "changeProfile", "status": 400, "message": "Profile error"}),
        ),
        (
            serde_json::json!({"route": "currentContext"}),
            serde_json::json!({"route": "currentContext", "status": 200, "message": "Main"}),
        ),
        (
            serde_json::json!({"route": "changeContext", "message": "Nope"}),
            serde_json::json!({"route": "changeContext", "status": 404, "message": "Context does not exist in shortcut collection attached to profile"}),
        ),
        (
            serde_json::json!({"route": "changeContext"}),
            serde_json::json!({"route": "changeContext", "status": 400, "message": "Shortcut error"}),
        ),
        (
            serde_json::json!({"route": "changeContext", "message": "Main"}),
            serde_json::json!({"route": "changeContext", "status": 200, "message": "Context updated successfully"}),
        ),
        (
            serde_json::json!({"route": "somethingElse"}),
            serde_json::json!({"status": 404, "message": "Route not found"}),
        ),
        // The selection changes even without a keyboard; the reply says it couldn't be applied.
        (
            serde_json::json!({"route": "changeProfile", "message": "VS Code"}),
            serde_json::json!({"route": "changeProfile", "status": 503, "message": "Keyboard not connected"}),
        ),
        (
            serde_json::json!({"route": "currentContext"}),
            serde_json::json!({"route": "currentContext", "status": 200, "message": "Editor"}),
        ),
    ];
    for (req, expected) in cases {
        assert_eq!(ws_call(&mut ws, req.clone()).await, expected, "{req}");
    }
    ws.send(Message::Text("not json".into())).await.unwrap();
    let reply = loop {
        if let Message::Text(t) = ws.next().await.unwrap().unwrap() {
            let v = json(&t);
            if v.get("event").is_none() {
                break v;
            }
        }
    };
    assert_eq!(
        reply,
        serde_json::json!({"status": 400, "message": "Invalid JSON"})
    );

    // changeContext also pushes an event to every connection.
    let mut other = ws_connect(addr, &token).await;
    assert_eq!(
        ws_call(
            &mut ws,
            serde_json::json!({"route": "changeContext", "message": "Debug"})
        )
        .await["status"],
        200
    );
    let pushed = loop {
        if let Message::Text(t) = tokio::time::timeout(Duration::from_secs(3), other.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        {
            break json(&t);
        }
    };
    assert_eq!(
        pushed,
        serde_json::json!({"event": "shortcutContext", "data": {"shortcutContext": "Debug"}})
    );

    // Invalid tokens are refused at the handshake.
    let mut req = format!("ws://{addr}/v1/configChange")
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("Authorization", "Bearer nope".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(req).await.is_err());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn open_sockets_follow_access_changes() {
    let tmp = temp_dir("ws_access");
    let state = persistent_state(&tmp);
    let addr = serve(state.clone()).await;
    let token = enroll_app(addr, "Live App").await;
    let fp = bearer(FIRST_PARTY);

    // A permission revoked mid-connection applies to the next message.
    let mut tp = ws_connect(addr, &token).await;
    assert_eq!(
        ws_call(
            &mut tp,
            serde_json::json!({"route": "changeContext", "message": "Main"})
        )
        .await["status"],
        200
    );
    put_settings(addr, serde_json::json!({"shortcutChange": false})).await;
    assert_eq!(
        ws_call(
            &mut tp,
            serde_json::json!({"route": "changeContext", "message": "Main"})
        )
        .await,
        serde_json::json!({"route": "changeContext", "status": 403, "message": "User denied request"})
    );
    put_settings(addr, serde_json::json!({"shortcutChange": true})).await;

    // Third-party sockets don't see registration requests.
    let reg = tokio::spawn(async move {
        http(
            addr,
            "POST",
            "/v1/authentication/register",
            &[("User-Agent", "Other App")],
            Some("{}"),
        )
        .await
    });
    let other_id = wait_pending(addr, "Other App").await;
    let (reply, events) =
        ws_call_with_events(&mut tp, serde_json::json!({"route": "currentContext"})).await;
    assert_eq!(reply["status"], 200);
    assert!(
        !events.iter().any(|e| e["event"] == "registrationPending"),
        "{events:?}"
    );
    assert_eq!(decide(addr, &other_id, "deny").await.0, 200);
    let _ = reg.await;

    // Turning the API off closes third-party sockets and leaves first-party ones open.
    let mut own = ws_connect(addr, FIRST_PARTY).await;
    put_settings(addr, serde_json::json!({"apiActive": false})).await;
    assert_eq!(
        ws_closed(&mut tp, Duration::from_secs(3)).await,
        Some(third_party::WS_ACCESS_ENDED_CODE)
    );
    let before = state.inner.lock().unwrap().profiles[0]
        .active_context
        .clone();
    assert_eq!(
        ws_call(&mut own, serde_json::json!({"route": "currentContext"})).await["status"],
        200
    );
    assert_eq!(
        state.inner.lock().unwrap().profiles[0].active_context,
        before
    );
    put_settings(addr, serde_json::json!({"apiActive": true})).await;

    // Revoking and denying an app closes its sockets and invalidates its token.
    let mut tp2 = ws_connect(addr, &token).await;
    let (code, body) = http(
        addr,
        "POST",
        "/v1/third-party/apps/revoke",
        &[("Authorization", &fp)],
        Some(r#"{"app":"Live App","deny":true}"#),
    )
    .await;
    assert_eq!(
        (code, json(&body)["revoked"].clone()),
        (200, serde_json::json!(1))
    );
    assert_eq!(
        ws_closed(&mut tp2, Duration::from_secs(3)).await,
        Some(third_party::WS_ACCESS_ENDED_CODE)
    );
    assert_eq!(
        http(
            addr,
            "GET",
            "/v1/authentication/check",
            &[("Authorization", &bearer(&token))],
            None
        )
        .await
        .0,
        401
    );
    assert_eq!(
        http(
            addr,
            "POST",
            "/v1/authentication/register",
            &[("User-Agent", "Live App")],
            Some("{}")
        )
        .await
        .0,
        403
    );
    assert_eq!(
        ws_call(&mut own, serde_json::json!({"route": "currentProfile"})).await["status"],
        200
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn approved_pending_and_denied_apps_are_listed_for_keyglass_clients() {
    let tmp = temp_dir("listing");
    let addr = serve(persistent_state(&tmp)).await;
    enroll_app(addr, "Listed App").await;
    let fp = bearer(FIRST_PARTY);
    let reg = tokio::spawn(async move {
        http(
            addr,
            "POST",
            "/v1/authentication/register",
            &[("User-Agent", "Waiting App")],
            Some("{}"),
        )
        .await
    });
    let id = wait_pending(addr, "Waiting App").await;
    let (_, body) = http(
        addr,
        "GET",
        "/v1/third-party/registrations",
        &[("Authorization", &fp)],
        None,
    )
    .await;
    let v = json(&body);
    assert_eq!(
        v["approved"],
        serde_json::json!([{"app": "Listed App", "tokens": 1}])
    );
    assert_eq!(v["pending"][0]["id"], id);
    assert_eq!(v["pending"][0]["app"], "Waiting App");
    assert!(v["pending"][0]["requestedAt"].as_u64().unwrap() > 0);
    assert_eq!(v["denied"], serde_json::json!([]));
    assert_eq!(decide(addr, &id, "deny").await.0, 200);
    let _ = reg.await;
    let (_, body) = http(
        addr,
        "GET",
        "/v1/third-party/registrations",
        &[("Authorization", &fp)],
        None,
    )
    .await;
    assert_eq!(
        json(&body)["denied"],
        serde_json::json!([{"app": "Waiting App"}])
    );
    assert_eq!(
        decide(addr, &id, "approve").await.0,
        404,
        "a decided registration is no longer pending"
    );
    let (code, body) = decide(addr, "x", "maybe").await;
    assert_eq!(
        (code, json(&body)),
        (
            400,
            serde_json::json!({"message": "decision must be \"approve\" or \"deny\""})
        )
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
