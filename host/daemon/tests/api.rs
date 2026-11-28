//! Keyglass local API: HTTP, HTTPS and WebSocket behaviour of the daemon's routes, driven over real
//! sockets against in-process and spawned daemons.

use futures_util::{SinkExt, StreamExt};
use keyglassd::api::build_router;
use keyglassd::api::state::{ApiState, DeviceSink, NoopDeviceSink};
use keyglassd::api::tls::{
    ensure_tls_bundle, serve_tls_router_with_limits, write_keys_discovery_json,
};
use keyglassd::api::worker::{reconcile_tick, UsbWorkerHandle};
use keyglassd::bridges::{ActiveWindowInfo, HardwareMetrics, MediaSnapshot, OsSensorBridge};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::CertificateDer;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsConnector;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

fn isolated_temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "keyglassd_test_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

async fn spawn_test_server(
    device: Arc<dyn DeviceSink>,
    token: Option<String>,
    ttl: Duration,
    icon_dir: PathBuf,
) -> (std::net::SocketAddr, Arc<ApiState>) {
    let state = ApiState::with_dirs(device, token, ttl, icon_dir);
    let app = build_router(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (addr, state)
}

async fn http_req(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    let payload = body.unwrap_or("");
    let mut raw = format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    for (k, v) in headers {
        raw.push_str(&format!("{k}: {v}\r\n"));
    }
    if !payload.is_empty() {
        raw.push_str("Content-Type: application/json\r\n");
        raw.push_str(&format!("Content-Length: {}\r\n", payload.len()));
    }
    raw.push_str("\r\n");
    raw.push_str(payload);
    stream.write_all(raw.as_bytes()).await.unwrap();

    let mut resp = Vec::new();
    stream.read_to_end(&mut resp).await.unwrap();
    let txt = String::from_utf8_lossy(&resp).to_string();
    let (hdr, body_str) = txt.split_once("\r\n\r\n").unwrap_or((&txt, ""));
    let status: u16 = hdr
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, body_str.to_string())
}

/// Opens a test barrier when dropped, so a failing assertion never leaves a worker blocked on it.
struct OpenOnDrop(Arc<(Mutex<bool>, std::sync::Condvar)>);

impl Drop for OpenOnDrop {
    fn drop(&mut self) {
        let (m, cv) = &*self.0;
        *m.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_all();
    }
}

/// A test keyboard's copy of `path`: the appearance, settings or mosaic document of the newest of
/// `docs` (save plans or pulled snapshots, oldest first) that has it.
fn keyboard_file<'a>(
    docs: impl DoubleEndedIterator<Item = &'a serde_json::Value>,
    path: &str,
) -> Option<Vec<u8>> {
    use keyglassd::device_paths::{APPEARANCE_JSON, MOSAIC_JSON, SETTINGS_JSON};
    let key = [
        (APPEARANCE_JSON, "appearance"),
        (SETTINGS_JSON, "settings"),
        (MOSAIC_JSON, "mosaic"),
    ]
    .into_iter()
    .find(|(p, _)| *p == path)?
    .1;
    docs.rev()
        .find_map(|d| d.get(key))
        .map(|d| serde_json::to_vec(d).unwrap())
}

struct RecordingDeviceSink {
    connected: Mutex<(bool, bool)>,
    fail_writes: bool,
    block_duration: Duration,
    calls: Mutex<Vec<String>>,
    saved_snapshots: Mutex<Vec<serde_json::Value>>,
    pulled_snapshot: Mutex<Option<serde_json::Value>>,
    pomo_state: Mutex<Option<serde_json::Value>>,
}

impl RecordingDeviceSink {
    fn new(connected: (bool, bool), fail_writes: bool) -> Arc<Self> {
        Arc::new(Self {
            connected: Mutex::new(connected),
            fail_writes,
            block_duration: Duration::ZERO,
            calls: Mutex::new(Vec::new()),
            saved_snapshots: Mutex::new(Vec::new()),
            pulled_snapshot: Mutex::new(None),
            pomo_state: Mutex::new(None),
        })
    }
}

impl DeviceSink for RecordingDeviceSink {
    fn is_connected(&self) -> (bool, bool) {
        *self.connected.lock().unwrap()
    }
    fn versions(&self) -> serde_json::Value {
        serde_json::json!({"firmware": "keyglass-firmware 1.0.0", "bridge": "keyglass-bridge 1.0.0"})
    }
    fn pull_config_dir(&self) -> Result<serde_json::Value, String> {
        self.pulled_snapshot
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| "no pulled snapshot configured".to_string())
    }
    fn backup_device(&self, out_dir: &str) -> Result<String, String> {
        self.calls.lock().unwrap().push(format!("backup:{out_dir}"));
        Ok(format!("backed up to {out_dir}"))
    }
    fn flash_uf2(&self, uf2_path: &str) -> Result<String, String> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("flash_uf2:{uf2_path}"));
        Ok(format!("flashed {uf2_path}"))
    }
    fn sideload_ipk(&self, ipk_path: &str) -> Result<String, String> {
        let fname = std::path::Path::new(ipk_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("pkg.ipk");
        let dest = format!("/home/root/{fname}");
        self.calls
            .lock()
            .unwrap()
            .push(format!("sideload_ipk:{dest}"));
        Ok(format!("Uploaded {fname} to {dest}"))
    }
    fn send_live_json(&self, payload: &serde_json::Value) -> Result<(), String> {
        if !self.block_duration.is_zero() {
            std::thread::sleep(self.block_duration);
        }
        if self.fail_writes {
            return Err("USB NAK".into());
        }
        self.calls.lock().unwrap().push(format!("live:{payload}"));
        Ok(())
    }
    fn set_active_profile(&self, idx: u32) -> Result<(), String> {
        if !self.block_duration.is_zero() {
            std::thread::sleep(self.block_duration);
        }
        if self.fail_writes {
            return Err("firmware NAK".into());
        }
        self.calls.lock().unwrap().push(format!("profile:{idx}"));
        Ok(())
    }
    fn set_language(&self, code: u32) -> Result<(), String> {
        if !self.block_duration.is_zero() {
            std::thread::sleep(self.block_duration);
        }
        if self.fail_writes {
            return Err("firmware NAK".into());
        }
        self.calls.lock().unwrap().push(format!("lang:{code}"));
        Ok(())
    }
    fn set_platform(&self, code: u32) -> Result<(), String> {
        if self.fail_writes {
            return Err("firmware NAK".into());
        }
        self.calls.lock().unwrap().push(format!("platform:{code}"));
        Ok(())
    }
    fn save_config_dir(&self, save_plan: &serde_json::Value) -> Result<(), String> {
        if !self.block_duration.is_zero() {
            std::thread::sleep(self.block_duration);
        }
        if self.fail_writes {
            return Err("bridge write failed".into());
        }
        self.calls.lock().unwrap().push("save".into());
        self.saved_snapshots.lock().unwrap().push(save_plan.clone());
        Ok(())
    }
    fn read_device_pomodoro_state(&self) -> Option<serde_json::Value> {
        self.pomo_state.lock().unwrap().clone()
    }
    fn get_file(&self, path: &str) -> Result<Vec<u8>, String> {
        let pulled = self.pulled_snapshot.lock().unwrap().clone();
        let saved = self.saved_snapshots.lock().unwrap();
        keyboard_file(pulled.iter().chain(saved.iter()), path)
            .ok_or_else(|| format!("{path} not found"))
    }
    fn stat_file(&self, path: &str) -> Result<bool, String> {
        Ok(self.get_file(path).is_ok())
    }
}

struct StubBridge {
    lang: u32,
}
impl OsSensorBridge for StubBridge {
    fn sample_hardware(&mut self) -> HardwareMetrics {
        HardwareMetrics {
            cpu_pct: 25.0,
            mem_used_gb: 8.0,
            mem_total_gb: 32.0,
            gpu_pct: 10.0,
            vram_used_gb: Some(7.4),
            vram_total_gb: Some(10.0),
            gpu_unified_mem: false,
            net_down: "1 MB/s".into(),
            net_up: "0 MB/s".into(),
        }
    }
    fn active_window(&mut self) -> Option<ActiveWindowInfo> {
        None
    }
    fn active_language_code(&mut self) -> Option<u32> {
        Some(self.lang)
    }
    fn media_snapshot(&mut self) -> MediaSnapshot {
        MediaSnapshot::default()
    }
}

