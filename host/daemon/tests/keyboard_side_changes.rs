//! Changes made on the keyboard itself (function keys for the profile, actuation mode, platform and
//! theme) are kept by the daemon rather than undone, driven through a mock keyboard that behaves
//! like the real one: its firmware state changes when a key is pressed, and its configuration
//! files change when its display writes them.

use openfluxd::api::build_router;
use openfluxd::api::state::{ApiState, DeviceSink, FirmwareRuntime};
use openfluxd::api::worker::reconcile_tick;
use openfluxd::bridges::{
    ActiveWindowInfo, AppProfileRule, HardwareMetrics, MediaSnapshot, OsSensorBridge,
};
use openfluxd::device_paths;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const TOKEN: &str = "tok_kbd";
const MACOS: u32 = 2;

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "openfluxd_kbd_{tag}_{}_{}",
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

/// The firmware's side of the mock keyboard.
struct Firmware {
    attached: bool,
    /// Whether it answers the runtime-state read.
    reports_runtime: bool,
    active_slot: u32,
    actuation_mode: u32,
    platform: u32,
    /// The actuation mode of each saved profile slot; activating a slot selects it.
    slot_modes: Vec<u32>,
    /// The next command sent to the firmware fails.
    fail_next_write: bool,
}

struct MockKeyboard {
    firmware: Mutex<Firmware>,
    /// The bridge's files, by path.
    files: Mutex<BTreeMap<String, Vec<u8>>>,
    /// Commands the host sent to the firmware, as `platform:2`, `profile:1`, `lang:471`.
    calls: Mutex<Vec<String>>,
    /// Every save plan the host executed.
    saves: Mutex<Vec<Value>>,
    /// Every live-data message the host sent to the display.
    live: Mutex<Vec<Value>>,
    /// How many of the next file reads fail with a transfer error.
    failing_reads: Mutex<u32>,
}

fn profile(id: &str, theme: &str, mode: usize) -> Value {
    json!({"id": id, "name": id, "theme_id": theme, "contexts": ["Main"], "active_context": "Main",
           "actuation_mode_index": mode, "default_layer": 0})
}

impl MockKeyboard {
    /// A keyboard holding three profiles (`default`, `coding`, `gaming`; the last uses actuation
    /// mode 1) and an appearance with themes `a` and `b`, `a` selected.
    fn new(reports_runtime: bool) -> Arc<Self> {
        let profiles = json!([
            profile("default", "a", 0),
            profile("coding", "a", 0),
            profile("gaming", "b", 1)
        ]);
        let mut files = BTreeMap::new();
        files.insert(
            device_paths::APPEARANCE_JSON.to_string(),
            serde_json::to_vec(&json!({
                "selectedId": "a",
                "defaultTheme": {"id": "default", "legendColor": 1},
                "customThemes": [{"id": "a"}, {"id": "b"}]
            }))
            .unwrap(),
        );
        files.insert(
            device_paths::PROFILES_JSON.to_string(),
            serde_json::to_vec(&profiles).unwrap(),
        );
        Arc::new(Self {
            firmware: Mutex::new(Firmware {
                attached: true,
                reports_runtime,
                active_slot: 0,
                actuation_mode: 0,
                platform: 3,
                slot_modes: vec![0, 0, 1],
                fail_next_write: false,
            }),
            files: Mutex::new(files),
            calls: Mutex::new(Vec::new()),
            saves: Mutex::new(Vec::new()),
            live: Mutex::new(Vec::new()),
            failing_reads: Mutex::new(0),
        })
    }

    fn press_profile_next(&self) {
        let mut fw = self.firmware.lock().unwrap();
        fw.active_slot = (fw.active_slot + 1) % fw.slot_modes.len() as u32;
        fw.actuation_mode = fw.slot_modes[fw.active_slot as usize];
    }

    fn press_actuation_mode_next(&self) {
        self.firmware.lock().unwrap().actuation_mode += 1;
    }

    fn press_platform(&self, code: u32) {
        self.firmware.lock().unwrap().platform = code;
    }

    /// The display's theme cycle: `selectedId` in `appearance.json` changes on the keyboard.
    fn display_selects_theme(&self, id: &str) {
        let mut files = self.files.lock().unwrap();
        let path = device_paths::APPEARANCE_JSON.to_string();
        let mut doc: Value = serde_json::from_slice(&files[&path]).unwrap();
        doc["selectedId"] = json!(id);
        files.insert(path, serde_json::to_vec(&doc).unwrap());
    }

