//! Update discovery, keyboard updates, conversion and reset to factory, driven through the local
//! API against a signed test index and a simulated keyboard.

use openflux_update::compat::Compat;
use openflux_update::manifest::{Artifact, Channel, Index, Release};
use openflux_update::signing::{generate, GeneratedKey};
use openflux_update::{Component, TrustAnchors};
use openfluxd::api::build_router;
use openfluxd::api::state::{ApiState, DeviceSink};
use openfluxd::updates::{Timings, UpdateConfig, UpdateService};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TOKEN: &str = "updates_test_token";

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "openfluxd_updates_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sha256(data: &[u8]) -> String {
    openflux_update::trust::sha256_hex(data).unwrap().0
}

// ---------------------------------------------------------------------------------------------
// A signed test index
// ---------------------------------------------------------------------------------------------

struct Keys {
    root: GeneratedKey,
    release: GeneratedKey,
}

fn keys() -> Keys {
    Keys {
        root: generate().unwrap(),
        release: generate().unwrap(),
    }
}

fn anchors(k: &Keys) -> TrustAnchors {
    TrustAnchors {
        root_keys: vec![k.root.secret.public_key().clone()],
        release_keys: vec![k.release.secret.public_key().clone()],
    }
}

fn compat() -> Compat {
    Compat::parse(openfluxd::updates::COMPAT_TOML).unwrap()
}

fn artifact_name(c: Component, v: &str) -> String {
    match c {
        Component::Firmware => format!("openflux-firmware-{v}.uf2"),
        Component::Bridge => format!("openflux-bridge_{v}_aarch64.ipk"),
        Component::Display => format!("openflux-display_{v}_aarch64.ipk"),
        Component::Host => format!("openflux-host-{v}.tar.gz"),
    }
}

/// Writes a signed artifact into `dir` and returns its index entry.
fn artifact(k: &Keys, dir: &Path, name: &str, content: &[u8]) -> Artifact {
    std::fs::write(dir.join(name), content).unwrap();
    Artifact {
        name: name.into(),
        url: name.into(),
        sha256: sha256(content),
        size: content.len() as u64,
        signatures: vec![k.release.secret.sign_bytes(content, "artifact").unwrap()],
    }
}

/// A bridge package as the bridge's packaging builds it: an `ar` archive with `data.tar.gz`.
fn bridge_package(v: &str) -> Vec<u8> {
    let mut tar_gz = Vec::new();
    {
        let gz = flate2::write::GzEncoder::new(&mut tar_gz, flate2::Compression::default());
        let mut tar = tar::Builder::new(gz);
        let bin = format!("bridge binary {v}");
        for (path, data) in [
            ("./usr/lib/openflux/bridge/openflux-bridge", bin.as_bytes()),
            (
                "./usr/lib/openflux/bridge/openflux-autolaunch.sh",
                b"#!/bin/sh\nexec openflux-bridge\n".as_slice(),
            ),
        ] {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            tar.append_data(&mut h, path, data).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
    }
    let mut out = b"!<arch>\n".to_vec();
    for (name, payload) in [
        ("debian-binary", b"2.0\n".to_vec()),
        ("control.tar.gz", Vec::new()),
        ("data.tar.gz", tar_gz),
    ] {
        out.extend_from_slice(
            format!(
                "{name:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
                0,
                0,
                0,
                100644,
                payload.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(&payload);
        if payload.len() % 2 == 1 {
            out.push(b'\n');
        }
    }
    out
}

fn release(
    k: &Keys,
    dir: &Path,
    c: Component,
    v: &str,
    interfaces: Option<openflux_update::InterfaceSet>,
) -> Release {
    let version: semver::Version = v.parse().unwrap();
    let content = if c == Component::Bridge {
        bridge_package(v)
    } else {
        format!("{c} {v} contents").into_bytes()
    };
    let mut artifacts = vec![artifact(k, dir, &artifact_name(c, v), &content)];
    if c == Component::Bridge {
        artifacts.push(artifact(
            k,
            dir,
            &format!("openflux-factory-restore_{v}_aarch64.ipk"),
            b"restore trigger",
        ));
    }
    Release {
        schema: 1,
        component: c,
        channel: Channel::of(&version),
        released: time::OffsetDateTime::now_utc(),
        notes_url: format!("https://example.invalid/{c}/{v}"),
        interfaces: interfaces.unwrap_or_else(|| compat().interface_set(c)),
        artifacts,
        withdrawn: false,
        version,
    }
}

/// Signs `index` (plus extra top-level fields) and writes `index.json` and its signature to `dir`.
fn publish(k: &Keys, dir: &Path, releases: Vec<Release>, extra: Option<(&str, serde_json::Value)>) {
    let now = time::OffsetDateTime::now_utc();
    let mut by_channel: BTreeMap<Channel, Vec<Release>> = BTreeMap::new();
    for r in releases {
        by_channel.entry(r.channel).or_default().push(r);
    }
    let index = Index {
        schema: 1,
        sequence: 1,
        issued: now,
        expires: now + time::Duration::days(30),
        key_id: k.release.secret.id(),
        releases: by_channel,
        key_statements: Vec::new(),
        factory: None,
        press_refinement_source: None,
    };
    let mut doc: serde_json::Value = serde_json::from_str(&index.to_json()).unwrap();
    if let Some((key, v)) = extra {
        doc[key] = v;
    }
    let json = serde_json::to_vec_pretty(&doc).unwrap();
    std::fs::write(dir.join("index.json"), &json).unwrap();
    std::fs::write(
        dir.join("index.json.sig"),
        k.release.secret.sign_bytes(&json, "index").unwrap(),
    )
    .unwrap();
}

fn baseline(k: &Keys, dir: &Path) -> Vec<Release> {
    [Component::Firmware, Component::Bridge, Component::Display]
        .into_iter()
        .map(|c| release(k, dir, c, "1.0.0", None))
        .collect()
}

fn file_url(dir: &Path) -> String {
    format!("file://{}/index.json", dir.display())
}

// ---------------------------------------------------------------------------------------------
// A simulated keyboard
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Kb {
    versions: BTreeMap<String, Option<String>>,
    files: BTreeMap<String, Vec<u8>>,
    connected: (bool, bool),
    /// Every operation that changes the keyboard.
    writes: Vec<String>,
    /// Version the next installed bridge reports instead of its own.
    bridge_reports: Option<String>,
    /// Versions the factory restore package installs: (bridge, display).
    factory_versions: Option<(String, String)>,
    /// Version the firmware reports after flashing each file name.
    firmware_for: BTreeMap<String, String>,
    attempts: u32,
    /// Files written under this prefix are stored damaged.
    corrupt_prefix: Option<String>,
    /// The firmware's keymap blob, kept across firmware flashes like the rest of its storage.
    fw_keymap: Option<Vec<u8>>,
    /// What flashing firmware fails with, while set.
    flash_error: Option<String>,
    saved_plans: Vec<serde_json::Value>,
    /// The firmware's press-refinement storage, kept across firmware flashes.
    press: openfluxd::usb::press_refinement::FirmwareStore,
}

struct MockKeyboard {
    kb: Arc<Mutex<Kb>>,
    /// While set, an upload is applied and then never returns (a daemon that stops mid-step).
    hang_after_upload: Mutex<bool>,
    /// While set, uploads wait for it to clear.
    hold_uploads: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

impl MockKeyboard {
    fn new(versions: [(&str, &str); 3]) -> Arc<Self> {
        let mut kb = Kb {
            connected: (true, true),
            ..Default::default()
        };
        for (c, v) in versions {
            kb.versions.insert(c.into(), Some(v.into()));
        }
        Self::with(Arc::new(Mutex::new(kb)))
    }

    fn with(kb: Arc<Mutex<Kb>>) -> Arc<Self> {
        Arc::new(Self {
            kb,
            hang_after_upload: Mutex::new(false),
            hold_uploads: Arc::new((Mutex::new(false), std::sync::Condvar::new())),
        })
    }

    fn version(&self, c: &str) -> Option<String> {
        self.kb.lock().unwrap().versions.get(c).cloned().flatten()
    }

    fn writes(&self) -> Vec<String> {
        self.kb.lock().unwrap().writes.clone()
    }

    fn hold(&self, on: bool) {
        *self.hold_uploads.0.lock().unwrap() = on;
        self.hold_uploads.1.notify_all();
    }

    /// What the keyboard does with a package dropped into its install folder.
    fn install(kb: &Arc<Mutex<Kb>>, name: &str) {
        let kb = kb.clone();
        let name = name.to_string();
        let mut g = kb.lock().unwrap();
        if let Some(rest) = name.strip_prefix("openflux-bridge_") {
            let v = rest.split('_').next().unwrap().to_string();
            g.attempts += 1;
            let id = format!("{:04x}", 0xa000 + g.attempts);
            let target = format!("openflux-bridge {v}");
            g.files.insert(
                "/home/root/.openflux/rollback/bridge/attempt".into(),
                format!("attempt_id={id}\nversion={target}\n").into_bytes(),
            );
            g.files.insert(
                "/home/root/.openflux/rollback/bridge/armed".into(),
                Vec::new(),
            );
            let live = g.bridge_reports.clone().unwrap_or(target.clone());
            drop(g);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(30));
                let mut g = kb.lock().unwrap();
                g.versions.insert("bridge".into(), Some(live.clone()));
                g.files.insert(
                    format!("/home/root/.openflux/rollback/bridge/trial-{id}"),
                    format!("version={live}\npid=7\n").into_bytes(),
                );
            });
        } else if let Some(rest) = name.strip_prefix("openflux-display_") {
            let v = rest.split('_').next().unwrap().to_string();
            drop(g);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(30));
                kb.lock()
                    .unwrap()
                    .versions
                    .insert("display".into(), Some(format!("openflux-display {v}")));
            });
        } else if name.starts_with("openflux-factory-restore_") {
            let staged = g
                .files
                .contains_key("/home/root/.openflux/factory-restore/manifest.sha256");
            let versions = g.factory_versions.clone();
            drop(g);
            if let (true, Some((b, d))) = (staged, versions) {
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(30));
                    let mut g = kb.lock().unwrap();
                    g.versions.insert("bridge".into(), Some(b));
                    g.versions.insert("display".into(), Some(d));
                });
            }
        }
    }
}

