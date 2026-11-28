//! Layers and actuation modes added, renamed and deleted through the config API reach the
//! keyboard as a firmware keymap it accepts, and come back unchanged when the keyboard is read
//! again, driven through a mock keyboard that keeps whatever a save writes.

use keyglassd::api::build_router;
use keyglassd::api::state::{ApiState, DeviceSink};
use keyglassd::api::worker::reconcile_tick;
use keyglassd::bridges::{ActiveWindowInfo, HardwareMetrics, MediaSnapshot, OsSensorBridge};
use keyglassd::codecs::keymap::{decode_keymap_blob, FirmwareKeymapBlob};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const TOKEN: &str = "tok_layers";

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "keyglassd_layers_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A keyboard holding a keymap document, its firmware keymap and profiles; a save replaces
/// whichever of them the save plan carries.
struct MockKeyboard {
    keymap: Mutex<Value>,
    blob: Mutex<Vec<u8>>,
    profiles: Mutex<(Value, String)>,
}

impl MockKeyboard {
    fn new() -> Arc<Self> {
        let settings = keyglassd::codecs::keymap::ProfileSettingsFile {
            settings: vec![keyglassd::codecs::keymap::ProfileSetting::from_layer(0, 0).unwrap()],
            file_version: 0,
            file_type: 0,
        }
        .to_transfer_bytes()
        .unwrap();
        Arc::new(Self {
            keymap: Mutex::new(
                serde_json::from_str(include_str!("fixtures/default_keymap.json")).unwrap(),
            ),
            blob: Mutex::new(include_bytes!("fixtures/default_keymap.bin").to_vec()),
            profiles: Mutex::new((
                json!([{"id": "default", "name": "Default", "theme_id": "default", "contexts": ["Main"],
                        "active_context": "Main", "actuation_mode_index": 0, "default_layer": 0}]),
                hex(&settings),
            )),
        })
    }

    fn firmware_keymap(&self) -> FirmwareKeymapBlob {
        decode_keymap_blob(&self.blob.lock().unwrap()).unwrap()
    }

    /// The first section's header words, as the keyboard stores them.
    fn first_section_header(&self) -> Vec<u32> {
        let b = self.blob.lock().unwrap();
        (6..13)
            .map(|w| u32::from_le_bytes(b[w * 4..w * 4 + 4].try_into().unwrap()))
            .collect()
    }
}