    fn appearance_on_keyboard(&self) -> Value {
        serde_json::from_slice(&self.files.lock().unwrap()[device_paths::APPEARANCE_JSON]).unwrap()
    }

    /// The live data the host sent last.
    fn last_live(&self) -> Value {
        self.live
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("live data")
    }

    fn take_calls(&self) -> Vec<String> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }

    fn firmware_command(
        &self,
        call: String,
        apply: impl FnOnce(&mut Firmware),
    ) -> Result<(), String> {
        let mut fw = self.firmware.lock().unwrap();
        if !fw.attached {
            return Err("Keyboard firmware not connected".into());
        }
        if std::mem::take(&mut fw.fail_next_write) {
            return Err("firmware NAK".into());
        }
        apply(&mut fw);
        self.calls.lock().unwrap().push(call);
        Ok(())
    }
}

impl DeviceSink for MockKeyboard {
    fn is_connected(&self) -> (bool, bool) {
        (true, self.firmware.lock().unwrap().attached)
    }
    fn versions(&self) -> Value {
        json!({"firmware": "openflux-firmware 1.0.0", "bridge": "openflux-bridge 1.0.0"})
    }
    fn send_live_json(&self, payload: &Value) -> Result<(), String> {
        self.live.lock().unwrap().push(payload.clone());
        Ok(())
    }
    fn set_active_profile(&self, idx: u32) -> Result<(), String> {
        self.firmware_command(format!("profile:{idx}"), |fw| {
            fw.active_slot = idx;
            fw.actuation_mode = fw.slot_modes.get(idx as usize).copied().unwrap_or(0);
        })
    }
    fn set_language(&self, code: u32) -> Result<(), String> {
        self.firmware_command(format!("lang:{code}"), |_| {})
    }
    fn set_platform(&self, code: u32) -> Result<(), String> {
        self.firmware_command(format!("platform:{code}"), |fw| fw.platform = code)
    }
    fn read_firmware_runtime(&self) -> Result<FirmwareRuntime, String> {
        let fw = self.firmware.lock().unwrap();
        if !fw.attached || !fw.reports_runtime {
            return Err("not reported".into());
        }
        Ok(FirmwareRuntime {
            active_profile_slot: fw.active_slot,
            actuation_mode_index: fw.actuation_mode,
            platform_code: fw.platform,
        })
    }
    fn get_file(&self, path: &str) -> Result<Vec<u8>, String> {
        let mut failing = self.failing_reads.lock().unwrap();
        if *failing > 0 {
            *failing -= 1;
            return Err("USB transfer timed out".into());
        }
        drop(failing);
        self.files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| format!("{path} not found"))
    }
    fn stat_file(&self, path: &str) -> Result<bool, String> {
        Ok(self.files.lock().unwrap().contains_key(path))
    }
    fn save_config_dir(&self, plan: &Value) -> Result<(), String> {
        let mut files = self.files.lock().unwrap();
        for (key, path) in [
            ("appearance", device_paths::APPEARANCE_JSON),
            ("settings", device_paths::SETTINGS_JSON),
            ("mosaic", device_paths::MOSAIC_JSON),
        ] {
            if let Some(doc) = plan.get(key) {
                files.insert(path.to_string(), serde_json::to_vec(doc).unwrap());
            }
        }
        self.saves.lock().unwrap().push(plan.clone());
        Ok(())
    }
    fn pull_config_dir(&self) -> Result<Value, String> {
        let files = self.files.lock().unwrap();
        let profiles: Value = serde_json::from_slice(&files[device_paths::PROFILES_JSON]).unwrap();
        let slot_modes = self.firmware.lock().unwrap().slot_modes.clone();
        let settings = openfluxd::codecs::keymap::ProfileSettingsFile {
            settings: slot_modes
                .iter()
                .map(|m| openfluxd::codecs::keymap::ProfileSetting::from_layer(*m, 0).unwrap())
                .collect(),
            file_version: 0,
            file_type: 0,
        }
        .to_transfer_bytes()
        .unwrap();
        let mut out = json!({
            "profiles": profiles,
            "firmwareProfileSettingsHex": hex(&settings),
            "keymap": serde_json::from_str::<Value>(include_str!("fixtures/default_keymap.json")).unwrap(),
            "firmwareKeymapBlobHex": hex(include_bytes!("fixtures/default_keymap.bin")),
        });
        if let Some(raw) = files.get(device_paths::APPEARANCE_JSON) {
            out["appearance"] = serde_json::from_slice(raw).unwrap();
        }
        Ok(out)
    }
}