impl DeviceSink for MockKeyboard {
    fn is_connected(&self) -> (bool, bool) {
        self.kb.lock().unwrap().connected
    }
    fn versions(&self) -> serde_json::Value {
        let g = self.kb.lock().unwrap();
        serde_json::json!({
            "firmware": g.versions.get("firmware").cloned().flatten(),
            "bridge": g.versions.get("bridge").cloned().flatten(),
            "display": g.versions.get("display").cloned().flatten(),
            "os": "2.1",
        })
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
    fn set_system_time(&self, _: &str, zone: &str) -> Result<(), String> {
        self.kb.lock().unwrap().writes.push(format!("time {zone}"));
        Ok(())
    }
    fn save_config_dir(&self, plan: &serde_json::Value) -> Result<(), String> {
        let mut g = self.kb.lock().unwrap();
        g.writes.push("save".into());
        g.saved_plans.push(plan.clone());
        if let Some(hex) = plan.get("keymapBlobHex").and_then(|v| v.as_str()) {
            g.fw_keymap = Some(unhex(hex));
        }
        for (path, key) in openfluxd::device_paths::CONFIG_DOCUMENTS {
            if let Some(doc) = plan.get(key) {
                g.files
                    .insert(path.into(), serde_json::to_vec_pretty(doc).unwrap());
            }
        }
        Ok(())
    }
    fn read_firmware_keymap(&self) -> Result<Vec<u8>, String> {
        self.kb
            .lock()
            .unwrap()
            .fw_keymap
            .clone()
            .ok_or_else(|| "NAK".into())
    }
    fn write_firmware_keymap(&self, blob: &[u8]) -> Result<(), String> {
        let mut g = self.kb.lock().unwrap();
        g.writes.push("write firmware keymap".into());
        g.fw_keymap = Some(blob.to_vec());
        Ok(())
    }
    fn pull_config_dir(&self) -> Result<serde_json::Value, String> {
        let g = self.kb.lock().unwrap();
        let mut out = serde_json::json!({"appearance": {"selectedId": "default"}});
        for (path, key) in openfluxd::device_paths::CONFIG_DOCUMENTS {
            if let Some(doc) = g
                .files
                .get(path)
                .and_then(|raw| serde_json::from_slice::<serde_json::Value>(raw).ok())
            {
                out[key] = doc;
            }
        }
        if let Some(b) = &g.fw_keymap {
            out["firmwareKeymapBlobHex"] =
                serde_json::json!(b.iter().map(|x| format!("{x:02x}")).collect::<String>());
        }
        Ok(out)
    }
    fn get_file(&self, path: &str) -> Result<Vec<u8>, String> {
        self.kb
            .lock()
            .unwrap()
            .files
            .get(path)
            .cloned()
            .ok_or_else(|| "NAK".into())
    }
    fn put_file(&self, path: &str, data: &[u8]) -> Result<(), String> {
        let mut g = self.kb.lock().unwrap();
        g.writes.push(format!("put {path}"));
        let mut stored = data.to_vec();
        if g.corrupt_prefix
            .as_ref()
            .is_some_and(|p| path.starts_with(p.as_str()))
        {
            if let Some(b) = stored.first_mut() {
                *b ^= 0xff;
            }
        }
        g.files.insert(path.into(), stored);
        Ok(())
    }
    fn stat_file(&self, path: &str) -> Result<bool, String> {
        Ok(self.kb.lock().unwrap().files.contains_key(path))
    }
    fn list_files(&self, dir: &str) -> Result<Vec<(String, u64)>, String> {
        let prefix = format!("{dir}/");
        Ok(self
            .kb
            .lock()
            .unwrap()
            .files
            .iter()
            .filter_map(|(p, d)| {
                p.strip_prefix(&prefix)
                    .filter(|n| !n.contains('/'))
                    .map(|n| (n.to_string(), d.len() as u64))
            })
            .collect())
    }
    fn sideload_ipk(&self, ipk_path: &str) -> Result<String, String> {
        {
            let (lock, cv) = &*self.hold_uploads;
            let mut held = lock.lock().unwrap();
            while *held {
                held = cv.wait(held).unwrap();
            }
        }
        let name = Path::new(ipk_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let data = std::fs::read(ipk_path).map_err(|e| e.to_string())?;
        {
            let mut g = self.kb.lock().unwrap();
            g.writes.push(format!("sideload {name}"));
            g.files.insert(format!("/home/root/{name}"), data);
        }
        Self::install(&self.kb, &name);
        if *self.hang_after_upload.lock().unwrap() {
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        Ok(format!("Uploaded {name}"))
    }
    fn flash_uf2(&self, uf2_path: &str) -> Result<String, String> {
        let name = Path::new(uf2_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let mut g = self.kb.lock().unwrap();
        g.writes.push(format!("flash {name}"));
        if let Some(e) = g.flash_error.clone() {
            return Err(e);
        }
        let v = g.firmware_for.get(&name).cloned().or_else(|| {
            name.strip_prefix("openflux-firmware-")
                .and_then(|r| r.strip_suffix(".uf2"))
                .map(|v| format!("openflux-firmware {v}"))
        });
        g.versions.insert("firmware".into(), v);
        Ok("flashed with the calibration region unchanged".into())
    }
    fn bridge_armed_sentinel_present(&self) -> Result<bool, String> {
        Ok(self
            .kb
            .lock()
            .unwrap()
            .files
            .contains_key("/home/root/.openflux/rollback/bridge/armed"))
    }
    fn confirm_bridge_install(&self) -> Result<(), String> {
        let mut g = self.kb.lock().unwrap();
        g.writes.push("confirm bridge".into());
        g.files.remove("/home/root/.openflux/rollback/bridge/armed");
        Ok(())
    }
    fn press_refinement_status(&self) -> Result<openfluxd::usb::press_refinement::Status, String> {
        let g = self.kb.lock().unwrap();
        if !g.connected.1 {
            return Err("Device not found".into());
        }
        Ok(g.press.status())
    }
    fn put_press_refinement(&self, record: &[u8]) -> Result<(), String> {
        let mut g = self.kb.lock().unwrap();
        g.writes.push("put press refinement".into());
        g.press.put(record).map_err(|n| n.0)
    }
    fn get_press_refinement(&self) -> Result<Vec<u8>, String> {
        self.kb.lock().unwrap().press.get().map_err(|n| n.0)
    }
}

fn unhex(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

/// OpenFlux's default keymap blob.
fn default_keymap_blob() -> Vec<u8> {
    openfluxd::codecs::keymap::encode_keymap_json_to_firmware_blob(
        &openfluxd::defaults::default_keymap(),
        None,
        None,
    )
    .unwrap()
}

/// Consumer-control bit indexes of the factory encoding and the HID usages OpenFlux stores.
const CONSUMER: [(u32, u32); 11] = [
    (0x01, 0x6F),
    (0x02, 0x70),
    (0x03, 0xB5),
    (0x04, 0xB6),
    (0x07, 0xCD),
    (0x08, 0xE2),
    (0x0B, 0xE9),
    (0x0C, 0xEA),
    (0x20, 0x29D),
    (0x21, 0x29F),
    (0x22, 0x2A0),
];

/// Every consumer word of a blob's layers rewritten with `map` (usage -> usage).
fn map_consumer(blob: &[u8], map: impl Fn(u32) -> Option<u32>) -> Vec<u8> {
    let mut km = openfluxd::codecs::keymap::decode_keymap_blob(blob).unwrap();
    for s in &mut km.keymaps {
        for l in &mut s.layers {
            for w in l.iter_mut() {
                if *w >> 28 == 4 {
                    if let Some(u) = map(*w & 0xFFFF) {
                        *w = (*w & !0xFFFF) | u;
                    }
                }
            }
        }
    }
    km.to_bytes().unwrap()
}

fn consumer_words(blob: &[u8]) -> Vec<u32> {
    let km = openfluxd::codecs::keymap::decode_keymap_blob(blob).unwrap();
    km.keymaps
        .iter()
        .flat_map(|s| s.layers.iter().flatten().copied())
        .filter(|w| *w >> 28 == 4)
        .collect()
}

fn keymap_blob(words: Vec<u32>) -> Vec<u8> {
    use openfluxd::codecs::keymap::{FirmwareKeymapBlob, KeymapSection};
    let n = words.len() as u32;
    FirmwareKeymapBlob {
        num_keymaps: 1,
        layout_bitmask: 1,
        layout_offset_0: 0,
        layout_offset_1: 0,
        layout_offset_2: 0,
        layout_offset_3: 0,
        keymaps: vec![KeymapSection {
            num_actuation_modes: 1,
            num_keys: n,
            num_layers: 1,
            reserved: [0; 3],
            layers: vec![words],
            actuation_tables: vec![vec![0; n as usize]],
            macros: Vec::new(),
        }],
        trailing_words: Vec::new(),
    }
    .to_bytes()
    .unwrap()
}

const OPENFLUX_1_0_0: [(&str, &str); 3] = [
    ("firmware", "openflux-firmware 1.0.0"),
    ("bridge", "openflux-bridge 1.0.0"),
    ("display", "openflux-display 1.0.0"),
];

// ---------------------------------------------------------------------------------------------
// The daemon under test
// ---------------------------------------------------------------------------------------------

struct Daemon {
    addr: std::net::SocketAddr,
    state: Arc<ApiState>,
    updates: Arc<UpdateService>,
}

fn config(data: &Path, index_url: String, bundled: Option<PathBuf>, k: &Keys) -> UpdateConfig {
    UpdateConfig {
        data_dir: data.to_path_buf(),
        index_url,
        bundled_dir: bundled,
        anchors: anchors(k),
        check_interval: Duration::from_secs(3600),
        timings: Timings {
            install_wait: Duration::from_secs(3),
            reconnect_wait: Duration::from_millis(500),
            poll: Duration::from_millis(10),
        },
        factory_container_url: None,
        // Nothing listens here, so a press-refinement container is unreachable unless a test serves one.
        press_refinement_container_url: Some("http://127.0.0.1:9/unreachable.msix".into()),
    }
}

async fn daemon(device: Arc<MockKeyboard>, config: UpdateConfig) -> Daemon {
    let state = ApiState::with_dirs(
        device.clone(),
        Some(TOKEN.into()),
        Duration::from_secs(20),
        config.data_dir.join("icons"),
    );
    state.set_registration_secret("updates_secret");
    let updates = UpdateService::new(config, device, state.lifecycle.clone());
    state.set_updates(updates.clone());
    {
        let u = updates.clone();
        tokio::task::spawn_blocking(move || u.check())
            .await
            .unwrap();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Daemon {
        addr,
        state,
        updates,
    }
}

async fn http(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, serde_json::Value) {
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    let payload = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nAuthorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut resp = Vec::new();
    s.read_to_end(&mut resp).await.unwrap();
    let txt = String::from_utf8_lossy(&resp).to_string();
    let (head, body) = txt.split_once("\r\n\r\n").unwrap_or((&txt, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (
        status,
        serde_json::from_str(body).unwrap_or(serde_json::Value::Null),
    )
}

async fn wait_job(d: &Daemon, id: &str) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let (code, job) = http(d.addr, "GET", &format!("/v1/jobs/{id}"), None).await;
        assert_eq!(code, 200, "{job}");
        if job["state"] != "running" {
            return job;
        }
        assert!(Instant::now() < deadline, "job still running: {job}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn start_job(d: &Daemon, path: &str, body: &str) -> String {
    let (code, resp) = http(d.addr, "POST", path, Some(body)).await;
    assert_eq!(code, 202, "{resp}");
    resp["jobId"].as_str().unwrap().to_string()
}

fn component<'a>(status: &'a serde_json::Value, c: &str) -> &'a serde_json::Value {
    status["keyboard"]["components"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["component"] == c)
        .unwrap()
}

// ---------------------------------------------------------------------------------------------
// Discovery and status
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn an_up_to_date_keyboard_offers_nothing() {
    let tmp = temp_dir("uptodate");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    publish(&k, &site, baseline(&k, &site), None);
    let kb = MockKeyboard::new(OPENFLUX_1_0_0);
    let d = daemon(kb, config(&tmp.join("data"), file_url(&site), None, &k)).await;

    let (code, s) = http(d.addr, "GET", "/v1/updates", None).await;
    assert_eq!(code, 200);
    assert_eq!(s["indexSource"], "online");
    assert_eq!(s["error"], serde_json::Value::Null);
    assert!(s["checkedAt"].is_string());
    assert_eq!(s["channel"], "stable");
    assert_eq!(s["host"]["installed"], "1.0.0");
    assert_eq!(s["keyboard"]["mode"], "openflux");
    for c in ["firmware", "bridge", "display"] {
        assert_eq!(component(&s, c)["state"], "upToDate", "{s}");
        assert_eq!(component(&s, c)["kind"], "openflux");
        assert_eq!(component(&s, c)["installed"], "1.0.0");
    }
    assert_eq!(s["keyboard"]["plan"], serde_json::Value::Null);
    let (code, _) = http(d.addr, "POST", "/v1/updates/apply", Some("{}")).await;
    assert_eq!(code, 409, "nothing to apply");

    let (code, s) = http(
        d.addr,
        "PUT",
        "/v1/updates/settings",
        Some(r#"{"channel":"beta","autoCheck":false}"#),
    )
    .await;
    assert_eq!(
        (code, s["channel"].clone(), s["autoCheck"].clone()),
        (200, "beta".into(), false.into())
    );
    let (code, _) = http(
        d.addr,
        "PUT",
        "/v1/updates/settings",
        Some(r#"{"channel":"nightly"}"#),
    )
    .await;
    assert_eq!(code, 400);
    let (_, dev) = http(d.addr, "GET", "/v1/device/status", None).await;
    assert_eq!(dev["compatibility"]["config"], "ok", "{dev}");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn an_available_update_is_planned_and_installed() {
    let tmp = temp_dir("apply");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let mut releases = baseline(&k, &site);
    releases.push(release(&k, &site, Component::Bridge, "1.0.1", None));
    releases.push(release(&k, &site, Component::Display, "1.0.1", None));
    publish(&k, &site, releases, None);
    let kb = MockKeyboard::new(OPENFLUX_1_0_0);
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;

    let (_, s) = http(d.addr, "GET", "/v1/updates", None).await;
    assert_eq!(component(&s, "bridge")["state"], "updateAvailable", "{s}");
    assert_eq!(component(&s, "bridge")["latest"], "1.0.1");
    assert_eq!(
        component(&s, "bridge")["notesUrl"],
        "https://example.invalid/bridge/1.0.1"
    );
    assert_eq!(component(&s, "firmware")["state"], "upToDate");
    let steps = s["keyboard"]["plan"]["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2, "{s}");

    let id = start_job(&d, "/v1/updates/apply", "{}").await;
    let job = wait_job(&d, &id).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    assert_eq!(job["kind"], "update");
    assert_eq!(job["progress"], 1.0);
    assert_eq!(
        kb.version("bridge").as_deref(),
        Some("openflux-bridge 1.0.1")
    );
    assert_eq!(
        kb.version("display").as_deref(),
        Some("openflux-display 1.0.1")
    );
    let writes = kb.writes();
    assert!(writes.contains(&"confirm bridge".to_string()), "{writes:?}");
    assert!(
        !kb.bridge_armed_sentinel_present().unwrap(),
        "the new bridge is confirmed"
    );

    let (_, s) = http(d.addr, "GET", "/v1/updates", None).await;
    assert_eq!(component(&s, "bridge")["state"], "upToDate", "{s}");
    assert_eq!(s["job"]["id"], id);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn a_tampered_artifact_is_refused_before_anything_reaches_the_keyboard() {
    let tmp = temp_dir("tampered");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let mut releases = baseline(&k, &site);
    releases.push(release(&k, &site, Component::Bridge, "1.0.1", None));
    releases.push(release(&k, &site, Component::Display, "1.0.1", None));
    publish(&k, &site, releases, None);
    // The display package is replaced after it was signed.
    std::fs::write(
        site.join(artifact_name(Component::Display, "1.0.1")),
        b"display 1.0.1 contentX",
    )
    .unwrap();
    let kb = MockKeyboard::new(OPENFLUX_1_0_0);
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;

    let id = start_job(&d, "/v1/updates/apply", "{}").await;
    let job = wait_job(&d, &id).await;
    assert_eq!(job["state"], "failed");
    assert_eq!(job["error"]["code"], "verification", "{job}");
    assert!(job["error"]["recovery"]
        .as_str()
        .unwrap()
        .contains("Nothing was installed"));
    assert!(
        kb.writes().is_empty(),
        "no keyboard write: {:?}",
        kb.writes()
    );
    assert_eq!(
        kb.version("bridge").as_deref(),
        Some("openflux-bridge 1.0.0")
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn a_component_that_comes_back_with_the_wrong_version_fails_the_job_with_a_recovery() {
    let tmp = temp_dir("mismatch");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let mut releases = baseline(&k, &site);
    releases.push(release(&k, &site, Component::Bridge, "1.0.1", None));
    releases.push(release(&k, &site, Component::Display, "1.0.1", None));
    publish(&k, &site, releases, None);
    let kb = MockKeyboard::new(OPENFLUX_1_0_0);
    kb.kb.lock().unwrap().bridge_reports = Some("openflux-bridge 1.0.0".into());
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;

    let id = start_job(&d, "/v1/updates/apply", "{}").await;
    let job = wait_job(&d, &id).await;
    assert_eq!(job["state"], "failed");
    assert_eq!(job["error"]["code"], "versionMismatch", "{job}");
    assert!(
        job["error"]["recovery"]
            .as_str()
            .unwrap()
            .contains("restores its previous bridge"),
        "{job}"
    );
    let writes = kb.writes();
    assert!(
        !writes.contains(&"confirm bridge".to_string()),
        "a bridge that isn't the new one is never confirmed: {writes:?}"
    );
    assert!(
        !writes.iter().any(|w| w.contains("openflux-display")),
        "later steps don't run: {writes:?}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn only_a_changed_calibration_region_is_reported_as_a_calibration_problem() {
    let refused_image = "UF2 block 3 overlaps protected LittleFS calibration/keymap region";
    for (flash_error, code) in [
        (refused_image, "installFailed"),
        (
            openfluxd::usb::picoboot::CALIBRATION_CHANGED,
            "calibrationChanged",
        ),
    ] {
        let tmp = temp_dir("flash_error");
        let k = keys();
        let site = tmp.join("site");
        std::fs::create_dir_all(&site).unwrap();
        let mut releases = baseline(&k, &site);
        releases.push(release(&k, &site, Component::Firmware, "1.0.1", None));
        publish(&k, &site, releases, None);
        let kb = MockKeyboard::new(OPENFLUX_1_0_0);
        kb.kb.lock().unwrap().flash_error = Some(flash_error.into());
        let d = daemon(
            kb.clone(),
            config(&tmp.join("data"), file_url(&site), None, &k),
        )
        .await;

        let id = start_job(&d, "/v1/updates/apply", "{}").await;
        let job = wait_job(&d, &id).await;
        assert_eq!(job["state"], "failed", "{job}");
        assert_eq!(job["error"]["code"], code, "{job}");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

#[tokio::test]
async fn a_keyboard_that_needs_conversion_is_offered_only_conversion() {
    let tmp = temp_dir("needsconv");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let mut releases = baseline(&k, &site);
    releases.push(release(&k, &site, Component::Bridge, "1.0.1", None));
    publish(&k, &site, releases, None);
    let kb = MockKeyboard::new([
        ("firmware", "1.4.12"),
        ("bridge", "openflux-bridge 1.0.0"),
        ("display", "openflux-display 1.0.0"),
    ]);
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;

    let (_, s) = http(d.addr, "GET", "/v1/updates", None).await;
    assert_eq!(s["keyboard"]["mode"], "needsConversion", "{s}");
    assert_eq!(component(&s, "firmware")["kind"], "factory");
    for c in ["firmware", "bridge", "display"] {
        assert_eq!(component(&s, c)["state"], "conversionRequired", "{s}");
    }
    assert_eq!(s["keyboard"]["plan"], serde_json::Value::Null);
    let install: Vec<String> = s["keyboard"]["conversion"]["install"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            format!(
                "{} {}",
                i["component"].as_str().unwrap(),
                i["version"].as_str().unwrap()
            )
        })
        .collect();
    assert_eq!(install, ["firmware 1.0.0", "bridge 1.0.1", "display 1.0.0"]);

    let (code, body) = http(
        d.addr,
        "POST",
        "/v1/updates/apply",
        Some(r#"{"components":["bridge"]}"#),
    )
    .await;
    assert_eq!(code, 409);
    assert_eq!(body["error"]["code"], "conversionRequired");
    let (code, _) = http(d.addr, "POST", "/v1/config/save", Some("{}")).await;
    assert_eq!(code, 409, "ordinary keyboard writes are refused");
    assert!(kb.writes().is_empty());
    let (_, conv) = http(d.addr, "GET", "/v1/conversion", None).await;
    assert_eq!(conv["canConvert"], true, "{conv}");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn conversion_installs_openflux_and_imports_the_existing_settings() {
    let tmp = temp_dir("convert");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    publish(&k, &site, baseline(&k, &site), None);
    let kb = MockKeyboard::new([
        ("firmware", "1.4.12"),
        ("bridge", "1.4.11"),
        ("display", "1.4.28"),
    ]);
    {
        let mut g = kb.kb.lock().unwrap();
        g.files.insert(
            "/home/root/flux/config/appearance.json".into(),
            br#"{"selectedId":"mine"}"#.to_vec(),
        );
        g.files.insert(
            "/home/root/flux/config/settings.json".into(),
            br#"{"brightness":3}"#.to_vec(),
        );
        g.files
            .insert("/home/root/flux/media/bg.mp4".into(), b"VIDEO".to_vec());
        g.files.insert(
            "/home/root/flux/media/key_icons/a.png".into(),
            b"ICON".to_vec(),
        );
        g.files.insert(
            "/home/root/flux/config/keymap.json".into(),
            br#"{"keymaps":{}}"#.to_vec(),
        );
        // The keyboard's firmware keymap stores volume up as a consumer bit index.
        g.fw_keymap = Some(keymap_blob(vec![0x04, 0x4000_000B]));
    }
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;

    let (code, body) = http(d.addr, "POST", "/v1/conversion/convert", Some("{}")).await;
    assert_eq!(
        (code, body["error"]["code"].clone()),
        (400, "confirmationRequired".into())
    );
    assert!(kb.writes().is_empty());

    let id = start_job(&d, "/v1/conversion/convert", r#"{"confirm":true}"#).await;
    let job = wait_job(&d, &id).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    assert_eq!(job["kind"], "convert");
    for (c, v) in OPENFLUX_1_0_0 {
        assert_eq!(kb.version(c).as_deref(), Some(v));
    }
    let g = kb.kb.lock().unwrap();
    assert_eq!(
        g.files["/home/root/.openflux/rescue/openflux-bridge"],
        b"bridge binary 1.0.0"
    );
    let manifest =
        String::from_utf8(g.files["/home/root/.openflux/rescue/manifest.sha256"].clone()).unwrap();
    assert!(
        manifest.contains(&format!(
            "{}  openflux-bridge\n",
            sha256(b"bridge binary 1.0.0")
        )),
        "{manifest}"
    );
    assert!(
        manifest.contains("  openflux-autolaunch.sh\n"),
        "{manifest}"
    );
    let rescue_staged = g
        .writes
        .iter()
        .position(|w| w == "put /home/root/.openflux/rescue/manifest.sha256")
        .unwrap();
    let first_install = g
        .writes
        .iter()
        .position(|w| w.starts_with("flash") || w.starts_with("sideload"))
        .unwrap();
    assert!(
        rescue_staged < first_install,
        "the rescue pair is staged before anything is installed: {:?}",
        g.writes
    );
    assert_eq!(
        g.files["/home/root/openflux/config/appearance.json"],
        br#"{"selectedId":"mine"}"#
    );
    assert_eq!(
        g.files["/home/root/openflux/config/settings.json"],
        br#"{"brightness":3}"#
    );
    assert_eq!(g.files["/home/root/openflux/media/bg.mp4"], b"VIDEO");
    assert_eq!(
        g.files["/home/root/openflux/media/key_icons/a.png"],
        b"ICON"
    );
    let order: Vec<&String> = g
        .writes
        .iter()
        .filter(|w| {
            w.starts_with("flash") || w.starts_with("sideload") || w.contains("openflux/config")
        })
        .collect();
    assert!(
        order[0].starts_with("flash openflux-firmware-1.0.0"),
        "{order:?}"
    );
    assert!(
        order[1].starts_with("sideload openflux-bridge_1.0.0"),
        "{order:?}"
    );
    assert!(
        order[2].starts_with("sideload openflux-display_1.0.0"),
        "{order:?}"
    );
    assert!(
        order[3].contains("openflux/config"),
        "settings are imported last: {order:?}"
    );
    assert!(g.writes.contains(&"confirm bridge".to_string()));
    let plan = g
        .saved_plans
        .last()
        .expect("the keymap is saved with the firmware keymap");
    assert_eq!(plan["keymap"], serde_json::json!({"keymaps": {}}));
    let blob = unhex(plan["keymapBlobHex"].as_str().unwrap());
    let layer = openfluxd::codecs::keymap::decode_keymap_blob(&blob)
        .unwrap()
        .keymaps[0]
        .layers[0]
        .clone();
    assert_eq!(
        layer,
        vec![0x04, 0x4000_00E9],
        "volume up is the HID consumer usage"
    );
    drop(g);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn a_rescue_pair_that_doesnt_read_back_stops_the_conversion_before_anything_is_installed() {
    let tmp = temp_dir("rescue_tampered");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    publish(&k, &site, baseline(&k, &site), None);
    let kb = MockKeyboard::new([
        ("firmware", "1.4.12"),
        ("bridge", "1.4.11"),
        ("display", "1.4.28"),
    ]);
    kb.kb.lock().unwrap().corrupt_prefix = Some("/home/root/.openflux/rescue/".into());
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;
    let id = start_job(&d, "/v1/conversion/convert", r#"{"confirm":true}"#).await;
    let job = wait_job(&d, &id).await;
    assert_eq!(job["state"], "failed");
    assert_eq!(job["error"]["code"], "verification", "{job}");
    let writes = kb.writes();
    assert!(
        !writes
            .iter()
            .any(|w| w.starts_with("flash") || w.starts_with("sideload")),
        "{writes:?}"
    );
    assert_eq!(kb.version("firmware").as_deref(), Some("1.4.12"));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn offline_the_bundled_index_still_describes_and_installs_updates() {
    let tmp = temp_dir("offline");
    let k = keys();
    let bundle = tmp.join("bundle");
    std::fs::create_dir_all(&bundle).unwrap();
    let mut releases = baseline(&k, &bundle);
    releases.push(release(&k, &bundle, Component::Bridge, "1.0.1", None));
    publish(&k, &bundle, releases, None);
    let closed = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let kb = MockKeyboard::new(OPENFLUX_1_0_0);
    let d = daemon(
        kb.clone(),
        config(
            &tmp.join("data"),
            format!("http://{closed}/index.json"),
            Some(bundle.clone()),
            &k,
        ),
    )
    .await;

    let (_, s) = http(d.addr, "GET", "/v1/updates", None).await;
    assert_eq!(s["indexSource"], "bundled", "{s}");
    assert_eq!(s["error"]["code"], "offline", "{s}");
    assert_eq!(component(&s, "bridge")["state"], "updateAvailable");
    let id = start_job(&d, "/v1/updates/apply", r#"{"components":["bridge"]}"#).await;
    let job = wait_job(&d, &id).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    assert_eq!(
        kb.version("bridge").as_deref(),
        Some("openflux-bridge 1.0.1")
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn an_index_signed_by_an_unknown_key_is_refused_and_the_bundled_one_is_used() {
    let tmp = temp_dir("untrusted");
    let k = keys();
    let stranger = keys();
    let site = tmp.join("site");
    let bundle = tmp.join("bundle");
    std::fs::create_dir_all(&site).unwrap();
    std::fs::create_dir_all(&bundle).unwrap();
    let mut releases = baseline(&stranger, &site);
    releases.push(release(&stranger, &site, Component::Bridge, "9.0.0", None));
    publish(&stranger, &site, releases, None);
    publish(&k, &bundle, baseline(&k, &bundle), None);
    let kb = MockKeyboard::new(OPENFLUX_1_0_0);
    let d = daemon(
        kb,
        config(&tmp.join("data"), file_url(&site), Some(bundle), &k),
    )
    .await;
    let (_, s) = http(d.addr, "GET", "/v1/updates", None).await;
    assert_eq!(s["error"]["code"], "untrusted", "{s}");
    assert_eq!(s["indexSource"], "bundled");
    assert_eq!(
        component(&s, "bridge")["state"],
        "upToDate",
        "nothing from the refused index is offered"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------------------------
// Jobs, the busy lease and restarts
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_stop_during_a_job_waits_for_every_step_and_refuses_other_work() {
    let tmp = temp_dir("deferred");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let mut releases = baseline(&k, &site);
    releases.push(release(&k, &site, Component::Bridge, "1.0.1", None));
    releases.push(release(&k, &site, Component::Display, "1.0.1", None));
    publish(&k, &site, releases, None);
    let kb = MockKeyboard::new(OPENFLUX_1_0_0);
    kb.hold(true);
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;

    let id = start_job(&d, "/v1/updates/apply", "{}").await;
    // Studio checks before installing a host update: the daemon is busy.
    let (code, life) = http(
        d.addr,
        "POST",
        "/v1/daemon/lifecycle",
        Some(r#"{"action":"status"}"#),
    )
    .await;
    assert_eq!(code, 200);
    assert_eq!(life["busy"], serde_json::json!(["updating the keyboard"]));
    let (_, life) = http(
        d.addr,
        "POST",
        "/v1/daemon/lifecycle",
        Some(r#"{"action":"stop"}"#),
    )
    .await;
    assert_eq!(life["outcome"], "deferred");
    assert_eq!(d.state.lifecycle.exit_due(), None);

    // Unrelated keyboard work is refused while the job runs and the stop is pending.
    let (code, _) = http(d.addr, "POST", "/v1/config/save", Some("{}")).await;
    assert_eq!(code, 409);
    let (code, body) = http(d.addr, "POST", "/v1/updates/apply", Some("{}")).await;
    assert_eq!((code, body["error"]["code"].clone()), (409, "busy".into()));
    // So are commands already queued for the USB worker, rather than interleaved with the job.
    let (tx, rx) = tokio::sync::oneshot::channel();
    let ran = openfluxd::api::worker::handle_usb_cmd(
        &d.state,
        &*kb,
        openfluxd::api::worker::UsbCommand::SendLiveJson {
            payload: serde_json::json!({}),
            reply: tx,
        },
    );
    assert!(!ran);
    assert!(rx.await.unwrap().unwrap_err().contains("being updated"));

    // The admitted job finishes all of its steps, then the daemon may exit.
    kb.hold(false);
    let job = wait_job(&d, &id).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    assert_eq!(
        kb.version("display").as_deref(),
        Some("openflux-display 1.0.1")
    );
    assert_eq!(
        d.state.lifecycle.wait_exit_due(Duration::from_secs(2)),
        Some(openfluxd::lifecycle::ExitKind::Stop)
    );
    assert!(!kb.writes().contains(&"save".to_string()));
    let _ = std::fs::remove_dir_all(&tmp);
}

/// A host with nothing to report.
struct QuietHost;

impl openfluxd::bridges::OsSensorBridge for QuietHost {
    fn sample_hardware(&mut self) -> openfluxd::bridges::HardwareMetrics {
        openfluxd::bridges::HardwareMetrics {
            cpu_pct: 0.0,
            mem_used_gb: 0.0,
            mem_total_gb: 0.0,
            gpu_pct: 0.0,
            vram_used_gb: None,
            vram_total_gb: None,
            gpu_unified_mem: false,
            net_down: String::new(),
            net_up: String::new(),
        }
    }
    fn active_window(&mut self) -> Option<openfluxd::bridges::ActiveWindowInfo> {
        None
    }
    fn active_language_code(&mut self) -> Option<u32> {
        None
    }
    fn media_snapshot(&mut self) -> openfluxd::bridges::MediaSnapshot {
        openfluxd::bridges::MediaSnapshot::default()
    }
}

#[tokio::test]
async fn the_keyboard_clock_waits_for_a_running_job_and_for_conversion() {
    let clock_writes = |kb: &MockKeyboard| {
        kb.writes()
            .into_iter()
            .filter(|w| w.starts_with("time "))
            .count()
    };
    let tick = |d: &Daemon, kb: &MockKeyboard, w: &mut openfluxd::api::worker::WorkerLoopState| {
        openfluxd::api::worker::reconcile_tick_with_state(&d.state, &mut QuietHost, kb, 2, w);
    };

    // While a job has the keyboard, its clock isn't touched; it is set once the job finishes.
    let tmp = temp_dir("clock_job");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let mut releases = baseline(&k, &site);
    releases.push(release(&k, &site, Component::Display, "1.0.1", None));
    publish(&k, &site, releases, None);
    let kb = MockKeyboard::new(OPENFLUX_1_0_0);
    kb.hold(true);
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;
    let id = start_job(&d, "/v1/updates/apply", "{}").await;
    let mut w = openfluxd::api::worker::WorkerLoopState::default();
    tick(&d, &kb, &mut w);
    assert_eq!(clock_writes(&kb), 0, "{:?}", kb.writes());
    kb.hold(false);
    assert_eq!(wait_job(&d, &id).await["state"], "succeeded");
    tick(&d, &kb, &mut w);
    assert_eq!(clock_writes(&kb), 1, "{:?}", kb.writes());
    let _ = std::fs::remove_dir_all(&tmp);

    // A keyboard that needs conversion is left alone.
    let tmp = temp_dir("clock_conversion");
    let kb = MockKeyboard::new([
        ("firmware", "1.4.12"),
        ("bridge", "1.4.11"),
        ("display", "1.4.28"),
    ]);
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;
    let mut w = openfluxd::api::worker::WorkerLoopState::default();
    tick(&d, &kb, &mut w);
    tick(&d, &kb, &mut w);
    assert_eq!(clock_writes(&kb), 0, "{:?}", kb.writes());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn a_signal_or_tray_quit_between_job_steps_lets_the_job_finish_before_exit() {
    use std::sync::atomic::{AtomicBool, Ordering};
    for via_signal in [true, false] {
        let tmp = temp_dir("quit");
        let k = keys();
        let site = tmp.join("site");
        std::fs::create_dir_all(&site).unwrap();
        let mut releases = baseline(&k, &site);
        releases.push(release(&k, &site, Component::Bridge, "1.0.1", None));
        releases.push(release(&k, &site, Component::Display, "1.0.1", None));
        publish(&k, &site, releases, None);
        let kb = MockKeyboard::new(OPENFLUX_1_0_0);
        kb.hold(true);
        let d = daemon(
            kb.clone(),
            config(&tmp.join("data"), file_url(&site), None, &k),
        )
        .await;
        let signalled = Arc::new(AtomicBool::new(false));
        let running = Arc::new(AtomicBool::new(true));
        let watcher = {
            let (lc, s, r) = (
                d.state.lifecycle.clone(),
                signalled.clone(),
                running.clone(),
            );
            std::thread::spawn(move || openfluxd::lifecycle::watch_exit(&lc, &s, &r))
        };
        let id = start_job(&d, "/v1/updates/apply", "{}").await;
        if via_signal {
            signalled.store(true, Ordering::SeqCst);
        } else {
            // What the tray's Quit does.
            d.state
                .lifecycle
                .request_exit(openfluxd::lifecycle::ExitKind::Stop);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            running.load(Ordering::SeqCst),
            "the daemon keeps running while the job does"
        );
        kb.hold(false);
        let job = wait_job(&d, &id).await;
        assert_eq!(job["state"], "succeeded", "{job}");
        watcher.join().unwrap();
        assert!(!running.load(Ordering::SeqCst));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

#[tokio::test]
async fn a_job_interrupted_by_a_daemon_restart_resumes_from_its_journal() {
    let tmp = temp_dir("restart");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let mut releases = baseline(&k, &site);
    releases.push(release(&k, &site, Component::Bridge, "1.0.1", None));
    releases.push(release(&k, &site, Component::Display, "1.0.1", None));
    publish(&k, &site, releases, None);
    let kb = MockKeyboard::new(OPENFLUX_1_0_0);
    *kb.hang_after_upload.lock().unwrap() = true;
    let data = tmp.join("data");
    let first = daemon(kb.clone(), config(&data, file_url(&site), None, &k)).await;
    let id = start_job(&first, "/v1/updates/apply", "{}").await;
    // The first daemon stops in the middle of the bridge step: the package is on the keyboard.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !kb
        .writes()
        .iter()
        .any(|w| w.starts_with("sideload openflux-bridge"))
    {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // A second daemon on the same data directory and keyboard.
    let kb2 = MockKeyboard::with(kb.kb.clone());
    let second = daemon(kb2.clone(), config(&data, file_url(&site), None, &k)).await;
    let (_, s) = http(second.addr, "GET", "/v1/updates", None).await;
    assert_eq!(s["job"]["id"], id, "the interrupted job is reported");
    second.updates.start();
    let job = wait_job(&second, &id).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    assert!(
        job["log"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().unwrap().contains("restarted")),
        "{job}"
    );
    let writes = kb.writes();
    assert_eq!(
        writes
            .iter()
            .filter(|w| w.starts_with("sideload openflux-bridge"))
            .count(),
        1,
        "the bridge isn't installed twice: {writes:?}"
    );
    assert!(writes.contains(&"confirm bridge".to_string()), "{writes:?}");
    assert_eq!(
        kb.version("display").as_deref(),
        Some("openflux-display 1.0.1")
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------------------------
// Compatibility outside the updater
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn a_keyboard_with_a_newer_data_format_is_read_only_for_that_area() {
    let tmp = temp_dir("readonly");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let mut display = compat().interface_set(Component::Display);
    display
        .provides
        .insert("config".into(), vec!["1.1".parse().unwrap()]);
    let mut releases = baseline(&k, &site);
    releases.push(release(
        &k,
        &site,
        Component::Display,
        "1.1.0",
        Some(display),
    ));
    publish(&k, &site, releases, None);
    let kb = MockKeyboard::new([
        ("firmware", "openflux-firmware 1.0.0"),
        ("bridge", "openflux-bridge 1.0.0"),
        ("display", "openflux-display 1.1.0"),
    ]);
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;
    {
        let mut g = d.state.inner.lock().unwrap();
        g.cached_connected = (true, true);
    }
    let (_, dev) = http(d.addr, "GET", "/v1/device/status", None).await;
    assert_eq!(dev["compatibility"]["config"], "readOnly", "{dev}");
    assert_eq!(dev["compatibility"]["keymap"], "ok");
    let (code, body) = http(d.addr, "POST", "/v1/config/save", Some("{}")).await;
    assert_eq!(code, 409, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("newer format"),
        "{body}"
    );
    assert!(!kb.writes().contains(&"save".to_string()));
    let (code, _) = http(d.addr, "GET", "/v1/config", None).await;
    assert_eq!(code, 200, "reading still works");
    let (_, s) = http(d.addr, "GET", "/v1/updates", None).await;
    assert_eq!(component(&s, "display")["state"], "newerThanHost", "{s}");
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------------------------
// Reset to factory
// ---------------------------------------------------------------------------------------------

/// Bytes that don't compress.
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// A ZIP64 container holding the factory files under an application-package layout.
fn factory_container(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .large_file(true);
        zip.start_file("AppxManifest.xml", opts).unwrap();
        zip.write_all(&noise(300_000, 1)).unwrap();
        for (name, data) in files {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }
    buf.into_inner()
}

/// Serves `data` over HTTP, answering range requests when `ranges` is set. Returns the URL and
/// the requests seen.
fn serve_container(data: Vec<u8>, ranges: bool) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://{}/factory/package.msix",
        listener.local_addr().unwrap()
    );
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let data = data.clone();
            let log = log.clone();
            std::thread::spawn(move || {
                let mut req = Vec::new();
                let mut b = [0u8; 1];
                while !req.ends_with(b"\r\n\r\n") && s.read(&mut b).unwrap_or(0) == 1 {
                    req.push(b[0]);
                }
                let req = String::from_utf8_lossy(&req).to_string();
                let ua = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("user-agent:"))
                    .unwrap_or("")
                    .to_string();
                let range = req
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("range:"))
                    .and_then(|l| l.split_once("bytes=").map(|(_, r)| r.trim().to_string()));
                log.lock()
                    .unwrap()
                    .push(format!("{} | {}", range.clone().unwrap_or_default(), ua));
                let (head, body) = match (ranges, range) {
                    (true, Some(r)) => {
                        let (a, z) = r.split_once('-').unwrap();
                        let a: usize = a.parse().unwrap();
                        let z: usize = z.parse::<usize>().unwrap().min(data.len() - 1);
                        (
                            format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {a}-{z}/{}\r\nConnection: close\r\n\r\n",
                                z - a + 1,
                                data.len()
                            ),
                            data[a..=z].to_vec(),
                        )
                    }
                    _ => (
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            data.len()
                        ),
                        data.clone(),
                    ),
                };
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&body);
            });
        }
    });
    (url, seen)
}

/// The factory container's URL, the requests it saw, and the factory bridge and display packages.
type FactorySite = (String, Arc<Mutex<Vec<String>>>, Vec<u8>, Vec<u8>);

/// Publishes a signed index with the baseline releases and a factory reference, and serves the
/// factory container.
fn factory_site(k: &Keys, site: &Path, ranges: bool) -> FactorySite {
    let fw = noise(4096, 2);
    let bridge_pkg = b"factory bridge package".to_vec();
    let display_pkg = b"factory display package".to_vec();
    let members = [
        ("data/assets/firmware/factory_fw.uf2", &fw[..]),
        ("data/assets/soc/fb_1.4.11_aarch64.ipk", &bridge_pkg[..]),
        ("data/assets/soc/fd_1.4.28_aarch64.ipk", &display_pkg[..]),
    ];
    let container = factory_container(&members);
    let size = container.len();
    let (url, seen) = serve_container(container, ranges);
    let file = |component: &str, member: &str, version: &str, data: &[u8]| serde_json::json!({"component": component, "member": member, "version": version, "size": data.len(), "sha256": sha256(data)});
    let factory = serde_json::json!({
        "label": "Factory software 1.4",
        "container_url": "https://downloads.example.invalid/factory/package.msix",
        "container_size": size,
        "files": [
            file("firmware", members[0].0, "1.4.12", &fw),
            file("bridge", members[1].0, "1.4.11", &bridge_pkg),
            file("display", members[2].0, "1.4.28", &display_pkg),
        ],
    });
    publish(k, site, baseline(k, site), Some(("factory", factory)));
    (url, seen, bridge_pkg, display_pkg)
}

fn factory_keyboard_behaviour(kb: &MockKeyboard) {
    let mut g = kb.kb.lock().unwrap();
    g.factory_versions = Some(("1.4.11".into(), "1.4.28".into()));
    g.firmware_for
        .insert("factory_fw.uf2".into(), "1.4.12".into());
}

async fn restore_with(ranges: bool) {
    let tmp = temp_dir(if ranges {
        "restore_ranges"
    } else {
        "restore_whole"
    });
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let (url, seen, bridge_pkg, display_pkg) = factory_site(&k, &site, ranges);
    let kb = MockKeyboard::new(OPENFLUX_1_0_0);
    factory_keyboard_behaviour(&kb);
    kb.kb.lock().unwrap().fw_keymap = Some(default_keymap_blob());
    let mut cfg = config(&tmp.join("data"), file_url(&site), None, &k);
    cfg.factory_container_url = Some(url);
    let d = daemon(kb.clone(), cfg).await;
    let (_, status) = http(d.addr, "GET", "/v1/updates", None).await;
    assert_eq!(status["error"], serde_json::Value::Null, "{status}");
    let (_, conv) = http(d.addr, "GET", "/v1/conversion", None).await;
    assert_eq!(conv["canRestore"], true, "{conv}");
    assert_eq!(conv["factory"]["label"], "Factory software 1.4");

    let (code, _) = http(d.addr, "POST", "/v1/conversion/restore", Some("{}")).await;
    assert_eq!(code, 400, "a restore needs confirmation");
    let id = start_job(&d, "/v1/conversion/restore", r#"{"confirm":true}"#).await;
    let job = wait_job(&d, &id).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    assert_eq!(kb.version("bridge").as_deref(), Some("1.4.11"));
    assert_eq!(kb.version("display").as_deref(), Some("1.4.28"));
    assert_eq!(kb.version("firmware").as_deref(), Some("1.4.12"));
    let g = kb.kb.lock().unwrap();
    assert_eq!(
        g.files["/home/root/.openflux/factory-restore/fb_1.4.11_aarch64.ipk"],
        bridge_pkg
    );
    assert_eq!(
        g.files["/home/root/.openflux/factory-restore/fd_1.4.28_aarch64.ipk"],
        display_pkg
    );
    let manifest =
        String::from_utf8(g.files["/home/root/.openflux/factory-restore/manifest.sha256"].clone())
            .unwrap();
    assert!(
        manifest.contains(&format!("{}  fb_1.4.11_aarch64.ipk", sha256(&bridge_pkg))),
        "{manifest}"
    );
    let order: Vec<&String> = g
        .writes
        .iter()
        .filter(|w| w.starts_with("put") || w.starts_with("sideload") || w.starts_with("flash"))
        .collect();
    let trigger = order
        .iter()
        .position(|w| w.contains("openflux-factory-restore"))
        .unwrap();
    let flash = order.iter().position(|w| w.starts_with("flash")).unwrap();
    assert!(
        order[..trigger].iter().all(|w| w.starts_with("put")),
        "staged before the trigger: {order:?}"
    );
    assert!(
        flash > trigger,
        "the firmware is flashed after the factory bridge runs: {order:?}"
    );
    drop(g);
    let requests = seen.lock().unwrap().clone();
    assert!(
        requests.iter().all(|r| r.contains("OpenFlux/")),
        "{requests:?}"
    );
    if ranges {
        assert!(
            requests.iter().all(|r| !r.starts_with(" |")),
            "every request is a range request: {requests:?}"
        );
    } else {
        assert!(
            requests.iter().any(|r| r.starts_with(" |")),
            "the whole container is downloaded: {requests:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn converting_and_resetting_to_factory_round_trips_the_persisted_keymap() {
    let tmp = temp_dir("keymap_round_trip");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let (url, _, _, _) = factory_site(&k, &site, true);
    // The factory keyboard's keymap: the default layout in the factory encoding, with a consumer
    // value OpenFlux has no name for (bit index 0x23, past the factory bitmap's 35 bits).
    let default_factory = map_consumer(&default_keymap_blob(), |u| {
        CONSUMER.iter().find(|(_, n)| *n == u).map(|(o, _)| *o)
    });
    let mut km = openfluxd::codecs::keymap::decode_keymap_blob(&default_factory).unwrap();
    km.keymaps[0].layers[0][2] = 0x4000_0023;
    km.keymaps[0].layers[0][3] = 0x4000_0000 | (0x8 << 16) | 0x0B; // Ctrl + volume up
    let factory_keymap = km.to_bytes().unwrap();
    let kb = MockKeyboard::new([
        ("firmware", "1.4.12"),
        ("bridge", "1.4.11"),
        ("display", "1.4.28"),
    ]);
    factory_keyboard_behaviour(&kb);
    {
        let mut g = kb.kb.lock().unwrap();
        g.fw_keymap = Some(factory_keymap.clone());
    }
    let mut cfg = config(&tmp.join("data"), file_url(&site), None, &k);
    cfg.factory_container_url = Some(url);
    let d = daemon(kb.clone(), cfg).await;

    // Convert: the stored keymap is rewritten in OpenFlux's encoding.
    let id = start_job(&d, "/v1/conversion/convert", r#"{"confirm":true}"#).await;
    let job = wait_job(&d, &id).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    let openflux = kb.kb.lock().unwrap().fw_keymap.clone().unwrap();
    let openflux_km = openfluxd::codecs::keymap::decode_keymap_blob(&openflux).unwrap();
    assert_eq!(
        openflux_km.keymaps[0].layers[0][3],
        0x4000_0000 | (0x8 << 16) | 0xE9
    );
    assert_eq!(
        openflux_km.keymaps[0].layers[0][2], 0x4000_0023,
        "an unknown media key is kept as it is"
    );
    let standard: Vec<u32> = CONSUMER.iter().map(|(_, n)| *n).collect();
    assert!(
        consumer_words(&openflux)
            .iter()
            .all(|w| standard.contains(&(w & 0xFFFF)) || w & 0xFFFF == 0x23),
        "every named media key is a HID usage"
    );

    // Reset to factory: the stored keymap is back in the factory encoding.
    let id = start_job(&d, "/v1/conversion/restore", r#"{"confirm":true}"#).await;
    let job = wait_job(&d, &id).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    let g = kb.kb.lock().unwrap();
    let back = g.fw_keymap.clone().unwrap();
    assert_eq!(
        openfluxd::codecs::keymap::decode_keymap_blob(&back).unwrap(),
        openfluxd::codecs::keymap::decode_keymap_blob(&factory_keymap).unwrap(),
        "the persisted keymap is back in the factory encoding, key for key"
    );
    assert!(
        job["log"].as_array().unwrap().iter().any(|l| l
            .as_str()
            .unwrap()
            .contains("Wrote the keymap for the factory firmware")),
        "{job}"
    );
    let keymap_write = g
        .writes
        .iter()
        .rposition(|w| w == "write firmware keymap")
        .expect("the factory keymap is written");
    let flash = g
        .writes
        .iter()
        .rposition(|w| w.starts_with("flash factory_fw"))
        .unwrap();
    assert!(
        keymap_write > flash,
        "the keymap is written once the factory firmware runs: {:?}",
        g.writes
    );
    drop(g);
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn reset_to_factory_reads_the_pinned_files_with_range_requests() {
    restore_with(true).await;
}

#[tokio::test]
async fn reset_to_factory_downloads_the_whole_container_without_range_support() {
    restore_with(false).await;
}

#[tokio::test]
async fn reset_to_factory_is_not_available_without_a_factory_reference() {
    let tmp = temp_dir("norestore");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    publish(&k, &site, baseline(&k, &site), None);
    let d = daemon(
        MockKeyboard::new(OPENFLUX_1_0_0),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;
    let (code, body) = http(
        d.addr,
        "POST",
        "/v1/conversion/restore",
        Some(r#"{"confirm":true}"#),
    )
    .await;
    assert_eq!(
        (code, body["error"]["code"].clone()),
        (501, "notAvailable".into())
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------------------------
// The keyboard's keymap
// ---------------------------------------------------------------------------------------------

/// OpenFlux's default keymap document written with the factory software's key names.
fn factory_named_default_keymap() -> serde_json::Value {
    let mut km = openfluxd::defaults::default_keymap();
    assert!(openfluxd::codecs::keycode_names::to_factory(&mut km) > 0);
    km
}

fn has_factory_names(v: &serde_json::Value) -> bool {
    v.to_string().contains("KC_FF_")
}

const KEYMAP_JSON: &str = "/home/root/openflux/config/keymap.json";

#[tokio::test]
async fn converting_a_keyboard_with_factory_key_names_loads_its_keymap() {
    let tmp = temp_dir("convert_names");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    publish(&k, &site, baseline(&k, &site), None);
    let kb = MockKeyboard::new([
        ("firmware", "1.4.12"),
        ("bridge", "1.4.11"),
        ("display", "1.4.28"),
    ]);
    // The factory keyboard's keymap.json uses its own key names, and its firmware keymap stores
    // media keys as consumer bit indexes.
    let factory_blob = map_consumer(&default_keymap_blob(), |u| {
        CONSUMER.iter().find(|(_, n)| *n == u).map(|(o, _)| *o)
    });
    {
        let mut g = kb.kb.lock().unwrap();
        g.files.insert(
            "/home/root/flux/config/keymap.json".into(),
            serde_json::to_vec(&factory_named_default_keymap()).unwrap(),
        );
        g.files.insert(
            "/home/root/flux/config/appearance.json".into(),
            br#"{"selectedId":"default","defaultTheme":{"stylemaps":{"1":{"styles":{"10":{"keyCode":"KC_FF_START_PAUSE_POMODORO"}}}}}}"#.to_vec(),
        );
        g.fw_keymap = Some(factory_blob);
    }
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;

    let id = start_job(&d, "/v1/conversion/convert", r#"{"confirm":true}"#).await;
    let job = wait_job(&d, &id).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    let log: Vec<&str> = job["log"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| l.as_str())
        .collect();
    assert!(
        log.iter()
            .any(|l| l.starts_with("keymap.json: ") && l.contains("key names translated")),
        "{log:?}"
    );
    assert!(
        !log.iter().any(|l| l.contains("rebuilt")),
        "translated, keymap.json matches the firmware keymap as it is: {log:?}"
    );
    {
        let g = kb.kb.lock().unwrap();
        let plan = g.saved_plans.last().expect("the keymap is saved");
        assert!(!has_factory_names(&plan["keymap"]), "{}", plan["keymap"]);
        assert_eq!(
            plan["keymap"]["keymaps"]["ansi"]["layers"][1],
            openfluxd::defaults::default_keymap()["keymaps"]["ansi"]["layers"][1],
            "the FN layer reads the same as OpenFlux's own"
        );
        let blob = unhex(plan["keymapBlobHex"].as_str().unwrap());
        assert!(
            openfluxd::codecs::keymap::verify_keymap_matches_blob(&plan["keymap"], &blob).is_ok()
        );
        let written: serde_json::Value = serde_json::from_slice(&g.files[KEYMAP_JSON]).unwrap();
        assert!(!has_factory_names(&written), "{written}");
        let appearance: serde_json::Value =
            serde_json::from_slice(&g.files["/home/root/openflux/config/appearance.json"]).unwrap();
        assert_eq!(
            appearance["defaultTheme"]["stylemaps"]["1"]["styles"]["10"]["keyCode"],
            "KC_OF_START_PAUSE_POMODORO"
        );
    }

    // The daemon reads the converted keyboard, and the keymap loads.
    let mut w = openfluxd::api::worker::WorkerLoopState::default();
    openfluxd::api::worker::reconcile_tick_with_state(&d.state, &mut QuietHost, &*kb, 2, &mut w);
    let (_, dev) = http(d.addr, "GET", "/v1/device/status", None).await;
    assert_eq!(
        dev["keymapSync"],
        serde_json::json!({"state": "ok", "reason": null}),
        "{dev}"
    );
    let (_, cfg) = http(d.addr, "GET", "/v1/config", None).await;
    assert_eq!(cfg["hydrated"]["keymap"], true, "{}", cfg["hydrated"]);
    assert!(!has_factory_names(&cfg["keymap"]));
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn a_keymap_that_disagrees_with_the_firmware_is_reported_and_the_firmware_keymap_adopted() {
    let tmp = temp_dir("adopt");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    publish(&k, &site, baseline(&k, &site), None);
    let kb = MockKeyboard::new(OPENFLUX_1_0_0);
    {
        let mut g = kb.kb.lock().unwrap();
        g.fw_keymap = Some(default_keymap_blob());
        g.files.insert(
            KEYMAP_JSON.into(),
            serde_json::to_vec(&factory_named_default_keymap()).unwrap(),
        );
    }
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;
    let mut w = openfluxd::api::worker::WorkerLoopState::default();
    let mut tick = |kb: &MockKeyboard| {
        openfluxd::api::worker::reconcile_tick_with_state(&d.state, &mut QuietHost, kb, 2, &mut w)
    };

    // Nothing has been read yet: the keymap is loading and there is no firmware keymap to adopt.
    let (_, dev) = http(d.addr, "GET", "/v1/device/status", None).await;
    assert_eq!(dev["keymapSync"]["state"], "pulling", "{dev}");
    let (code, body) = http(d.addr, "POST", "/v1/keymap/adopt-firmware", None).await;
    assert_eq!(
        (code, body["error"]["code"].clone()),
        (409, "noFirmwareKeymap".into()),
        "{body}"
    );

    // keymap.json disagrees with the firmware keymap: reported, and the keymap stays locked.
    tick(&kb);
    let (_, dev) = http(d.addr, "GET", "/v1/device/status", None).await;
    assert_eq!(dev["keymapSync"]["state"], "mismatch", "{dev}");
    let reason = dev["keymapSync"]["reason"].as_str().unwrap();
    assert!(reason.contains("KC_FF_"), "{reason}");
    let (_, cfg) = http(d.addr, "GET", "/v1/config", None).await;
    assert_eq!(cfg["hydrated"]["keymap"], false);
    let saves = kb.kb.lock().unwrap().saved_plans.len();

    // Adopting the firmware keymap writes a keymap.json that matches it, and the keymap loads.
    let (code, cfg) = http(d.addr, "POST", "/v1/keymap/adopt-firmware", None).await;
    assert_eq!(code, 200, "{cfg}");
    assert_eq!(cfg["hydrated"]["keymap"], true, "{}", cfg["hydrated"]);
    assert!(!has_factory_names(&cfg["keymap"]));
    let default = openfluxd::defaults::default_keymap();
    for layout in ["ansi", "iso"] {
        assert_eq!(
            cfg["keymap"]["keymaps"][layout]["layers"], default["keymaps"][layout]["layers"],
            "{layout}: OpenFlux names, with the layer names and ids kept"
        );
    }
    {
        let g = kb.kb.lock().unwrap();
        assert_eq!(g.saved_plans.len(), saves + 1);
        let plan = g.saved_plans.last().unwrap();
        assert_eq!(plan["keymap"], cfg["keymap"]);
        assert_eq!(
            unhex(plan["keymapBlobHex"].as_str().unwrap()),
            default_keymap_blob()
        );
        let written: serde_json::Value = serde_json::from_slice(&g.files[KEYMAP_JSON]).unwrap();
        assert_eq!(written, cfg["keymap"]);
    }
    let (_, dev) = http(d.addr, "GET", "/v1/device/status", None).await;
    assert_eq!(
        dev["keymapSync"],
        serde_json::json!({"state": "ok", "reason": null})
    );

    // Reading the keyboard again finds the same; unplugging it disconnects the keymap.
    tick(&kb);
    let (_, dev) = http(d.addr, "GET", "/v1/device/status", None).await;
    assert_eq!(dev["keymapSync"]["state"], "ok", "{dev}");
    kb.kb.lock().unwrap().connected = (false, false);
    tick(&kb);
    let (_, dev) = http(d.addr, "GET", "/v1/device/status", None).await;
    assert_eq!(
        dev["keymapSync"],
        serde_json::json!({"state": "disconnected", "reason": null})
    );
    let (code, body) = http(d.addr, "POST", "/v1/keymap/adopt-firmware", None).await;
    assert_eq!(
        (code, body["error"]["code"].clone()),
        (409, "noFirmwareKeymap".into()),
        "{body}"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn adopting_the_firmware_keymap_is_refused_while_a_job_runs_or_conversion_is_needed() {
    let tmp = temp_dir("adopt_refused");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let mut releases = baseline(&k, &site);
    releases.push(release(&k, &site, Component::Display, "1.0.1", None));
    publish(&k, &site, releases, None);
    let kb = MockKeyboard::new(OPENFLUX_1_0_0);
    kb.kb.lock().unwrap().fw_keymap = Some(default_keymap_blob());
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;
    let mut w = openfluxd::api::worker::WorkerLoopState::default();
    openfluxd::api::worker::reconcile_tick_with_state(&d.state, &mut QuietHost, &*kb, 2, &mut w);
    assert!(d.state.inner.lock().unwrap().device_keymap_blob.is_some());

    kb.hold(true);
    let id = start_job(&d, "/v1/updates/apply", "{}").await;
    let (code, body) = http(d.addr, "POST", "/v1/keymap/adopt-firmware", None).await;
    assert_eq!(
        (code, body["error"]["code"].clone()),
        (409, "busy".into()),
        "{body}"
    );
    kb.hold(false);
    assert_eq!(wait_job(&d, &id).await["state"], "succeeded");
    assert!(
        !kb.writes().contains(&"save".to_string()),
        "{:?}",
        kb.writes()
    );
    let _ = std::fs::remove_dir_all(&tmp);

    let tmp = temp_dir("adopt_conversion");
    let kb = MockKeyboard::new([
        ("firmware", "1.4.12"),
        ("bridge", "1.4.11"),
        ("display", "1.4.28"),
    ]);
    kb.kb.lock().unwrap().fw_keymap = Some(default_keymap_blob());
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;
    let mut w = openfluxd::api::worker::WorkerLoopState::default();
    openfluxd::api::worker::reconcile_tick_with_state(&d.state, &mut QuietHost, &*kb, 2, &mut w);
    let (code, body) = http(d.addr, "POST", "/v1/keymap/adopt-firmware", None).await;
    assert_eq!(
        (code, body["error"]["code"].clone()),
        (409, "conversionRequired".into()),
        "{body}"
    );
    assert!(kb.writes().iter().all(|w| w != "save"), "{:?}", kb.writes());
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn converting_rebuilds_a_keymap_json_that_disagrees_with_the_firmware_keymap() {
    let tmp = temp_dir("convert_rebuild");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    publish(&k, &site, baseline(&k, &site), None);
    let kb = MockKeyboard::new([
        ("firmware", "1.4.12"),
        ("bridge", "1.4.11"),
        ("display", "1.4.28"),
    ]);
    // keymap.json says the first key is Q; the firmware, which is what the keyboard does, says
    // Escape.
    let mut stale = factory_named_default_keymap();
    stale["keymaps"]["ansi"]["layers"][0]["keyActions"][0]["keyCode"] = "KC_Q".into();
    stale["keymaps"]["ansi"]["layers"][0]["name"] = "My base".into();
    {
        let mut g = kb.kb.lock().unwrap();
        g.files.insert(
            "/home/root/flux/config/keymap.json".into(),
            serde_json::to_vec(&stale).unwrap(),
        );
        g.fw_keymap = Some(default_keymap_blob());
    }
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;
    let id = start_job(&d, "/v1/conversion/convert", r#"{"confirm":true}"#).await;
    let job = wait_job(&d, &id).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    assert!(
        job["log"].as_array().unwrap().iter().any(|l| l
            .as_str()
            .unwrap()
            .contains("rebuilt from the firmware keymap")),
        "{job}"
    );
    let plan = kb.kb.lock().unwrap().saved_plans.last().cloned().unwrap();
    let layer = &plan["keymap"]["keymaps"]["ansi"]["layers"][0];
    assert_eq!(layer["keyActions"][0]["keyCode"], "KC_ESCAPE");
    assert_eq!(layer["name"], "My base", "the document's names are kept");
    assert!(!has_factory_names(&plan["keymap"]));

    let mut w = openfluxd::api::worker::WorkerLoopState::default();
    openfluxd::api::worker::reconcile_tick_with_state(&d.state, &mut QuietHost, &*kb, 2, &mut w);
    let (_, dev) = http(d.addr, "GET", "/v1/device/status", None).await;
    assert_eq!(dev["keymapSync"]["state"], "ok", "{dev}");
    let _ = std::fs::remove_dir_all(&tmp);
}

// ---------------------------------------------------------------------------------------------
// Press-refinement data
// ---------------------------------------------------------------------------------------------

use openfluxd::conversion::press_refinement::{PressRefinementSource, SourceLocation, SourceRange};
use openfluxd::usb::press_refinement::{crc32, FirmwareStore, State as PressState};

/// One UF2 block for the RP2040 at `addr`.
fn uf2_block(addr: u32, data: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; 512];
    b[0..4].copy_from_slice(&0x0A32_4655u32.to_le_bytes());
    b[4..8].copy_from_slice(&0x9E5D_5157u32.to_le_bytes());
    b[8..12].copy_from_slice(&0x2000u32.to_le_bytes());
    b[12..16].copy_from_slice(&addr.to_le_bytes());
    b[16..20].copy_from_slice(&(data.len() as u32).to_le_bytes());
    b[28..32].copy_from_slice(&0xE48B_FF56u32.to_le_bytes());
    b[32..32 + data.len()].copy_from_slice(data);
    b[508..512].copy_from_slice(&0x0AB1_6F30u32.to_le_bytes());
    b
}

/// A synthetic refinement: a firmware UF2 (its blocks out of address order) inside a container.
struct SyntheticRefinementSource {
    refinement: PressRefinementSource,
    uf2: Vec<u8>,
    container: Vec<u8>,
    payload: Vec<u8>,
}

fn synthetic_refinement_source() -> SyntheticRefinementSource {
    let image = noise(32 * 256, 7);
    let mut uf2 = Vec::new();
    for page in (0..32u32).rev() {
        let at = page as usize * 256;
        uf2.extend_from_slice(&uf2_block(0x1000_0000 + page * 256, &image[at..at + 256]));
    }
    let member = "data/flutter_assets/assets/firmware/synthetic_fw.uf2";
    let container = factory_container(&[(member, &uf2)]);
    let mut payload = image[0x300..0x300 + 1452].to_vec();
    payload.extend_from_slice(&image[0x24..0x28]);
    let refinement = PressRefinementSource {
        description: "Synthetic software".into(),
        sources: vec![SourceLocation::Container {
            url: "https://downloads.example.invalid/software/package.msix".into(),
            size: container.len() as u64,
            sha256: sha256(&container),
        }],
        member: member.into(),
        member_size: uf2.len() as u64,
        member_sha256: sha256(&uf2),
        image_base: 0x1000_0000,
        image_size: image.len() as u64,
        image_sha256: sha256(&image),
        payload_ranges: vec![
            SourceRange {
                offset: 0x300,
                length: 1452,
            },
            SourceRange {
                offset: 0x24,
                length: 4,
            },
        ],
        payload_length: 1456,
        payload_sha256: sha256(&payload),
    };
    SyntheticRefinementSource {
        refinement,
        uf2,
        container,
        payload,
    }
}

/// Publishes the baseline releases with `refinement` as the signed index's press-refinement source.
fn publish_with_refinement_source(
    k: &Keys,
    site: &Path,
    refinement: Option<&PressRefinementSource>,
) {
    publish(
        k,
        site,
        baseline(k, site),
        refinement.map(|d| ("press_refinement_source", serde_json::to_value(d).unwrap())),
    );
}

fn factory_keyboard() -> Arc<MockKeyboard> {
    MockKeyboard::new([
        ("firmware", "1.4.12"),
        ("bridge", "1.4.11"),
        ("display", "1.4.28"),
    ])
}

fn log_line<'a>(job: &'a serde_json::Value, prefix: &str) -> Option<&'a str> {
    job["log"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| l.as_str())
        .find(|l| l.starts_with(prefix))
}

async fn install(d: &Daemon, body: &str) -> (u16, serde_json::Value) {
    http(
        d.addr,
        "POST",
        "/v1/keyboard/press-refinement/install",
        Some(body),
    )
    .await
}

#[tokio::test]
async fn conversion_installs_press_refinement_data_from_the_index_and_a_reinstall_works_offline() {
    let tmp = temp_dir("press_convert");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let s = synthetic_refinement_source();
    publish_with_refinement_source(&k, &site, Some(&s.refinement));
    let (url, seen) = serve_container(s.container.clone(), true);
    let kb = factory_keyboard();
    let mut cfg = config(&tmp.join("data"), file_url(&site), None, &k);
    cfg.press_refinement_container_url = Some(url);
    let d = daemon(kb.clone(), cfg).await;

    let id = start_job(&d, "/v1/conversion/convert", r#"{"confirm":true}"#).await;
    let job = wait_job(&d, &id).await;
    assert_eq!(job["state"], "succeeded", "{job}");
    assert!(
        log_line(&job, "Installed press-refinement data").is_some(),
        "{job}"
    );
    let crc = crc32(&s.payload);
    assert_eq!(
        job["pressRefinement"],
        serde_json::json!({"installed": true, "active": false, "crc32": crc})
    );
    let status = kb.kb.lock().unwrap().press.status();
    assert_eq!(
        (status.state, status.payload_crc32),
        (PressState::InstalledInactive, crc)
    );
    assert!(
        seen.lock().unwrap().iter().all(|r| !r.starts_with(" |")),
        "only range requests: {:?}",
        seen.lock().unwrap()
    );
    let (code, body) = http(d.addr, "GET", "/v1/keyboard/press-refinement", None).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(
        body,
        serde_json::json!({"state": "installed", "crc32": crc, "active": false})
    );
    let kept = tmp
        .join("data/press-refinement")
        .join(format!("{}.bin", s.refinement.payload_sha256));
    assert_eq!(std::fs::read(&kept).unwrap(), s.payload);

    // A daemon that can't reach the source installs the kept copy, here on a firmware build that
    // applies it.
    kb.kb.lock().unwrap().press = FirmwareStore {
        applies: true,
        ..Default::default()
    };
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;
    let (code, body) = http(d.addr, "GET", "/v1/keyboard/press-refinement", None).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body, serde_json::json!({"state": "absent", "crc32": null}));
    let (code, body) = install(&d, "{}").await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["state"], "installed");
    assert_eq!(body["crc32"], crc);
    assert_eq!(body["fromCache"], true);
    assert_eq!(body["active"], true);
    assert_eq!(body["alreadyInstalled"], false);
    let (_, body) = install(&d, "{}").await;
    assert_eq!(body["alreadyInstalled"], true, "{body}");
    let puts = kb
        .writes()
        .iter()
        .filter(|w| *w == "put press refinement")
        .count();
    assert_eq!(puts, 2, "the conversion and the first reinstall write it");
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn a_container_downloaded_whole_must_match_its_pinned_hash() {
    for pinned_matches in [true, false] {
        let tmp = temp_dir("press_whole");
        let k = keys();
        let site = tmp.join("site");
        std::fs::create_dir_all(&site).unwrap();
        let mut s = synthetic_refinement_source();
        if !pinned_matches {
            if let SourceLocation::Container { sha256, .. } = &mut s.refinement.sources[0] {
                *sha256 = "f".repeat(64);
            }
        }
        publish_with_refinement_source(&k, &site, Some(&s.refinement));
        let (url, seen) = serve_container(s.container.clone(), false);
        let kb = MockKeyboard::new(OPENFLUX_1_0_0);
        let mut cfg = config(&tmp.join("data"), file_url(&site), None, &k);
        cfg.press_refinement_container_url = Some(url);
        let d = daemon(kb.clone(), cfg).await;
        let (code, body) = install(&d, "{}").await;
        assert!(
            seen.lock().unwrap().iter().any(|r| r.starts_with(" |")),
            "the whole container is downloaded"
        );
        if pinned_matches {
            assert_eq!(code, 200, "{body}");
        } else {
            assert_eq!(code, 502, "{body}");
            assert_eq!(body["error"]["code"], "hashMismatch");
            assert!(kb.kb.lock().unwrap().press.stored.is_none());
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

#[tokio::test]
async fn a_file_the_user_already_has_is_verified_and_installed() {
    let tmp = temp_dir("press_file");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let s = synthetic_refinement_source();
    publish_with_refinement_source(&k, &site, Some(&s.refinement));
    let kb = MockKeyboard::new(OPENFLUX_1_0_0);
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;
    let files = tmp.join("downloads");
    std::fs::create_dir_all(&files).unwrap();
    let bare = files.join("oates.uf2");
    std::fs::write(&bare, &s.uf2).unwrap();
    let package = files.join("polymath.msix");
    std::fs::write(&package, &s.container).unwrap();
    let mut other = s.uf2.clone();
    other[600] ^= 1;
    let damaged = files.join("damaged.uf2");
    std::fs::write(&damaged, &other).unwrap();
    let notes = files.join("notes.txt");
    std::fs::write(&notes, b"not a firmware file").unwrap();
    let body = |p: &Path| serde_json::json!({ "path": p }).to_string();

    let (code, resp) = install(&d, &body(&damaged)).await;
    assert_eq!(
        (code, resp["error"]["code"].clone()),
        (502, "hashMismatch".into()),
        "{resp}"
    );
    let (code, resp) = install(&d, &body(&notes)).await;
    assert_eq!(
        (code, resp["error"]["code"].clone()),
        (400, "fileUnusable".into()),
        "{resp}"
    );
    let (code, resp) = install(&d, r#"{"path":"relative/oates.uf2"}"#).await;
    assert_eq!(
        (code, resp["error"]["code"].clone()),
        (400, "badRequest".into()),
        "{resp}"
    );
    assert!(!kb.writes().iter().any(|w| w == "put press refinement"));

    let (code, resp) = install(&d, &body(&bare)).await;
    assert_eq!(code, 200, "{resp}");
    assert_eq!(resp["fromCache"], false);
    assert_eq!(resp["crc32"], crc32(&s.payload));
    kb.kb.lock().unwrap().press = FirmwareStore::default();
    let (code, resp) = install(&d, &body(&package)).await;
    assert_eq!(code, 200, "{resp}");
    assert_eq!(resp["alreadyInstalled"], false);
    assert_eq!(
        kb.kb.lock().unwrap().press.get().unwrap()[16..],
        s.payload[..]
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

#[tokio::test]
async fn a_conversion_succeeds_when_press_refinement_data_cant_be_installed() {
    for case in ["notListed", "offline", "hashMismatch", "storageUnavailable"] {
        let tmp = temp_dir("press_fails");
        let k = keys();
        let site = tmp.join("site");
        std::fs::create_dir_all(&site).unwrap();
        let kb = factory_keyboard();
        let mut cfg = config(&tmp.join("data"), file_url(&site), None, &k);
        let mut s = synthetic_refinement_source();
        match case {
            "notListed" => publish_with_refinement_source(&k, &site, None),
            "offline" => publish_with_refinement_source(&k, &site, Some(&s.refinement)),
            "hashMismatch" => {
                s.refinement.payload_sha256 = sha256(b"other coefficients");
                publish_with_refinement_source(&k, &site, Some(&s.refinement));
                cfg.press_refinement_container_url = Some(serve_container(s.container, true).0);
            }
            _ => {
                publish_with_refinement_source(&k, &site, Some(&s.refinement));
                cfg.press_refinement_container_url = Some(serve_container(s.container, true).0);
                kb.kb.lock().unwrap().press.storage_unavailable = true;
            }
        }
        let d = daemon(kb.clone(), cfg).await;
        let id = start_job(&d, "/v1/conversion/convert", r#"{"confirm":true}"#).await;
        let job = wait_job(&d, &id).await;
        assert_eq!(job["state"], "succeeded", "{case}: {job}");
        assert_eq!(job["error"], serde_json::Value::Null, "{case}: {job}");
        assert!(
            log_line(&job, "Press refinement not installed").is_some(),
            "{case}: {job}"
        );
        assert_eq!(job["pressRefinement"]["installed"], false, "{case}: {job}");
        let want = match case {
            "notListed" => "notAvailable",
            "offline" => "offline",
            other => other,
        };
        assert_eq!(job["pressRefinement"]["code"], want, "{case}: {job}");
        for (c, v) in OPENFLUX_1_0_0 {
            assert_eq!(kb.version(c).as_deref(), Some(v), "{case}");
        }
        assert!(kb.kb.lock().unwrap().press.stored.is_none(), "{case}");
        assert!(
            !kb.writes().iter().any(|w| w == "put press refinement"),
            "{case}: nothing is written without verified data and usable storage"
        );
        let (code, body) = install(&d, "{}").await;
        let status = match case {
            "notListed" => 501,
            "offline" => 503,
            "hashMismatch" => 502,
            _ => 409,
        };
        assert_eq!(code, status, "{case}: {body}");
        assert_eq!(body["error"]["code"], want, "{case}");
        if case == "storageUnavailable" {
            let (_, body) = http(d.addr, "GET", "/v1/keyboard/press-refinement", None).await;
            assert_eq!(body["state"], "storageUnavailable", "{body}");
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

#[tokio::test]
async fn press_refinement_routes_wait_for_conversion_and_running_jobs() {
    let tmp = temp_dir("press_gates");
    let k = keys();
    let site = tmp.join("site");
    std::fs::create_dir_all(&site).unwrap();
    let s = synthetic_refinement_source();
    let mut releases = baseline(&k, &site);
    releases.push(release(&k, &site, Component::Bridge, "1.0.1", None));
    publish(
        &k,
        &site,
        releases,
        Some((
            "press_refinement_source",
            serde_json::to_value(&s.refinement).unwrap(),
        )),
    );

    // A keyboard that doesn't run OpenFlux is converted first.
    let kb = factory_keyboard();
    let d = daemon(
        kb.clone(),
        config(&tmp.join("data"), file_url(&site), None, &k),
    )
    .await;
    for (method, path) in [
        ("GET", "/v1/keyboard/press-refinement"),
        ("POST", "/v1/keyboard/press-refinement/install"),
    ] {
        let (code, body) = http(d.addr, method, path, Some("{}")).await;
        assert_eq!(code, 409, "{path}: {body}");
        assert_eq!(body["error"]["code"], "conversionRequired", "{path}");
    }
    assert!(kb.writes().is_empty());

    // While press-refinement data is being installed, jobs wait.
    let kb = MockKeyboard::new(OPENFLUX_1_0_0);
    let mut cfg = config(&tmp.join("data2"), file_url(&site), None, &k);
    cfg.press_refinement_container_url = Some(serve_container(s.container.clone(), true).0);
    let d = daemon(kb.clone(), cfg).await;
    let hold = d
        .updates
        .hold_keyboard("installing press-refinement data")
        .unwrap();
    let (code, body) = http(d.addr, "POST", "/v1/updates/apply", Some("{}")).await;
    assert_eq!(code, 409, "{body}");
    assert_eq!(body["error"]["code"], "busy");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("installing press-refinement data"),
        "{body}"
    );
    assert!(d.updates.hold_keyboard("another").is_err());
    drop(hold);

    // While a job has the keyboard, both routes are refused.
    kb.hold(true);
    let id = start_job(&d, "/v1/updates/apply", "{}").await;
    for (method, path) in [
        ("GET", "/v1/keyboard/press-refinement"),
        ("POST", "/v1/keyboard/press-refinement/install"),
    ] {
        let (code, body) = http(d.addr, method, path, Some("{}")).await;
        assert_eq!(code, 409, "{path}: {body}");
        assert_eq!(body["error"]["code"], "busy", "{path}");
    }
    kb.hold(false);
    assert_eq!(wait_job(&d, &id).await["state"], "succeeded");
    assert!(!kb.writes().iter().any(|w| w == "put press refinement"));
    let (code, body) = install(&d, "{}").await;
    assert_eq!(code, 200, "{body}");
    let _ = std::fs::remove_dir_all(&tmp);
}