impl DeviceSink for MockKeyboard {
    fn is_connected(&self) -> (bool, bool) {
        (true, true)
    }
    fn versions(&self) -> Value {
        json!({"firmware": "keyglass-firmware 1.0.0", "bridge": "keyglass-bridge 1.0.0"})
    }
    fn send_live_json(&self, _: &Value) -> Result<(), String> {
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
    fn save_config_dir(&self, plan: &Value) -> Result<(), String> {
        if let (Some(k), Some(h)) = (plan.get("keymap"), plan["keymapBlobHex"].as_str()) {
            *self.keymap.lock().unwrap() = k.clone();
            *self.blob.lock().unwrap() = ApiState::decode_hex_bytes(h)?;
        }
        if let (Some(p), Some(h)) = (plan.get("profiles"), plan["profileSettingsHex"].as_str()) {
            *self.profiles.lock().unwrap() = (p.clone(), h.to_string());
        }
        Ok(())
    }
    fn pull_config_dir(&self) -> Result<Value, String> {
        let (profiles, settings) = self.profiles.lock().unwrap().clone();
        Ok(json!({
            "appearance": {"selectedId": "default"},
            "profiles": profiles,
            "firmwareProfileSettingsHex": settings,
            "keymap": self.keymap.lock().unwrap().clone(),
            "firmwareKeymapBlobHex": hex(&self.blob.lock().unwrap()),
        }))
    }
}

struct Host;

impl OsSensorBridge for Host {
    fn sample_hardware(&mut self) -> HardwareMetrics {
        HardwareMetrics {
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
    fn active_window(&mut self) -> Option<ActiveWindowInfo> {
        None
    }
    fn active_language_code(&mut self) -> Option<u32> {
        None
    }
    fn media_snapshot(&mut self) -> MediaSnapshot {
        MediaSnapshot::default()
    }
}

struct Daemon {
    addr: std::net::SocketAddr,
    state: Arc<ApiState>,
}

impl Daemon {
    /// A daemon attached to `keyboard`, which has read the keyboard's configuration.
    async fn attach(keyboard: Arc<MockKeyboard>) -> Self {
        let state = ApiState::with_dirs(
            keyboard.clone(),
            Some(TOKEN.into()),
            Duration::from_secs(20),
            temp_dir("icons"),
        );
        let app = build_router(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        reconcile_tick(&state, &mut Host, &*keyboard, 2);
        Self { addr, state }
    }

    async fn request(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut stream = TcpStream::connect(self.addr).await.unwrap();
        let payload = body.map(|b| b.to_string()).unwrap_or_default();
        let mut raw = format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAuthorization: Bearer {TOKEN}\r\n",
            self.addr
        );
        if !payload.is_empty() {
            raw.push_str("Content-Type: application/json\r\n");
            raw.push_str(&format!("Content-Length: {}\r\n", payload.len()));
        }
        raw.push_str("\r\n");
        raw.push_str(&payload);
        stream.write_all(raw.as_bytes()).await.unwrap();
        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).await.unwrap();
        let txt = String::from_utf8_lossy(&resp).to_string();
        let (hdr, body) = txt.split_once("\r\n\r\n").unwrap_or((&txt, ""));
        let status = hdr
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        (status, serde_json::from_str(body).unwrap_or(Value::Null))
    }

    async fn config(&self) -> Value {
        let (code, body) = self.request("GET", "/v1/config", None).await;
        assert_eq!(code, 200, "{body}");
        body
    }

    async fn put(&self, body: Value) {
        let (code, resp) = self.request("PUT", "/v1/config", Some(body)).await;
        assert_eq!(code, 200, "{resp}");
    }

    async fn save(&self) {
        let (code, body) = self.request("POST", "/v1/config/save", None).await;
        assert_eq!(code, 200, "{body}");
    }
}

fn names(modes: &Value) -> Vec<String> {
    modes
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn layers_and_modes_made_in_studio_reach_the_keyboard_and_come_back() {
    let keyboard = MockKeyboard::new();
    let d = Daemon::attach(keyboard.clone()).await;
    let cfg = d.config().await;
    assert_eq!(
        cfg["keymapLimits"],
        json!({"maxLayers": 8, "maxActuationModes": 8})
    );
    assert_eq!(names(&cfg["actuationModes"]), ["Typing", "Gaming"]);

    // A third layer, the first mode renamed and a third mode added.
    let mut keymap = cfg["keymap"].clone();
    for layout in keymap["keymaps"].as_object_mut().unwrap().values_mut() {
        let keys = layout["layers"][0]["keyActions"].as_array().unwrap().len();
        layout["layers"].as_array_mut().unwrap().push(json!({
            "name": "Numpad",
            "keyActions": vec![json!({"type": 0, "keyCode": "KC_KP_1"}); keys],
        }));
    }
    let mut modes = cfg["actuationModes"].clone();
    modes[0]["name"] = json!("Prose");
    modes.as_array_mut().unwrap().push(json!({
        "id": "mode-new", "name": "Writing", "type": 1, "actuationPointMm": 0.8,
        "releasePointMm": 0.6, "rapidTrigger": false, "touched": true, "globalTouched": true,
    }));
    d.put(json!({"keymap": keymap, "actuationModes": modes}))
        .await;
    d.save().await;

    // Three layers, 85 keys, three modes, in the order the keyboard reads them.
    assert_eq!(keyboard.first_section_header(), [3, 85, 3, 0, 0, 0, 255]);
    let before = keyboard.firmware_keymap();
    for sec in &before.keymaps {
        assert_eq!((sec.num_layers, sec.num_actuation_modes), (3, 3));
    }

    // Delete the first mode; the others keep their tables.
    let cfg = d.config().await;
    assert_eq!(
        names(&cfg["actuationModes"]),
        ["Prose", "Gaming", "Writing"]
    );
    let kept: Vec<Value> = cfg["actuationModes"].as_array().unwrap()[1..].to_vec();
    d.put(json!({"actuationModes": kept})).await;
    d.save().await;
    let after = keyboard.firmware_keymap();
    for (old, new) in before.keymaps.iter().zip(&after.keymaps) {
        assert_eq!(new.num_actuation_modes, 2);
        assert_eq!(new.actuation_tables, old.actuation_tables[1..].to_vec());
        assert_eq!(new.layers, old.layers);
    }
    assert_eq!(keyboard.first_section_header(), [3, 85, 2, 0, 0, 0, 255]);

    // Another daemon reading the same keyboard sees the same layers and modes, names included.
    drop(d);
    let d2 = Daemon::attach(keyboard.clone()).await;
    let cfg = d2.config().await;
    assert_eq!(names(&cfg["actuationModes"]), ["Gaming", "Writing"]);
    let layers = cfg["keymap"]["keymaps"]["ansi"]["layers"]
        .as_array()
        .unwrap();
    assert_eq!(layers.len(), 3);
    assert_eq!(layers[2]["name"], "Numpad");
    assert!(d2.state.inner.lock().unwrap().keymap_hydrated);
}

#[tokio::test]
async fn more_layers_than_the_keyboard_holds_are_refused() {
    let d = Daemon::attach(MockKeyboard::new()).await;
    let mut keymap = d.config().await["keymap"].clone();
    for layout in keymap["keymaps"].as_object_mut().unwrap().values_mut() {
        let layers = layout["layers"].as_array_mut().unwrap();
        while layers.len() < 9 {
            layers.push(layers[1].clone());
        }
    }
    let (code, body) = d
        .request("PUT", "/v1/config", Some(json!({"keymap": keymap})))
        .await;
    assert_eq!(code, 422, "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("9 layers"),
        "{body}"
    );
}