/// The host's side: the foreground app and the input language.
#[derive(Clone, Default)]
struct Host {
    window: Arc<Mutex<Option<ActiveWindowInfo>>>,
    lang: Arc<Mutex<u32>>,
}

impl Host {
    fn new() -> Self {
        let h = Self::default();
        *h.lang.lock().unwrap() = 471;
        h
    }
    fn focus(&self, app: &str) {
        *self.window.lock().unwrap() = Some(ActiveWindowInfo {
            app_id: app.into(),
            process_name: app.into(),
            window_title: format!("{app} window"),
        });
    }
}

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
        self.window.lock().unwrap().clone()
    }
    fn active_language_code(&mut self) -> Option<u32> {
        Some(*self.lang.lock().unwrap())
    }
    fn media_snapshot(&mut self) -> MediaSnapshot {
        MediaSnapshot::default()
    }
}

struct Daemon {
    addr: std::net::SocketAddr,
    state: Arc<ApiState>,
    keyboard: Arc<MockKeyboard>,
    host: Host,
}

impl Daemon {
    async fn start(keyboard: Arc<MockKeyboard>, state: Arc<ApiState>) -> Self {
        let app = build_router(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            addr,
            state,
            keyboard,
            host: Host::new(),
        }
    }

    async fn new(reports_runtime: bool) -> Self {
        let keyboard = MockKeyboard::new(reports_runtime);
        let state = ApiState::with_dirs(
            keyboard.clone(),
            Some(TOKEN.into()),
            Duration::from_secs(20),
            temp_dir("icons"),
        );
        Self::start(keyboard, state).await
    }

    /// One pass of the daemon's keyboard loop.
    fn tick(&self) {
        let mut host = self.host.clone();
        reconcile_tick(&self.state, &mut host, &*self.keyboard, MACOS);
    }