#[tokio::test]
async fn enrollment_secret_issues_unique_first_party_tokens_and_untrusted_origins_are_refused() {
    let tmp1 = isolated_temp_dir("reg1");
    let tmp2 = isolated_temp_dir("reg2");
    let (addr1, st1) = spawn_test_server(
        Arc::new(NoopDeviceSink),
        None,
        Duration::from_secs(20),
        tmp1.clone(),
    )
    .await;
    let (addr2, st2) = spawn_test_server(
        Arc::new(NoopDeviceSink),
        None,
        Duration::from_secs(20),
        tmp2.clone(),
    )
    .await;
    st1.set_registration_secret("secret_1");
    st2.set_registration_secret("secret_2");

    let (code, body) = http_req(
        addr1,
        "POST",
        "/v1/authentication/register",
        &[],
        Some(r#"{"enrollmentSecret":"wrong"}"#),
    )
    .await;
    assert_eq!(code, 403);
    assert!(body.contains("User denied the request"), "{body}");

    for bad_orig in [
        "https://evil.example",
        "https://localhost.attacker.example",
        "http://127.0.0.1.attacker.example",
        "https://tauri.localhost.attacker.example",
        "http://user@localhost:1420",
        "null",
    ] {
        let (code_orig, _) = http_req(
            addr1,
            "POST",
            "/v1/authentication/register",
            &[("Origin", bad_orig)],
            Some(r#"{"enrollmentSecret":"secret_1"}"#),
        )
        .await;
        assert_eq!(code_orig, 403, "Expected 403 for Origin={bad_orig}");
    }

    let (c1, b1) = http_req(
        addr1,
        "POST",
        "/v1/authentication/register",
        &[("Origin", "http://localhost:1420")],
        Some(r#"{"enrollmentSecret":"secret_1"}"#),
    )
    .await;
    let (c2, b2) = http_req(
        addr2,
        "POST",
        "/v1/authentication/register",
        &[],
        Some(r#"{"enrollmentSecret":"secret_2"}"#),
    )
    .await;
    assert_eq!(c1, 200);
    assert_eq!(c2, 200);
    let t1 = serde_json::from_str::<serde_json::Value>(&b1).unwrap()["message"]
        .as_str()
        .unwrap()
        .to_string();
    let t2 = serde_json::from_str::<serde_json::Value>(&b2).unwrap()["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(t1, t2);
    assert!(t1.starts_with("keyglass_") && t1.len() == 73, "{t1}");
    let auth = format!("Bearer {t1}");
    assert_eq!(
        http_req(
            addr1,
            "GET",
            "/v1/device/status",
            &[("Authorization", &auth)],
            None
        )
        .await
        .0,
        200
    );

    let _ = std::fs::remove_dir_all(&tmp1);
    let _ = std::fs::remove_dir_all(&tmp2);
}

#[tokio::test]
async fn tls_accepts_a_client_using_the_discovered_certificate_and_reaps_stalled_handshakes() {
    let tmp = isolated_temp_dir("tls");
    let tls_dir = tmp.join("tls");
    let bundle = ensure_tls_bundle(&tls_dir).unwrap();

    // The private key and its directory are owner-only on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let kmode = std::fs::metadata(&bundle.key_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let dmode = std::fs::metadata(&tls_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(kmode, 0o600, "key.pem mode was 0o{kmode:o}");
        assert_eq!(dmode, 0o700, "tls dir mode was 0o{dmode:o}");
    }

    // A leftover permissive temporary file is never reused when key.pem is regenerated.
    #[cfg(unix)]
    let bundle = {
        use std::os::unix::fs::PermissionsExt;
        let stale_tmp = tls_dir.join(format!(".tmp_key.pem_{}", std::process::id()));
        std::fs::write(&stale_tmp, b"stale").unwrap();
        std::fs::set_permissions(&stale_tmp, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _ = std::fs::remove_file(&bundle.key_path);
        let _ = std::fs::remove_file(&bundle.cert_path);
        let b2 = ensure_tls_bundle(&tls_dir).unwrap();
        let kmode2 = std::fs::metadata(&b2.key_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(kmode2, 0o600);
        let _ = std::fs::remove_file(&stale_tmp);
        b2
    };

    let state = ApiState::with_dirs(
        Arc::new(NoopDeviceSink),
        Some("tls_tok".into()),
        Duration::from_secs(20),
        tmp.join("icons"),
    );
    // An ephemeral port, as when 52323 is taken.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bound = listener.local_addr().unwrap();
    let keys_path = write_keys_discovery_json(
        &tmp.join("config"),
        bound,
        &bundle.cert_path,
        &bundle.cert_sha256,
    )
    .unwrap();

    // Keyglass's discovery file: `openPort`, `host`, `publicKeyLocation` (the certificate file), `certHash`.
    let keys_doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&keys_path).unwrap()).unwrap();
    assert_eq!(keys_doc["openPort"], bound.port());
    assert_eq!(keys_doc["certHash"], bundle.cert_sha256);
    let cert_file = PathBuf::from(keys_doc["publicKeyLocation"].as_str().unwrap());
    assert!(cert_file.is_file());

    let running = Arc::new(AtomicBool::new(true));
    let r_srv = running.clone();
    let app = build_router(state);
    let acc = bundle.acceptor;
    tokio::spawn(async move {
        // At most 4 concurrent connections, 150 ms handshake timeout.
        serve_tls_router_with_limits(listener, acc, app, r_srv, 4, Duration::from_millis(150))
            .await;
    });

    // Twelve sockets that never start a TLS handshake are reaped by the timeout, so a real client
    // still gets through.
    let mut stalled = Vec::new();
    for _ in 0..12 {
        if let Ok(s) = TcpStream::connect(bound).await {
            stalled.push(s);
        }
    }
    tokio::time::sleep(Duration::from_millis(220)).await;

    let discovered_port = keys_doc["openPort"].as_u64().unwrap() as u16;
    let discovered_addr: std::net::SocketAddr =
        format!("127.0.0.1:{discovered_port}").parse().unwrap();

    // A client trusting only the certificate named by keys.json.
    let mut root_store = rustls::RootCertStore::empty();
    let cert_bytes = std::fs::read(&cert_file).unwrap();
    for cert in CertificateDer::pem_slice_iter(&cert_bytes) {
        root_store.add(cert.unwrap()).unwrap();
    }
    let tls_cfg = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(tls_cfg));
    let tcp = TcpStream::connect(discovered_addr).await.unwrap();
    let server_name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let mut tls_stream = connector.connect(server_name, tcp).await.unwrap();
    tls_stream
        .write_all(b"GET /v1/docs HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut resp = Vec::new();
    tls_stream.read_to_end(&mut resp).await.unwrap();
    let txt = String::from_utf8_lossy(&resp);
    assert!(
        txt.contains("200 OK") && txt.contains(keyglassd::api::third_party::DOCS_URL),
        "{txt}"
    );

    drop(stalled);
    running.store(false, Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn the_asset_catalog_lists_every_asset_directory_and_serves_preset_thumbnails() {
    let tmp = isolated_temp_dir("catalog");
    let icon_store = tmp.join("key_icons");
    std::fs::create_dir_all(tmp.join("media")).unwrap();
    std::fs::write(tmp.join("media/clip.mp4"), b"MP4").unwrap();
    std::fs::create_dir_all(tmp.join("default_backgrounds/thumbnails")).unwrap();
    std::fs::write(
        tmp.join("default_backgrounds/thumbnails/aurora_velvet_thumb.jpg"),
        b"\xff\xd8\xffTHUMB",
    )
    .unwrap();
    std::fs::create_dir_all(tmp.join("fonts")).unwrap();
    std::fs::write(tmp.join("fonts/Plain.TTF"), b"FONT").unwrap();
    let (addr, _st) = spawn_test_server(
        Arc::new(NoopDeviceSink),
        Some("tok_cat".into()),
        Duration::from_secs(20),
        icon_store,
    )
    .await;
    let auth = [("Authorization", "Bearer tok_cat")];

    let (code, body) = http_req(addr, "GET", "/v1/assets", &auth, None).await;
    assert_eq!(code, 200, "{body}");
    let cat: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(
        cat["media"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["name"] == "clip.mp4" && m["category"] == "media"),
        "{cat}"
    );
    assert!(
        cat["fonts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["family"] == "Plain"),
        "the extension isn't part of the family, whatever its case: {cat}"
    );
    let thumb = cat["presets"][0]["thumbnail"].as_str().unwrap().to_string();
    assert_eq!(thumb, "aurora_velvet_thumb.jpg");
    let (code, _) = http_req(
        addr,
        "GET",
        &format!("/v1/asset-file?cat=presets&name={thumb}"),
        &auth,
        None,
    )
    .await;
    assert_eq!(code, 200);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn rest_ws_keystyles_icons_imports_and_saves_round_trip() {
    let tmp = isolated_temp_dir("full");
    let icon_store = tmp.join("icon_store");
    let sink = RecordingDeviceSink::new((true, true), false);
    let (addr, st) = spawn_test_server(
        sink.clone(),
        Some("tok_init".into()),
        Duration::from_millis(60),
        icon_store.clone(),
    )
    .await;
    st.set_registration_secret("pair_123");

    let (c_reg, b_reg) = http_req(
        addr,
        "POST",
        "/v1/authentication/register",
        &[],
        Some(r#"{"enrollmentSecret":"pair_123"}"#),
    )
    .await;
    assert_eq!(c_reg, 200);
    let tok = serde_json::from_str::<serde_json::Value>(&b_reg).unwrap()["message"]
        .as_str()
        .unwrap()
        .to_string();
    let auth_hdr = format!("Bearer {tok}");

    // 0. Clean no-op save before any edit must not overwrite any device file (appearance/settings/mosaic/keymap omitted)
    assert_eq!(
        http_req(
            addr,
            "POST",
            "/v1/config/save",
            &[("Authorization", &auth_hdr)],
            Some("{}")
        )
        .await
        .0,
        200
    );
    {
        let saved = sink.saved_snapshots.lock().unwrap();
        assert!(saved[0].get("appearance").is_none());
        assert!(saved[0].get("settings").is_none());
        assert!(saved[0].get("mosaic").is_none());
        assert!(saved[0].get("keymap").is_none());
        assert!(saved[0].get("keymapBlobHex").is_none());
    }

    // Two independent generic tiles (slot 0 + slot 1) emit distinct slot index + type
    assert_eq!(
        http_req(
            addr,
            "POST",
            "/v1/config/addGenericTile",
            &[("Authorization", &auth_hdr)],
            Some(r#"{"title":"CPU","type":0,"index":0,"length":60}"#)
        )
        .await
        .0,
        200
    );
    assert_eq!(
        http_req(
            addr,
            "POST",
            "/v1/config/addGenericTile",
            &[("Authorization", &auth_hdr)],
            Some(r#"{"title":"FPS","type":1,"index":1,"length":60}"#)
        )
        .await
        .0,
        200
    );
    assert_eq!(
        http_req(
            addr,
            "PUT",
            "/v1/config/updateGenericTile",
            &[("Authorization", &auth_hdr)],
            Some(r#"{"id":"FPS","subTitle":"GPU","unit":"fps","currentValue":"60"}"#)
        )
        .await
        .0,
        200
    );
    let calls_str = sink.calls.lock().unwrap().join("\n");
    assert!(
        calls_str.contains(r#""index":1"#) && calls_str.contains(r#""type":1"#),
        "{calls_str}"
    );

    // Collision-safe content-addressed addIcon: same basename /a/icon.png and /b/icon.png with different bytes must not overwrite.
    let dir_a = tmp.join("a");
    let dir_b = tmp.join("b");
    std::fs::create_dir_all(&dir_a).unwrap();
    std::fs::create_dir_all(&dir_b).unwrap();
    let ic_a = dir_a.join("icon.png");
    let ic_b = dir_b.join("icon.png");
    std::fs::write(&ic_a, b"\x89PNG\r\n\x1a\nAAAA").unwrap();
    std::fs::write(&ic_b, b"\x89PNG\r\n\x1a\nBBBB").unwrap();

    let (ca, ba) = http_req(
        addr,
        "POST",
        "/v1/config/addIcon",
        &[("Authorization", &auth_hdr)],
        Some(&serde_json::json!({"icons":[{"file": ic_a.to_string_lossy()}]}).to_string()),
    )
    .await;
    let (cb, bb) = http_req(
        addr,
        "POST",
        "/v1/config/addIcon",
        &[("Authorization", &auth_hdr)],
        Some(&serde_json::json!({"icons":[{"file": ic_b.to_string_lossy()}]}).to_string()),
    )
    .await;
    assert_eq!(ca, 200);
    assert_eq!(cb, 200);
    let id_a = serde_json::from_str::<serde_json::Value>(&ba).unwrap()["iconPaths"][0]
        .as_str()
        .unwrap()
        .to_string();
    let id_b = serde_json::from_str::<serde_json::Value>(&bb).unwrap()["iconPaths"][0]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(id_a, id_b);
    assert_eq!(
        std::fs::read(icon_store.join(&id_a)).unwrap(),
        b"\x89PNG\r\n\x1a\nAAAA"
    );
    assert_eq!(
        std::fs::read(icon_store.join(&id_b)).unwrap(),
        b"\x89PNG\r\n\x1a\nBBBB"
    );

    // Canonical KeyModel row/index mapping + per-profile theme isolation + unknown profile 404
    let (c_unk, _) = http_req(addr, "PUT", "/v1/config/addKeystyleToAppearance", &[("Authorization", &auth_hdr)],
        Some(r#"{"styles":[{"profileName":"Nonexistent","layer":0,"model":{"width":1,"frameLayoutType":"ansi","row":0,"index":0},"newStyle":{"style":1,"label":"X"}}]}"#)).await;
    assert_eq!(c_unk, 404);

    // 0b. A patch to a base that was never hydrated (e.g. addKeystyleToAppearance before the real
    // appearance is loaded) is refused on save (422) rather than overwriting the device themes with defaults.
    assert_eq!(
        http_req(addr, "PUT", "/v1/config/addKeystyleToAppearance", &[("Authorization", &auth_hdr)],
            Some(&serde_json::json!({
                "styles":[{"profileName":"Default","layer":0,"model":{"width":1,"frameLayoutType":"ansi","row":0,"index":0},"newStyle":{"style":2,"label":"R0","iconAssetPath": id_a}}]
            }).to_string())).await.0,
        200
    );
    assert_eq!(
        http_req(
            addr,
            "POST",
            "/v1/config/save",
            &[("Authorization", &auth_hdr)],
            Some("{}")
        )
        .await
        .0,
        422
    );

    // Normal PUT without explicitReset must not self-certify unhydrated defaults as hydrated;
    // saving after an ordinary unhydrated GET->PUT still fails closed (422).
    assert_eq!(
        http_req(addr, "PUT", "/v1/config", &[("Authorization", &auth_hdr)],
            Some(r#"{"appearance":{"selectedId":"default","defaultTheme":{"id":"default","name":"Unhydrated"}}}"#)).await.0,
        200
    );
    assert_eq!(
        http_req(
            addr,
            "POST",
            "/v1/config/save",
            &[("Authorization", &auth_hdr)],
            Some("{}")
        )
        .await
        .0,
        422
    );

    // Explicit reset hydrates themes and mosaic while preserving unrelated entries.
    assert_eq!(
        http_req(addr, "PUT", "/v1/config", &[("Authorization", &auth_hdr)],
            Some(&serde_json::json!({
                "explicitReset": true,
                "appearance": {
                    "selectedId": "default",
                    "defaultTheme": { "id": "default", "name": "User Existing Theme", "stylemaps": { "0": { "styles": { "5": { "style": 1, "label": "KEEP_K5" } } } } },
                    "customThemes": [{ "id": "theme_coding", "name": "VS Code", "stylemaps": {} }, { "id": "keep_other", "name": "Unrelated Custom", "stylemaps": {} }]
                },
                "mosaic": {
                    "tileTypes": [12, 8, 15, 13],
                    "tileConfigs": [{}, {}, {"city":"Kyoto"}, {"main":"25"}]
                }
            }).to_string())).await.0,
        200
    );

    // Re-add generic tiles at slots 0 & 1 onto the hydrated mosaic (slots 2 & 3 must stay 15 & 13)
    assert_eq!(
        http_req(
            addr,
            "POST",
            "/v1/config/addGenericTile",
            &[("Authorization", &auth_hdr)],
            Some(r#"{"title":"CPU","type":0,"index":0,"length":60}"#)
        )
        .await
        .0,
        200
    );
    assert_eq!(
        http_req(
            addr,
            "POST",
            "/v1/config/addGenericTile",
            &[("Authorization", &auth_hdr)],
            Some(r#"{"title":"FPS","type":1,"index":1,"length":60}"#)
        )
        .await
        .0,
        200
    );

    // Edit row=0/index=0 (key 0), row=0/index=15 (key 15), row=1/index=0 (key 16) on "Default",
    // ISO row=3/index=14 (key 60), and row=0/index=0 on "VS Code"
    let style_req = serde_json::json!({
        "styles": [
            {"profileName":"Default","layer":0,"model":{"width":1,"frameLayoutType":"ansi","row":0,"index":0},"newStyle":{"style":2,"label":"R0","iconAssetPath": id_a}},
            {"profileName":"Default","layer":0,"model":{"width":1,"frameLayoutType":"ansi","row":0,"index":15},"newStyle":{"style":2,"label":"R0_15"}},
            {"profileName":"Default","layer":0,"model":{"width":1,"frameLayoutType":"ansi","row":1,"index":0},"newStyle":{"style":2,"label":"R1","iconAssetPath": id_b}},
            {"profileName":"VS Code","layer":0,"model":{"width":1,"frameLayoutType":"ansi","row":0,"index":0},"newStyle":{"style":2,"label":"CODE"}}
        ]
    });
    assert_eq!(
        http_req(
            addr,
            "PUT",
            "/v1/config/addKeystyleToAppearance",
            &[("Authorization", &auth_hdr)],
            Some(&style_req.to_string())
        )
        .await
        .0,
        200
    );
    assert_eq!(
        http_req(addr, "PUT", "/v1/config/addKeystyleToAppearance", &[("Authorization", &auth_hdr)],
            Some(r#"{"styles":[{"profileName":"Default","layer":0,"model":{"width":1,"frameLayoutType":"ansi","row":0,"index":16},"newStyle":{"style":2,"label":"BAD"}}]}"#)).await.0,
        400
    );
    assert_eq!(
        keyglassd::api::third_party::canonical_key_index("iso", 3, 14).unwrap(),
        60
    );

    // Verify Default theme preserved unrelated key "5" ("KEEP_K5") and unrelated customThemes[1] ("Unrelated Custom").
    let (_, b_cfg1) = http_req(
        addr,
        "GET",
        "/v1/config",
        &[("Authorization", &auth_hdr)],
        None,
    )
    .await;
    let v1: serde_json::Value = serde_json::from_str(&b_cfg1).unwrap();
    let def_styles = &v1["appearance"]["defaultTheme"]["stylemaps"]["0"]["styles"];
    assert_eq!(def_styles["0"]["label"], "R0");
    assert_eq!(def_styles["5"]["label"], "KEEP_K5");
    assert_eq!(def_styles["15"]["label"], "R0_15");
    assert_eq!(def_styles["16"]["label"], "R1");
    assert_eq!(
        v1["appearance"]["customThemes"][0]["stylemaps"]["0"]["styles"]["0"]["label"],
        "CODE"
    );
    assert_eq!(
        v1["appearance"]["customThemes"][1]["name"],
        "Unrelated Custom"
    );
    // Mosaic preserved slots 2 and 3 (15 and 13) when only slots 0 and 1 were replaced.
    assert_eq!(
        v1["mosaic"]["tileTypes"],
        serde_json::json!([18, 18, 15, 13])
    );
    assert_eq!(v1["mosaic"]["tileConfigs"][2]["city"], "Kyoto");

    // 1. Hydrated appearance+mosaic save (no keymap edits) saves appearance + mosaic + icons, omitting unedited settings/keymap.
    assert_eq!(
        http_req(
            addr,
            "POST",
            "/v1/config/save",
            &[("Authorization", &auth_hdr)],
            Some("{}")
        )
        .await
        .0,
        200
    );
    {
        let saved = sink.saved_snapshots.lock().unwrap();
        assert_eq!(
            saved[1]["appearance"]["defaultTheme"]["name"],
            "User Existing Theme"
        );
        assert_eq!(
            saved[1]["mosaic"]["tileTypes"],
            serde_json::json!([18, 18, 15, 13])
        );
        assert!(
            saved[1].get("settings").is_none(),
            "Save must omit unedited settings"
        );
        assert!(
            saved[1].get("keymap").is_none(),
            "Save must omit unchanged keymap"
        );
        assert!(
            saved[1].get("keymapBlobHex").is_none(),
            "Save must omit keymapBlobHex"
        );
        assert_eq!(saved[1]["icons"].as_array().unwrap().len(), 2);
    }

    // 2. Malformed firmwareKeymapBlobHex ("0", "€") and unpaired firmwareKeymapBlobHex must return 400 cleanly without poisoning mutex
    for bad_hex in ["0", "€", "zz"] {
        let (c_bh, _) = http_req(
            addr, "PUT", "/v1/config", &[("Authorization", &auth_hdr)],
            Some(&serde_json::json!({"appearance": {"selectedId":"bad"}, "firmwareKeymapBlobHex": bad_hex}).to_string()),
        ).await;
        assert_eq!(c_bh, 400);
    }
    let default_km: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/default_keymap.json")).unwrap();
    let default_blob = include_bytes!("fixtures/default_keymap.bin");
    let decoded_default = keyglassd::codecs::keymap::decode_keymap_blob(default_blob).unwrap();
    assert_eq!(decoded_default.num_keymaps, 2);
    assert_eq!(decoded_default.keymaps[0].layers[0][3], 0x3C); // KC_F3
    let expected_hex: String = default_blob.iter().map(|b| format!("{b:02x}")).collect();

    // Unpaired firmwareKeymapBlobHex (without keymap) must fail 400
    assert_eq!(
        http_req(
            addr,
            "PUT",
            "/v1/config",
            &[("Authorization", &auth_hdr)],
            Some(&serde_json::json!({"firmwareKeymapBlobHex": expected_hex}).to_string())
        )
        .await
        .0,
        400
    );

    // Paired keymap + firmwareKeymapBlobHex succeeds and saves the blob exactly as given
    assert_eq!(
        http_req(
            addr,
            "PUT",
            "/v1/config",
            &[("Authorization", &auth_hdr)],
            Some(
                &serde_json::json!({"keymap": default_km, "firmwareKeymapBlobHex": expected_hex})
                    .to_string()
            )
        )
        .await
        .0,
        200
    );
    let (ckm, bkm) = http_req(
        addr,
        "GET",
        "/v1/config/keymapData",
        &[("Authorization", &auth_hdr)],
        None,
    )
    .await;
    assert_eq!(ckm, 200);
    let km_proj: serde_json::Value = serde_json::from_str(&bkm).unwrap();
    assert_eq!(km_proj["message"]["0"].as_array().unwrap().len(), 85);
    assert_eq!(km_proj["message"]["0"][16]["location"]["row"], 1);
    assert_eq!(km_proj["message"]["0"][16]["location"]["index"], 0);
    assert_eq!(km_proj["message"]["0"][16]["action"]["keyCode"], "KC_GRAVE");

    assert_eq!(
        http_req(
            addr,
            "POST",
            "/v1/config/save",
            &[("Authorization", &auth_hdr)],
            Some("{}")
        )
        .await
        .0,
        200
    );
    {
        let saved = sink.saved_snapshots.lock().unwrap();
        assert_eq!(saved[2]["keymap"]["selectedFrameLayoutType"], "ansi");
        assert_eq!(saved[2]["keymapBlobHex"].as_str().unwrap(), expected_hex);
    }

    // A later keymap PUT with an unsupported keycode is rejected before commit (422) and leaves the valid state intact.
    let mut bad_km = default_km.clone();
    bad_km["keymaps"]["ansi"]["layers"][0]["keyActions"][0]["keyCode"] =
        serde_json::json!("KC_NONEXISTENT_XYZ");
    assert_eq!(
        http_req(
            addr,
            "PUT",
            "/v1/config",
            &[("Authorization", &auth_hdr)],
            Some(&serde_json::json!({"keymap": bad_km}).to_string())
        )
        .await
        .0,
        422
    );
    assert_eq!(
        http_req(
            addr,
            "POST",
            "/v1/config/save",
            &[("Authorization", &auth_hdr)],
            Some("{}")
        )
        .await
        .0,
        200
    );

    // Keymap JSON edit with valid QMK keycodes automatically re-encodes validated FirmwareKeymapBlob -> save succeeds 200.
    assert_eq!(
        http_req(
            addr,
            "PUT",
            "/v1/config",
            &[("Authorization", &auth_hdr)],
            Some(&serde_json::json!({"keymap": default_km}).to_string())
        )
        .await
        .0,
        200
    );
    assert_eq!(
        http_req(
            addr,
            "POST",
            "/v1/config/save",
            &[("Authorization", &auth_hdr)],
            Some("{}")
        )
        .await
        .0,
        200
    );

    // Real HTTP PUT round-trip for actuationModes + appRules must persist into GET /v1/config
    // and an actuation-only PUT -> Save must emit updated FirmwareKeymapBlob with rapid-trigger 0.8mm/0.2mm.
    assert_eq!(
        http_req(addr, "PUT", "/v1/config", &[("Authorization", &auth_hdr)],
            Some(&serde_json::json!({
                "actuationModes": [{"id":"rt1","name":"Rapid","type":2,"actuationPointMm":0.8,"rapidTrigger":true,"rapidTriggerSensitivityMm":0.2}],
                "appRules": [{"matcher":"com.apple.Terminal","profile_index":1,"language_code":null}]
            }).to_string())).await.0,
        200
    );
    let (_, b_rt) = http_req(
        addr,
        "GET",
        "/v1/config",
        &[("Authorization", &auth_hdr)],
        None,
    )
    .await;
    let v_rt: serde_json::Value = serde_json::from_str(&b_rt).unwrap();
    assert_eq!(v_rt["actuationModes"][0]["actuationPointMm"], 0.8);
    assert_eq!(v_rt["appRules"][0]["matcher"], "com.apple.Terminal");
    assert_eq!(
        http_req(
            addr,
            "POST",
            "/v1/config/save",
            &[("Authorization", &auth_hdr)],
            Some("{}")
        )
        .await
        .0,
        200
    );
    {
        let saved = sink.saved_snapshots.lock().unwrap();
        let last_hex = saved.last().unwrap()["keymapBlobHex"].as_str().unwrap();
        let raw_b: Vec<u8> = (0..last_hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&last_hex[i..i + 2], 16).unwrap())
            .collect();
        let dec_b = keyglassd::codecs::keymap::decode_keymap_blob(&raw_b).unwrap();
        let expected_rt = keyglassd::codecs::keymap::encode_actuation_word(2, 800, 200).unwrap();
        assert_eq!(dec_b.keymaps[0].actuation_tables[0][0], expected_rt);
    }

    // .flux import reads the .flux schema (appearance/profiles lists) and returns only the imported profile names.
    let manifest_file = tmp.join("gaming.flux");
    std::fs::write(
        &manifest_file,
        serde_json::json!({
            "appearance": [{"id": "theme_gaming", "name": "Gaming"}],
            "profiles": [{"id": "gaming", "name": "Gaming Pack", "themeId": "theme_gaming"}],
            "media": [],
            "actuation": [],
            "shortcuts": []
        })
        .to_string(),
    )
    .unwrap();
    // Unhydrated .flux theme import marks appearance_dirty without hydrating -> save refuses 422.
    let (addr_unh, _st_unh) = spawn_test_server(
        RecordingDeviceSink::new((true, true), false),
        Some("tok_unh".into()),
        Duration::from_secs(20),
        tmp.join("unh_icons"),
    )
    .await;
    assert_eq!(
        http_req(
            addr_unh,
            "POST",
            "/v1/config/import",
            &[("Authorization", "Bearer tok_unh")],
            Some(&serde_json::json!({"file": manifest_file.to_string_lossy()}).to_string())
        )
        .await
        .0,
        200
    );
    assert_eq!(
        http_req(
            addr_unh,
            "POST",
            "/v1/config/save",
            &[("Authorization", "Bearer tok_unh")],
            Some("{}")
        )
        .await
        .0,
        422
    );

    let (c_imp, b_imp) = http_req(
        addr,
        "POST",
        "/v1/config/import",
        &[("Authorization", &auth_hdr)],
        Some(&serde_json::json!({"file": manifest_file.to_string_lossy()}).to_string()),
    )
    .await;
    assert_eq!(c_imp, 200);
    let imp_v: serde_json::Value = serde_json::from_str(&b_imp).unwrap();
    assert_eq!(imp_v["profiles"], serde_json::json!(["Gaming Pack"]));

    // Imported theme from .flux must be persisted by /v1/config/save (appearance_dirty == true).
    assert_eq!(
        http_req(
            addr,
            "POST",
            "/v1/config/save",
            &[("Authorization", &auth_hdr)],
            Some("{}")
        )
        .await
        .0,
        200
    );
    {
        let saved = sink.saved_snapshots.lock().unwrap();
        let last_saved = saved.last().unwrap();
        assert!(last_saved.get("appearance").is_some());
        let custom_str = last_saved["appearance"]["customThemes"].to_string();
        assert!(custom_str.contains("theme_gaming"), "{custom_str}");
    }

    // WebSocket /v1/configChange route checks
    for bad_orig in ["https://evil.example", "https://localhost.attacker.example"] {
        let mut bad_ws_req = format!("ws://{addr}/v1/configChange")
            .into_client_request()
            .unwrap();
        bad_ws_req
            .headers_mut()
            .insert("Authorization", auth_hdr.parse().unwrap());
        bad_ws_req
            .headers_mut()
            .insert("Origin", bad_orig.parse().unwrap());
        assert!(tokio_tungstenite::connect_async(bad_ws_req).await.is_err());
    }
    let mut ws_req = format!("ws://{addr}/v1/configChange")
        .into_client_request()
        .unwrap();
    ws_req
        .headers_mut()
        .insert("Authorization", auth_hdr.parse().unwrap());
    let (mut ws, _) = tokio_tungstenite::connect_async(ws_req).await.unwrap();
    ws.send(Message::Text(
        r#"{"route":"changeProfile","message":"VS Code"}"#.into(),
    ))
    .await
    .unwrap();
    let m1 = ws.next().await.unwrap().unwrap().into_text().unwrap();
    let m2 = ws.next().await.unwrap().unwrap().into_text().unwrap();
    assert!(format!("{m1}\n{m2}").contains("VS Code"));

    // WebSocket session semaphore caps concurrent upgraded sessions (16 max -> 17th rejected with 503)
    let mut extra_ws = Vec::new();
    for _ in 0..15 {
        let mut r = format!("ws://{addr}/v1/configChange")
            .into_client_request()
            .unwrap();
        r.headers_mut()
            .insert("Authorization", auth_hdr.parse().unwrap());
        extra_ws.push(tokio_tungstenite::connect_async(r).await.unwrap().0);
    }
    let mut r_over = format!("ws://{addr}/v1/configChange")
        .into_client_request()
        .unwrap();
    r_over
        .headers_mut()
        .insert("Authorization", auth_hdr.parse().unwrap());
    assert!(tokio_tungstenite::connect_async(r_over).await.is_err());
    drop(extra_ws);
    drop(ws);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn blocked_usb_worker_keeps_status_auth_and_shutdown_responsive() {
    let tmp = isolated_temp_dir("blocked_usb");
    let sink = Arc::new(RecordingDeviceSink {
        connected: Mutex::new((true, true)),
        fail_writes: false,
        block_duration: Duration::from_millis(800),
        calls: Mutex::new(Vec::new()),
        saved_snapshots: Mutex::new(Vec::new()),
        pulled_snapshot: Mutex::new(None),
        pomo_state: Mutex::new(None),
    });
    let state = ApiState::with_dirs(
        sink.clone(),
        Some("tok_blk".into()),
        Duration::from_secs(20),
        tmp.join("icons"),
    );
    let running = Arc::new(AtomicBool::new(true));
    let worker = UsbWorkerHandle::spawn(
        state.clone(),
        sink,
        Box::new(StubBridge { lang: 471 }),
        2,
        running.clone(),
    );
    state.set_worker(worker);

    let app = build_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Fire a blocking /v1/config/save in the background (blocks USB worker for 800ms)
    let save_task = tokio::spawn(async move {
        http_req(
            addr,
            "POST",
            "/v1/config/save",
            &[("Authorization", "Bearer tok_blk")],
            Some("{}"),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(30)).await;

    // Auth checks and cached device status must finish within 200 ms while USB is blocked.
    let t0 = Instant::now();
    let (c_chk, _) = http_req(
        addr,
        "GET",
        "/v1/authentication/check",
        &[("Authorization", "Bearer tok_blk")],
        None,
    )
    .await;
    let (c_st, _) = http_req(
        addr,
        "GET",
        "/v1/device/status",
        &[("Authorization", "Bearer tok_blk")],
        None,
    )
    .await;
    assert_eq!(c_chk, 200);
    assert_eq!(c_st, 200);
    assert!(
        t0.elapsed() < Duration::from_millis(200),
        "Reactor blocked: {:?}",
        t0.elapsed()
    );

    let (c_sv, _) = save_task.await.unwrap();
    assert_eq!(c_sv, 200);
    running.store(false, Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn reconcile_loop_override_expiry_restores_os_layout_and_resyncs_reconnect() {
    let tmp = isolated_temp_dir("reconcile");
    let sink = RecordingDeviceSink::new((false, true), false);
    let ps_replug = keyglassd::codecs::keymap::ProfileSettingsFile {
        settings: vec![
            keyglassd::codecs::keymap::ProfileSetting::from_layer(0, 0).unwrap(),
            keyglassd::codecs::keymap::ProfileSetting::from_layer(0, 0).unwrap(),
        ],
        file_version: 0,
        file_type: 0,
    }
    .to_transfer_bytes()
    .unwrap();
    let ps_replug_hex: String = ps_replug.iter().map(|b| format!("{b:02x}")).collect();
    *sink.pulled_snapshot.lock().unwrap() = Some(serde_json::json!({
        "profiles": [
            {"id":"default","name":"Default","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":0,"default_layer":0},
            {"id":"coding","name":"VS Code","theme_id":"theme_coding","contexts":["Editor","Debug","Terminal"],"active_context":"Editor","actuation_mode_index":0,"default_layer":0}
        ],
        "firmwareProfileSettingsHex": ps_replug_hex,
    }));
    let (addr, st) = spawn_test_server(
        sink.clone(),
        Some("tok_rec".into()),
        Duration::from_millis(60),
        tmp.join("icons"),
    )
    .await;
    let mut bridge = StubBridge { lang: 471 };

    // 1. Start-already-connected: initial reconcile tick must fetch versions without hotplug.
    reconcile_tick(&st, &mut bridge, &*sink, 2);
    assert_eq!(
        *sink.calls.lock().unwrap(),
        vec!["platform:2", "profile:0", "lang:471"]
    );
    let (c_st0, b_st0) = http_req(
        addr,
        "GET",
        "/v1/device/status",
        &[("Authorization", "Bearer tok_rec")],
        None,
    )
    .await;
    assert_eq!(c_st0, 200);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&b_st0).unwrap()["versions"]["firmware"],
        "keyglass-firmware 1.0.0"
    );

    // 2. API override sets German(334) and active process rule matches VS Code (profile 1)
    st.inner
        .lock()
        .unwrap()
        .app_rules
        .push(keyglassd::bridges::AppProfileRule {
            matcher: "code".into(),
            profile_index: 1,
            language_code: None,
        });
    let (c_proc, _) = http_req(
        addr,
        "POST",
        "/v1/config/updateActiveProcess",
        &[("Authorization", "Bearer tok_rec")],
        Some(r#"{"process":"code"}"#),
    )
    .await;
    assert_eq!(c_proc, 200);
    let (c_lang, _) = http_req(
        addr,
        "POST",
        "/v1/config/updateInputLanguage",
        &[("Authorization", "Bearer tok_rec")],
        Some(r#"{"layout":"de"}"#),
    )
    .await;
    assert_eq!(c_lang, 200);
    assert_eq!(st.inner.lock().unwrap().applied_language_code, Some(334));

    // Reconcile while override active -> applies profile 1 without overwriting manual base desired_profile_index (0).
    reconcile_tick(&st, &mut bridge, &*sink, 2);
    assert_eq!(st.inner.lock().unwrap().applied_profile_index, Some(1));
    assert_eq!(st.inner.lock().unwrap().desired_profile_index, 0);

    // 3. TTL expires (detected app matches no rule) -> restores base manual profile(0) and OS US(471).
    tokio::time::sleep(Duration::from_millis(80)).await;
    reconcile_tick(&st, &mut bridge, &*sink, 2);
    assert_eq!(st.inner.lock().unwrap().applied_profile_index, Some(0));
    assert_eq!(st.inner.lock().unwrap().applied_language_code, Some(471));

    // 4. Firmware unplug and replug: platform, profile and language are sent again.
    *sink.connected.lock().unwrap() = (false, false);
    reconcile_tick(&st, &mut bridge, &*sink, 2);
    assert!(!st.inner.lock().unwrap().firmware_attached);

    sink.calls.lock().unwrap().clear();
    *sink.connected.lock().unwrap() = (false, true);
    reconcile_tick(&st, &mut bridge, &*sink, 2);
    assert_eq!(
        *sink.calls.lock().unwrap(),
        vec!["platform:2", "profile:0", "lang:471"]
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

struct ChildGuard(Option<std::process::Child>);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut c) = self.0.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

#[tokio::test]
async fn binary_enrollment_persistence_and_tls_restart() {
    let bin = env!("CARGO_BIN_EXE_keyglassd");
    let tmp = isolated_temp_dir("bin_restart");

    let child1 = ChildGuard(Some(
        std::process::Command::new(bin)
            .args(["--no-tray", "--no-usb"])
            .env("KEYGLASS_DATA_DIR", &tmp)
            .env("KEYGLASS_HOME_OVERRIDE", &tmp)
            .env(
                "KEYGLASS_UPDATE_INDEX_URL",
                format!("file://{}/no-index.json", tmp.display()),
            )
            .env("HOME", &tmp)
            .env("USERPROFILE", &tmp)
            .env("APPDATA", tmp.join("AppData/Roaming"))
            .env("LOCALAPPDATA", tmp.join("AppData/Local"))
            .env_remove("XDG_DATA_HOME")
            .env("KEYGLASS_API_ADDR", "127.0.0.1:0")
            .spawn()
            .unwrap(),
    ));

    let keys_path = tmp.join("config/keys.json");
    let secret_path = tmp.join("config/enrollment.secret");
    for _ in 0..80 {
        if keys_path.is_file() && secret_path.is_file() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(keys_path.is_file() && secret_path.is_file());

    let keys1: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&keys_path).unwrap()).unwrap();
    let port1 = keys1["openPort"].as_u64().unwrap() as u16;
    let cert_file = PathBuf::from(keys1["publicKeyLocation"].as_str().unwrap());
    let secret = std::fs::read_to_string(&secret_path)
        .unwrap()
        .trim()
        .to_string();

    let mut root_store = rustls::RootCertStore::empty();
    let cert_bytes = std::fs::read(&cert_file).unwrap();
    for cert in CertificateDer::pem_slice_iter(&cert_bytes) {
        root_store.add(cert.unwrap()).unwrap();
    }
    let tls_cfg = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(tls_cfg));
    let sname = rustls::pki_types::ServerName::try_from("localhost").unwrap();

    // 1. Unauthorized register (wrong secret) -> 403
    let mut s_bad = connector
        .connect(
            sname.clone(),
            TcpStream::connect(("127.0.0.1", port1)).await.unwrap(),
        )
        .await
        .unwrap();
    let bad_req = "{\"enrollmentSecret\":\"wrong\"}";
    s_bad.write_all(format!("POST /v1/authentication/register HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", bad_req.len(), bad_req).as_bytes()).await.unwrap();
    let mut resp_bad = Vec::new();
    let _ = s_bad.read_to_end(&mut resp_bad).await;
    assert!(String::from_utf8_lossy(&resp_bad).contains("403"));

    // 2. Authorized register with 0600 enrollment.secret -> 200 + token
    let mut s_ok = connector
        .connect(
            sname.clone(),
            TcpStream::connect(("127.0.0.1", port1)).await.unwrap(),
        )
        .await
        .unwrap();
    let ok_req = serde_json::json!({"enrollmentSecret": secret}).to_string();
    s_ok.write_all(format!("POST /v1/authentication/register HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", ok_req.len(), ok_req).as_bytes()).await.unwrap();
    let mut resp_ok = Vec::new();
    let _ = s_ok.read_to_end(&mut resp_ok).await;
    let txt_ok = String::from_utf8_lossy(&resp_ok);
    assert!(txt_ok.contains("200"), "{txt_ok}");
    let body_ok = txt_ok.split_once("\r\n\r\n").unwrap().1;
    let tok = serde_json::from_str::<serde_json::Value>(body_ok).unwrap()["message"]
        .as_str()
        .unwrap()
        .to_string();

    drop(child1);
    let _ = std::fs::remove_file(&keys_path);

    // 3. A second keyglassd (--no-usb) on the same KEYGLASS_DATA_DIR accepts the token from the first run.
    let child2 = ChildGuard(Some(
        std::process::Command::new(bin)
            .args(["--no-tray", "--no-usb"])
            .env("KEYGLASS_DATA_DIR", &tmp)
            .env("KEYGLASS_HOME_OVERRIDE", &tmp)
            .env(
                "KEYGLASS_UPDATE_INDEX_URL",
                format!("file://{}/no-index.json", tmp.display()),
            )
            .env("HOME", &tmp)
            .env("USERPROFILE", &tmp)
            .env("APPDATA", tmp.join("AppData/Roaming"))
            .env("LOCALAPPDATA", tmp.join("AppData/Local"))
            .env_remove("XDG_DATA_HOME")
            .env("KEYGLASS_API_ADDR", "127.0.0.1:0")
            .spawn()
            .unwrap(),
    ));
    for _ in 0..80 {
        if keys_path.is_file() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let keys2: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&keys_path).unwrap()).unwrap();
    let port2 = keys2["openPort"].as_u64().unwrap() as u16;
    let mut s_chk = connector
        .connect(
            sname,
            TcpStream::connect(("127.0.0.1", port2)).await.unwrap(),
        )
        .await
        .unwrap();
    s_chk.write_all(format!("GET /v1/authentication/check HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {tok}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut resp_chk = Vec::new();
    let _ = s_chk.read_to_end(&mut resp_chk).await;
    assert!(
        String::from_utf8_lossy(&resp_chk).contains("200 OK"),
        "{}",
        String::from_utf8_lossy(&resp_chk)
    );

    drop(child2);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn actuation_only_edit_and_device_pull_provenance_and_maintenance_contracts() {
    let tmp = isolated_temp_dir("pull_act_maint");
    let sink = RecordingDeviceSink::new((true, true), false);
    let (addr, st) = spawn_test_server(
        sink.clone(),
        Some("tok_pm".into()),
        Duration::from_secs(30),
        tmp.join("icons"),
    )
    .await;
    let auth = [("Authorization", "Bearer tok_pm")];

    let default_km: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/default_keymap.json")).unwrap();
    let default_blob_hex: String = include_bytes!("fixtures/default_keymap.bin")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    // 1. Unhydrated keymap + unhydrated actuation-only edits must retain dirty & fail closed on Save (422).
    assert_eq!(
        http_req(
            addr,
            "PUT",
            "/v1/config",
            &auth,
            Some(r#"{"actuationModes":[{"name":"Gaming","actuationPointMm":1.8,"rapidTrigger":true,"rapidTriggerSensitivityMm":0.2}]}"#)
        ).await.0,
        200
    );
    assert!(st.inner.lock().unwrap().keymap_dirty);
    assert_eq!(
        http_req(addr, "POST", "/v1/config/save", &auth, Some("{}"))
            .await
            .0,
        422
    );

    // Default pull without discardUnsaved returns 409 Conflict when unsaved dirty edits exist;
    // explicit discardUnsaved=true rebases cleanly onto device baseline (and hydrates actuationModes from blob).
    *sink.pulled_snapshot.lock().unwrap() = Some(serde_json::json!({
        "appearance": { "selectedId": "default", "defaultTheme": { "id": "default", "name": "Pulled" } },
        "settings": { "brightness": 80 },
        "mosaic": { "tileTypes": [12, 8, 15, 13] },
        "keymap": default_km,
        "firmwareKeymapBlobHex": default_blob_hex,
    }));
    assert_eq!(
        http_req(addr, "POST", "/v1/config/pull", &auth, Some("{}"))
            .await
            .0,
        409
    );
    assert_eq!(
        http_req(
            addr,
            "POST",
            "/v1/config/pull",
            &auth,
            Some(r#"{"discardUnsaved":true}"#)
        )
        .await
        .0,
        200
    );
    assert!(st.inner.lock().unwrap().keymap_hydrated);
    assert!(!st.inner.lock().unwrap().keymap_dirty);

    // 2. Unsupported modifier object fails PUT pre-commit with 422 and leaves state intact
    let mut bad_km = default_km.clone();
    bad_km["keymaps"]["ansi"]["layers"][0]["keyActions"][0]["modifiers"] =
        serde_json::json!({"ctrl": true});
    assert_eq!(
        http_req(
            addr,
            "PUT",
            "/v1/config",
            &auth,
            Some(&serde_json::json!({"keymap": bad_km}).to_string())
        )
        .await
        .0,
        422
    );

    // 2b. Mismatched {keymap, firmwareKeymapBlobHex} rejected pre-commit (422) without mutating state.
    let mut mismatch_km = default_km.clone();
    mismatch_km["keymaps"]["ansi"]["layers"][0]["keyActions"][0]["keyCode"] =
        serde_json::json!("KC_Z");
    let before_name = st.inner.lock().unwrap().keymap["keymaps"]["ansi"]["layers"][0]["keyActions"]
        [0]["keyCode"]
        .clone();
    assert_eq!(
        http_req(
            addr,
            "PUT",
            "/v1/config",
            &auth,
            Some(&serde_json::json!({"keymap": mismatch_km, "firmwareKeymapBlobHex": default_blob_hex}).to_string())
        ).await.0,
        422
    );
    assert_eq!(
        st.inner.lock().unwrap().keymap["keymaps"]["ansi"]["layers"][0]["keyActions"][0]["keyCode"],
        before_name
    );

    // 2c. Paired PUT {keymap, firmwareKeymapBlobHex} with nondefault actuation must hydrate GET actuationModes
    // and reject contradictory {keymap, firmwareKeymapBlobHex, actuationModes} with 422.
    let mut nondef_blob = keyglassd::codecs::keymap::decode_keymap_blob(include_bytes!(
        "fixtures/default_keymap.bin"
    ))
    .unwrap();
    let nd_word = keyglassd::codecs::keymap::encode_actuation_word(2, 1400, 300).unwrap();
    nondef_blob.keymaps[0].actuation_tables[0] = vec![nd_word; 85];
    let nd_hex: String = nondef_blob
        .to_bytes()
        .unwrap()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(
        http_req(
            addr,
            "PUT",
            "/v1/config",
            &auth,
            Some(
                &serde_json::json!({"keymap": default_km, "firmwareKeymapBlobHex": nd_hex})
                    .to_string()
            )
        )
        .await
        .0,
        200
    );
    let (c_get_nd, b_get_nd) = http_req(addr, "GET", "/v1/config", &auth, None).await;
    assert_eq!(c_get_nd, 200);
    let get_nd_json: serde_json::Value = serde_json::from_str(&b_get_nd).unwrap();
    assert_eq!(
        get_nd_json["actuationModes"][0]["rapidTrigger"],
        serde_json::json!(true)
    );
    assert_eq!(
        get_nd_json["actuationModes"][0]["actuationPointMm"],
        serde_json::json!(1.4)
    );
    assert_eq!(
        get_nd_json["actuationModes"][0]["rapidTriggerSensitivityMm"],
        serde_json::json!(0.3)
    );

    assert_eq!(
        http_req(
            addr,
            "PUT",
            "/v1/config",
            &auth,
            Some(&serde_json::json!({
                "keymap": default_km,
                "firmwareKeymapBlobHex": nd_hex,
                "actuationModes": [{"name": "Contradiction", "actuationPointMm": 2.4, "rapidTrigger": false}]
            }).to_string())
        ).await.0,
        422
    );

    // 3. Edit only Mode 0 (touched: true) while Mode 1 has touched: false -> Mode 1 table stays byte-identical.
    let default_dec = keyglassd::codecs::keymap::decode_keymap_blob(include_bytes!(
        "fixtures/default_keymap.bin"
    ))
    .unwrap();
    let mut modes_edit = st.inner.lock().unwrap().actuation_modes.clone();
    modes_edit[0]["actuationPointMm"] = serde_json::json!(2.0);
    modes_edit[0]["rapidTrigger"] = serde_json::json!(true);
    modes_edit[0]["rapidTriggerSensitivityMm"] = serde_json::json!(0.2);
    modes_edit[0]["touched"] = serde_json::json!(true);
    assert_eq!(
        http_req(
            addr,
            "PUT",
            "/v1/config",
            &auth,
            Some(&serde_json::json!({"actuationModes": modes_edit}).to_string())
        )
        .await
        .0,
        200
    );
    assert_eq!(
        http_req(addr, "POST", "/v1/config/save", &auth, Some("{}"))
            .await
            .0,
        200
    );
    assert!(!st.inner.lock().unwrap().keymap_dirty);
    {
        let snaps = sink.saved_snapshots.lock().unwrap();
        let last = snaps.last().unwrap();
        let hex = last["keymapBlobHex"].as_str().unwrap();
        let blob_bytes = ApiState::decode_hex_bytes(hex).unwrap();
        let dec = keyglassd::codecs::keymap::decode_keymap_blob(&blob_bytes).unwrap();
        assert_eq!(
            dec.keymaps[0].actuation_tables[0][0],
            keyglassd::codecs::keymap::encode_actuation_word(2, 2000, 200).unwrap()
        );
        assert_eq!(
            dec.keymaps[0].actuation_tables[1],
            default_dec.keymaps[0].actuation_tables[1]
        );
    }

    // 4. Edit -> Save -> Pull succeeds cleanly without 409.
    assert_eq!(
        http_req(addr, "POST", "/v1/config/pull", &auth, Some("{}"))
            .await
            .0,
        200
    );

    // 5. A JSON-only pull (no firmwareKeymapBlobHex) invalidates keymap provenance rather than pairing the pulled JSON with the earlier blob.
    *sink.pulled_snapshot.lock().unwrap() = Some(serde_json::json!({
        "keymap": default_km,
    }));
    assert_eq!(
        http_req(addr, "POST", "/v1/config/pull", &auth, Some("{}"))
            .await
            .0,
        200
    );
    assert!(!st.inner.lock().unwrap().keymap_hydrated);
    assert!(st.inner.lock().unwrap().firmware_keymap_blob.is_none());
    assert_eq!(
        http_req(
            addr,
            "PUT",
            "/v1/config",
            &auth,
            Some(&serde_json::json!({"keymap": default_km}).to_string())
        )
        .await
        .0,
        200
    );
    assert_eq!(
        http_req(addr, "POST", "/v1/config/save", &auth, Some("{}"))
            .await
            .0,
        422
    );

    // 6. Unsaved edit -> normal pull returns 409 Conflict and preserves dirty edit;
    assert_eq!(
        http_req(addr, "PUT", "/v1/config", &auth, Some(r#"{"appearance":{"selectedId":"default","defaultTheme":{"id":"default","name":"UserUnsavedEdit"}}}"#)).await.0,
        200
    );
    assert!(st.inner.lock().unwrap().appearance_dirty);
    *sink.pulled_snapshot.lock().unwrap() = Some(serde_json::json!({
        "appearance": { "selectedId": "default", "defaultTheme": { "id": "default", "name": "OldDevice" } },
    }));
    assert_eq!(
        http_req(addr, "POST", "/v1/config/pull", &auth, Some("{}"))
            .await
            .0,
        409
    );
    assert!(st.inner.lock().unwrap().appearance_dirty);
    assert_eq!(
        st.inner.lock().unwrap().appearance["defaultTheme"]["name"],
        "UserUnsavedEdit"
    );

    // 7. Maintenance: a package is uploaded to /home/root, and flash-uf2 refuses images that touch LittleFS.
    let dummy_ipk = tmp.join("keyglass-display_test.ipk");
    std::fs::write(&dummy_ipk, b"IPK_DATA").unwrap();
    let (c_ipk, b_ipk) = http_req(
        addr,
        "POST",
        "/v1/maintenance/sideload-ipk",
        &auth,
        Some(&serde_json::json!({"ipkPath": dummy_ipk.to_string_lossy()}).to_string()),
    )
    .await;
    assert_eq!(c_ipk, 200, "{b_ipk}");
    assert!(sink
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c == "sideload_ipk:/home/root/keyglass-display_test.ipk"));

    // LittleFS UF2 overwrite guard (>= 0x10F00000) returns 400.
    let mut uf2_bad = vec![0u8; 512];
    uf2_bad[0..4].copy_from_slice(&0x0A32_4655u32.to_le_bytes());
    uf2_bad[4..8].copy_from_slice(&0x9E5D_5157u32.to_le_bytes());
    uf2_bad[8..12].copy_from_slice(&0x0000_2000u32.to_le_bytes());
    uf2_bad[12..16].copy_from_slice(&0x10F0_0000u32.to_le_bytes());
    uf2_bad[16..20].copy_from_slice(&256u32.to_le_bytes());
    uf2_bad[24..28].copy_from_slice(&1u32.to_le_bytes());
    uf2_bad[508..512].copy_from_slice(&0x0AB1_6F30u32.to_le_bytes());
    let uf2_path = tmp.join("bad_lfs.uf2");
    std::fs::write(&uf2_path, &uf2_bad).unwrap();
    assert_eq!(
        http_req(
            addr,
            "POST",
            "/v1/maintenance/flash-uf2",
            &auth,
            Some(&serde_json::json!({"uf2Path": uf2_path.to_string_lossy()}).to_string()),
        )
        .await
        .0,
        400
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn concurrent_edit_during_blocked_pull_and_blocked_save_worker_barriers() {
    struct BarrierSink {
        pull_armed: AtomicBool,
        pull_entered: Arc<tokio::sync::Notify>,
        pull_release: Arc<(Mutex<bool>, std::sync::Condvar)>,
        save_entered: Arc<tokio::sync::Notify>,
        save_release: Arc<(Mutex<bool>, std::sync::Condvar)>,
        snapshot: serde_json::Value,
        /// The save plans written, oldest first.
        saved: Mutex<Vec<serde_json::Value>>,
    }
    impl DeviceSink for BarrierSink {
        fn is_connected(&self) -> (bool, bool) {
            (true, true)
        }
        fn versions(&self) -> serde_json::Value {
            serde_json::json!({"firmware": "keyglass-firmware 1.0.0"})
        }
        fn send_live_json(&self, _: &serde_json::Value) -> Result<(), String> {
            Ok(())
        }
        fn set_active_profile(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn set_language(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn set_platform(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn save_config_dir(&self, plan: &serde_json::Value) -> Result<(), String> {
            self.save_entered.notify_waiters();
            let (m, cv) = &*self.save_release;
            let mut g = m.lock().unwrap();
            while !*g {
                g = cv.wait(g).unwrap();
            }
            self.saved.lock().unwrap().push(plan.clone());
            Ok(())
        }
        fn get_file(&self, path: &str) -> Result<Vec<u8>, String> {
            let saved = self.saved.lock().unwrap();
            keyboard_file(std::iter::once(&self.snapshot).chain(saved.iter()), path)
                .ok_or_else(|| format!("{path} not found"))
        }
        fn stat_file(&self, path: &str) -> Result<bool, String> {
            Ok(self.get_file(path).is_ok())
        }
        fn pull_config_dir(&self) -> Result<serde_json::Value, String> {
            if !self.pull_armed.load(Ordering::SeqCst) {
                return Ok(self.snapshot.clone());
            }
            self.pull_entered.notify_waiters();
            let (m, cv) = &*self.pull_release;
            let mut g = m.lock().unwrap();
            while !*g {
                g = cv.wait(g).unwrap();
            }
            Ok(self.snapshot.clone())
        }
    }

    let tmp = isolated_temp_dir("barrier_races");
    let sink = Arc::new(BarrierSink {
        pull_armed: AtomicBool::new(false),
        pull_entered: Arc::new(tokio::sync::Notify::new()),
        pull_release: Arc::new((Mutex::new(false), std::sync::Condvar::new())),
        save_entered: Arc::new(tokio::sync::Notify::new()),
        save_release: Arc::new((Mutex::new(false), std::sync::Condvar::new())),
        snapshot: serde_json::json!({
            "appearance": { "selectedId": "default", "defaultTheme": { "id": "default", "name": "StaleDevice" } },
            "settings": { "brightness": 50 },
            "mosaic": { "tileTypes": [12, 8, 15, 13] }
        }),
        saved: Mutex::new(Vec::new()),
    });
    let st = ApiState::with_dirs(
        sink.clone(),
        Some("tok_race".into()),
        Duration::from_secs(30),
        tmp.join("icons"),
    );
    let running = Arc::new(AtomicBool::new(true));
    let worker = UsbWorkerHandle::spawn(
        st.clone(),
        sink.clone(),
        Box::new(StubBridge { lang: 471 }),
        2,
        running.clone(),
    );
    st.set_worker(worker);

    // Wait for initial background hydration to finish before arming explicit-pull barrier
    for _ in 0..40 {
        if st.inner.lock().unwrap().appearance_hydrated {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(st.inner.lock().unwrap().appearance_hydrated);
    assert!(!st.inner.lock().unwrap().appearance_dirty);
    sink.pull_armed.store(true, Ordering::SeqCst);

    let app = build_router(st.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // A. Blocked Pull Race: start POST /pull -> wait for async Notify -> PUT edit -> release pull -> 409 in-flight conflict + edit intact.
    let pull_entered_fut = sink.pull_entered.notified();
    let pull_task = tokio::spawn(async move {
        http_req(
            addr,
            "POST",
            "/v1/config/pull",
            &[("Authorization", "Bearer tok_race")],
            Some("{}"),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), pull_entered_fut)
        .await
        .expect("pull_entered timeout");

    assert_eq!(
        http_req(
            addr,
            "PUT",
            "/v1/config",
            &[("Authorization", "Bearer tok_race")],
            Some(r#"{"explicitReset":true,"appearance":{"selectedId":"default","defaultTheme":{"id":"default","name":"ConcurrentEditDuringPull"}}}"#)
        ).await.0,
        200
    );
    {
        let (m, cv) = &*sink.pull_release;
        *m.lock().unwrap() = true;
        cv.notify_all();
    }
    let (c_pull, b_pull) = pull_task.await.unwrap();
    assert_eq!(c_pull, 409, "{b_pull}");
    assert!(
        b_pull.contains("changed while the keyboard was being read"),
        "{b_pull}"
    );
    assert!(st.inner.lock().unwrap().appearance_dirty);
    assert_eq!(
        st.inner.lock().unwrap().appearance["defaultTheme"]["name"],
        "ConcurrentEditDuringPull"
    );

    // B. Blocked Save Race: start POST /save -> wait for async Notify -> PUT edit -> release save -> appearance_dirty must stay true.
    let save_entered_fut = sink.save_entered.notified();
    let save_task = tokio::spawn(async move {
        http_req(
            addr,
            "POST",
            "/v1/config/save",
            &[("Authorization", "Bearer tok_race")],
            Some("{}"),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), save_entered_fut)
        .await
        .expect("save_entered timeout");

    assert_eq!(
        http_req(
            addr,
            "PUT",
            "/v1/config",
            &[("Authorization", "Bearer tok_race")],
            Some(r#"{"appearance":{"selectedId":"default","defaultTheme":{"id":"default","name":"ConcurrentEditDuringSave"}}}"#)
        ).await.0,
        200
    );
    {
        let (m, cv) = &*sink.save_release;
        *m.lock().unwrap() = true;
        cv.notify_all();
    }
    let (c_save, _) = save_task.await.unwrap();
    assert_eq!(c_save, 200);
    assert!(st.inner.lock().unwrap().appearance_dirty);
    assert_eq!(
        st.inner.lock().unwrap().appearance["defaultTheme"]["name"],
        "ConcurrentEditDuringSave"
    );

    // C. Mismatched JSON is rejected against a valid firmware blob.
    let default_km: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/default_keymap.json")).unwrap();
    let mut diff_km = default_km.clone();
    diff_km["keymaps"]["ansi"]["layers"][0]["keyActions"][0]["keyCode"] = serde_json::json!("KC_Z");
    let default_bytes = include_bytes!("fixtures/default_keymap.bin");
    assert!(
        keyglassd::codecs::keymap::verify_keymap_matches_blob(&diff_km, default_bytes).is_err()
    );

    // D. Graceful worker shutdown during active save drains in-flight save & rejects new writes.
    {
        let (m, _) = &*sink.save_release;
        *m.lock().unwrap() = false;
    }
    let _open_save = OpenOnDrop(sink.save_release.clone());
    let entered2_fut = sink.save_entered.notified();
    let w_handle = st.get_worker().unwrap();
    let save_task2 = tokio::spawn(async move {
        http_req(
            addr,
            "POST",
            "/v1/config/save",
            &[("Authorization", "Bearer tok_race")],
            Some("{}"),
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered2_fut)
        .await
        .expect("save_entered2 timeout");
    let join_fut = tokio::task::spawn_blocking(move || w_handle.shutdown_and_join());
    tokio::time::sleep(Duration::from_millis(30)).await;
    // New save after shutdown_and_join begins must be rejected cleanly.
    let (c_rej, _) = http_req(
        addr,
        "POST",
        "/v1/config/save",
        &[("Authorization", "Bearer tok_race")],
        Some("{}"),
    )
    .await;
    assert_eq!(c_rej, 502);
    // Release active save -> active save completes (200) and worker join completes cleanly.
    {
        let (m, cv) = &*sink.save_release;
        *m.lock().unwrap() = true;
        cv.notify_all();
    }
    assert_eq!(save_task2.await.unwrap().0, 200);
    join_fut.await.unwrap();

    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn zip_export_import_nonblocking_media_import_and_save_media_upload() {
    let tmp = std::env::temp_dir().join(format!("keyglassd_bundle_media_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("assets_a/key_icons")).unwrap();
    std::fs::create_dir_all(tmp.join("assets_b/key_icons")).unwrap();

    let default_km: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/default_keymap.json")).unwrap();
    let mut km_a = default_km.clone();
    km_a["keymaps"]["ansi"]["layers"][0]["keyActions"][0] =
        serde_json::json!({"type": 1, "macroId": 0});
    km_a["macros"] = serde_json::json!([{
        "name": "CopyMacro",
        "steps": [{"keyCode": "KC_C", "delayMs": 15, "isDown": true}]
    }]);

    let sink_a = RecordingDeviceSink::new((true, true), false);
    let st_a = ApiState::with_dirs(
        sink_a,
        Some("tok_a".into()),
        Duration::from_secs(20),
        tmp.join("assets_a/key_icons"),
    );
    let lst_a = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr_a = lst_a.local_addr().unwrap();
    let st_a_ref = st_a.clone();
    tokio::spawn(async move {
        axum::serve(lst_a, build_router(st_a_ref)).await.unwrap();
    });

    {
        let mut ga = st_a.inner.lock().unwrap();
        ga.profiles
            .push(keyglassd::api::state::ShortcutProfileState {
                id: "studio_mix".into(),
                name: "Studio Mix".into(),
                theme_id: "default".into(),
                contexts: vec!["Mixing".into(), "Mastering".into()],
                active_context: "Mastering".into(),
                actuation_mode_index: 1,
                default_layer: 0,
                shortcut_pack: None,
                app_matcher: None,
            });
    }

    let sink_b = RecordingDeviceSink::new((true, true), false);
    let st_b = ApiState::with_dirs(
        sink_b.clone(),
        Some("tok_b".into()),
        Duration::from_secs(20),
        tmp.join("assets_b/key_icons"),
    );
    let lst_b = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr_b = lst_b.local_addr().unwrap();
    let st_b_ref = st_b.clone();
    tokio::spawn(async move {
        axum::serve(lst_b, build_router(st_b_ref)).await.unwrap();
    });

    // 1. Configure A with custom keymap+macro and two same-basename distinct media files (/a/clip.mp4 vs /b/clip.mp4)
    std::fs::create_dir_all(tmp.join("dir_a")).unwrap();
    std::fs::create_dir_all(tmp.join("dir_b")).unwrap();
    let abs_bg_a = tmp.join("dir_a/clip.mp4");
    let abs_bg_b = tmp.join("dir_b/clip.mp4");
    std::fs::write(&abs_bg_a, b"MP4PORTABLE_A").unwrap();
    std::fs::write(&abs_bg_b, b"MP4PORTABLE_B").unwrap();
    let mut app_a = st_a.inner.lock().unwrap().appearance.clone();
    app_a["defaultTheme"]["backgroundImagePath"] = serde_json::json!(abs_bg_a.to_string_lossy());
    let mut custom_th = app_a["defaultTheme"].clone();
    custom_th["id"] = serde_json::json!("custom_colliding_bg");
    custom_th["backgroundImagePath"] = serde_json::json!(abs_bg_b.to_string_lossy());
    app_a["customThemes"] = serde_json::json!([custom_th]);
    let (ca, _) = http_req(
        addr_a,
        "PUT",
        "/v1/config",
        &[("Authorization", "Bearer tok_a")],
        Some(
            &serde_json::json!({"appearance": app_a, "keymap": km_a, "explicitReset": true})
                .to_string(),
        ),
    )
    .await;
    assert_eq!(ca, 200);
    let bundle_path = tmp.join("exported_bundle.keyglass");
    let (ce, _) = http_req(
        addr_a,
        "POST",
        "/v1/config/export",
        &[("Authorization", "Bearer tok_a")],
        Some(&serde_json::json!({"outPath": bundle_path.to_string_lossy()}).to_string()),
    )
    .await;
    assert_eq!(ce, 200);
    assert!(bundle_path.is_file());
    // Delete original absolute sources before B imports -> B must still resolve & preserve both.
    std::fs::remove_file(&abs_bg_a).unwrap();
    std::fs::remove_file(&abs_bg_b).unwrap();

    // A "~/" output path expands against HOME.
    let fake_home = tmp.join("home_tilde");
    std::fs::create_dir_all(&fake_home).unwrap();
    let prev_home = std::env::var_os("HOME");
    std::env::set_var("HOME", &fake_home);
    let (ce_tilde, be_tilde) = http_req(
        addr_a,
        "POST",
        "/v1/config/export",
        &[("Authorization", "Bearer tok_a")],
        Some(r#"{"outPath":"~/.local/share/keyglass/export.keyglass"}"#),
    )
    .await;
    match prev_home {
        Some(v) => std::env::set_var("HOME", v),
        None => std::env::remove_var("HOME"),
    }
    assert_eq!(ce_tilde, 200, "tilde export failed: {be_tilde}");
    assert!(fake_home
        .join(".local/share/keyglass/export.keyglass")
        .is_file());

    // 2. Importing A's bundle into a clean B brings the keymap JSON with its firmware blob, both media
    //    files under distinct names, and the multi-context profile.
    let (ci, bi) = http_req(
        addr_b,
        "POST",
        "/v1/config/import",
        &[("Authorization", "Bearer tok_b")],
        Some(&serde_json::json!({"file": bundle_path.to_string_lossy()}).to_string()),
    )
    .await;
    assert_eq!(ci, 200, "ZIP import failed: {bi}");
    {
        let gb = st_b.inner.lock().unwrap();
        let def_bg = gb.appearance["defaultTheme"]["backgroundImagePath"]
            .as_str()
            .unwrap()
            .to_string();
        let cust_bg = gb.appearance["customThemes"][0]["backgroundImagePath"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(def_bg, "clip.mp4");
        assert_ne!(
            def_bg, cust_bg,
            "colliding basename media must be disambiguated"
        );
        assert_eq!(
            std::fs::read(tmp.join("assets_b/media_themes").join(&def_bg)).unwrap(),
            b"MP4PORTABLE_A"
        );
        assert_eq!(
            std::fs::read(tmp.join("assets_b/media_themes").join(&cust_bg)).unwrap(),
            b"MP4PORTABLE_B"
        );
        assert_eq!(
            gb.keymap["keymaps"]["ansi"]["layers"][0]["keyActions"][0]["macroId"],
            0
        );
        let blob_b = gb
            .firmware_keymap_blob
            .as_ref()
            .expect("ZIP import must install firmware_keymap_blob");
        let dec_b = keyglassd::codecs::keymap::decode_keymap_blob(blob_b).unwrap();
        assert_eq!(dec_b.keymaps[0].layers[0][0], 0x1000_0000);
        assert_eq!(dec_b.keymaps[0].macros.len(), 1);
        let prof = gb
            .profiles
            .iter()
            .find(|p| p.id == "studio_mix")
            .expect("missing exported profile");
        assert_eq!(
            prof.contexts,
            vec!["Mixing".to_string(), "Mastering".to_string()]
        );
        assert_eq!(prof.active_context, "Mastering");
    }

    // 2b. A bundle whose keymap.json has no "macros" but whose firmware_keymap.bin has macro 0: the
    //     macro is decoded into the JSON.
    {
        let mut no_macro_km = default_km.clone();
        no_macro_km["keymaps"]["ansi"]["layers"][0]["keyActions"][0] =
            serde_json::json!({"type": 1, "macroId": 0});
        let blob_with_m =
            keyglassd::codecs::keymap::encode_keymap_json_to_firmware_blob(&km_a, None, None)
                .unwrap();
        let blob_zip = tmp.join("blob_only_macro.keyglass");
        {
            let f = std::fs::File::create(&blob_zip).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("keymap.json", opts).unwrap();
            std::io::Write::write_all(&mut zw, &serde_json::to_vec(&no_macro_km).unwrap()).unwrap();
            zw.start_file("firmware_keymap.bin", opts).unwrap();
            std::io::Write::write_all(&mut zw, &blob_with_m).unwrap();
            zw.finish().unwrap();
        }
        let (ci2, bi2) = http_req(
            addr_b,
            "POST",
            "/v1/config/import",
            &[("Authorization", "Bearer tok_b")],
            Some(&serde_json::json!({"file": blob_zip.to_string_lossy()}).to_string()),
        )
        .await;
        assert_eq!(ci2, 200, "blob-only macro ZIP failed: {bi2}");
        let mut hydrated_km = st_b.inner.lock().unwrap().keymap.clone();
        let hydrated_macros = hydrated_km["macros"]
            .as_array()
            .expect("blob-only macros must hydrate into JSON")
            .clone();
        assert_eq!(hydrated_macros.len(), 1);
        // Appending a macro keeps macro 0.
        let mut next_macros = hydrated_macros.clone();
        next_macros.push(serde_json::json!({"name": "New", "steps": [{"keyCode": "KC_D", "delayMs": 10, "isDown": true}]}));
        hydrated_km["macros"] = serde_json::Value::Array(next_macros);
        let (cp2, _) = http_req(
            addr_b,
            "PUT",
            "/v1/config",
            &[("Authorization", "Bearer tok_b")],
            Some(&serde_json::json!({"keymap": hydrated_km}).to_string()),
        )
        .await;
        assert_eq!(cp2, 200);
        let dec2 = keyglassd::codecs::keymap::decode_keymap_blob(
            st_b.inner
                .lock()
                .unwrap()
                .firmware_keymap_blob
                .as_ref()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(dec2.keymaps[0].macros.len(), 2);
        assert_eq!(dec2.keymaps[0].macros[0][0] & 0xFF, 0x06); // KC_C
    }

    // 2c. An export whose media exceeds the bundle entry limit is refused before the destination is
    //     created.
    {
        let big = tmp.join("big.mp4");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(keyglassd::api::assets::MAX_BUNDLE_ENTRY_BYTES + 1)
            .unwrap();
        let mut th = app_a["defaultTheme"].clone();
        th["id"] = serde_json::json!("big");
        th["backgroundImagePath"] = serde_json::json!(big.to_string_lossy());
        let mut big_app = app_a.clone();
        big_app["customThemes"] = serde_json::json!([th]);
        let (cbig_p, _) = http_req(
            addr_a,
            "PUT",
            "/v1/config",
            &[("Authorization", "Bearer tok_a")],
            Some(&serde_json::json!({"appearance": big_app}).to_string()),
        )
        .await;
        assert_eq!(cbig_p, 200);
        let untouched_out = tmp.join("should_not_create.keyglass");
        let (ce_big, _) = http_req(
            addr_a,
            "POST",
            "/v1/config/export",
            &[("Authorization", "Bearer tok_a")],
            Some(&serde_json::json!({"outPath": untouched_out.to_string_lossy()}).to_string()),
        )
        .await;
        assert_eq!(ce_big, 413);
        assert!(
            !untouched_out.exists(),
            "a refused export doesn't create the destination"
        );
        let _ = std::fs::remove_file(&big);
    }

    // 2d. When media_themes can't be written, the import fails and leaves the configuration and
    //     existing icons unchanged.
    {
        let sink_c = RecordingDeviceSink::new((true, true), false);
        let st_c = ApiState::with_dirs(
            sink_c,
            Some("tok_c".into()),
            Duration::from_secs(20),
            tmp.join("assets_c/key_icons"),
        );
        std::fs::create_dir_all(tmp.join("assets_c/key_icons")).unwrap();
        std::fs::write(tmp.join("assets_c/key_icons/keep_me.png"), b"ORIGINAL_ICON").unwrap();
        std::fs::write(tmp.join("assets_c/media_themes"), b"not-a-directory").unwrap();
        let prev_c_app = st_c.inner.lock().unwrap().appearance.clone();

        // A bundle with an updated key_icons/keep_me.png and media/clip.mp4.
        let atomic_zip = tmp.join("atomic_asset_test.keyglass");
        {
            let f = std::fs::File::create(&atomic_zip).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("appearance.json", opts).unwrap();
            std::io::Write::write_all(&mut zw, &serde_json::to_vec(&prev_c_app).unwrap()).unwrap();
            zw.start_file("key_icons/keep_me.png", opts).unwrap();
            std::io::Write::write_all(&mut zw, b"OVERWRITTEN_ICON").unwrap();
            zw.start_file("media/clip.mp4", opts).unwrap();
            std::io::Write::write_all(&mut zw, b"CLIPBYTES").unwrap();
            zw.finish().unwrap();
        }

        let lst_c = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_c = lst_c.local_addr().unwrap();
        let st_c_ref = st_c.clone();
        tokio::spawn(async move {
            axum::serve(lst_c, build_router(st_c_ref)).await.unwrap();
        });

        let (cc_imp, _) = http_req(
            addr_c,
            "POST",
            "/v1/config/import",
            &[("Authorization", "Bearer tok_c")],
            Some(&serde_json::json!({"file": atomic_zip.to_string_lossy()}).to_string()),
        )
        .await;
        assert_eq!(
            cc_imp, 500,
            "the import fails when media_themes can't be written"
        );
        assert_eq!(
            st_c.inner.lock().unwrap().appearance,
            prev_c_app,
            "a failed import leaves the configuration unchanged"
        );
        assert_eq!(
            std::fs::read(tmp.join("assets_c/key_icons/keep_me.png")).unwrap(),
            b"ORIGINAL_ICON",
            "existing icons stay untouched"
        );

        // 2e. Nested key_icons entries (key_icons/a/keep_me.png and key_icons/b/keep_me.png) are rejected
        //     before anything is staged.
        let dup_zip = tmp.join("dup_icon_dest.keyglass");
        {
            let f = std::fs::File::create(&dup_zip).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("key_icons/a/keep_me.png", opts).unwrap();
            std::io::Write::write_all(&mut zw, b"A_BYTES").unwrap();
            zw.start_file("key_icons/b/keep_me.png", opts).unwrap();
            std::io::Write::write_all(&mut zw, b"B_BYTES").unwrap();
            zw.finish().unwrap();
        }
        let (cc_dup, _) = http_req(
            addr_c,
            "POST",
            "/v1/config/import",
            &[("Authorization", "Bearer tok_c")],
            Some(&serde_json::json!({"file": dup_zip.to_string_lossy()}).to_string()),
        )
        .await;
        assert_eq!(cc_dup, 400);
        assert_eq!(
            std::fs::read(tmp.join("assets_c/key_icons/keep_me.png")).unwrap(),
            b"ORIGINAL_ICON"
        );

        // On case-insensitive filesystems keep_me.png and KEEP_ME.png collide while staging, before
        // anything is committed.
        let is_case_insensitive = tmp.join("assets_c/key_icons/KEEP_ME.PNG").exists();
        let alias_zip = tmp.join("alias_icon_dest.keyglass");
        {
            let f = std::fs::File::create(&alias_zip).unwrap();
            let mut zw = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("key_icons/keep_me.png", opts).unwrap();
            std::io::Write::write_all(&mut zw, b"LOWER_BYTES").unwrap();
            zw.start_file("key_icons/KEEP_ME.png", opts).unwrap();
            std::io::Write::write_all(&mut zw, b"UPPER_BYTES").unwrap();
            zw.finish().unwrap();
        }
        let (cc_alias, _) = http_req(
            addr_c,
            "POST",
            "/v1/config/import",
            &[("Authorization", "Bearer tok_c")],
            Some(&serde_json::json!({"file": alias_zip.to_string_lossy()}).to_string()),
        )
        .await;
        if is_case_insensitive {
            assert_eq!(cc_alias, 400);
            assert_eq!(
                std::fs::read(tmp.join("assets_c/key_icons/keep_me.png")).unwrap(),
                b"ORIGINAL_ICON"
            );
        } else {
            assert_eq!(cc_alias, 200);
        }
    }

    // 3. A slow ffmpeg import doesn't block GET /v1/device/status.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let fake_mov = tmp.join("clip_src.mov");
        std::fs::write(&fake_mov, vec![0xAAu8; 4096]).unwrap();
        let fake_ff = tmp.join("slow_ff.sh");
        std::fs::write(
            &fake_ff,
            "#!/bin/sh\nsleep 0.6\nfor last; do :; done\nprintf 'MP4DATA' > \"$last\"\nexit 0\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&fake_ff).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake_ff, perms.clone()).unwrap();
        let fake_probe = tmp.join("keyglass-ffprobe");
        std::fs::write(&fake_probe, "#!/bin/sh\necho 6.0\nexit 0\n").unwrap();
        std::fs::set_permissions(&fake_probe, perms).unwrap();
        let prev_ff = std::env::var_os("KEYGLASS_HOST_FFMPEG");
        std::env::set_var("KEYGLASS_HOST_FFMPEG", &fake_ff);

        let t0 = std::time::Instant::now();
        let imp_task = tokio::spawn(async move {
            http_req(
                addr_b,
                "POST",
                "/v1/media/import",
                &[("Authorization", "Bearer tok_b")],
                Some(&serde_json::json!({"file": fake_mov.to_string_lossy()}).to_string()),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(60)).await;
        let (cs, _) = http_req(
            addr_b,
            "GET",
            "/v1/device/status",
            &[("Authorization", "Bearer tok_b")],
            None,
        )
        .await;
        assert_eq!(cs, 200);
        assert!(
            t0.elapsed() < Duration::from_millis(300),
            "status blocked during 600ms media import: {:?}",
            t0.elapsed()
        );

        let (cm, bm) = imp_task.await.unwrap();
        assert_eq!(cm, 200, "media import failed: {bm}");
        let asset_name = serde_json::from_str::<serde_json::Value>(&bm).unwrap()["assetPath"]
            .as_str()
            .unwrap()
            .to_string();

        // The save plan uploads the imported media.
        let (csave, _) = http_req(
            addr_b,
            "POST",
            "/v1/config/save",
            &[("Authorization", "Bearer tok_b")],
            Some("{}"),
        )
        .await;
        assert_eq!(csave, 200);
        let saved = sink_b.saved_snapshots.lock().unwrap();
        let last_plan = saved.last().unwrap();
        let mf = last_plan["mediaFiles"].as_array().unwrap();
        assert!(
            mf.iter().any(|x| x["name"] == asset_name),
            "expected imported media in save plan"
        );

        match prev_ff {
            Some(v) => std::env::set_var("KEYGLASS_HOST_FFMPEG", v),
            None => std::env::remove_var("KEYGLASS_HOST_FFMPEG"),
        }
    }

    // 4. Named profiles survive a restart and keep their app rules by stable ID when an earlier profile
    //    is deleted.
    {
        let persist_dir = tmp.join("persist_prof");
        std::fs::create_dir_all(persist_dir.join("config")).unwrap();
        let tok_path = persist_dir.join("config/api_tokens.json");
        let sink_p = RecordingDeviceSink::new((true, true), false);
        let st_p1 = ApiState::with_persistent_stores(
            sink_p.clone(),
            Some("tok_p".into()),
            Duration::from_secs(20),
            persist_dir.join("assets/key_icons"),
            tok_path.clone(),
            "sec_p".into(),
        );
        st_p1.persist_tokens().unwrap();
        let lst_p1 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_p1 = lst_p1.local_addr().unwrap();
        let st_p1_srv = st_p1.clone();
        tokio::spawn(async move {
            axum::serve(lst_p1, build_router(st_p1_srv)).await.unwrap();
        });

        // A custom theme, so a profile switch can be seen applying its theme.
        let custom_app_p1 = serde_json::json!({
            "selectedId": "default",
            "defaultTheme": {"id": "default", "name": "Default"},
            "customThemes": [{"id": "fps_theme", "name": "FPS Theme"}]
        });
        let custom_prof = serde_json::json!([
            {"id":"default","name":"Default","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":0,"default_layer":0},
            {"id":"temp_drop","name":"Temp","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":0,"default_layer":0,"app_matcher":"com.temp.drop"},
            {"id":"fps_pro","name":"FPS Pro","theme_id":"fps_theme","contexts":["Main"],"active_context":"Main","actuation_mode_index":1,"default_layer":2,"shortcut_pack":"Blender.csv","app_matcher":"com.game.fps"}
        ]);
        let (cp_put, bp_put) = http_req(
            addr_p1, "PUT", "/v1/config",
            &[("Authorization", "Bearer tok_p")],
            Some(&serde_json::json!({"appearance": custom_app_p1, "profiles": custom_prof, "desiredProfileIndex": 2, "explicitReset": true}).to_string()),
        ).await;
        assert_eq!(cp_put, 200, "profiles PUT failed: {bp_put}");

        // Deleting "temp_drop" (index 1, before the active "fps_pro" at 2) moves the selection and the app
        // rule to index 1.
        let remaining_prof = serde_json::json!([
            {"id":"default","name":"Default","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":0,"default_layer":0},
            {"id":"fps_pro","name":"FPS Pro","theme_id":"fps_theme","contexts":["Main"],"active_context":"Main","actuation_mode_index":1,"default_layer":2,"shortcut_pack":"Blender.csv","app_matcher":"com.game.fps"}
        ]);
        let (cp_del, _) = http_req(
            addr_p1,
            "PUT",
            "/v1/config",
            &[("Authorization", "Bearer tok_p")],
            Some(&serde_json::json!({"profiles": remaining_prof}).to_string()),
        )
        .await;
        assert_eq!(cp_del, 200);

        let (cp_save, bp_save) = http_req(
            addr_p1,
            "POST",
            "/v1/config/save",
            &[("Authorization", "Bearer tok_p")],
            Some("{}"),
        )
        .await;
        assert_eq!(cp_save, 200, "save failed: {bp_save}");
        {
            let snaps = sink_p.saved_snapshots.lock().unwrap();
            let last_snap = snaps.last().unwrap();
            let ps_hex = last_snap["profileSettingsHex"]
                .as_str()
                .expect("missing profileSettingsHex on save");
            let ps_bytes = ApiState::decode_hex_bytes(ps_hex).unwrap();
            let ps_dec =
                keyglassd::codecs::keymap::decode_profile_settings(&ps_bytes, true).unwrap();
            assert_eq!(ps_dec.settings.len(), 2);
            assert_eq!(ps_dec.settings[1].actuation_mode_index, 1);
            assert_eq!(ps_dec.settings[1].default_layer_bitfield, 1 << 2);
        }

        // profiles_dirty clears after a save; more than 64 profiles or an invalid profile is refused.
        assert!(
            !st_p1.inner.lock().unwrap().profiles_dirty,
            "profiles_dirty must clear after save"
        );
        let too_many_profs: Vec<serde_json::Value> = (0..65)
            .map(|i| serde_json::json!({"id":format!("p_{i}"),"name":format!("P {i}"),"theme_id":"default","contexts":["Main"],"active_context":"Main"}))
            .collect();
        let (c_over, _) = http_req(
            addr_p1,
            "PUT",
            "/v1/config",
            &[("Authorization", "Bearer tok_p")],
            Some(&serde_json::json!({"profiles": too_many_profs}).to_string()),
        )
        .await;
        assert_eq!(c_over, 400);
        let (c_bad_layer, _) = http_req(
            addr_p1, "PUT", "/v1/config",
            &[("Authorization", "Bearer tok_p")],
            Some(r#"{"profiles":[{"id":"default","name":"Default","theme_id":"default","contexts":["Main"],"active_context":"Main","default_layer":99}]}"#),
        ).await;
        assert_eq!(c_bad_layer, 400);
        // A refused PUT leaves the profiles unchanged.
        assert_eq!(st_p1.inner.lock().unwrap().profiles.len(), 2);

        // A restart, loading from disk.
        let sink_p2 = RecordingDeviceSink::new((true, true), false);
        let st_p2 = ApiState::with_persistent_stores(
            sink_p2.clone(),
            None,
            Duration::from_secs(20),
            persist_dir.join("assets/key_icons"),
            tok_path,
            "sec_p".into(),
        );
        {
            let mut g2 = st_p2.inner.lock().unwrap();
            g2.appearance = custom_app_p1;
            g2.appearance_hydrated = true;
            g2.versions_loaded = true;
            g2.cached_versions = sink_p2.versions();
            g2.committed_profile_ids = Some(vec!["default".into(), "fps_pro".into()]);
        }
        let lst_p2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_p2 = lst_p2.local_addr().unwrap();
        let st_p2_srv = st_p2.clone();
        tokio::spawn(async move {
            axum::serve(lst_p2, build_router(st_p2_srv)).await.unwrap();
        });

        let (cg_p2, bg_p2) = http_req(
            addr_p2,
            "GET",
            "/v1/config",
            &[("Authorization", "Bearer tok_p")],
            None,
        )
        .await;
        assert_eq!(cg_p2, 200);
        let cfg_p2: serde_json::Value = serde_json::from_str(&bg_p2).unwrap();
        assert_eq!(cfg_p2["profiles"][1]["name"], "FPS Pro");
        assert_eq!(cfg_p2["profiles"][1]["actuation_mode_index"], 1);
        assert_eq!(cfg_p2["profiles"][1]["default_layer"], 2);
        assert_eq!(cfg_p2["appRules"].as_array().unwrap().len(), 1);
        assert_eq!(cfg_p2["appRules"][0]["matcher"], "com.game.fps");
        assert_eq!(cfg_p2["appRules"][0]["profile_index"], 1);

        let (cs_p2, _) = http_req(
            addr_p2,
            "PUT",
            "/v1/config",
            &[("Authorization", "Bearer tok_p")],
            Some(r#"{"desiredProfileIndex": 1}"#),
        )
        .await;
        assert_eq!(cs_p2, 200);
        // Verify theme selectedId switched + appearance marked dirty + reconcile_tick sends set_active_profile(1).
        {
            let g2 = st_p2.inner.lock().unwrap();
            assert_eq!(g2.desired_profile_index, 1);
            assert_eq!(g2.appearance["selectedId"], "fps_theme");
            assert!(g2.appearance_dirty);
        }
        let mut fake_bridge = StubBridge { lang: 471 };
        reconcile_tick(&st_p2, &mut fake_bridge, &*sink_p2, 2);
        assert!(sink_p2
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c == "profile:1"));

        // Renaming a profile while preserving a manual rule with language_code, and clearing last app_matcher.
        let (cr_man, _) = http_req(
            addr_p2, "PUT", "/v1/config",
            &[("Authorization", "Bearer tok_p")],
            Some(r#"{"appRules":[{"matcher":"com.manual.lang","profile_index":1,"language_code":471},{"matcher":"com.game.fps","profile_index":1,"language_code":null}]}"#),
        ).await;
        assert_eq!(cr_man, 200);
        let renamed_cleared = serde_json::json!([
            {"id":"default","name":"Default","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":0,"default_layer":0},
            {"id":"fps_pro","name":"FPS Renamed","theme_id":"fps_theme","contexts":["Main"],"active_context":"Main","actuation_mode_index":1,"default_layer":2,"shortcut_pack":"Blender.csv","app_matcher":null}
        ]);
        let (cp_clr, _) = http_req(
            addr_p2,
            "PUT",
            "/v1/config",
            &[("Authorization", "Bearer tok_p")],
            Some(&serde_json::json!({"profiles": renamed_cleared}).to_string()),
        )
        .await;
        assert_eq!(cp_clr, 200);
        let (_, bg_clr) = http_req(
            addr_p2,
            "GET",
            "/v1/config",
            &[("Authorization", "Bearer tok_p")],
            None,
        )
        .await;
        let v_clr: serde_json::Value = serde_json::from_str(&bg_clr).unwrap();
        let rules_clr = v_clr["appRules"].as_array().unwrap();
        assert_eq!(rules_clr.len(), 1, "cleared app_matcher must remove profile-generated rule while preserving manual language rule");
        assert_eq!(rules_clr[0]["matcher"], "com.manual.lang");
        assert_eq!(rules_clr[0]["language_code"], 471);

        // Verify ZIP import of a profile marks profiles_dirty -> Save emits profileSettingsHex -> restart persists it.
        let mut imp_zip_buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut imp_zip_buf));
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("profiles.json", opts).unwrap();
            std::io::Write::write_all(
                &mut zw,
                br#"[{"id":"cad_imp","name":"CAD Imported","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":2,"default_layer":3}]"#,
            )
            .unwrap();
            zw.finish().unwrap();
        }
        let imp_zip_path = persist_dir.join("import_prof.keyglass");
        std::fs::write(&imp_zip_path, &imp_zip_buf).unwrap();
        let (c_iz, b_iz) = http_req(
            addr_p2,
            "POST",
            "/v1/config/import",
            &[("Authorization", "Bearer tok_p")],
            Some(&serde_json::json!({"file": imp_zip_path.to_string_lossy()}).to_string()),
        )
        .await;
        assert_eq!(c_iz, 200, "zip import failed: {b_iz}");
        assert!(
            st_p2.inner.lock().unwrap().profiles_dirty,
            "ZIP import must mark profiles_dirty"
        );
        let (c_iz_save, b_iz_save) = http_req(
            addr_p2,
            "POST",
            "/v1/config/save",
            &[("Authorization", "Bearer tok_p")],
            Some("{}"),
        )
        .await;
        assert_eq!(c_iz_save, 200, "save after zip import failed: {b_iz_save}");
        {
            let snaps = sink_p2.saved_snapshots.lock().unwrap();
            let last_snap = snaps.last().unwrap();
            let ps_hex = last_snap["profileSettingsHex"]
                .as_str()
                .expect("imported profile must emit profileSettingsHex on save");
            let ps_bytes = ApiState::decode_hex_bytes(ps_hex).unwrap();
            let ps_dec =
                keyglassd::codecs::keymap::decode_profile_settings(&ps_bytes, true).unwrap();
            assert_eq!(ps_dec.settings.last().unwrap().actuation_mode_index, 2);
            assert_eq!(
                ps_dec.settings.last().unwrap().default_layer_bitfield,
                1 << 3
            );
        }

        // Pull conflict/discard on dirty profiles, and reordered background profile hydration with paired PROFILE_SETTINGS.
        let sink_pull = RecordingDeviceSink::new((true, true), false);
        let ps_ba = keyglassd::codecs::keymap::ProfileSettingsFile {
            settings: vec![
                keyglassd::codecs::keymap::ProfileSetting::from_layer(1, 1).unwrap(),
                keyglassd::codecs::keymap::ProfileSetting::from_layer(0, 0).unwrap(),
            ],
            file_version: 0,
            file_type: 0,
        }
        .to_transfer_bytes()
        .unwrap();
        let ps_ba_hex: String = ps_ba.iter().map(|b| format!("{b:02x}")).collect();
        let ps_ab = keyglassd::codecs::keymap::ProfileSettingsFile {
            settings: vec![
                keyglassd::codecs::keymap::ProfileSetting::from_layer(0, 0).unwrap(),
                keyglassd::codecs::keymap::ProfileSetting::from_layer(1, 1).unwrap(),
            ],
            file_version: 0,
            file_type: 0,
        }
        .to_transfer_bytes()
        .unwrap();
        let ps_ab_hex: String = ps_ab.iter().map(|b| format!("{b:02x}")).collect();

        // Missing firmwareProfileSettingsHex must leave committed_profile_ids = None.
        *sink_pull.pulled_snapshot.lock().unwrap() = Some(serde_json::json!({
            "profiles": [
                {"id":"p_b","name":"Second","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":1,"default_layer":1},
                {"id":"p_a","name":"First","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":0,"default_layer":0}
            ]
        }));
        let st_pull_unverified = ApiState::new(sink_pull.clone(), None);
        keyglassd::api::worker::apply_device_pull(&st_pull_unverified, &*sink_pull, true).unwrap();
        assert!(st_pull_unverified
            .inner
            .lock()
            .unwrap()
            .committed_profile_ids
            .is_none());

        // Mismatched firmwareProfileSettingsHex (AB blob with BA JSON) must also leave committed_profile_ids = None.
        *sink_pull.pulled_snapshot.lock().unwrap() = Some(serde_json::json!({
            "profiles": [
                {"id":"p_b","name":"Second","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":1,"default_layer":1},
                {"id":"p_a","name":"First","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":0,"default_layer":0}
            ],
            "firmwareProfileSettingsHex": ps_ab_hex,
        }));
        keyglassd::api::worker::apply_device_pull(&st_pull_unverified, &*sink_pull, true).unwrap();
        assert!(st_pull_unverified
            .inner
            .lock()
            .unwrap()
            .committed_profile_ids
            .is_none());

        // Paired valid firmwareProfileSettingsHex certifies committed_profile_ids.
        *sink_pull.pulled_snapshot.lock().unwrap() = Some(serde_json::json!({
            "profiles": [
                {"id":"p_b","name":"Second","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":1,"default_layer":1},
                {"id":"p_a","name":"First","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":0,"default_layer":0}
            ],
            "firmwareProfileSettingsHex": ps_ba_hex,
        }));
        let st_pull = ApiState::new(sink_pull.clone(), None);
        {
            let mut gp = st_pull.inner.lock().unwrap();
            gp.profiles = vec![
                keyglassd::api::state::ShortcutProfileState {
                    id: "p_a".into(),
                    name: "First".into(),
                    theme_id: "default".into(),
                    contexts: vec!["Main".into()],
                    active_context: "Main".into(),
                    actuation_mode_index: 0,
                    default_layer: 0,
                    shortcut_pack: None,
                    app_matcher: None,
                },
                keyglassd::api::state::ShortcutProfileState {
                    id: "p_b".into(),
                    name: "Second".into(),
                    theme_id: "default".into(),
                    contexts: vec!["Main".into()],
                    active_context: "Main".into(),
                    actuation_mode_index: 1,
                    default_layer: 1,
                    shortcut_pack: None,
                    app_matcher: None,
                },
            ];
            gp.desired_profile_index = 1; // p_b
            gp.app_rules = vec![keyglassd::bridges::AppProfileRule {
                matcher: "com.reordered.app".into(),
                profile_index: 1, // p_b
                language_code: Some(471),
            }];
            gp.profiles_dirty = true;
        }
        // Without discardUnsaved, dirty profiles block pull.
        let err_p =
            keyglassd::api::worker::apply_device_pull(&st_pull, &*sink_pull, false).unwrap_err();
        assert!(err_p.contains("Conflict"), "{err_p}");
        // With discardUnsaved=true, reordered device list replaces profiles, clears profiles_dirty, and remaps desired_profile_index + app_rules by stable ID.
        keyglassd::api::worker::apply_device_pull(&st_pull, &*sink_pull, true).unwrap();
        {
            let gp = st_pull.inner.lock().unwrap();
            assert!(!gp.profiles_dirty);
            assert_eq!(gp.profiles[0].id, "p_b");
            assert_eq!(gp.desired_profile_index, 0);
            assert_eq!(gp.app_rules[0].profile_index, 0);
            assert_eq!(gp.app_rules[0].language_code, Some(471));
            assert_eq!(
                gp.committed_profile_ids.as_deref(),
                Some(&["p_b".to_string(), "p_a".to_string()][..])
            );
        }
        // Background hydration during full reconcile_tick with need_versions + another reorder also remaps before sending set_active_profile.
        *sink_pull.pulled_snapshot.lock().unwrap() = Some(serde_json::json!({
            "profiles": [
                {"id":"p_a","name":"First","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":0,"default_layer":0},
                {"id":"p_b","name":"Second","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":1,"default_layer":1}
            ],
            "firmwareProfileSettingsHex": ps_ab_hex,
        }));
        {
            let mut gp = st_pull.inner.lock().unwrap();
            gp.versions_loaded = false;
            gp.appearance_hydrated = false;
            gp.keymap_hydrated = false;
            gp.applied_profile_index = Some(0);
        }
        sink_pull.calls.lock().unwrap().clear();
        let mut br_pull = StubBridge { lang: 471 };
        reconcile_tick(&st_pull, &mut br_pull, &*sink_pull, 2);
        {
            let gp = st_pull.inner.lock().unwrap();
            assert_eq!(gp.profiles[1].id, "p_b");
            assert_eq!(gp.desired_profile_index, 1);
            assert_eq!(gp.app_rules[0].profile_index, 1);
        }
        assert!(sink_pull
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|c| c == "profile:1"));

        // Combined profiles reorder + explicit appRules PUT keeps explicit new-order indices.
        let reordered_with_explicit_rules = serde_json::json!({
            "profiles": [
                {"id":"fps_pro","name":"FPS Renamed","theme_id":"fps_theme","contexts":["Main"],"active_context":"Main","actuation_mode_index":1,"default_layer":2},
                {"id":"default","name":"Default","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":0,"default_layer":0}
            ],
            "appRules": [
                {"matcher":"com.neworder.fps","profile_index":0,"language_code":471}
            ]
        });
        let (c_combo, _) = http_req(
            addr_p2,
            "PUT",
            "/v1/config",
            &[("Authorization", "Bearer tok_p")],
            Some(&reordered_with_explicit_rules.to_string()),
        )
        .await;
        assert_eq!(c_combo, 200);
        {
            let g2 = st_p2.inner.lock().unwrap();
            assert_eq!(g2.app_rules.len(), 1);
            assert_eq!(g2.app_rules[0].matcher, "com.neworder.fps");
            assert_eq!(
                g2.app_rules[0].profile_index, 0,
                "explicit appRules must use new profile order, not old order"
            );
        }

        // Rejected invalid ZIP & JSON imports must leave state, revisions, and icon directory untouched.
        let (prev_app_rev, prev_prof_rev, prev_prof_len) = {
            let g2 = st_p2.inner.lock().unwrap();
            (g2.appearance_rev, g2.profiles_rev, g2.profiles.len())
        };
        let mut bad_zip_buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut bad_zip_buf));
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("key_icons/bad_should_not_commit.png", opts)
                .unwrap();
            std::io::Write::write_all(&mut zw, b"\x89PNG\r\n\x1a\n").unwrap();
            zw.start_file("profiles.json", opts).unwrap();
            std::io::Write::write_all(
                &mut zw,
                br#"[{"id":"bad_p","name":"Bad","default_layer":99}]"#,
            )
            .unwrap();
            zw.finish().unwrap();
        }
        let bad_zip_path = persist_dir.join("bad_prof.keyglass");
        std::fs::write(&bad_zip_path, &bad_zip_buf).unwrap();
        let (c_bad_zip, _) = http_req(
            addr_p2,
            "POST",
            "/v1/config/import",
            &[("Authorization", "Bearer tok_p")],
            Some(&serde_json::json!({"file": bad_zip_path.to_string_lossy()}).to_string()),
        )
        .await;
        assert_eq!(c_bad_zip, 400);
        assert!(!persist_dir
            .join("assets/key_icons/bad_should_not_commit.png")
            .exists());
        let bad_json_path = persist_dir.join("bad_prof_json.flux");
        std::fs::write(
            &bad_json_path,
            r#"{"appearance":[{"id":"ghost_theme","name":"Ghost"}],"profiles":[{"id":"p1","name":"OK"},{"id":"p2","name":"Bad","default_layer":99}]}"#,
        ).unwrap();
        let (c_bad_json, _) = http_req(
            addr_p2,
            "POST",
            "/v1/config/import",
            &[("Authorization", "Bearer tok_p")],
            Some(&serde_json::json!({"file": bad_json_path.to_string_lossy()}).to_string()),
        )
        .await;
        assert_eq!(c_bad_json, 400);
        {
            let g2 = st_p2.inner.lock().unwrap();
            assert_eq!(g2.appearance_rev, prev_app_rev);
            assert_eq!(g2.profiles_rev, prev_prof_rev);
            assert_eq!(g2.profiles.len(), prev_prof_len);
        }

        // ZIP import of profiles [B, A] with archive app_rule index 0 (pointing to B) into local [A, B]
        // must remap the imported rule from archive index 0 -> merged index 1 (B).
        let mut swap_zip_buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut swap_zip_buf));
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("profiles.json", opts).unwrap();
            std::io::Write::write_all(
                &mut zw,
                br#"[{"id":"p_b","name":"B"},{"id":"p_a","name":"A"}]"#,
            )
            .unwrap();
            zw.start_file("app_rules.json", opts).unwrap();
            std::io::Write::write_all(
                &mut zw,
                br#"[{"matcher":"com.archive.b","profile_index":0,"language_code":471}]"#,
            )
            .unwrap();
            zw.finish().unwrap();
        }
        let swap_zip_path = persist_dir.join("swap_rules.keyglass");
        std::fs::write(&swap_zip_path, &swap_zip_buf).unwrap();
        let st_swap = ApiState::new(
            RecordingDeviceSink::new((true, true), false),
            Some("tok_sw".into()),
        );
        {
            let mut gsw = st_swap.inner.lock().unwrap();
            gsw.profiles = vec![
                keyglassd::api::state::ShortcutProfileState {
                    id: "p_a".into(),
                    name: "A".into(),
                    theme_id: "default".into(),
                    contexts: vec!["Main".into()],
                    active_context: "Main".into(),
                    actuation_mode_index: 0,
                    default_layer: 0,
                    shortcut_pack: None,
                    app_matcher: None,
                },
                keyglassd::api::state::ShortcutProfileState {
                    id: "p_b".into(),
                    name: "B".into(),
                    theme_id: "default".into(),
                    contexts: vec!["Main".into()],
                    active_context: "Main".into(),
                    actuation_mode_index: 0,
                    default_layer: 0,
                    shortcut_pack: None,
                    app_matcher: None,
                },
            ];
        }
        let lst_sw = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_sw = lst_sw.local_addr().unwrap();
        let st_sw_srv = st_swap.clone();
        tokio::spawn(async move {
            axum::serve(lst_sw, build_router(st_sw_srv)).await.unwrap();
        });
        let (c_sw, b_sw) = http_req(
            addr_sw,
            "POST",
            "/v1/config/import",
            &[("Authorization", "Bearer tok_sw")],
            Some(&serde_json::json!({"file": swap_zip_path.to_string_lossy()}).to_string()),
        )
        .await;
        assert_eq!(c_sw, 200, "swap import failed: {b_sw}");
        {
            let gsw = st_swap.inner.lock().unwrap();
            assert!(
                gsw.profiles_dirty,
                "ZIP import including app_rules.json must still mark profiles_dirty"
            );
            assert_eq!(gsw.profiles[1].id, "p_b");
            assert_eq!(gsw.app_rules.len(), 1);
            assert_eq!(gsw.app_rules[0].matcher, "com.archive.b");
            assert_eq!(
                gsw.app_rules[0].profile_index, 1,
                "archive index 0 (B) must map to merged index 1 (B)"
            );
        }

        // Concurrent profile addition between prevalidation & commit: merge under final lock preserves both.
        let mut add_zip_buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut add_zip_buf));
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("profiles.json", opts).unwrap();
            std::io::Write::write_all(&mut zw, br#"[{"id":"p_c","name":"C"}]"#).unwrap();
            zw.finish().unwrap();
        }
        let add_zip_path = persist_dir.join("conc_prof.keyglass");
        std::fs::write(&add_zip_path, &add_zip_buf).unwrap();
        // Deterministic barrier pause between ZIP staging and commit lock -> concurrent PUT executes mid-flight -> both survive.
        let add_zip_body = serde_json::json!({"file": add_zip_path.to_string_lossy()}).to_string();
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let release =
            std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        *st_swap.import_barrier.lock().unwrap() = Some((entered.clone(), release.clone()));
        let imp_task = tokio::spawn(async move {
            http_req(
                addr_sw,
                "POST",
                "/v1/config/import",
                &[("Authorization", "Bearer tok_sw")],
                Some(&add_zip_body),
            )
            .await
        });
        entered.notified().await;
        let (c_pc, b_pc) = http_req(
            addr_sw, "PUT", "/v1/config",
            &[("Authorization", "Bearer tok_sw")],
            Some(r#"{"profiles":[{"id":"p_a","name":"A","theme_id":"default","contexts":["Main"],"active_context":"Main"},{"id":"p_b","name":"B","theme_id":"default","contexts":["Main"],"active_context":"Main"},{"id":"p_d","name":"D","theme_id":"default","contexts":["Main"],"active_context":"Main"}]}"#),
        ).await;
        {
            let (m, cv) = &*release;
            *m.lock().unwrap() = true;
            cv.notify_all();
        }
        assert_eq!(c_pc, 200, "concurrent PUT failed: {b_pc}");
        let (c_ic, b_ic) = imp_task.await.unwrap();
        assert_eq!(c_ic, 200, "concurrent zip import failed: {b_ic}");
        *st_swap.import_barrier.lock().unwrap() = None;
        {
            let gsw = st_swap.inner.lock().unwrap();
            assert!(
                gsw.profiles.iter().any(|p| p.id == "p_d"),
                "concurrent PUT profile must survive"
            );
            assert!(
                gsw.profiles.iter().any(|p| p.id == "p_c"),
                "concurrent ZIP profile must survive"
            );
        }

        // Verify interleaved persist_profiles() calls serialize cleanly and never produce stale revision on disk.
        let st_ser = ApiState::with_persistent_stores(
            RecordingDeviceSink::new((true, true), false),
            Some("tok_ser".into()),
            Duration::from_secs(20),
            persist_dir.join("assets/key_icons"),
            persist_dir.join("ser_tokens.json"),
            "sec_ser".into(),
        );
        let s1 = st_ser.clone();
        let s2 = st_ser.clone();
        let h1 = std::thread::spawn(move || {
            for i in 0..10u64 {
                {
                    let mut g = s1.inner.lock().unwrap();
                    g.profiles_rev = i + 1;
                }
                s1.persist_profiles().unwrap();
            }
        });
        let h2 = std::thread::spawn(move || {
            for i in 10..20u64 {
                {
                    let mut g = s2.inner.lock().unwrap();
                    g.profiles_rev = i + 1;
                }
                s2.persist_profiles().unwrap();
            }
        });
        h1.join().unwrap();
        h2.join().unwrap();
        let expected_rev = st_ser.inner.lock().unwrap().profiles_rev;
        let persisted_doc: serde_json::Value = serde_json::from_slice(
            &std::fs::read(persist_dir.join("profiles_store.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            persisted_doc["rev"], expected_rev,
            "serialized persist_profiles must leave final revision on disk"
        );

        // Persistence failure (directory blocking profiles_store.json) must propagate 500 instead of silent 200.
        let fail_dir = persist_dir.join("fail_persist");
        std::fs::create_dir_all(fail_dir.join("profiles_store.json")).unwrap();
        let st_fail = ApiState::with_persistent_stores(
            RecordingDeviceSink::new((true, true), false),
            Some("tok_pf".into()),
            Duration::from_secs(20),
            persist_dir.join("assets/key_icons"),
            fail_dir.join("tokens.json"),
            "sec_pf".into(),
        );
        let lst_pf = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_pf = lst_pf.local_addr().unwrap();
        let st_pf_srv = st_fail.clone();
        tokio::spawn(async move {
            axum::serve(lst_pf, build_router(st_pf_srv)).await.unwrap();
        });
        let (c_pf, b_pf) = http_req(
            addr_pf, "PUT", "/v1/config",
            &[("Authorization", "Bearer tok_pf")],
            Some(r#"{"profiles":[{"id":"default","name":"Default","theme_id":"default","contexts":["Main"],"active_context":"Main"}]}"#),
        ).await;
        assert_eq!(
            c_pf, 500,
            "persist_profiles failure must fail PUT instead of claiming success: {b_pf}"
        );
        assert!(
            st_fail.inner.lock().unwrap().profiles_dirty,
            "failed persist must keep draft dirty/retryable"
        );

        // Exact-ID-first ZIP import of (id=p_b, name=A) into [p_a(A), p_b(B)] updates p_b without duplicating IDs,
        // while an archive containing duplicate stable IDs is rejected with 400.
        let mut coll_zip_buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut coll_zip_buf));
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("profiles.json", opts).unwrap();
            std::io::Write::write_all(&mut zw, br#"[{"id":"p_b","name":"A"}]"#).unwrap();
            zw.finish().unwrap();
        }
        let coll_zip_path = persist_dir.join("coll_prof.keyglass");
        std::fs::write(&coll_zip_path, &coll_zip_buf).unwrap();
        let (c_coll, b_coll) = http_req(
            addr_sw,
            "POST",
            "/v1/config/import",
            &[("Authorization", "Bearer tok_sw")],
            Some(&serde_json::json!({"file": coll_zip_path.to_string_lossy()}).to_string()),
        )
        .await;
        assert_eq!(
            c_coll, 200,
            "exact ID match should update p_b without duplicate IDs: {b_coll}"
        );
        {
            let gsw = st_swap.inner.lock().unwrap();
            let pb_count = gsw.profiles.iter().filter(|p| p.id == "p_b").count();
            assert_eq!(
                pb_count, 1,
                "id=p_b must remain unique after ID-first merge"
            );
        }

        let mut dup_zip_buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut dup_zip_buf));
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("profiles.json", opts).unwrap();
            std::io::Write::write_all(
                &mut zw,
                br#"[{"id":"dup","name":"X"},{"id":"dup","name":"Y"}]"#,
            )
            .unwrap();
            zw.finish().unwrap();
        }
        let dup_zip_path = persist_dir.join("dup_prof.keyglass");
        std::fs::write(&dup_zip_path, &dup_zip_buf).unwrap();
        let (c_dup, _) = http_req(
            addr_sw,
            "POST",
            "/v1/config/import",
            &[("Authorization", "Bearer tok_sw")],
            Some(&serde_json::json!({"file": dup_zip_path.to_string_lossy()}).to_string()),
        )
        .await;
        assert_eq!(c_dup, 400, "duplicate stable IDs in archive must reject");

        // An unsaved reorder [A, B] -> [B, A] with A selected still addresses firmware slot 0 (A) before
        // Save, survives a restart with profiles_dirty=true, and switches to slot 1 only after Save.
        let sink_ro = RecordingDeviceSink::new((true, true), false);
        // Its own directory, so it starts from a fresh profile store: [default, coding], default
        // selected.
        let ro_dir = persist_dir.join("reorder");
        std::fs::create_dir_all(&ro_dir).unwrap();
        let ro_tok_path = ro_dir.join("tokens.json");
        let st_ro = ApiState::with_persistent_stores(
            sink_ro.clone(),
            Some("tok_ro".into()),
            Duration::from_secs(20),
            persist_dir.join("assets/key_icons"),
            ro_tok_path.clone(),
            "sec_ro".into(),
        );
        st_ro.persist_tokens().unwrap();
        {
            let g = st_ro.inner.lock().unwrap();
            let ids: Vec<&str> = g.profiles.iter().map(|p| p.id.as_str()).collect();
            assert_eq!(
                (ids, g.desired_profile_index),
                (vec!["default", "coding"], 0)
            );
        }
        let lst_ro = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_ro = lst_ro.local_addr().unwrap();
        let st_ro_srv = st_ro.clone();
        tokio::spawn(async move {
            axum::serve(lst_ro, build_router(st_ro_srv)).await.unwrap();
        });
        // Default committed order: ["default", "coding"], selected = 0 ("default")
        // Reorder locally without Save -> profiles_dirty=true, committed_profile_ids still None (or ["default","coding"]).
        let (c_ro_put, _) = http_req(
            addr_ro, "PUT", "/v1/config",
            &[("Authorization", "Bearer tok_ro")],
            Some(r#"{"profiles":[{"id":"coding","name":"VS Code","theme_id":"theme_coding","contexts":["Editor"],"active_context":"Editor"},{"id":"default","name":"Default","theme_id":"default","contexts":["Main"],"active_context":"Main"}]}"#),
        ).await;
        assert_eq!(c_ro_put, 200);
        // Simulated daemon restart while the reorder is still unsaved:
        // Case A: device pull fails -> draft stays dirty, committed_profile_ids stays None, no guessed slot sent.
        let sink_ro2 = RecordingDeviceSink::new((true, true), false);
        let st_ro2 = ApiState::with_persistent_stores(
            sink_ro2.clone(),
            None,
            Duration::from_secs(20),
            persist_dir.join("assets/key_icons"),
            ro_tok_path.clone(),
            "sec_ro".into(),
        );
        {
            let gr = st_ro2.inner.lock().unwrap();
            assert!(
                gr.profiles_dirty,
                "unsaved reorder draft must remain dirty across restart"
            );
            assert!(
                gr.committed_profile_ids.is_none(),
                "draft profiles.json must never be certified as committed hardware slots"
            );
            assert!(
                ApiState::hardware_profile_slot(&gr, 1).is_none(),
                "must refuse guessing hardware slot when dirty and unhydrated"
            );
        }
        let mut br_ro = StubBridge { lang: 471 };
        reconcile_tick(&st_ro2, &mut br_ro, &*sink_ro2, 2);
        assert!(
            !sink_ro2
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.starts_with("profile:")),
            "must not send guessed hardware slot before Save/Pull"
        );

        // Case B: startup with persisted dirty draft [coding, default] (desired="default" at local 1) + versions_loaded=false
        // + valid device pull [default, coding]: pull refreshes committed_profile_ids=[default, coding] without clobbering dirty draft,
        // and immediately activates "default" at hardware slot 0.
        let ps_dc = keyglassd::codecs::keymap::ProfileSettingsFile {
            settings: vec![
                keyglassd::codecs::keymap::ProfileSetting::from_layer(0, 0).unwrap(),
                keyglassd::codecs::keymap::ProfileSetting::from_layer(0, 0).unwrap(),
            ],
            file_version: 0,
            file_type: 0,
        }
        .to_transfer_bytes()
        .unwrap();
        let ps_dc_hex: String = ps_dc.iter().map(|b| format!("{b:02x}")).collect();
        let sink_ro_rec = RecordingDeviceSink::new((true, true), false);
        *sink_ro_rec.pulled_snapshot.lock().unwrap() = Some(serde_json::json!({
            "profiles": [
                {"id":"default","name":"Default","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":0,"default_layer":0},
                {"id":"coding","name":"VS Code","theme_id":"theme_coding","contexts":["Editor"],"active_context":"Editor","actuation_mode_index":0,"default_layer":0}
            ],
            "firmwareProfileSettingsHex": ps_dc_hex,
        }));
        let st_ro_rec = ApiState::with_persistent_stores(
            sink_ro_rec.clone(),
            None,
            Duration::from_secs(20),
            persist_dir.join("assets/key_icons"),
            ro_tok_path,
            "sec_ro".into(),
        );
        {
            let mut gr = st_ro_rec.inner.lock().unwrap();
            gr.desired_profile_index = 1; // "default" in dirty draft [coding, default]
        }
        reconcile_tick(&st_ro_rec, &mut br_ro, &*sink_ro_rec, 2);
        {
            let gr = st_ro_rec.inner.lock().unwrap();
            assert!(
                gr.profiles_dirty,
                "dirty draft must not be clobbered by background pull"
            );
            assert_eq!(
                gr.profiles[0].id, "coding",
                "dirty reordered draft must remain intact"
            );
            assert_eq!(
                gr.committed_profile_ids.as_deref(),
                Some(&["default".to_string(), "coding".to_string()][..])
            );
        }
        assert!(
            sink_ro_rec
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c == "profile:0"),
            "recovered map resolves local 1 ('default') -> hardware slot 0"
        );

        // Same-version disconnect + reconnect with failed pull invalidates committed_profile_ids.
        *sink_ro_rec.connected.lock().unwrap() = (false, false);
        reconcile_tick(&st_ro_rec, &mut br_ro, &*sink_ro_rec, 2);
        assert!(st_ro_rec
            .inner
            .lock()
            .unwrap()
            .committed_profile_ids
            .is_none());
        *sink_ro_rec.pulled_snapshot.lock().unwrap() = None;
        sink_ro_rec.calls.lock().unwrap().clear();
        *sink_ro_rec.connected.lock().unwrap() = (true, true);
        reconcile_tick(&st_ro_rec, &mut br_ro, &*sink_ro_rec, 2);
        assert!(
            !sink_ro_rec
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c.starts_with("profile:")),
            "same-version reconnect with failed pull must not reuse stale slot map"
        );

        // Save via the real USB worker -> the worker commits committed_profile_ids and invalidates applied_profile_index -> the next reconcile sends profile:1.
        let running_ro = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let worker_ro = keyglassd::api::worker::UsbWorkerHandle::spawn(
            st_ro2.clone(),
            sink_ro2.clone(),
            Box::new(StubBridge { lang: 471 }),
            2,
            running_ro.clone(),
        );
        st_ro2.set_worker(worker_ro.clone());
        let lst_ro2 = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_ro2 = lst_ro2.local_addr().unwrap();
        let st_ro2_srv = st_ro2.clone();
        tokio::spawn(async move {
            axum::serve(lst_ro2, build_router(st_ro2_srv))
                .await
                .unwrap();
        });
        {
            let mut gr2 = st_ro2.inner.lock().unwrap();
            gr2.versions_loaded = true;
            gr2.cached_versions = sink_ro2.versions();
        }
        let (c_ro_save, _) = http_req(
            addr_ro2,
            "POST",
            "/v1/config/save",
            &[("Authorization", "Bearer tok_ro")],
            Some("{}"),
        )
        .await;
        assert_eq!(c_ro_save, 200);
        // After Save, committed_profile_ids = ["coding", "default"], desired_profile_index = 1 ("default") -> reconcile sends profile:1.
        reconcile_tick(&st_ro2, &mut br_ro, &*sink_ro2, 2);
        assert!(
            sink_ro2
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|c| c == "profile:1"),
            "after Save, slot 1 is default"
        );
        running_ro.store(false, std::sync::atomic::Ordering::SeqCst);
        worker_ro.shutdown_and_join();

        // Uncertain PARTIAL_SAVE_UNKNOWN outcome invalidates committed_profile_ids.
        {
            let mut g_unc = st_ro2.inner.lock().unwrap();
            ApiState::invalidate_committed_profiles_on_uncertain_save(
                &mut g_unc,
                &serde_json::json!({"profiles": []}),
                "PARTIAL_SAVE_UNKNOWN: simulated uncertain state",
            );
            assert!(g_unc.committed_profile_ids.is_none());
            assert!(g_unc.profiles_dirty);
        }

        // Firmware-only unplug and replug (true,true)->(true,false)->(true,true) with appearance_hydrated=true
        // & unchanged versions must still recover committed_profile_ids, and mismatched pair
        // in apply_device_pull_bg must drop stale trust & invalidate applied_profile_index.
        let sink_oa = RecordingDeviceSink::new((true, true), false);
        *sink_oa.pulled_snapshot.lock().unwrap() = Some(serde_json::json!({
            "profiles": [
                {"id":"default","name":"Default","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":0,"default_layer":0},
                {"id":"coding","name":"VS Code","theme_id":"theme_coding","contexts":["Editor"],"active_context":"Editor","actuation_mode_index":0,"default_layer":0}
            ],
            "firmwareProfileSettingsHex": ps_dc_hex,
        }));
        let st_oa = ApiState::new(sink_oa.clone(), None);
        {
            let mut goa = st_oa.inner.lock().unwrap();
            goa.appearance_hydrated = true;
            goa.keymap_hydrated = true;
            goa.versions_loaded = true;
            goa.cached_versions = sink_oa.versions();
            goa.committed_profile_ids = Some(vec!["default".into(), "coding".into()]);
            goa.applied_profile_index = Some(0);
        }
        *sink_oa.connected.lock().unwrap() = (true, false);
        reconcile_tick(&st_oa, &mut br_ro, &*sink_oa, 2);
        assert!(st_oa.inner.lock().unwrap().committed_profile_ids.is_none());
        *sink_oa.connected.lock().unwrap() = (true, true);
        reconcile_tick(&st_oa, &mut br_ro, &*sink_oa, 2);
        assert_eq!(
            st_oa.inner.lock().unwrap().committed_profile_ids.as_deref(),
            Some(&["default".to_string(), "coding".to_string()][..])
        );
        // Unchanged-connectivity retry after failed/partial save clears map:
        {
            let mut goa = st_oa.inner.lock().unwrap();
            goa.committed_profile_ids = None;
            goa.next_pull_retry_at = None;
            goa.versions_loaded = true;
            goa.appearance_hydrated = true;
            goa.keymap_hydrated = true;
        }
        *sink_oa.pulled_snapshot.lock().unwrap() = None;
        reconcile_tick(&st_oa, &mut br_ro, &*sink_oa, 2);
        assert!(st_oa.inner.lock().unwrap().committed_profile_ids.is_none());
        // Simulate backoff elapsed + paired snapshot arrives with unchanged connectivity.
        *sink_oa.pulled_snapshot.lock().unwrap() = Some(serde_json::json!({
            "profiles": [
                {"id":"default","name":"Default","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":0,"default_layer":0},
                {"id":"coding","name":"VS Code","theme_id":"theme_coding","contexts":["Editor"],"active_context":"Editor","actuation_mode_index":0,"default_layer":0}
            ],
            "firmwareProfileSettingsHex": ps_dc_hex,
        }));
        st_oa.inner.lock().unwrap().next_pull_retry_at =
            Some(std::time::Instant::now() - Duration::from_secs(1));
        reconcile_tick(&st_oa, &mut br_ro, &*sink_oa, 2);
        assert_eq!(
            st_oa.inner.lock().unwrap().committed_profile_ids.as_deref(),
            Some(&["default".to_string(), "coding".to_string()][..])
        );
        // A mismatched pulled pair (wrong actuation_mode_index vs ps_dc_hex) drops trust & invalidates applied_profile_index.
        *sink_oa.pulled_snapshot.lock().unwrap() = Some(serde_json::json!({
            "profiles": [
                {"id":"default","name":"Default","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":5,"default_layer":0},
                {"id":"coding","name":"VS Code","theme_id":"theme_coding","contexts":["Editor"],"active_context":"Editor","actuation_mode_index":0,"default_layer":0}
            ],
            "firmwareProfileSettingsHex": ps_dc_hex,
        }));
        keyglassd::api::worker::apply_device_pull_bg(&st_oa, &*sink_oa);
        {
            let goa = st_oa.inner.lock().unwrap();
            assert!(goa.committed_profile_ids.is_none());
            assert!(goa.applied_profile_index.is_none());
        }
        // Partial snapshot without profiles must also revoke existing trust while preserving dirty draft.
        {
            let mut goa = st_oa.inner.lock().unwrap();
            goa.committed_profile_ids = Some(vec!["default".into(), "coding".into()]);
            goa.applied_profile_index = Some(0);
            goa.profiles_dirty = true;
        }
        *sink_oa.pulled_snapshot.lock().unwrap() = Some(serde_json::json!({
            "appearance": {"selectedId": "default"}
        }));
        keyglassd::api::worker::apply_device_pull_bg(&st_oa, &*sink_oa);
        {
            let goa = st_oa.inner.lock().unwrap();
            assert!(
                goa.committed_profile_ids.is_none(),
                "partial pull without profiles must revoke map trust"
            );
            assert!(goa.applied_profile_index.is_none());
            assert!(goa.profiles_dirty, "dirty draft must stay preserved");
        }

        // Concurrent selection of unsaved profile B while set_active_profile(A) is blocked in flight:
        // A's ACK must not emit B's theme tagged with A's slot.
        let sink_race = Arc::new(RecordingDeviceSink {
            connected: Mutex::new((true, true)),
            fail_writes: false,
            block_duration: Duration::from_millis(60),
            calls: Mutex::new(Vec::new()),
            saved_snapshots: Mutex::new(Vec::new()),
            pulled_snapshot: Mutex::new(None),
            pomo_state: Mutex::new(None),
        });
        let st_race = ApiState::new(sink_race.clone(), Some("tok_rc".into()));
        {
            let mut gr = st_race.inner.lock().unwrap();
            gr.versions_loaded = true;
            gr.cached_versions = sink_race.versions();
            gr.appearance_hydrated = true;
            gr.keymap_hydrated = true;
            gr.appearance = serde_json::json!({
                "selectedId": "default",
                "defaultTheme": {"id": "default", "name": "Default"},
                "customThemes": [{"id": "theme_b", "name": "Theme B"}]
            });
            gr.profiles = vec![
                keyglassd::api::state::ShortcutProfileState {
                    id: "p_a".into(),
                    name: "A".into(),
                    theme_id: "default".into(),
                    contexts: vec!["Main".into()],
                    active_context: "Main".into(),
                    actuation_mode_index: 0,
                    default_layer: 0,
                    shortcut_pack: None,
                    app_matcher: None,
                },
                keyglassd::api::state::ShortcutProfileState {
                    id: "p_b_unsaved".into(),
                    name: "B Unsaved".into(),
                    theme_id: "theme_b".into(),
                    contexts: vec!["Main".into()],
                    active_context: "Main".into(),
                    actuation_mode_index: 0,
                    default_layer: 0,
                    shortcut_pack: None,
                    app_matcher: None,
                },
            ];
            gr.committed_profile_ids = Some(vec!["p_a".into()]);
            gr.desired_profile_index = 0; // p_a
        }
        let lst_rc = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_rc = lst_rc.local_addr().unwrap();
        let st_rc_srv = st_race.clone();
        tokio::spawn(async move {
            axum::serve(lst_rc, build_router(st_rc_srv)).await.unwrap();
        });
        let st_rc_bg = st_race.clone();
        let sink_rc_bg = sink_race.clone();
        let rec_jh = std::thread::spawn(move || {
            let mut br = StubBridge { lang: 471 };
            reconcile_tick(&st_rc_bg, &mut br, &*sink_rc_bg, 2);
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        let (c_rc_put, _) = http_req(
            addr_rc,
            "PUT",
            "/v1/config",
            &[("Authorization", "Bearer tok_rc")],
            Some(r#"{"desiredProfileIndex": 1}"#),
        )
        .await;
        assert_eq!(c_rc_put, 200);
        rec_jh.join().unwrap();
        let cached_live_str = st_race
            .inner
            .lock()
            .unwrap()
            .cached_live_data_json
            .clone()
            .unwrap_or_default();
        let cached_live_val: serde_json::Value =
            serde_json::from_str(&cached_live_str).unwrap_or(serde_json::json!({}));
        assert_ne!(
            cached_live_val.get("activeThemeId").and_then(|v| v.as_str()),
            Some("theme_b"),
            "concurrent unsaved B selection must never emit theme_b tagged with A's slot: live={cached_live_str}"
        );

        // Selection-only PUT {"desiredProfileIndex": 1} must persist across daemon restart.
        let sel_dir = persist_dir.join("sel_only_persist");
        let _ = std::fs::create_dir_all(&sel_dir);
        let sel_tok_path = sel_dir.join("sel_tokens.json");
        let st_sel1 = ApiState::with_persistent_stores(
            RecordingDeviceSink::new((true, true), false),
            Some("tok_sel".into()),
            Duration::from_secs(20),
            persist_dir.join("assets/key_icons"),
            sel_tok_path.clone(),
            "sec_sel".into(),
        );
        st_sel1.persist_tokens().unwrap();
        st_sel1.persist_profiles().unwrap();
        let pre_put: serde_json::Value =
            serde_json::from_slice(&std::fs::read(sel_dir.join("profiles_store.json")).unwrap())
                .unwrap();
        assert_eq!(
            pre_put["desiredProfileIndex"], 0,
            "fresh store must start at desiredProfileIndex 0"
        );
        let lst_sel = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_sel = lst_sel.local_addr().unwrap();
        let st_sel_srv = st_sel1.clone();
        tokio::spawn(async move {
            axum::serve(lst_sel, build_router(st_sel_srv))
                .await
                .unwrap();
        });
        let (c_sel_put, _) = http_req(
            addr_sel,
            "PUT",
            "/v1/config",
            &[("Authorization", "Bearer tok_sel")],
            Some(r#"{"desiredProfileIndex": 1}"#),
        )
        .await;
        assert_eq!(c_sel_put, 200);
        let st_sel2 = ApiState::with_persistent_stores(
            RecordingDeviceSink::new((true, true), false),
            None,
            Duration::from_secs(20),
            persist_dir.join("assets/key_icons"),
            sel_tok_path,
            "sec_sel".into(),
        );
        assert_eq!(
            st_sel2.inner.lock().unwrap().desired_profile_index,
            1,
            "selection-only PUT must survive restart"
        );
        // Clean-store partial snapshot without profiles must persist revoked committedIds too,
        // and startup with all-null versions + failed first pull must not trust disk committedIds.
        let rev_tok_path = persist_dir.join("rev_tokens.json");
        let sink_rev = RecordingDeviceSink::new((true, true), false);
        let st_rev1 = ApiState::with_persistent_stores(
            sink_rev.clone(),
            Some("tok_rv".into()),
            Duration::from_secs(20),
            persist_dir.join("assets/key_icons"),
            rev_tok_path.clone(),
            "sec_rv".into(),
        );
        {
            let mut gr = st_rev1.inner.lock().unwrap();
            gr.committed_profile_ids = Some(vec!["default".into(), "coding".into()]);
            gr.profiles_dirty = false;
        }
        st_rev1.persist_profiles().unwrap();
        // Restart -> committed_profile_ids must start None even though profiles_store.json had committedIds.
        struct NullVersionsSink;
        impl DeviceSink for NullVersionsSink {
            fn is_connected(&self) -> (bool, bool) {
                (true, true)
            }
            fn versions(&self) -> serde_json::Value {
                keyglassd::api::state::empty_versions()
            }
            fn pull_config_dir(&self) -> Result<serde_json::Value, String> {
                Err("transient startup USB failure".into())
            }
            fn backup_device(&self, _out_dir: &str) -> Result<String, String> {
                Err("unused".into())
            }
            fn flash_uf2(&self, _uf2_path: &str) -> Result<String, String> {
                Err("unused".into())
            }
            fn sideload_ipk(&self, _ipk_path: &str) -> Result<String, String> {
                Err("unused".into())
            }
            fn send_live_json(&self, _payload: &serde_json::Value) -> Result<(), String> {
                Ok(())
            }
            fn set_active_profile(&self, _idx: u32) -> Result<(), String> {
                Ok(())
            }
            fn set_language(&self, _code: u32) -> Result<(), String> {
                Ok(())
            }
            fn set_platform(&self, _code: u32) -> Result<(), String> {
                Ok(())
            }
            fn save_config_dir(&self, _snapshot: &serde_json::Value) -> Result<(), String> {
                Ok(())
            }
        }
        let sink_null = Arc::new(NullVersionsSink);
        let st_rev2 = ApiState::with_persistent_stores(
            sink_null.clone(),
            None,
            Duration::from_secs(20),
            persist_dir.join("assets/key_icons"),
            rev_tok_path.clone(),
            "sec_rv".into(),
        );
        assert!(st_rev2
            .inner
            .lock()
            .unwrap()
            .committed_profile_ids
            .is_none());
        let mut br_null = StubBridge { lang: 471 };
        reconcile_tick(&st_rev2, &mut br_null, &*sink_null, 2);
        assert!(st_rev2
            .inner
            .lock()
            .unwrap()
            .committed_profile_ids
            .is_none());
        // Clean-store partial pull without profiles persists committedIds = null to profiles_store.json:
        {
            let mut gr = st_rev1.inner.lock().unwrap();
            gr.committed_profile_ids = Some(vec!["default".into(), "coding".into()]);
            gr.profiles_dirty = false;
        }
        *sink_rev.pulled_snapshot.lock().unwrap() =
            Some(serde_json::json!({"appearance": {"selectedId": "default"}}));
        keyglassd::api::worker::apply_device_pull_bg(&st_rev1, &*sink_rev);
        let store_after: serde_json::Value = serde_json::from_slice(
            &std::fs::read(persist_dir.join("profiles_store.json")).unwrap(),
        )
        .unwrap();
        assert!(
            store_after["committedIds"].is_null(),
            "clean partial pull must persist revoked committedIds: {store_after}"
        );

        // Duplicate-ID paired pull (["x", "x"]) must revoke map trust and preserve local draft.
        let before_first_id = {
            let mut gr = st_rev1.inner.lock().unwrap();
            gr.committed_profile_ids = Some(vec!["default".into(), "coding".into()]);
            gr.profiles_dirty = false;
            gr.profiles[0].id.clone()
        };
        *sink_rev.pulled_snapshot.lock().unwrap() = Some(serde_json::json!({
            "profiles": [
                {"id":"x","name":"First X","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":0,"default_layer":0},
                {"id":"x","name":"Dup X","theme_id":"default","contexts":["Main"],"active_context":"Main","actuation_mode_index":0,"default_layer":0}
            ],
            "firmwareProfileSettingsHex": ps_dc_hex,
        }));
        keyglassd::api::worker::apply_device_pull_bg(&st_rev1, &*sink_rev);
        {
            let gr = st_rev1.inner.lock().unwrap();
            assert!(
                gr.committed_profile_ids.is_none(),
                "duplicate-ID paired pull must revoke map trust"
            );
            assert_eq!(
                gr.profiles[0].id, before_first_id,
                "local profiles must not be overwritten by duplicate-ID pull"
            );
        }

        // Disconnected device + select custom theme -> running multiple reconcile ticks must preserve custom theme.
        let sink_off = RecordingDeviceSink::new((false, false), false);
        let st_off = ApiState::new(sink_off.clone(), None);
        {
            let mut go = st_off.inner.lock().unwrap();
            go.appearance = serde_json::json!({
                "selectedId": "custom_neon",
                "defaultTheme": {"id": "default", "name": "Default"},
                "customThemes": [{"id": "custom_neon", "name": "Custom Neon"}]
            });
        }
        let mut br_off = StubBridge { lang: 471 };
        reconcile_tick(&st_off, &mut br_off, &*sink_off, 2);
        reconcile_tick(&st_off, &mut br_off, &*sink_off, 2);
        assert_eq!(
            st_off.inner.lock().unwrap().appearance["selectedId"],
            "custom_neon",
            "offline worker reconcile ticks must not undo chosen custom theme"
        );

        // Same-profile theme_id rebind + updateActiveProcess -> unmatched fallback restores Default theme.
        let sink_rebind = RecordingDeviceSink::new((true, true), false);
        let st_rebind = ApiState::new(sink_rebind.clone(), Some("tok_rb".into()));
        {
            let mut gr = st_rebind.inner.lock().unwrap();
            gr.versions_loaded = true;
            gr.cached_versions = sink_rebind.versions();
            gr.committed_profile_ids = Some(vec!["default".into(), "coding".into()]);
            gr.appearance = serde_json::json!({
                "selectedId": "default",
                "defaultTheme": {"id": "default", "name": "Default"},
                "customThemes": [
                    {"id": "theme_coding", "name": "Coding"},
                    {"id": "theme_v2", "name": "Coding V2"}
                ]
            });
        }
        let lst_rb = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_rb = lst_rb.local_addr().unwrap();
        let st_rb_srv = st_rebind.clone();
        tokio::spawn(async move {
            axum::serve(lst_rb, build_router(st_rb_srv)).await.unwrap();
        });
        let (c_p_b, _) = http_req(
            addr_rb,
            "POST",
            "/v1/config/updateActiveProcess",
            &[("Authorization", "Bearer tok_rb")],
            Some(r#"{"process":"Visual Studio Code"}"#),
        )
        .await;
        assert_eq!(c_p_b, 200);
        // Manually trigger app-rule match -> theme_coding, then unmatched process -> reconcile_tick restores default.
        st_rebind
            .inner
            .lock()
            .unwrap()
            .app_rules
            .push(keyglassd::bridges::AppProfileRule {
                matcher: "vscode".into(),
                profile_index: 1,
                language_code: None,
            });
        let _ = http_req(
            addr_rb,
            "POST",
            "/v1/config/updateActiveProcess",
            &[("Authorization", "Bearer tok_rb")],
            Some(r#"{"process":"vscode"}"#),
        )
        .await;
        assert_eq!(
            st_rebind.inner.lock().unwrap().appearance["selectedId"],
            "theme_coding"
        );
        let _ = http_req(
            addr_rb,
            "POST",
            "/v1/config/updateActiveProcess",
            &[("Authorization", "Bearer tok_rb")],
            Some(r#"{"process":"otherapp"}"#),
        )
        .await;
        let mut br_rb = StubBridge { lang: 471 };
        reconcile_tick(&st_rebind, &mut br_rb, &*sink_rebind, 2);
        assert_eq!(
            st_rebind.inner.lock().unwrap().appearance["selectedId"],
            "default",
            "unmatched process must restore Default theme"
        );
        // Rebind default profile's theme_id -> theme_v2 -> reconcile_tick applies theme_v2 even though profile ID stayed 'default'.
        let (c_rb2, _) = http_req(
            addr_rb, "PUT", "/v1/config",
            &[("Authorization", "Bearer tok_rb")],
            Some(r#"{"profiles":[{"id":"default","name":"Default","theme_id":"theme_v2","contexts":["Main"],"active_context":"Main"},{"id":"coding","name":"VS Code","theme_id":"theme_coding","contexts":["Editor"],"active_context":"Editor"}]}"#),
        ).await;
        assert_eq!(c_rb2, 200);
        reconcile_tick(&st_rebind, &mut br_rb, &*sink_rebind, 2);
        assert_eq!(
            st_rebind.inner.lock().unwrap().appearance["selectedId"],
            "theme_v2"
        );
    }

    // Authoritative Pomodoro timer + 7-min initial seed + fail-closed disconnect
    {
        let sink_pomo = RecordingDeviceSink::new((true, true), false);
        let st_pomo = ApiState::new(sink_pomo.clone(), Some("tok_pomo".into()));
        {
            let mut gp = st_pomo.inner.lock().unwrap();
            gp.mosaic = serde_json::json!({
                "tileTypes": [12, 13, 0, 0],
                "tileConfigs": [{"main": 99}, {"main": 7}, {}, {}]
            });
        }
        let lst_p = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_p = lst_p.local_addr().unwrap();
        let st_p_srv = st_pomo.clone();
        tokio::spawn(async move {
            axum::serve(lst_p, build_router(st_p_srv)).await.unwrap();
        });

        // 1. Fresh state + 7-minute Pomodoro (at slot 1, while slot 0 has unrelated main:99) -> first Toggle starts at 420s.
        let (c_t1, b_t1) = http_req(
            addr_p,
            "POST",
            "/v1/pomodoro",
            &[("Authorization", "Bearer tok_pomo")],
            Some(r#"{"action":"toggle"}"#),
        )
        .await;
        assert_eq!(c_t1, 200);
        let v_t1: serde_json::Value = serde_json::from_str(&b_t1).unwrap();
        assert_eq!(v_t1["pomodoro"]["running"], true);
        assert_eq!(v_t1["pomodoro"]["remainingS"], 420);

        // 2. Telemetry refreshes without pomodoro must not wipe authoritative state
        let mut br_p = StubBridge { lang: 471 };
        reconcile_tick(&st_pomo, &mut br_p, &*sink_pomo, 2);
        reconcile_tick(&st_pomo, &mut br_p, &*sink_pomo, 2);
        let (c_t2, b_t2) = http_req(
            addr_p,
            "POST",
            "/v1/pomodoro",
            &[("Authorization", "Bearer tok_pomo")],
            Some(r#"{"action":"toggle"}"#),
        )
        .await;
        assert_eq!(c_t2, 200);
        let v_t2: serde_json::Value = serde_json::from_str(&b_t2).unwrap();
        assert_eq!(
            v_t2["pomodoro"]["running"], false,
            "second toggle after telemetry ticks must pause, not restart"
        );

        // 3. Physical keyboard starts & counts down -> Studio toggle must pause at device remainingS
        *sink_pomo.pomo_state.lock().unwrap() = Some(serde_json::json!({
            "running": true,
            "remainingS": 388,
            "devRev": 1,
            "hostCmdSeq": st_pomo.inner.lock().unwrap().pomo_cmd_seq,
            "devBootId": "boot-1",
        }));
        let (c_t3, b_t3) = http_req(
            addr_p,
            "POST",
            "/v1/pomodoro",
            &[("Authorization", "Bearer tok_pomo")],
            Some(r#"{"action":"toggle"}"#),
        )
        .await;
        assert_eq!(c_t3, 200);
        let v_t3: serde_json::Value = serde_json::from_str(&b_t3).unwrap();
        assert_eq!(
            v_t3["pomodoro"]["running"], false,
            "UI toggle after keyboard start must pause"
        );
        assert_eq!(
            v_t3["pomodoro"]["remainingS"], 388,
            "UI toggle must preserve device elapsed remainingS"
        );

        // 3b. Independent display restart (devBootId changes, devRev resets to 1, hostCmdSeq resets to 0) must not be blocked
        *sink_pomo.pomo_state.lock().unwrap() = Some(serde_json::json!({
            "running": true,
            "remainingS": 350,
            "devRev": 1,
            "hostCmdSeq": 0,
            "devBootId": "boot-2-restarted",
        }));
        reconcile_tick(&st_pomo, &mut br_p, &*sink_pomo, 2);
        *sink_pomo.pomo_state.lock().unwrap() = Some(serde_json::json!({
            "running": true,
            "remainingS": 340,
            "devRev": 2,
            "hostCmdSeq": 0,
            "devBootId": "boot-2-restarted",
        }));
        let (c_t4, b_t4) = http_req(
            addr_p,
            "POST",
            "/v1/pomodoro",
            &[("Authorization", "Bearer tok_pomo")],
            Some(r#"{"action":"toggle"}"#),
        )
        .await;
        assert_eq!(c_t4, 200);
        let v_t4: serde_json::Value = serde_json::from_str(&b_t4).unwrap();
        assert_eq!(v_t4["pomodoro"]["running"], false);
        assert_eq!(v_t4["pomodoro"]["remainingS"], 340);

        // 4. Success -> failed send -> subsequent physical action must still sync
        let sink_flaky = Arc::new(RecordingDeviceSink {
            connected: Mutex::new((true, true)),
            fail_writes: false,
            block_duration: Duration::ZERO,
            calls: Mutex::new(Vec::new()),
            saved_snapshots: Mutex::new(Vec::new()),
            pulled_snapshot: Mutex::new(None),
            pomo_state: Mutex::new(None),
        });
        let st_flaky = ApiState::new(sink_flaky.clone(), Some("tok_flaky".into()));
        let lst_fl = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_fl = lst_fl.local_addr().unwrap();
        let st_fl_srv = st_flaky.clone();
        tokio::spawn(async move {
            axum::serve(lst_fl, build_router(st_fl_srv)).await.unwrap();
        });
        let (c_s1, b_s1) = http_req(
            addr_fl,
            "POST",
            "/v1/pomodoro",
            &[("Authorization", "Bearer tok_flaky")],
            Some(r#"{"action":"toggle"}"#),
        )
        .await;
        assert_eq!(c_s1, 200);
        let cmd1 = serde_json::from_str::<serde_json::Value>(&b_s1).unwrap()["pomodoro"]["cmdSeq"]
            .as_u64()
            .unwrap();
        // The keyboard drops off. A failing send is simulated with a second, offline sink whose
        // writes fail.
        sink_flaky.connected.lock().unwrap().0 = false;
        let sink_off_p = RecordingDeviceSink::new((false, false), true);
        let st_p_off = ApiState::new(sink_off_p, Some("tok_poff".into()));
        let lst_po = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr_po = lst_po.local_addr().unwrap();
        let st_po_srv = st_p_off.clone();
        tokio::spawn(async move {
            axum::serve(lst_po, build_router(st_po_srv)).await.unwrap();
        });
        // Seeded with the last successful command (seq=cmd1) and the same boot ID, a failed command leaves expected_cmd_seq at cmd1 while cmd_seq advances.
        {
            let mut gp = st_p_off.inner.lock().unwrap();
            gp.pomo_rev = 1;
            gp.pomo_cmd_seq = cmd1;
            gp.pomo_expected_cmd_seq = cmd1;
            gp.pomo_last_dev_rev = 2;
            gp.pomo_dev_boot_id = Some("boot-flaky".into());
            gp.pomo_running = true;
        }
        let (c_off, _) = http_req(
            addr_po,
            "POST",
            "/v1/pomodoro",
            &[("Authorization", "Bearer tok_poff")],
            Some(r#"{"action":"toggle"}"#),
        )
        .await;
        assert_eq!(c_off, 502);
        {
            let gp = st_p_off.inner.lock().unwrap();
            assert!(gp.pomo_running);
            assert_eq!(gp.pomo_expected_cmd_seq, cmd1);
            assert_eq!(gp.pomo_cmd_seq, cmd1 + 1);
        }
        keyglassd::api::worker::reconcile_pomodoro_from_device(
            &st_p_off,
            Some(&serde_json::json!({
                "running": false,
                "remainingS": 999,
                "devRev": 3,
                "hostCmdSeq": cmd1,
                "devBootId": "boot-flaky",
            })),
        );
        assert!(
            !st_p_off.inner.lock().unwrap().pomo_running,
            "failed send must not block later physical-key reconciliation"
        );
        assert_eq!(st_p_off.inner.lock().unwrap().pomo_anchor_rem_s, 999);
    }

    let _ = std::fs::remove_dir_all(&tmp);
}