    /// Adds an app rule sending `matcher` to the profile at `profile_index`. Rules refer to the
    /// profile list, so they are added once the keyboard's profiles are loaded.
    fn add_rule(&self, matcher: &str, profile_index: u32) {
        self.state
            .inner
            .lock()
            .unwrap()
            .app_rules
            .push(AppProfileRule {
                matcher: matcher.into(),
                profile_index,
                language_code: None,
            });
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

    async fn status(&self) -> Value {
        let (code, body) = self.request("GET", "/v1/device/status", None).await;
        assert_eq!(code, 200, "{body}");
        body
    }

    async fn config(&self) -> Value {
        let (code, body) = self.request("GET", "/v1/config", None).await;
        assert_eq!(code, 200, "{body}");
        body
    }
}

#[tokio::test]
async fn a_profile_chosen_on_the_keyboard_becomes_the_desired_profile_and_stays() {
    let d = Daemon::new(true).await;
    let mut events = d.state.events.subscribe();
    d.tick();
    assert_eq!(
        d.keyboard.take_calls(),
        ["platform:2", "profile:0", "lang:471"]
    );

    d.keyboard.press_profile_next();
    d.keyboard.press_profile_next();
    for _ in 0..3 {
        d.tick();
    }
    assert_eq!(
        d.keyboard.take_calls(),
        Vec::<String>::new(),
        "nothing is sent back"
    );
    assert_eq!(d.keyboard.firmware.lock().unwrap().active_slot, 2);

    let st = d.status().await;
    assert_eq!(st["desiredProfileIndex"], 2);
    assert_eq!(st["appliedProfileIndex"], 2);
    assert_eq!(st["firmwareRuntime"]["activeProfileSlot"], 2);
    assert_eq!(d.config().await["desiredProfileIndex"], 2);
    let ev = std::iter::from_fn(|| events.try_recv().ok())
        .find(|e| e.event == "profileChange")
        .expect("a profileChange event");
    assert_eq!(ev.data, json!({"profileIndex": 2, "source": "keyboard"}));

    // A selection made in Studio afterwards is sent as usual.
    let (code, _) = d
        .request("PUT", "/v1/config", Some(json!({"desiredProfileIndex": 1})))
        .await;
    assert_eq!(code, 200);
    d.tick();
    assert_eq!(d.keyboard.take_calls(), ["profile:1"]);
}

#[tokio::test]
async fn app_rules_resume_on_the_next_foreground_app_change() {
    let d = Daemon::new(true).await;
    d.tick();
    assert_eq!(
        d.keyboard.take_calls(),
        ["platform:2", "profile:0", "lang:471"]
    );
    d.add_rule("code", 1);
    d.host.focus("code");
    d.tick();
    assert_eq!(d.keyboard.take_calls(), ["profile:1"]);

    // The keyboard moves on to `gaming` while the editor is still in front.
    d.keyboard.press_profile_next();
    d.tick();
    d.host.focus("code"); // a new window title of the same app
    d.tick();
    assert_eq!(d.keyboard.take_calls(), Vec::<String>::new());
    let st = d.status().await;
    assert_eq!(st["appliedProfileIndex"], 2);
    assert_eq!(st["desiredProfileIndex"], 2);
    assert_eq!(
        st["autoProfileSwitch"],
        json!({"locked": false, "heldUntilAppChange": true})
    );

    // Another app without a rule: the keyboard's choice is still the profile.
    d.host.focus("browser");
    d.tick();
    assert_eq!(d.keyboard.take_calls(), Vec::<String>::new());
    assert_eq!(
        d.status().await["autoProfileSwitch"]["heldUntilAppChange"],
        false
    );

    // Back to the editor: its rule switches again.
    d.host.focus("code");
    d.tick();
    assert_eq!(d.keyboard.take_calls(), ["profile:1"]);
}

#[tokio::test]
async fn a_platform_chosen_on_the_keyboard_stays_until_the_keyboard_attaches_again() {
    let d = Daemon::new(true).await;
    d.tick();
    assert_eq!(
        d.keyboard.take_calls(),
        ["platform:2", "profile:0", "lang:471"]
    );

    d.keyboard.press_platform(1);
    d.tick();
    // A failed command makes the daemon send the profile and language again, but not the platform.
    d.keyboard.firmware.lock().unwrap().fail_next_write = true;
    *d.host.lang.lock().unwrap() = 334;
    d.tick();
    d.tick();
    assert_eq!(d.keyboard.take_calls(), ["profile:0", "lang:334"]);
    assert_eq!(d.keyboard.firmware.lock().unwrap().platform, 1);
    assert_eq!(
        d.status().await["platform"],
        json!({"code": 1, "source": "keyboard"})
    );

    // Unplugged and plugged in again: the host's platform is sent once more.
    d.keyboard.firmware.lock().unwrap().attached = false;
    d.tick();
    d.keyboard.firmware.lock().unwrap().attached = true;
    d.tick();
    assert_eq!(
        d.keyboard.take_calls(),
        ["platform:2", "profile:0", "lang:334"]
    );
}

#[tokio::test]
async fn without_a_runtime_report_the_platform_is_still_sent_once_per_attachment() {
    let d = Daemon::new(false).await;
    d.tick();
    d.keyboard.press_platform(1);
    d.keyboard.firmware.lock().unwrap().fail_next_write = true;
    *d.host.lang.lock().unwrap() = 334;
    d.tick();
    d.tick();
    assert_eq!(
        d.keyboard.take_calls(),
        [
            "platform:2",
            "profile:0",
            "lang:471",
            "profile:0",
            "lang:334"
        ]
    );
    assert_eq!(d.keyboard.firmware.lock().unwrap().platform, 1);
    let st = d.status().await;
    assert_eq!(st["platform"], json!({"code": 2, "source": "host"}));
    assert_eq!(st["firmwareRuntime"], Value::Null);
}

#[tokio::test]
async fn the_actuation_mode_shows_in_the_status() {
    let d_quiet = Daemon::new(false).await;
    d_quiet.tick();
    let modes = d_quiet.config().await["actuationModes"].clone();
    let st = d_quiet.status().await;
    assert_eq!(
        st["actuationMode"],
        json!({"index": 0, "name": modes[0]["name"], "source": "profile"})
    );

    let d_reporting = Daemon::new(true).await;
    d_reporting.tick();
    d_reporting.keyboard.press_actuation_mode_next();
    d_reporting.tick();
    let modes = d_reporting.config().await["actuationModes"].clone();
    assert_eq!(
        d_reporting.status().await["actuationMode"],
        json!({"index": 1, "name": modes[1]["name"], "source": "keyboard"})
    );
    assert_eq!(
        d_reporting.keyboard.take_calls(),
        ["platform:2", "profile:0", "lang:471"]
    );
}

#[tokio::test]
async fn a_save_keeps_the_theme_chosen_on_the_keyboard_unless_it_changes_the_theme() {
    let d = Daemon::new(true).await;
    d.tick();
    let cached = d.config().await["appearance"].clone();
    assert_eq!(cached["selectedId"], "a");

    d.keyboard.display_selects_theme("b");

    // Studio edits the default theme, sending its whole (stale) appearance.
    let mut edited = cached.clone();
    edited["defaultTheme"]["legendColor"] = json!(2);
    let (code, _) = d
        .request("PUT", "/v1/config", Some(json!({"appearance": edited})))
        .await;
    assert_eq!(code, 200);
    let (code, body) = d.request("POST", "/v1/config/save", None).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(
        body["keptFromKeyboard"],
        json!({"appearance": ["selectedId"]})
    );
    let on_keyboard = d.keyboard.appearance_on_keyboard();
    assert_eq!(on_keyboard["selectedId"], "b");
    assert_eq!(on_keyboard["defaultTheme"]["legendColor"], 2);
    assert_eq!(d.config().await["appearance"]["selectedId"], "b");

    // A save that picks a theme writes it.
    let mut picked = d.config().await["appearance"].clone();
    picked["selectedId"] = json!("a");
    d.request("PUT", "/v1/config", Some(json!({"appearance": picked})))
        .await;
    d.keyboard.display_selects_theme("b");
    let (code, body) = d.request("POST", "/v1/config/save", None).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["keptFromKeyboard"], json!({}));
    assert_eq!(d.keyboard.appearance_on_keyboard()["selectedId"], "a");
}

#[tokio::test]
async fn the_auto_profile_lock_keeps_app_rules_from_switching() {
    let dir = temp_dir("lock");
    let keyboard = MockKeyboard::new(true);
    let state = ApiState::with_persistent_stores(
        keyboard.clone(),
        Some(TOKEN.into()),
        Duration::from_secs(20),
        dir.join("icons"),
        dir.join("tokens.json"),
        "secret".into(),
    );
    let d = Daemon::start(keyboard, state).await;
    let mut events = d.state.events.subscribe();
    d.tick();
    assert_eq!(
        d.keyboard.take_calls(),
        ["platform:2", "profile:0", "lang:471"]
    );
    d.add_rule("code", 1);
    d.add_rule("game", 2);
    d.host.focus("code");
    d.tick();
    assert_eq!(d.keyboard.take_calls(), ["profile:1"]);

    assert!(d.state.toggle_auto_profile_lock());
    let st = d.status().await;
    assert_eq!(st["autoProfileSwitch"]["locked"], true);
    assert_eq!(
        st["desiredProfileIndex"], 1,
        "locking keeps the profile in effect"
    );
    let ev = std::iter::from_fn(|| events.try_recv().ok())
        .find(|e| e.event == "autoProfileLock")
        .expect("an autoProfileLock event");
    assert_eq!(ev.data, json!({"locked": true}));

    d.host.focus("game");
    d.tick();
    d.host.focus("browser");
    d.tick();
    // An app reported through the API is held back too.
    let (code, _) = d
        .request(
            "POST",
            "/v1/config/updateActiveProcess",
            Some(json!({"process": "game"})),
        )
        .await;
    assert_eq!(code, 200);
    assert_eq!(d.keyboard.take_calls(), Vec::<String>::new());

    // The lock outlives a restart.
    let again = ApiState::with_persistent_stores(
        Arc::new(openfluxd::api::state::NoopDeviceSink),
        None,
        Duration::from_secs(20),
        dir.join("icons"),
        dir.join("tokens.json"),
        "secret".into(),
    );
    assert!(again.inner.lock().unwrap().auto_profile_lock);

    assert!(!d.state.toggle_auto_profile_lock());
    d.host.focus("game");
    d.tick();
    assert_eq!(d.keyboard.take_calls(), ["profile:2"]);
    assert_eq!(d.status().await["autoProfileSwitch"]["locked"], false);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The keyboard's display running on the live data the host sends, as far as themes go.
struct Display {
    cfg: openflux_display::config::DisplayConfig,
    st: openflux_display::config::LiveState,
    seen: usize,
}

impl Display {
    fn new(keyboard: &MockKeyboard) -> Self {
        let mut cfg = openflux_display::config::DisplayConfig::default();
        cfg.load_appearance_json(&keyboard.appearance_on_keyboard().to_string());
        let files = keyboard.files.lock().unwrap();
        cfg.load_profiles_json(std::str::from_utf8(&files[device_paths::PROFILES_JSON]).unwrap());
        let st = openflux_display::config::LiveState::from_config(&cfg);
        Self { cfg, st, seen: 0 }
    }

    /// Takes in the live data sent since the last call and returns the theme shown.
    fn follow(&mut self, keyboard: &MockKeyboard) -> String {
        let live = keyboard.live.lock().unwrap();
        for msg in &live[self.seen..] {
            self.st.apply_live_json(&msg.to_string(), 0).unwrap();
            self.cfg.reconcile_live_theme(&self.st);
        }
        self.seen = live.len();
        self.cfg.active_theme_id.clone()
    }

    /// The theme cycle key: the display shows the next theme and writes it to `appearance.json`.
    fn cycle_theme(&mut self, keyboard: &MockKeyboard) -> String {
        assert!(self.cfg.cycle_theme(1, &mut self.st, None));
        let id = self.cfg.active_theme_id.clone();
        keyboard.display_selects_theme(&id);
        id
    }
}

fn theme_rev(live: &Value) -> u64 {
    live["themeRev"].as_u64().expect("themeRev in live data")
}

#[tokio::test]
async fn a_theme_picked_on_the_keyboard_stays_until_a_profile_is_picked_in_studio() {
    let d = Daemon::new(true).await;
    let mut display = Display::new(&d.keyboard);
    d.tick();
    let first = d.keyboard.last_live();
    assert_eq!(first["activeThemeId"], "a");
    assert_eq!(first["activeThemeProfileSlot"], 0);
    assert_eq!(display.follow(&d.keyboard), "a");

    assert_eq!(display.cycle_theme(&d.keyboard), "b");
    // Keepalives repeat the host's theme with the same rev; the display keeps the keyboard's pick.
    d.tick();
    d.tick();
    let keepalive = d.keyboard.last_live();
    assert_eq!(keepalive["activeThemeId"], "a");
    assert_eq!(theme_rev(&keepalive), theme_rev(&first));
    assert_eq!(display.follow(&d.keyboard), "b");

    // A save takes the keyboard's selection over without a new rev.
    let mut edited = d.config().await["appearance"].clone();
    edited["defaultTheme"]["legendColor"] = json!(2);
    d.request("PUT", "/v1/config", Some(json!({"appearance": edited})))
        .await;
    let (code, body) = d.request("POST", "/v1/config/save", None).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(
        body["keptFromKeyboard"],
        json!({"appearance": ["selectedId"]})
    );
    d.tick();
    assert_eq!(theme_rev(&d.keyboard.last_live()), theme_rev(&first));
    assert_eq!(display.follow(&d.keyboard), "b");

    // Picking the profile in Studio, even the one in effect, chooses its theme again.
    let (code, _) = d
        .request("PUT", "/v1/config", Some(json!({"desiredProfileIndex": 0})))
        .await;
    assert_eq!(code, 200);
    d.tick();
    let picked = d.keyboard.last_live();
    assert_eq!(picked["activeThemeId"], "a");
    assert_ne!(theme_rev(&picked), theme_rev(&first));
    assert_eq!(display.follow(&d.keyboard), "a");
    d.tick();
    assert_eq!(theme_rev(&d.keyboard.last_live()), theme_rev(&picked));
}

#[tokio::test]
async fn the_theme_rev_changes_when_the_host_changes_the_theme() {
    let d = Daemon::new(true).await;
    d.tick();
    let rev0 = theme_rev(&d.keyboard.last_live());
    d.add_rule("code", 1);
    d.add_rule("game", 2);

    // `coding` has the same theme as `default`: no new rev.
    d.host.focus("code");
    d.tick();
    let live = d.keyboard.last_live();
    assert_eq!(live["activeThemeProfileSlot"], 1);
    assert_eq!(theme_rev(&live), rev0);

    // `gaming` has theme `b`.
    d.host.focus("game");
    d.tick();
    let live = d.keyboard.last_live();
    assert_eq!(live["activeThemeId"], "b");
    let rev1 = theme_rev(&live);
    assert_ne!(rev1, rev0);

    // A profile chosen on the keyboard brings its theme without a new rev.
    d.keyboard.press_profile_next();
    d.tick();
    d.tick();
    let live = d.keyboard.last_live();
    assert_eq!(live["activeThemeProfileSlot"], 0);
    assert_eq!(live["activeThemeId"], "a");
    assert_eq!(theme_rev(&live), rev1);

    // Profiles read from the keyboard with another theme for the active profile: no new rev.
    let mut on_keyboard: Value =
        serde_json::from_slice(&d.keyboard.files.lock().unwrap()[device_paths::PROFILES_JSON])
            .unwrap();
    on_keyboard[0]["theme_id"] = json!("b");
    d.keyboard.files.lock().unwrap().insert(
        device_paths::PROFILES_JSON.to_string(),
        serde_json::to_vec(&on_keyboard).unwrap(),
    );
    let (code, body) = d
        .request(
            "POST",
            "/v1/config/pull",
            Some(json!({"discardUnsaved": true})),
        )
        .await;
    assert_eq!(code, 200, "{body}");
    d.tick();
    let live = d.keyboard.last_live();
    assert_eq!(live["activeThemeId"], "b");
    assert_eq!(theme_rev(&live), rev1);

    // Studio changes the active profile's theme.
    let mut profiles = d.config().await["profiles"].clone();
    profiles[0]["theme_id"] = json!("a");
    let (code, _) = d
        .request("PUT", "/v1/config", Some(json!({"profiles": profiles})))
        .await;
    assert_eq!(code, 200);
    d.tick();
    let live = d.keyboard.last_live();
    assert_eq!(live["activeThemeId"], "a");
    assert_ne!(theme_rev(&live), rev1);
}

#[tokio::test]
async fn a_restarted_daemon_sends_the_theme_rev_it_stored() {
    let dir = temp_dir("theme_rev");
    let keyboard = MockKeyboard::new(true);
    let open = |keyboard: &Arc<MockKeyboard>| {
        ApiState::with_persistent_stores(
            keyboard.clone(),
            Some(TOKEN.into()),
            Duration::from_secs(20),
            dir.join("icons"),
            dir.join("tokens.json"),
            "secret".into(),
        )
    };
    let d = Daemon::start(keyboard.clone(), open(&keyboard)).await;
    d.tick();
    d.request("PUT", "/v1/config", Some(json!({"desiredProfileIndex": 0})))
        .await;
    d.tick();
    let rev = theme_rev(&d.keyboard.last_live());

    let again = Daemon::start(keyboard.clone(), open(&keyboard)).await;
    again.tick();
    assert_eq!(theme_rev(&again.keyboard.last_live()), rev);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn studio_locks_and_unlocks_automatic_profile_switching() {
    let d = Daemon::new(true).await;
    d.tick();
    d.add_rule("code", 1);
    d.keyboard.take_calls();

    let (code, body) = d
        .request(
            "PUT",
            "/v1/profiles/autoLock",
            Some(json!({"locked": true})),
        )
        .await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body, json!({"locked": true, "heldUntilAppChange": false}));
    d.host.focus("code");
    d.tick();
    assert_eq!(d.keyboard.take_calls(), Vec::<String>::new());
    assert_eq!(d.status().await["autoProfileSwitch"]["locked"], true);

    let (code, body) = d
        .request(
            "PUT",
            "/v1/profiles/autoLock",
            Some(json!({"locked": "no"})),
        )
        .await;
    assert_eq!(code, 400, "{body}");

    let (code, body) = d
        .request(
            "PUT",
            "/v1/profiles/autoLock",
            Some(json!({"locked": false})),
        )
        .await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body, json!({"locked": false, "heldUntilAppChange": false}));
    d.tick();
    assert_eq!(d.keyboard.take_calls(), ["profile:1"]);
}

#[tokio::test]
async fn a_save_that_cant_read_the_keyboards_theme_writes_nothing() {
    let d = Daemon::new(true).await;
    d.tick();
    d.keyboard.display_selects_theme("b");
    let mut edited = d.config().await["appearance"].clone();
    assert_eq!(edited["selectedId"], "a");
    edited["defaultTheme"]["legendColor"] = json!(2);
    d.request("PUT", "/v1/config", Some(json!({"appearance": edited})))
        .await;

    *d.keyboard.failing_reads.lock().unwrap() = 1;
    let (code, body) = d.request("POST", "/v1/config/save", None).await;
    assert_ne!(code, 200, "{body}");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("try saving again"),
        "{body}"
    );
    assert!(d.keyboard.saves.lock().unwrap().is_empty());
    assert_eq!(d.keyboard.appearance_on_keyboard()["selectedId"], "b");

    // Tried again, the save keeps the keyboard's theme.
    let (code, body) = d.request("POST", "/v1/config/save", None).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(
        body["keptFromKeyboard"],
        json!({"appearance": ["selectedId"]})
    );
    let on_keyboard = d.keyboard.appearance_on_keyboard();
    assert_eq!(on_keyboard["selectedId"], "b");
    assert_eq!(on_keyboard["defaultTheme"]["legendColor"], 2);

    // A keyboard without the file gets the cached document.
    d.keyboard
        .files
        .lock()
        .unwrap()
        .remove(device_paths::APPEARANCE_JSON);
    let mut edited = d.config().await["appearance"].clone();
    edited["defaultTheme"]["legendColor"] = json!(5);
    d.request("PUT", "/v1/config", Some(json!({"appearance": edited})))
        .await;
    let (code, body) = d.request("POST", "/v1/config/save", None).await;
    assert_eq!(code, 200, "{body}");
    let on_keyboard = d.keyboard.appearance_on_keyboard();
    assert_eq!(on_keyboard["selectedId"], "b");
    assert_eq!(on_keyboard["defaultTheme"]["legendColor"], 5);
}

#[tokio::test]
async fn a_theme_picked_in_studio_is_saved_even_when_it_ends_on_the_loaded_one() {
    let d = Daemon::new(true).await;
    d.tick();
    let loaded = d.config().await["appearance"].clone();
    assert_eq!(loaded["selectedId"], "a");
    d.keyboard.display_selects_theme("b");

    // Studio picks `b`, then `a` again, before saving.
    for id in ["b", "a"] {
        let mut picked = d.config().await["appearance"].clone();
        picked["selectedId"] = json!(id);
        let (code, _) = d
            .request("PUT", "/v1/config", Some(json!({"appearance": picked})))
            .await;
        assert_eq!(code, 200);
    }
    // An edit of something else afterwards doesn't undo the pick.
    let mut edited = d.config().await["appearance"].clone();
    edited["defaultTheme"]["legendColor"] = json!(3);
    d.request("PUT", "/v1/config", Some(json!({"appearance": edited})))
        .await;
    let (code, body) = d.request("POST", "/v1/config/save", None).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["keptFromKeyboard"], json!({}));
    assert_eq!(d.keyboard.appearance_on_keyboard()["selectedId"], "a");

    // Once saved, the keyboard's own picks are kept again.
    d.keyboard.display_selects_theme("b");
    let mut edited = d.config().await["appearance"].clone();
    edited["defaultTheme"]["legendColor"] = json!(4);
    d.request("PUT", "/v1/config", Some(json!({"appearance": edited})))
        .await;
    let (code, body) = d.request("POST", "/v1/config/save", None).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(d.keyboard.appearance_on_keyboard()["selectedId"], "b");

    // Picking the theme Studio already shows counts when it says it is a pick.
    d.keyboard.display_selects_theme("a");
    let same = d.config().await["appearance"].clone();
    assert_eq!(same["selectedId"], "b");
    let (code, _) = d
        .request(
            "PUT",
            "/v1/config",
            Some(json!({"appearance": same, "themeSelection": true})),
        )
        .await;
    assert_eq!(code, 200);
    let (code, body) = d.request("POST", "/v1/config/save", None).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(d.keyboard.appearance_on_keyboard()["selectedId"], "b");
}

#[tokio::test]
async fn picking_the_profile_in_studio_saves_its_theme_over_the_keyboards() {
    let d = Daemon::new(true).await;
    d.tick();
    d.keyboard.display_selects_theme("b");
    let (code, _) = d
        .request("PUT", "/v1/config", Some(json!({"desiredProfileIndex": 0})))
        .await;
    assert_eq!(code, 200);
    let mut edited = d.config().await["appearance"].clone();
    assert_eq!(edited["selectedId"], "a");
    edited["defaultTheme"]["legendColor"] = json!(2);
    d.request("PUT", "/v1/config", Some(json!({"appearance": edited})))
        .await;
    let (code, body) = d.request("POST", "/v1/config/save", None).await;
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["keptFromKeyboard"], json!({}));
    assert_eq!(d.keyboard.appearance_on_keyboard()["selectedId"], "a");
}
