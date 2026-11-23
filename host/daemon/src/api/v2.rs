//! OpenFlux v2 host runtime and open localhost HTTP/WebSocket API.
//!
//! Exposes the hierarchical pub/sub bus (`openflux-bus`), unified module management & disclosure
//! inspection (`openflux-modules`), unrestricted host process execution, curated lists, secret-free
//! presets, synthetic input simulation, and live GPU compositor previews (`openflux-compositor`).

use crate::api::state::{default_data_dir, ApiState};
use crate::usb::vial::{ModuleActionsMap, VialHidRelay, VIAL_RAW_EPSIZE};
use axum::extract::ws::{Message as WsMessage, WebSocket};
use axum::extract::{Path as AxumPath, Query, Request, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use base64::Engine;
use openflux_bus::{
    topics, AnalogKeyFrame, AnalogKeySample, Broker, FluxActionEvent, FrameLayout,
    HostConnectionState, KeyEvent, KeyEventKind, KeyboardGeometry, PublishOptions,
    SubscribeOptions, SurfaceImpulse, WireFrame,
};
use openflux_compositor::{Compositor, CompositorOptions};
use openflux_modules::{
    inspect_module_dir, CuratedListSubscription, HostProcessManager, ModuleCategory, ModuleInstanceState, ModuleRegistry, ModuleRuntime, SetupPreset,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

pub struct V2State {
    /// Authoritative live host bus mirrored cleanly with the keyboard over OFV2.
    pub broker: Broker,
    /// Isolated preview-only broker for host-side display Lua modules (`runtime`).
    /// Fed strictly one-way (`broker -> preview_broker`) so Studio previews NEVER overwrite
    /// keyboard `keys/providers/*`, blank keycaps on unplug, or double-fire Flux actions!
    pub preview_broker: Broker,
    pub registry: ModuleRegistry,
    /// Display Lua runtime for Studio Live Preview (strictly isolated on `preview_broker`).
    pub runtime: Mutex<ModuleRuntime>,
    /// Non-display host-logic Lua runtime (`stages.is_empty()`, e.g. `analog-gamepad` App Linking)
    /// running cleanly on the live `broker` without ever emitting `keys/providers/*` display clutter!
    pub host_logic_runtime: Mutex<ModuleRuntime>,
    pub host_procs: Mutex<HostProcessManager>,
    pub modules_rev: AtomicU64,
    last_disk_sig: Mutex<String>,
    compositor: Mutex<Option<Compositor>>,
    geometry: KeyboardGeometry,
    pub vial_relay: VialHidRelay,
    pub vial_actions: Mutex<ModuleActionsMap>,
    vial_actions_path: PathBuf,
}

fn resolve_bundled_modules_dir() -> PathBuf {
    if let Ok(env_dir) = std::env::var("OPENFLUX_BUNDLED_MODULES_DIR") {
        if !env_dir.is_empty() {
            return PathBuf::from(env_dir);
        }
    }
    let mut candidates = vec![PathBuf::from("modules")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(p) = exe.parent() {
            candidates.push(p.join("modules"));
            if let Some(pp) = p.parent() {
                candidates.push(pp.join("Resources/modules"));
            }
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(&home).join("Applications/OpenFlux Studio.app/Contents/Resources/modules"));
        candidates.push(PathBuf::from(&home).join("git/flux/modules"));
    }
    for c in candidates {
        if c.is_dir() {
            return c;
        }
    }
    PathBuf::from("modules")
}

fn load_font_bytes() -> Option<Vec<u8>> {
    let mut candidates = vec![
        PathBuf::from("assets/bundled_fonts/Montserrat-Medium.ttf"),
        PathBuf::from("assets/bundled_fonts/Montserrat-VariableFont_wght.ttf"),
        PathBuf::from("dist/assets/fonts/Montserrat-Medium.ttf"),
        PathBuf::from("dist/assets/fonts/Montserrat-VariableFont_wght.ttf"),
    ];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(pp) = exe.parent().and_then(|p| p.parent()) {
            candidates.push(pp.join("Resources/assets/bundled_fonts/Montserrat-Medium.ttf"));
            candidates.push(pp.join("Resources/assets/bundled_fonts/Montserrat-VariableFont_wght.ttf"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(
            PathBuf::from(&home)
                .join("Applications/OpenFlux Studio.app/Contents/Resources/assets/bundled_fonts/Montserrat-Medium.ttf"),
        );
        candidates.push(
            PathBuf::from(&home)
                .join("Applications/OpenFlux Studio.app/Contents/Resources/assets/bundled_fonts/Montserrat-VariableFont_wght.ttf"),
        );
    }
    candidates.push(PathBuf::from("/System/Library/Fonts/Helvetica.ttc"));
    candidates.push(PathBuf::from("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"));
    for p in candidates {
        if let Ok(b) = std::fs::read(&p) {
            return Some(b);
        }
    }
    None
}

impl V2State {
    pub fn new(data_dir: Option<PathBuf>, bus_token: Option<String>) -> Arc<Self> {
        let base = data_dir.unwrap_or_else(default_data_dir);
        let bundled = resolve_bundled_modules_dir();
        let registry = ModuleRegistry::new(
            bundled,
            base.join("v2/modules/installed"),
            base.join("v2/modules/local"),
            base.join("v2/modules_state.json"),
        );
        let _ = registry.ensure_dirs();

        let broker = Broker::new("host");
        let preview_broker = Broker::new("host-preview");
        let geometry = KeyboardGeometry::build(FrameLayout::Ansi);
        let _ = broker.publish_typed(
            topics::KEYBOARD_GEOMETRY,
            &geometry,
            PublishOptions::retained(),
        );
        let _ = preview_broker.publish_typed(
            topics::KEYBOARD_GEOMETRY,
            &geometry,
            PublishOptions::retained(),
        );
        let _ = broker.publish_typed(
            topics::HOST_CONNECTION,
            &HostConnectionState {
                connected: true,
                host_asleep: false,
                last_seen_ms: openflux_bus::now_unix_ms(),
                host_platform: Some(std::env::consts::OS.to_string()),
            },
            PublishOptions::retained(),
        );
        // Seed live `display/parallax` and `display/power` from pure-v2 `v2/display_prefs.json`
        // (migrating once from legacy `config/settings.json` if present!).
        let prefs_path = base.join("v2/display_prefs.json");
        let prefs_doc: serde_json::Value = std::fs::read_to_string(&prefs_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .or_else(|| {
                std::fs::read_to_string(base.join("config/settings.json"))
                    .ok()
                    .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                    .map(|s| {
                        json!({
                            "parallax": {
                                "parallaxActive": s.get("parallaxActive").and_then(|v| v.as_bool()).unwrap_or(true),
                                "eyeHeight": s.get("eyeHeight").and_then(|v| v.as_f64()).unwrap_or(425.0),
                                "eyeDistanceFromFront": s.get("eyeDistanceFromFront").and_then(|v| v.as_f64()).unwrap_or(300.0),
                                "footStandMode": s.get("footStandMode").and_then(|v| v.as_str()).unwrap_or("none"),
                                "displayOffsetX": s.get("displayOffsetX").and_then(|v| v.as_i64()).unwrap_or(0),
                                "displayOffsetY": s.get("displayOffsetY").and_then(|v| v.as_i64()).unwrap_or(0),
                                "showAlignmentGrid": s.get("showAlignmentGrid").and_then(|v| v.as_bool()).unwrap_or(false),
                            },
                            "power": {
                                "brightness": s.get("brightness").and_then(|v| v.as_u64()).unwrap_or(80),
                                "timeoutSec": s.get("displayTimeoutSec").and_then(|v| v.as_u64()).unwrap_or(300),
                            }
                        })
                    })
            })
            .unwrap_or_else(|| {
                json!({
                    "parallax": {
                        "parallaxActive": true,
                        "eyeHeight": 425.0,
                        "eyeDistanceFromFront": 300.0,
                        "footStandMode": "none",
                        "displayOffsetX": 0,
                        "displayOffsetY": 0,
                        "showAlignmentGrid": false,
                    },
                    "power": {
                        "brightness": 80,
                        "timeoutSec": 300,
                    }
                })
            });
        if let Some(p) = prefs_doc.get("parallax") {
            let _ = broker.publish("display/parallax", p.clone(), PublishOptions::retained());
        }
        if let Some(pw) = prefs_doc.get("power") {
            let _ = broker.publish("display/power", pw.clone(), PublishOptions::retained());
        }
        let att_payload = build_attachments_bus_payload(prefs_doc.get("attachments"));
        let _ = broker.publish("keyboard/attachments", att_payload, PublishOptions::retained());

        // Run host-side display Lua modules ONLY inside the isolated `preview_broker` so they
        // never write onto the live keyboard-mirrored `broker` (no split-brain `keys/providers/*`,
        // no unplug keycap blanking, no double-firing actions!).
        let mut runtime = ModuleRuntime::new(preview_broker.clone(), geometry.clone());
        let mut host_logic_runtime = ModuleRuntime::new(broker.clone(), geometry.clone());
        let discovered = registry.discover();
        let mut state_file = registry.load_state();

        // Seed a great default v2 showcase stack on first run if no modules are configured yet.
        if state_file.modules.is_empty() && !discovered.is_empty() {
            let defaults = [
                ("bg-aurora-shader", None),
                ("dock-top-bar", None),
                ("keys-renderer", None),
                ("keys-keymap", None),
                ("keys-shortcuts", None),
                ("fx-water-ripple-3d", None),
                ("fx-analog-depth-glow", None),
                ("fx-key-particles", None),
                ("widget-clock", Some(0)),
                ("widget-system-load", Some(1)),
                ("widget-weather", Some(2)),
                ("companion-idle-pet", Some(3)),
            ];
            for (id, slot) in defaults {
                if discovered.contains_key(id) {
                    state_file.modules.insert(
                        id.to_string(),
                        ModuleInstanceState {
                            enabled: true,
                            slot,
                            slot_pos: slot.map(|s| s as f32),
                            slot_span: None,
                            pin_tab: None,
                            config: BTreeMap::new(),
                        },
                    );
                }
            }
            if state_file.curated_lists.is_empty() {
                let official_path = registry.bundled_dir.join("official-list.json");
                state_file.curated_lists.push(CuratedListSubscription {
                    id: "official".into(),
                    name: "OpenFlux Official Curated List".into(),
                    source: official_path.to_string_lossy().into_owned(),
                    official: true,
                });
            }
            let _ = registry.save_state(&state_file);
        }

        runtime.sync_modules(&discovered, &state_file.modules);
        let host_logic_discovered: BTreeMap<String, _> = discovered
            .iter()
            .filter(|(_, dm)| dm.manifest.stages.is_empty() && dm.manifest.entrypoints.script.is_some())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        host_logic_runtime.sync_modules(&host_logic_discovered, &state_file.modules);

        let mut host_procs = HostProcessManager::new("http://127.0.0.1:52324").with_bus_token(bus_token);
        for (id, dm) in &discovered {
            if let Some(inst) = state_file.modules.get(id) {
                if inst.enabled && dm.manifest.entrypoints.host.is_some() {
                    let _ = host_procs.start_module(dm, inst);
                }
            }
        }

        let vial_actions_path = base.join("v2/module_actions.json");
        let mut vial_actions = ModuleActionsMap::load(&vial_actions_path);
        vial_actions.reconcile_from_registry(&registry);
        let _ = vial_actions.save(&vial_actions_path);

        let initial_sig = Self::compute_disk_sig(&registry);
        let state = Arc::new(Self {
            broker,
            preview_broker,
            registry,
            runtime: Mutex::new(runtime),
            host_logic_runtime: Mutex::new(host_logic_runtime),
            host_procs: Mutex::new(host_procs),
            modules_rev: AtomicU64::new(1),
            last_disk_sig: Mutex::new(initial_sig),
            compositor: Mutex::new(None),
            geometry,
            vial_relay: VialHidRelay::new(),
            vial_actions: Mutex::new(vial_actions),
            vial_actions_path,
        });

        // Background watcher: detects direct edits to script.lua / module.toml / new local
        // folders / modules_state.json written by an AI agent or external editor and pushes
        // live `system/modules/changed` events so OpenFlux Studio updates immediately.
        let weak = Arc::downgrade(&state);
        std::thread::Builder::new()
            .name("openflux-v2-watch".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(350));
                let Some(st) = weak.upgrade() else {
                    break;
                };
                st.poll_disk_and_hot_reload();
            })
            .ok();

        state.publish_system_actions();
        state
    }

    fn compute_disk_sig(registry: &ModuleRegistry) -> String {
        let mut parts = Vec::new();
        let mtime_ms = |p: &std::path::Path| -> u128 {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|d| d.as_millis())
                .unwrap_or(0)
        };
        parts.push(format!("state:{}", mtime_ms(&registry.state_path)));
        for dir in [&registry.installed_dir, &registry.local_dir] {
            if let Ok(rd) = std::fs::read_dir(dir) {
                let mut names: Vec<String> = rd
                    .flatten()
                    .filter_map(|e| {
                        let p = e.path();
                        if p.is_dir() && p.join("module.toml").is_file() {
                            Some(format!(
                                "{}:{}",
                                p.display(),
                                mtime_ms(&p.join("module.toml"))
                            ))
                        } else {
                            None
                        }
                    })
                    .collect();
                names.sort();
                parts.extend(names);
            }
        }
        parts.join("|")
    }

    /// Copies retained live topics one-way (`broker -> preview_broker`) so Studio Preview sees
    /// live telemetry, clock, weather, Prusa status, and QMK keymap without ever writing back
    /// onto the live keyboard-mirrored bus!
    pub fn sync_live_to_preview(&self) {
        for msg in self.broker.snapshot_retained() {
            if msg.topic.starts_with("keys/providers/") {
                continue;
            }
            let _ = self.preview_broker.publish(
                msg.topic,
                msg.payload,
                PublishOptions::retained().with_origin("live-mirror"),
            );
        }
    }

    /// Stops all supervised host helper processes cleanly on daemon shutdown (zero orphans!).
    pub fn shutdown(&self) {
        self.host_procs.lock().unwrap().stop_all();
    }

    pub fn display_prefs_path(&self) -> PathBuf {
        self.registry
            .state_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("display_prefs.json")
    }

    pub fn load_display_prefs(&self) -> serde_json::Value {
        std::fs::read_to_string(self.display_prefs_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_else(|| {
                json!({
                    "parallax": self.broker.get_retained("display/parallax").map(|m| m.payload).unwrap_or_else(|| json!({
                        "parallaxActive": true,
                        "eyeHeight": 425.0,
                        "eyeDistanceFromFront": 300.0,
                        "footStandMode": "none",
                        "displayOffsetX": 0,
                        "displayOffsetY": 0,
                        "showAlignmentGrid": false,
                    })),
                    "power": self.broker.get_retained("display/power").map(|m| m.payload).unwrap_or_else(|| json!({
                        "brightness": 80,
                        "timeoutSec": 300,
                    })),
                })
            })
    }

    pub fn save_display_prefs(&self, doc: &serde_json::Value) {
        let path = self.display_prefs_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(doc) {
            let _ = std::fs::write(path, text);
        }
    }

    /// Publishes optical parallax settings onto retained `display/parallax` and persists them to
    /// pure-v2 `display_prefs.json` so the physical keyboard shifts keycaps in real time and
    /// remembers calibration across reboots!
    pub fn publish_parallax_from_settings(&self, s: &serde_json::Value) {
        let payload = serde_json::json!({
            "parallaxActive": s.get("parallaxActive").and_then(|v| v.as_bool()).unwrap_or(true),
            "eyeHeight": s.get("eyeHeight").and_then(|v| v.as_f64()).unwrap_or(425.0),
            "eyeDistanceFromFront": s.get("eyeDistanceFromFront").and_then(|v| v.as_f64()).unwrap_or(300.0),
            "footStandMode": s.get("footStandMode").and_then(|v| v.as_str()).unwrap_or("none"),
            "displayOffsetX": s.get("displayOffsetX").and_then(|v| v.as_i64()).unwrap_or(0),
            "displayOffsetY": s.get("displayOffsetY").and_then(|v| v.as_i64()).unwrap_or(0),
            "showAlignmentGrid": s.get("showAlignmentGrid").and_then(|v| v.as_bool()).unwrap_or(false),
        });
        let mut prefs = self.load_display_prefs();
        if let Some(obj) = prefs.as_object_mut() {
            obj.insert("parallax".into(), payload.clone());
        }
        self.save_display_prefs(&prefs);
        let _ = self.broker.publish("display/parallax", payload, PublishOptions::retained());
    }

    pub fn publish_power_prefs(&self, p: &serde_json::Value) {
        let payload = serde_json::json!({
            "brightness": p.get("brightness").and_then(|v| v.as_u64()).unwrap_or(80).clamp(5, 100),
            "timeoutSec": p.get("timeoutSec").and_then(|v| v.as_u64()).unwrap_or(300),
        });
        let mut prefs = self.load_display_prefs();
        if let Some(obj) = prefs.as_object_mut() {
            obj.insert("power".into(), payload.clone());
        }
        self.save_display_prefs(&prefs);
        let _ = self.broker.publish("display/power", payload, PublishOptions::retained());
    }

    /// Persists physical top-bar hardware attachments (Clear Window, Rotary Dial, Slider, Macro, etc.)
    /// and publishes `keyboard/attachments` with precomputed `clear_rects` & `blocked_rects` so ANY
    /// software module (like `ui-osd-toast` or `dock-top-bar`) can automatically dodge physical knobs!
    pub fn publish_attachments(&self, raw: &serde_json::Value) -> serde_json::Value {
        let payload = build_attachments_bus_payload(Some(raw));
        let mut prefs = self.load_display_prefs();
        if let Some(obj) = prefs.as_object_mut() {
            obj.insert("attachments".into(), payload.clone());
        }
        self.save_display_prefs(&prefs);
        let _ = self
            .broker
            .publish("keyboard/attachments", payload.clone(), PublishOptions::retained());
        payload
    }

    /// Reconciles enabled modules' advertised `[[actions]]` into QMK slots (`0..63`) and publishes
    /// the complete visual + dispatch registry onto retained `system/actions` (consumed by
    /// `keys-keymap`, `keys-custom`, keyboard standalone dispatch, and Studio's `KeycapsTab`!).
    pub fn publish_system_actions(&self) {
        let discovered = self.registry.discover();
        let mut slots_json = serde_json::Map::new();
        {
            let mut map = self.vial_actions.lock().unwrap();
            map.reconcile_from_registry(&self.registry);
            let _ = map.save(&self.vial_actions_path);
            for (slot, entry) in &map.entries {
                let module_dir = discovered
                    .get(entry.module_id.as_str())
                    .map(|dm| dm.path.to_string_lossy().into_owned());
                slots_json.insert(
                    slot.to_string(),
                    json!({
                        "slot": slot,
                        "keycode": entry.keycode,
                        "action_id": entry.action_id,
                        "module_id": entry.module_id,
                        "module_dir": module_dir,
                        "label": entry.name,
                        "short_label": entry.short_label,
                        "icon": entry.icon,
                        "accent": entry.accent,
                        "mode": entry.mode,
                        "topic": entry.topic,
                        "payload": entry.payload,
                        "release_payload": entry.release_payload,
                        "retain": entry.retain,
                        "active": entry.active,
                    }),
                );
            }
        }
        let _ = self.broker.publish(
            "system/actions",
            json!({ "slots": Value::Object(slots_json) }),
            PublishOptions::retained(),
        );
    }

    pub fn notify_modules_changed(&self, reason: &str, module_id: Option<&str>) {
        let rev = self.modules_rev.fetch_add(1, Ordering::SeqCst) + 1;
        *self.last_disk_sig.lock().unwrap() = Self::compute_disk_sig(&self.registry);
        self.publish_system_actions();
        let _ = self.broker.publish(
            "system/modules/changed",
            json!({
                "rev": rev,
                "reason": reason,
                "module_id": module_id,
                "timestamp_ms": openflux_bus::now_unix_ms(),
            }),
            PublishOptions::retained(),
        );
    }

    /// Dispatch a QK_KB_n (`0..63`) Flux/module custom key event received from the RP2040 over USB IF 4.
    /// Supports both `"tap"` actions (fire on press) and `"hold"` actions (fire on press AND release!).
    pub fn dispatch_flux_action(&self, slot: u16, pressed: bool, mods: u8, layer: u8) {
        let entry = self
            .vial_actions
            .lock()
            .ok()
            .and_then(|m| m.entries.get(&slot).cloned());
        let ev = FluxActionEvent {
            slot,
            action_id: entry.as_ref().map(|e| e.action_id.clone()),
            pressed,
            mods,
            layer,
            profile: 0,
        };
        let _ = self
            .broker
            .publish_typed(topics::INPUT_FLUX_ACTION, &ev, PublishOptions::default());

        if let Some(e) = entry {
            let sem_topic = format!("actions/{}", e.action_id.replace('.', "/").replace(':', "/"));
            let _ = self.broker.publish(
                &sem_topic,
                json!({ "pressed": pressed, "slot": slot, "mods": mods, "layer": layer }),
                PublishOptions::default(),
            );
            if let Some(topic) = e.topic {
                let opts = if e.retain {
                    PublishOptions::retained()
                } else {
                    PublishOptions::default()
                };
                if pressed {
                    let _ = self.broker.publish(&topic, e.payload, opts);
                } else if e.mode == "hold" {
                    let rel = e
                        .release_payload
                        .filter(|v| !v.is_null())
                        .unwrap_or_else(|| json!({ "active": false }));
                    let _ = self.broker.publish(&topic, rel, opts);
                }
            }
        }
    }

    pub fn poll_disk_and_hot_reload(&self) {
        let cur_sig = Self::compute_disk_sig(&self.registry);
        let disk_changed = {
            let mut prev = self.last_disk_sig.lock().unwrap();
            if *prev != cur_sig {
                *prev = cur_sig;
                true
            } else {
                false
            }
        };
        if disk_changed {
            self.sync_all_with_reason("disk_changed", None);
            return;
        }
        // Step non-display host-logic Lua modules (e.g. `analog-gamepad` App Linking on `host/app/active`)
        {
            let mut hrt = self.host_logic_runtime.lock().unwrap();
            let _ = hrt.check_hot_reload();
            let _ = hrt.step_frame(0.35, openflux_bus::now_unix_ms());
        }
        let reloaded = self.runtime.lock().unwrap().check_hot_reload();
        if !reloaded.is_empty() {
            for id in &reloaded {
                self.notify_modules_changed("hot_reload", Some(id));
            }
        }
    }

    pub fn sync_all(&self) {
        self.sync_all_with_reason("api_update", None);
    }

    pub fn sync_all_with_reason(&self, reason: &str, module_id: Option<&str>) {
        let discovered = self.registry.discover();
        let state = self.registry.load_state();
        self.runtime
            .lock()
            .unwrap()
            .sync_modules(&discovered, &state.modules);
        let host_logic_discovered: BTreeMap<String, _> = discovered
            .iter()
            .filter(|(_, dm)| dm.manifest.stages.is_empty() && dm.manifest.entrypoints.script.is_some())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        self.host_logic_runtime
            .lock()
            .unwrap()
            .sync_modules(&host_logic_discovered, &state.modules);

        let mut hp = self.host_procs.lock().unwrap();
        for (id, dm) in &discovered {
            let enabled = state.modules.get(id).is_some_and(|s| s.enabled);
            if enabled && dm.manifest.entrypoints.host.is_some() {
                let inst = state.modules.get(id).cloned().unwrap_or_default();
                let _ = hp.start_module(dm, &inst);
            } else {
                hp.stop_module(id);
            }
        }
        drop(hp);
        self.notify_modules_changed(reason, module_id);
    }
}

fn err_resp(status: StatusCode, msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (status, Json(json!({ "error": msg.into() })))
}

#[derive(Deserialize)]
struct BusPubBody {
    topic: String,
    payload: Value,
    #[serde(default)]
    retain: bool,
    #[serde(default)]
    ttl_ms: Option<u64>,
    #[serde(default)]
    origin: Option<String>,
}

async fn bus_pub(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<BusPubBody>,
) -> impl IntoResponse {
    let opts = PublishOptions {
        retain: body.retain,
        ttl_ms: body.ttl_ms,
        origin: body.origin,
        timestamp_ms: None,
    };
    match state.v2.broker.publish(body.topic, body.payload, opts) {
        Ok(msg) => (StatusCode::OK, Json(json!({ "message": msg }))),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

#[derive(Deserialize)]
struct PatternQuery {
    #[serde(default = "default_wildcard")]
    pattern: String,
}
fn default_wildcard() -> String {
    "#".to_string()
}

async fn bus_retained(
    State(state): State<Arc<ApiState>>,
    Query(q): Query<PatternQuery>,
) -> impl IntoResponse {
    match state.v2.broker.query_retained(&q.pattern) {
        Ok(messages) => (StatusCode::OK, Json(json!({ "messages": messages }))),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn bus_ws(
    State(state): State<Arc<ApiState>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_bus_ws(socket, state.v2.broker.clone()))
}

async fn handle_bus_ws(mut socket: WebSocket, broker: Broker) {
    let mut subs: Vec<openflux_bus::Subscription> = Vec::new();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(16));

    loop {
        tokio::select! {
            _ = tick.tick() => {
                let mut outbound = Vec::new();
                for sub in &subs {
                    outbound.extend(sub.drain());
                }
                for msg in outbound {
                    let frame = WireFrame::Message { message: msg };
                    if let Ok(txt) = serde_json::to_string(&frame) {
                        if socket.send(WsMessage::Text(txt)).await.is_err() {
                            return;
                        }
                    }
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(WsMessage::Text(txt))) => {
                        match WireFrame::from_json_str(&txt) {
                            Ok(WireFrame::Subscribe { pattern, replay_retained }) => {
                                let opts = SubscribeOptions {
                                    replay_retained,
                                    ignore_origin: None,
                                    capacity: 512,
                                };
                                match broker.subscribe(pattern, opts) {
                                    Ok(sub) => subs.push(sub),
                                    Err(e) => {
                                        let err = WireFrame::Error { message: e.to_string() };
                                        let _ = socket.send(WsMessage::Text(serde_json::to_string(&err).unwrap())).await;
                                    }
                                }
                            }
                            Ok(WireFrame::Unsubscribe { pattern }) => {
                                subs.retain(|s| s.pattern() != pattern);
                            }
                            Ok(WireFrame::Ping { nonce }) => {
                                let pong = WireFrame::Pong { nonce };
                                let _ = socket.send(WsMessage::Text(serde_json::to_string(&pong).unwrap())).await;
                            }
                            Ok(frame) => {
                                if let Err(e) = openflux_bus::apply_remote_frame(&broker, frame, "ws") {
                                    let err = WireFrame::Error { message: e };
                                    let _ = socket.send(WsMessage::Text(serde_json::to_string(&err).unwrap())).await;
                                }
                            }
                            Err(e) => {
                                let err = WireFrame::Error { message: format!("invalid WireFrame JSON: {e}") };
                                let _ = socket.send(WsMessage::Text(serde_json::to_string(&err).unwrap())).await;
                            }
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) | None => return,
                    _ => {}
                }
            }
        }
    }
}

async fn list_modules(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let discovered = state.v2.registry.discover();
    let saved = state.v2.registry.load_state();
    let statuses = state.v2.runtime.lock().unwrap().statuses();
    let status_map: BTreeMap<String, _> = statuses.into_iter().map(|s| (s.id.clone(), s)).collect();
    let dep_issues = state.v2.registry.check_all_dependencies(&discovered, &saved);
    let mut host_procs = state.v2.host_procs.lock().unwrap();

    let items: Vec<Value> = discovered
        .into_iter()
        .map(|(id, dm)| {
            let inst = saved.modules.get(&id).cloned().unwrap_or_default();
            let rt_status = status_map.get(&id);
            let origin = state.v2.registry.module_origin(&dm);
            let issues = dep_issues.get(&id).cloned().unwrap_or_default();
            let has_host = dm.manifest.entrypoints.host.is_some();
            let host_status = if inst.enabled {
                host_procs.status(&id, has_host)
            } else {
                openflux_modules::HostProcessStatus::NotRequired
            };
            let rt_healthy = rt_status.map(|s| s.healthy).unwrap_or(true);
            let host_healthy = !inst.enabled || host_status.is_healthy(dm.manifest.host_requirement);
            let healthy = rt_healthy && host_healthy;
            let last_error = rt_status
                .and_then(|s| s.last_error.clone())
                .or_else(|| if inst.enabled { host_status.error_message() } else { None });
            json!({
                "id": id,
                "manifest": dm.manifest,
                "tier": dm.tier,
                "path": dm.path,
                "state": inst,
                "origin": origin,
                "dependency_issues": issues,
                "host_status": host_status,
                "healthy": healthy,
                "last_error": last_error,
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(json!({
            "modules": items,
            "modules_rev": state.v2.modules_rev.load(Ordering::SeqCst),
        })),
    )
}

async fn get_module_detail(
    State(state): State<Arc<ApiState>>,
    AxumPath(id): AxumPath<String>,
) -> impl IntoResponse {
    let discovered = state.v2.registry.discover();
    let Some(dm) = discovered.get(&id) else {
        return err_resp(StatusCode::NOT_FOUND, format!("module {id:?} not found"));
    };
    let saved = state.v2.registry.load_state();
    let disclosure = inspect_module_dir(&dm.path).ok();
    let mut logs = state.v2.runtime.lock().unwrap().module_logs(&id);
    logs.extend(state.v2.host_procs.lock().unwrap().logs(&id));
    (
        StatusCode::OK,
        Json(json!({
            "id": id,
            "manifest": dm.manifest,
            "tier": dm.tier,
            "path": dm.path,
            "state": saved.modules.get(&id).cloned().unwrap_or_default(),
            "disclosure": disclosure,
            "logs": logs,
        })),
    )
}

#[derive(Deserialize)]
struct UpdateModuleStateBody {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    slot: Option<Option<u8>>,
    #[serde(default)]
    slot_pos: Option<Option<f32>>,
    #[serde(default)]
    slot_span: Option<Option<f32>>,
    #[serde(default)]
    pin_tab: Option<Option<bool>>,
    #[serde(default)]
    config: Option<BTreeMap<String, Value>>,
}

async fn update_module_state(
    State(state): State<Arc<ApiState>>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<UpdateModuleStateBody>,
) -> impl IntoResponse {
    let discovered = state.v2.registry.discover();
    let Some(dm) = discovered.get(&id) else {
        return err_resp(StatusCode::NOT_FOUND, format!("module {id:?} not found"));
    };
    let mut saved = state.v2.registry.load_state();

    // Determine effective target (pos, span) if this call changes placement or span.
    let target_pos_change: Option<Option<f32>> = if let Some(pos_opt) = body.slot_pos {
        Some(pos_opt.map(|v| ((v * 2.0).round() / 2.0).clamp(0.0, 3.5)))
    } else if let Some(slot_opt) = body.slot {
        if slot_opt.is_some_and(|s| s > 3) {
            return err_resp(StatusCode::BAD_REQUEST, "slot must be 0..=3");
        }
        Some(slot_opt.map(|s| s as f32))
    } else {
        None
    };

    let current_entry = saved.modules.get(&id).cloned().unwrap_or_default();
    let new_pos = target_pos_change.unwrap_or_else(|| current_entry.effective_slot_pos());
    let default_span = dm
        .manifest
        .top_bar
        .as_ref()
        .map(|t| t.default_span)
        .unwrap_or(1.0);
    let new_span = match body.slot_span {
        Some(Some(sp)) => sp.clamp(0.5, 4.0),
        Some(None) => default_span,
        None => current_entry.slot_span.unwrap_or(default_span),
    };

    // Unplace any other widget whose half-slot interval overlaps `[new_pos, new_pos + new_span)`!
    if target_pos_change.is_some() || body.slot_span.is_some() {
        if let Some(p0) = new_pos {
            let p1 = p0 + new_span;
            for (other_id, other) in saved.modules.iter_mut() {
                if other_id == &id {
                    continue;
                }
                if let Some(o0) = other.effective_slot_pos() {
                    let o_span = other.slot_span.unwrap_or(1.0).clamp(0.5, 4.0);
                    let o1 = o0 + o_span;
                    if o0 < p1 - 0.01 && p0 < o1 - 0.01 {
                        other.slot = None;
                        other.slot_pos = None;
                    }
                }
            }
        }
    }

    let entry = saved.modules.entry(id.clone()).or_default();
    if let Some(pos_opt) = target_pos_change {
        entry.slot_pos = pos_opt;
        entry.slot = pos_opt.map(|p| p.floor().clamp(0.0, 3.0) as u8);
    }
    if let Some(en) = body.enabled {
        entry.enabled = en;
    }
    if let Some(sp) = body.slot_span {
        entry.slot_span = sp.map(|v| v.clamp(0.5, 4.0));
    }
    if let Some(pin) = body.pin_tab {
        entry.pin_tab = pin;
    }
    if let Some(cfg) = body.config {
        for (k, v) in cfg {
            entry.config.insert(k, v);
        }
    }
    let updated = entry.clone();
    if let Err(e) = state.v2.registry.save_state(&saved) {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    state.v2.sync_all();
    (StatusCode::OK, Json(json!({ "id": id, "state": updated })))
}

#[derive(Deserialize)]
struct CreateModuleBody {
    id: String,
    category: ModuleCategory,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    enable: bool,
    #[serde(default)]
    slot: Option<u8>,
}

async fn create_module(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<CreateModuleBody>,
) -> impl IntoResponse {
    match state
        .v2
        .registry
        .scaffold_local(&body.id, body.category, body.name.as_deref())
    {
        Ok(dm) => {
            if body.enable || body.slot.is_some() {
                let mut saved = state.v2.registry.load_state();
                if let Some(s) = body.slot {
                    for (other_id, other) in saved.modules.iter_mut() {
                        if other_id != &body.id && other.slot == Some(s) {
                            other.slot = None;
                        }
                    }
                }
                let e = saved.modules.entry(body.id.clone()).or_default();
                e.enabled = body.enable || e.enabled;
                if let Some(s) = body.slot {
                    e.slot = Some(s);
                }
                let _ = state.v2.registry.save_state(&saved);
                state.v2.sync_all();
            }
            (StatusCode::OK, Json(json!({ "module": dm })))
        }
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
struct ForkModuleBody {
    source_id: String,
    new_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    enable: bool,
}

async fn fork_module(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<ForkModuleBody>,
) -> impl IntoResponse {
    match state
        .v2
        .registry
        .fork_module(&body.source_id, &body.new_id, body.name.as_deref())
    {
        Ok(dm) => {
            if body.enable {
                let mut saved = state.v2.registry.load_state();
                let e = saved.modules.entry(body.new_id.clone()).or_default();
                e.enabled = true;
                let _ = state.v2.registry.save_state(&saved);
                state.v2.sync_all();
            }
            (StatusCode::OK, Json(json!({ "module": dm })))
        }
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
struct InspectInstallBody {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    accept_disclosure: bool,
}

impl InspectInstallBody {
    fn effective_source(&self) -> Result<&str, String> {
        self.source
            .as_deref()
            .or(self.path.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "missing `source` or `path`".to_string())
    }
}

async fn inspect_module_api(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<InspectInstallBody>,
) -> impl IntoResponse {
    let src = match body.effective_source() {
        Ok(s) => s.to_string(),
        Err(e) => return err_resp(StatusCode::BAD_REQUEST, e),
    };
    let res = tokio::task::spawn_blocking(move || state.v2.registry.preview_install_source(&src))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
    match res {
        Ok(preview) => (StatusCode::OK, Json(json!({ "preview": preview }))),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

async fn install_module_api(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<InspectInstallBody>,
) -> impl IntoResponse {
    let src = match body.effective_source() {
        Ok(s) => s.to_string(),
        Err(e) => return err_resp(StatusCode::BAD_REQUEST, e),
    };
    let accept = body.accept_disclosure;
    let state_clone = state.clone();
    let res = tokio::task::spawn_blocking(move || {
        let preview = state_clone.v2.registry.preview_install_source(&src)?;
        if !accept {
            return Ok((false, None, preview));
        }
        let dm = state_clone.v2.registry.install_from_source(&src)?;
        Ok::<_, String>((true, Some(dm), preview))
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);

    match res {
        Ok((false, _, preview)) => (
            StatusCode::PRECONDITION_REQUIRED,
            Json(json!({
                "installed": false,
                "requires_acceptance": true,
                "preview": preview,
            })),
        ),
        Ok((true, Some(dm), preview)) => {
            state.v2.sync_all();
            (
                StatusCode::OK,
                Json(json!({ "installed": true, "module": dm, "preview": preview })),
            )
        }
        Ok((true, None, _)) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, "install returned empty"),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

async fn uninstall_module_api(
    State(state): State<Arc<ApiState>>,
    AxumPath(id): AxumPath<String>,
) -> impl IntoResponse {
    match state.v2.registry.uninstall_module(&id) {
        Ok(tier) => {
            state.v2.broker.purge_origin(&format!("module:{id}"));
            state.v2.sync_all();
            (
                StatusCode::OK,
                Json(json!({ "uninstalled": true, "id": id, "removed_tier": tier })),
            )
        }
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
struct UpdateModuleBody {
    #[serde(default)]
    accept_disclosure: bool,
}

async fn update_module_api(
    State(state): State<Arc<ApiState>>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<UpdateModuleBody>,
) -> impl IntoResponse {
    let accept = body.accept_disclosure;
    let state_clone = state.clone();
    let id_clone = id.clone();
    let res = tokio::task::spawn_blocking(move || {
        let (origin, preview) = state_clone.v2.registry.preview_update(&id_clone)?;
        if !accept {
            return Ok((false, None, origin, preview));
        }
        let dm = state_clone.v2.registry.update_module_from_source(&id_clone)?;
        Ok::<_, String>((true, Some(dm), origin, preview))
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);

    match res {
        Ok((false, _, origin, preview)) => (
            StatusCode::PRECONDITION_REQUIRED,
            Json(json!({
                "updated": false,
                "requires_acceptance": true,
                "origin": origin,
                "preview": preview,
            })),
        ),
        Ok((true, Some(dm), origin, preview)) => {
            state.v2.sync_all();
            (
                StatusCode::OK,
                Json(json!({ "updated": true, "module": dm, "origin": origin, "preview": preview })),
            )
        }
        Ok((true, None, _, _)) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, "update returned empty"),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

/// Lists video/image files in `~/.local/share/openflux/media` so Studio's `bg-media` picker can
/// offer one-click selection across all imported seamless loops and stills!
#[derive(Deserialize)]
struct ImportMediaBody {
    local_path: String,
}

async fn import_media_file_api(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<ImportMediaBody>,
) -> impl IntoResponse {
    let src = std::path::Path::new(body.local_path.trim());
    if !src.is_file() {
        return err_resp(StatusCode::BAD_REQUEST, format!("file not found: {}", src.display()));
    }
    let Some(fname) = src.file_name().and_then(|n| n.to_str()) else {
        return err_resp(StatusCode::BAD_REQUEST, "invalid file name");
    };
    let media_dir = state
        .v2
        .registry
        .state_path
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("media"))
        .unwrap_or_else(|| default_data_dir().join("media"));
    let _ = std::fs::create_dir_all(&media_dir);
    let dst = media_dir.join(fname);
    if src != dst {
        if let Err(e) = std::fs::copy(src, &dst) {
            return err_resp(StatusCode::INTERNAL_SERVER_ERROR, format!("copy failed: {e}"));
        }
    }
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "name": fname,
            "path": dst.to_string_lossy(),
        })),
    )
}

async fn list_media_library_api(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    // `~/.local/share/openflux/media` (sibling of `v2/` inside the user data directory)
    let media_dir = state
        .v2
        .registry
        .state_path
        .parent()
        .and_then(|v2_dir| v2_dir.parent())
        .map(|data_dir| data_dir.join("media"))
        .unwrap_or_else(|| std::path::PathBuf::from("media"));
    let mut files = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&media_dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let lower = name.to_ascii_lowercase();
            let is_media = lower.ends_with(".mp4")
                || lower.ends_with(".mov")
                || lower.ends_with(".mkv")
                || lower.ends_with(".webm")
                || lower.ends_with(".png")
                || lower.ends_with(".jpg")
                || lower.ends_with(".jpeg")
                || lower.ends_with(".gif");
            if is_media {
                let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
                files.push(json!({
                    "name": name,
                    "path": p.to_string_lossy(),
                    "bytes": bytes,
                }));
            }
        }
    }
    files.sort_by(|a, b| {
        a.get("name")
            .and_then(|v| v.as_str())
            .cmp(&b.get("name").and_then(|v| v.as_str()))
    });
    (
        StatusCode::OK,
        Json(json!({ "media_dir": media_dir, "files": files })),
    )
}

#[derive(Deserialize)]
struct TriggerActionBody {
    action_id: String,
}

async fn trigger_module_action(
    State(state): State<Arc<ApiState>>,
    AxumPath(id): AxumPath<String>,
    Json(body): Json<TriggerActionBody>,
) -> impl IntoResponse {
    let discovered = state.v2.registry.discover();
    let Some(dm) = discovered.get(&id) else {
        return err_resp(StatusCode::NOT_FOUND, format!("module {id:?} not found"));
    };
    let Some(act) = dm.manifest.actions.iter().find(|a| a.id == body.action_id) else {
        return err_resp(
            StatusCode::NOT_FOUND,
            format!("action {:?} not declared by module {id:?}", body.action_id),
        );
    };
    let opts = PublishOptions {
        retain: act.retain,
        origin: Some(format!("action:{id}")),
        ..Default::default()
    };
    match state.v2.broker.publish(act.topic.clone(), act.payload.clone(), opts) {
        Ok(msg) => (StatusCode::OK, Json(json!({ "ok": true, "message": msg }))),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e.to_string()),
    }
}

async fn reload_module_api(
    State(state): State<Arc<ApiState>>,
    AxumPath(id): AxumPath<String>,
) -> impl IntoResponse {
    let ok = state.v2.runtime.lock().unwrap().reload_module(&id);
    if !ok {
        state.v2.sync_all_with_reason("reload", Some(&id));
    } else {
        state.v2.notify_modules_changed("reload", Some(&id));
    }
    (StatusCode::OK, Json(json!({ "reloaded": id })))
}

async fn get_module_logs_api(
    State(state): State<Arc<ApiState>>,
    AxumPath(id): AxumPath<String>,
) -> impl IntoResponse {
    let mut logs = state.v2.runtime.lock().unwrap().module_logs(&id);
    logs.extend(state.v2.host_procs.lock().unwrap().logs(&id));
    logs.sort_by_key(|l| l.timestamp_ms);
    (StatusCode::OK, Json(json!({ "id": id, "logs": logs })))
}

async fn get_all_logs_api(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let statuses = state.v2.runtime.lock().unwrap().statuses();
    let mut entries = Vec::new();
    for s in statuses {
        let rt_logs = state.v2.runtime.lock().unwrap().module_logs(&s.id);
        for l in rt_logs {
            entries.push(json!({
                "module_id": s.id,
                "timestamp_ms": l.timestamp_ms,
                "level": l.level,
                "message": l.message,
            }));
        }
        let host_logs = state.v2.host_procs.lock().unwrap().logs(&s.id);
        for l in host_logs {
            entries.push(json!({
                "module_id": s.id,
                "timestamp_ms": l.timestamp_ms,
                "level": l.level,
                "message": l.message,
            }));
        }
    }
    entries.sort_by_key(|v| v.get("timestamp_ms").and_then(|t| t.as_u64()).unwrap_or(0));
    (StatusCode::OK, Json(json!({ "logs": entries })))
}

async fn v2_status_api(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let discovered = state.v2.registry.discover();
    let saved = state.v2.registry.load_state();
    let enabled_count = saved.modules.values().filter(|m| m.enabled).count();
    let statuses = state.v2.runtime.lock().unwrap().statuses();
    let adapter = state
        .v2
        .compositor
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| c.adapter_summary())
        .unwrap_or_else(|| "lazy-init".into());
    (
        StatusCode::OK,
        Json(json!({
            "pid": std::process::id(),
            "usb_protocol": if state.device.is_v2_mux_active() { "ofv2" } else { "legacy" },
            "modules_rev": state.v2.modules_rev.load(Ordering::SeqCst),
            "latest_bus_seq": state.v2.broker.latest_seq(),
            "bundled_dir": state.v2.registry.bundled_dir,
            "installed_dir": state.v2.registry.installed_dir,
            "local_dir": state.v2.registry.local_dir,
            "state_path": state.v2.registry.state_path,
            "discovered_modules": discovered.len(),
            "enabled_modules": enabled_count,
            "gpu_adapter": adapter,
            "runtime_modules": statuses,
        })),
    )
}

#[derive(Deserialize)]
struct BusEventsQuery {
    #[serde(default = "default_wildcard")]
    pattern: String,
    #[serde(default)]
    since_seq: u64,
    #[serde(default = "default_wait_ms")]
    wait_ms: u64,
}
fn default_wait_ms() -> u64 {
    2000
}

async fn bus_events_poll(
    State(state): State<Arc<ApiState>>,
    Query(q): Query<BusEventsQuery>,
) -> impl IntoResponse {
    let broker = state.v2.broker.clone();
    let pattern = q.pattern;
    let since_seq = q.since_seq;
    let wait = std::time::Duration::from_millis(q.wait_ms.min(10_000));
    let res = tokio::task::spawn_blocking(move || broker.poll_events_since(&pattern, since_seq, wait))
        .await;
    match res {
        Ok(Ok((events, latest_seq))) => (
            StatusCode::OK,
            Json(json!({ "events": events, "latest_seq": latest_seq })),
        ),
        Ok(Err(e)) => err_resp(StatusCode::BAD_REQUEST, e.to_string()),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn list_curated_lists(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let saved = state.v2.registry.load_state();
    (StatusCode::OK, Json(json!({ "lists": saved.curated_lists })))
}

async fn add_curated_list(
    State(state): State<Arc<ApiState>>,
    Json(sub): Json<CuratedListSubscription>,
) -> impl IntoResponse {
    let mut saved = state.v2.registry.load_state();
    saved.curated_lists.retain(|l| l.id != sub.id);
    saved.curated_lists.push(sub);
    if let Err(e) = state.v2.registry.save_state(&saved) {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    state.v2.notify_modules_changed("lists_updated", None);
    (StatusCode::OK, Json(json!({ "lists": saved.curated_lists })))
}

async fn remove_curated_list(
    State(state): State<Arc<ApiState>>,
    AxumPath(id): AxumPath<String>,
) -> impl IntoResponse {
    let mut saved = state.v2.registry.load_state();
    saved.curated_lists.retain(|l| l.id != id);
    if let Err(e) = state.v2.registry.save_state(&saved) {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    state.v2.notify_modules_changed("lists_updated", None);
    (StatusCode::OK, Json(json!({ "lists": saved.curated_lists })))
}

#[derive(Deserialize)]
struct BrowseListQuery {
    source: String,
}

async fn browse_curated_list(Query(q): Query<BrowseListQuery>) -> impl IntoResponse {
    let src = q.source;
    let res = tokio::task::spawn_blocking(move || {
        openflux_modules::fetch_curated_list_document(&src)
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    match res {
        Ok(doc) => (StatusCode::OK, Json(json!({ "list": doc }))),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
struct ExportPresetBody {
    #[serde(default = "default_preset_name")]
    name: String,
    #[serde(default)]
    description: String,
}
fn default_preset_name() -> String {
    "OpenFlux Setup".to_string()
}

async fn export_preset_api(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<ExportPresetBody>,
) -> impl IntoResponse {
    let preset = state.v2.registry.export_preset(&body.name, &body.description);
    (StatusCode::OK, Json(json!({ "preset": preset })))
}

async fn import_preset_api(
    State(state): State<Arc<ApiState>>,
    Json(preset): Json<SetupPreset>,
) -> impl IntoResponse {
    match state.v2.registry.import_preset_detailed(&preset) {
        Ok(report) => {
            state.v2.sync_all();
            (
                StatusCode::OK,
                Json(json!({
                    "imported": preset.name,
                    "modules": report.imported_count,
                    "missing_modules": report.missing_modules,
                })),
            )
        }
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

#[derive(Deserialize)]
struct SimKeyBody {
    key_index: u16,
    #[serde(default = "default_depth")]
    depth_um: u16,
    #[serde(default = "default_vel")]
    velocity_um_s: i32,
    #[serde(default)]
    release: bool,
}
fn default_depth() -> u16 {
    2200
}
fn default_vel() -> i32 {
    14000
}

async fn simulate_key(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<SimKeyBody>,
) -> impl IntoResponse {
    let Some(k) = state.v2.geometry.key(body.key_index) else {
        return err_resp(StatusCode::BAD_REQUEST, "key_index out of range");
    };
    let now_ms = openflux_bus::now_unix_ms();
    let ev = KeyEvent {
        key_index: body.key_index,
        kind: if body.release {
            KeyEventKind::Release
        } else {
            KeyEventKind::Press
        },
        timestamp_ms: now_ms,
        x_px: k.center_x,
        y_px: k.center_y,
        w_px: k.rect.w,
        h_px: k.rect.h,
        uv_x: k.uv_x,
        uv_y: k.uv_y,
        row: k.row,
        col: k.col,
        depth_um: body.depth_um,
        velocity_um_s: body.velocity_um_s,
        color: None,
    };
    let _ = state
        .v2
        .preview_broker
        .publish_typed(topics::INPUT_KEYS_EVENT, &ev, PublishOptions::default());
    let sample = AnalogKeySample::from_raw(
        body.key_index,
        body.depth_um,
        body.velocity_um_s,
        k.center_x,
        k.center_y,
    );
    let _ = state.v2.preview_broker.publish_typed(
        topics::INPUT_KEYS_ANALOG,
        &AnalogKeyFrame {
            timestamp_ms: now_ms,
            samples: vec![sample],
        },
        PublishOptions::default(),
    );
    (StatusCode::OK, Json(json!({ "ok": true, "event": ev })))
}

async fn simulate_impulse(
    State(state): State<Arc<ApiState>>,
    Json(imp): Json<SurfaceImpulse>,
) -> impl IntoResponse {
    let _ = state.v2.preview_broker.publish_typed(
        topics::FX_SURFACE_IMPULSE,
        &imp,
        PublishOptions::default(),
    );
    (StatusCode::OK, Json(json!({ "ok": true, "impulse": imp })))
}

/// Fires a dramatic multi-key ripple + particle + analog burst across the keyboard to showcase
/// 3D water refraction, particle-to-particle collisions, keycap wall bounces, and lens refraction!
async fn simulate_physics_burst(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let now_ms = openflux_bus::now_unix_ms();
    let burst_keys: [u16; 6] = [34, 37, 49, 52, 55, 67];
    let mut samples = Vec::new();
    for (i, &key_index) in burst_keys.iter().enumerate() {
        if let Some(k) = state.v2.geometry.key(key_index) {
            let depth = 2200 + ((i as u16) * 90) % 550;
            let vel = if i % 2 == 0 { 16500 } else { -11500 };
            let ev = KeyEvent {
                key_index,
                kind: KeyEventKind::Press,
                timestamp_ms: now_ms,
                x_px: k.center_x,
                y_px: k.center_y,
                w_px: k.rect.w,
                h_px: k.rect.h,
                uv_x: k.uv_x,
                uv_y: k.uv_y,
                row: k.row,
                col: k.col,
                depth_um: depth,
                velocity_um_s: vel,
                color: None,
            };
            let _ = state.v2.broker.publish_typed(
                topics::INPUT_KEYS_EVENT,
                &ev,
                PublishOptions::default(),
            );
            samples.push(AnalogKeySample::from_raw(
                key_index,
                depth,
                vel,
                k.center_x,
                k.center_y,
            ));
        }
    }
    let _ = state.v2.broker.publish_typed(
        topics::INPUT_KEYS_ANALOG,
        &AnalogKeyFrame {
            timestamp_ms: now_ms,
            samples,
        },
        PublishOptions::default(),
    );
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "keys_triggered": burst_keys.len() })),
    )
}

#[derive(Deserialize)]
struct PreviewFrameBody {
    #[serde(default = "default_step_ms")]
    step_ms: f32,
    #[serde(default = "default_steps")]
    steps: u32,
    #[serde(default = "default_pw")]
    width: u32,
    #[serde(default = "default_ph")]
    height: u32,
}
fn default_step_ms() -> f32 {
    16.667
}
fn default_steps() -> u32 {
    1
}
fn default_pw() -> u32 {
    960
}
fn default_ph() -> u32 {
    540
}

async fn render_preview_frame(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<PreviewFrameBody>,
) -> impl IntoResponse {
    state.v2.sync_live_to_preview();
    let mut rt = state.v2.runtime.lock().unwrap();
    let _ = rt.check_hot_reload();
    let mut comp_guard = state.v2.compositor.lock().unwrap();
    if comp_guard.is_none() {
        let font = load_font_bytes();
        match Compositor::new_headless(
            state.v2.geometry.clone(),
            font.as_deref(),
            CompositorOptions {
                width: body.width.clamp(320, 1920),
                height: body.height.clamp(180, 1080),
                draw_default_legends: true,
            },
        ) {
            Ok(c) => *comp_guard = Some(c),
            Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
        }
    }
    let comp = comp_guard.as_mut().unwrap();
    let steps = body.steps.clamp(1, 30);
    let dt_sec = (body.step_ms / 1000.0).clamp(0.001, 0.1);
    let now_ms = openflux_bus::now_unix_ms();
    let mut lua_ms = 0.0_f32;
    let mut last_timings = openflux_compositor::FrameTimings::default();
    for s in 0..steps {
        let t_lua = std::time::Instant::now();
        let scene = rt.step_frame(dt_sec, now_ms + (s as f32 * body.step_ms) as u64);
        lua_ms = t_lua.elapsed().as_secs_f32() * 1000.0;
        last_timings = comp.render_scene(&scene);
    }
    let rgba = match comp.readback_rgba() {
        Ok(b) => b,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let w = body.width.clamp(320, 1920);
    let h = body.height.clamp(180, 1080);
    let mut png_bytes = Vec::new();
    if let Some(img) = image::RgbaImage::from_raw(w, h, rgba) {
        let mut cursor = std::io::Cursor::new(&mut png_bytes);
        if let Err(e) = img.write_to(&mut cursor, image::ImageFormat::Png) {
            return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
        }
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    let statuses = rt.statuses();
    let adapter = comp.adapter_summary();
    (
        StatusCode::OK,
        Json(json!({
            "width": w,
            "height": h,
            "gpu_adapter": adapter,
            "modules_rev": state.v2.modules_rev.load(Ordering::SeqCst),
            "png_base64": b64,
            "modules": statuses,
            "timings": {
                "lua_ms": lua_ms,
                "physics_ms": last_timings.physics_ms,
                "canvas2d_ms": last_timings.canvas2d_ms,
                "gpu_submit_ms": last_timings.gpu_submit_ms,
                "total_frame_ms": lua_ms + last_timings.total_ms,
                "particle_count": last_timings.particle_count,
                "glow_count": last_timings.glow_count,
                "wave_enabled": last_timings.wave_enabled,
            }
        })),
    )
}

#[derive(Deserialize)]
struct KeyboardUploadMediaBody {
    local_path: String,
    #[serde(default)]
    remote_name: Option<String>,
}

async fn keyboard_upload_media(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<KeyboardUploadMediaBody>,
) -> impl IntoResponse {
    let local = std::path::PathBuf::from(&body.local_path);
    let fname = body
        .remote_name
        .filter(|s| !s.trim().is_empty())
        .or_else(|| local.file_name().and_then(|n| n.to_str()).map(str::to_string));
    let Some(fname) = fname else {
        return err_resp(StatusCode::BAD_REQUEST, "invalid file name");
    };
    let remote = format!("/home/root/openflux/media/{fname}");
    let device = state.device.clone();
    let res = tokio::task::spawn_blocking(move || {
        let bytes = std::fs::read(&local).map_err(|e| format!("read {}: {e}", local.display()))?;
        let size = bytes.len();
        device.put_file(&remote, &bytes)?;
        Ok::<(String, usize), String>((remote, size))
    })
    .await;
    match res {
        Ok(Ok((remote, size))) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "remote_path": remote, "bytes": size })),
        ),
        Ok(Err(e)) => err_resp(StatusCode::BAD_GATEWAY, e),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

#[derive(Deserialize)]
struct KeyboardSideloadBody {
    ipk_path: String,
}

async fn keyboard_sideload(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<KeyboardSideloadBody>,
) -> impl IntoResponse {
    let ipk = body.ipk_path.clone();
    let device = state.device.clone();
    let res = tokio::task::spawn_blocking(move || device.sideload_ipk(&ipk))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
    match res {
        Ok(msg) => (StatusCode::OK, Json(json!({ "ok": true, "message": msg }))),
        Err(e) => err_resp(StatusCode::BAD_GATEWAY, e),
    }
}

#[derive(Deserialize)]
struct KeyboardFlashUf2Body {
    uf2_path: String,
}

async fn keyboard_flash_uf2(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<KeyboardFlashUf2Body>,
) -> impl IntoResponse {
    let uf2 = body.uf2_path.clone();
    let device = state.device.clone();
    let res = tokio::task::spawn_blocking(move || device.flash_uf2(&uf2))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
    match res {
        Ok(msg) => (StatusCode::OK, Json(json!({ "ok": true, "message": msg }))),
        Err(e) => err_resp(StatusCode::BAD_GATEWAY, e),
    }
}

/// Pushes enabled local/installed v2 modules, `modules_state.json`, and any referenced media video
/// straight to `/home/root/openflux/v2/` on the connected Flux keyboard via USB `PUT_FILE`.
async fn keyboard_sync_v2(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let discovered = state.v2.registry.discover();
    let mut kb_state = state.v2.registry.load_state();
    let kb_state_path = state.v2.registry.state_path.clone();
    let device = state.device.clone();

    let res = tokio::task::spawn_blocking(move || {
        let (has_bridge, _) = device.is_connected();
        if !has_bridge {
            return Err("Keyboard USB bridge is not connected".to_string());
        }
        let mut uploaded_files = Vec::new();
        let mut uploaded_bytes = 0usize;

        // Single Source of Truth: upload ALL enabled modules (bundled, installed, AND local)
        // directly to `/home/root/openflux/v2/modules/<id>/` (skipping host-only `host/` scripts,
        // `.git`, `.DS_Store`, `__pycache__`), so the host is always authoritative and partial
        // manual `local/` shadows are never needed!
        for (id, dm) in &discovered {
            let is_enabled = kb_state.modules.get(id).map(|s| s.enabled).unwrap_or(false);
            if !is_enabled && dm.tier == openflux_modules::ModuleTier::Bundled {
                continue;
            }
            let mut stack = vec![dm.path.clone()];
            while let Some(dir) = stack.pop() {
                let Ok(rd) = std::fs::read_dir(&dir) else { continue };
                for entry in rd.flatten() {
                    let p = entry.path();
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name == ".git" || name == ".DS_Store" || name == "__pycache__" || name == "host" {
                        continue;
                    }
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.is_file() {
                        if let Ok(rel) = p.strip_prefix(&dm.path) {
                            let rel_str = rel.to_string_lossy().replace('\\', "/");
                            let remote = format!("/home/root/openflux/v2/modules/{id}/{rel_str}");
                            let bytes = std::fs::read(&p).map_err(|e| e.to_string())?;
                            uploaded_bytes += bytes.len();
                            device.put_file(&remote, &bytes)?;
                            uploaded_files.push(remote);
                        }
                    }
                }
            }
        }

        // Sync bundled shortcut JSON packs straight to `/home/root/openflux/shortcuts/` too
        // so updating `macos.json` or any app pack reaches the keyboard immediately without an .ipk rebuild!
        let shortcut_dirs = [
            kb_state_path.parent().and_then(|p| p.parent()).map(|p| p.join("assets/shortcuts")),
            std::env::current_exe().ok().and_then(|e| e.parent().and_then(|p| p.parent()).map(|p| p.join("Resources/assets/shortcuts"))),
        ];
        if let Some(sc_dir) = shortcut_dirs.into_iter().flatten().find(|d| d.is_dir()) {
            if let Ok(rd) = std::fs::read_dir(&sc_dir) {
                for entry in rd.flatten() {
                    let p = entry.path();
                    if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("json") {
                        if let Some(fname) = p.file_name().and_then(|n| n.to_str()) {
                            let remote = format!("/home/root/openflux/shortcuts/{fname}");
                            if let Ok(bytes) = std::fs::read(&p) {
                                uploaded_bytes += bytes.len();
                                device.put_file(&remote, &bytes)?;
                                uploaded_files.push(remote);
                            }
                        }
                    }
                }
            }
        }

        // Sync pure-v2 `display_prefs.json` (parallax + brightness + sleep timeout) to keyboard!
        let prefs_local = kb_state_path.parent().unwrap_or_else(|| std::path::Path::new(".")).join("display_prefs.json");
        if let Ok(bytes) = std::fs::read(&prefs_local) {
            let remote = "/home/root/openflux/v2/display_prefs.json".to_string();
            uploaded_bytes += bytes.len();
            device.put_file(&remote, &bytes)?;
            uploaded_files.push(remote);
        }

        // Sync `module_actions.json` (64-slot QMK action registry) to keyboard for offline use!
        let actions_local = kb_state_path.parent().unwrap_or_else(|| std::path::Path::new(".")).join("module_actions.json");
        if let Ok(bytes) = std::fs::read(&actions_local) {
            let remote = "/home/root/openflux/v2/module_actions.json".to_string();
            uploaded_bytes += bytes.len();
            device.put_file(&remote, &bytes)?;
            uploaded_files.push(remote);
        }

        // Sync custom PNG keycap icons referenced by `keys-custom` (`by_key` / `by_action`)
        if let Some(custom_inst) = kb_state.modules.get("keys-custom") {
            let media_dir = default_data_dir().join("media");
            for map_key in ["by_key", "by_action"] {
                if let Some(obj) = custom_inst.config.get(map_key).and_then(|v| v.as_object()) {
                    for ov in obj.values() {
                        if let Some(img_name) = ov.get("image").and_then(|v| v.as_str()) {
                            let fname = std::path::Path::new(img_name)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(img_name);
                            let lp = media_dir.join(fname);
                            if lp.is_file() {
                                let remote = format!("/home/root/openflux/media/{fname}");
                                if !device.stat_file(&remote).unwrap_or(false) {
                                    if let Ok(bytes) = std::fs::read(&lp) {
                                        uploaded_bytes += bytes.len();
                                        device.put_file(&remote, &bytes)?;
                                        uploaded_files.push(remote);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // For ANY enabled module with `kind = "file"` settings pointing at a local file,
        // auto-upload the file to `/home/root/openflux/media/` (skipping if already present)
        // and rewrite the keyboard copy of that setting to the remote filename.
        // Works identically for bundled `bg-media` AND any third-party module!
        for (id, dm) in &discovered {
            let Some(inst) = kb_state.modules.get_mut(id) else {
                continue;
            };
            if !inst.enabled {
                continue;
            }
            for field in &dm.manifest.settings {
                if field.kind != openflux_modules::SettingKind::File {
                    continue;
                }
                let Some(media_str) = inst
                    .config
                    .get(&field.key)
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                else {
                    continue;
                };
                let lp = std::path::Path::new(&media_str);
                if lp.is_file() {
                    if let Some(fname) = lp.file_name().and_then(|n| n.to_str()) {
                        let remote_media = format!("/home/root/openflux/media/{fname}");
                        let already_there = device.stat_file(&remote_media).unwrap_or(false);
                        if !already_there {
                            let bytes = std::fs::read(lp).map_err(|e| e.to_string())?;
                            uploaded_bytes += bytes.len();
                            device.put_file(&remote_media, &bytes)?;
                            uploaded_files.push(remote_media);
                        }
                        inst.config.insert(field.key.clone(), json!(fname));
                    }
                }
            }
        }

        // Sync user-edited custom shortcuts (from Studio's Shortcuts tab) if present
        if let Some(cfg_dir) = kb_state_path
            .parent()
            .and_then(|v2_dir| v2_dir.parent())
            .map(|data_dir| data_dir.join("config"))
        {
            let cs = cfg_dir.join("custom_shortcuts.json");
            if cs.is_file() {
                if let Ok(bytes) = std::fs::read(&cs) {
                    let remote_cs = "/home/root/openflux/v2/custom_shortcuts.json";
                    uploaded_bytes += bytes.len();
                    let _ = device.put_file(remote_cs, &bytes);
                    uploaded_files.push(remote_cs.to_string());
                }
            }
        }

        // Write `modules_state.json` atomically last so the keyboard hot-reloads cleanly.
        let state_bytes = serde_json::to_vec_pretty(&kb_state).map_err(|e| e.to_string())?;
        uploaded_bytes += state_bytes.len();
        let remote_state = "/home/root/openflux/v2/modules_state.json";
        device.put_file(remote_state, &state_bytes)?;
        uploaded_files.push(remote_state.to_string());

        Ok::<(Vec<String>, usize), String>((uploaded_files, uploaded_bytes))
    })
    .await;

    match res {
        Ok(Ok((files, bytes))) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "uploaded_files": files,
                "uploaded_bytes": bytes,
            })),
        ),
        Ok(Err(e)) => err_resp(StatusCode::BAD_GATEWAY, e),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// Real-time live video/frame streaming ingest (`POST /v2/streams/:name`).
/// Any PC-side module (YouTube/Twitch via ffmpeg, Prusa RTSP camera, screen capture, OBS) can
/// stream raw H.264 Annex-B (`Content-Type: video/h264`) or single JPEG frames (`image/jpeg`)
/// straight over `CH_MEDIA` (Priority 1) directly into the keyboard's hardware `rkmpp` VPU decoder!
/// Automatically sends `FLAG_STREAM_CLOSE` on EOF/disconnect so the keyboard module becomes 100% transparent.
async fn ingest_media_stream(
    State(state): State<Arc<ApiState>>,
    AxumPath(name): AxumPath<String>,
    headers: axum::http::HeaderMap,
    body: axum::body::Body,
) -> impl IntoResponse {
    use futures_util::StreamExt;
    let ctype = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("video/h264")
        .to_ascii_lowercase();
    let is_jpeg = ctype.contains("jpeg") || ctype.contains("jpg");
    let codec = if is_jpeg {
        openflux_bus::MediaCodec::Jpeg
    } else {
        openflux_bus::MediaCodec::H264AnnexB
    };

    let mut data_stream = body.into_data_stream();
    let mut total_bytes: u64 = 0;
    let mut chunks: u64 = 0;

    if is_jpeg {
        let mut frame = Vec::new();
        while let Some(Ok(chunk)) = data_stream.next().await {
            if frame.len() + chunk.len() <= 2 * 1024 * 1024 {
                frame.extend_from_slice(&chunk);
            }
        }
        if !frame.is_empty() {
            total_bytes = frame.len() as u64;
            let _ = state.device.send_media_frame(&name, codec, &frame, 0);
        }
        return (
            StatusCode::OK,
            Json(json!({ "ok": true, "stream": name, "codec": "jpeg", "bytes": total_bytes })),
        );
    }

    // Continuous streaming H.264 Annex-B body!
    // Only advertise `active: true` once real video bytes actually arrive (prevents flashing
    // when an upstream RTSP/YouTube source is offline and ffmpeg exits with 0 bytes!).
    let mut announced_live = false;

    while let Some(item) = data_stream.next().await {
        match item {
            Ok(chunk) if !chunk.is_empty() => {
                if !announced_live {
                    announced_live = true;
                    let _ = state.v2.broker.publish(
                        format!("modules/streams/{name}"),
                        json!({ "active": true, "codec": "h264", "name": name }),
                        PublishOptions::retained().with_origin("host-stream"),
                    );
                }
                total_bytes += chunk.len() as u64;
                chunks += 1;
                // Slice large chunks cleanly to <= 64 KB so P0 CH_BUS preempts freely!
                for part in chunk.chunks(64 * 1024) {
                    let _ = state.device.send_media_frame(&name, codec, part, 0);
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    // Stream ended or closed -> only send close if we ever actually went live!
    if announced_live {
        let _ = state.device.send_media_frame(
            &name,
            codec,
            &[],
            openflux_bus::FLAG_STREAM_CLOSE,
        );
        let _ = state.v2.broker.publish(
            format!("modules/streams/{name}"),
            json!({ "active": false, "codec": "closed", "name": name }),
            PublishOptions::retained().with_origin("host-stream"),
        );
    }

    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "stream": name,
            "codec": "h264",
            "chunks": chunks,
            "bytes": total_bytes,
        })),
    )
}

async fn close_media_stream(
    State(state): State<Arc<ApiState>>,
    AxumPath(name): AxumPath<String>,
) -> impl IntoResponse {
    let _ = state.device.send_media_frame(
        &name,
        openflux_bus::MediaCodec::H264AnnexB,
        &[],
        openflux_bus::FLAG_STREAM_CLOSE,
    );
    let _ = state.v2.broker.publish(
        format!("modules/streams/{name}"),
        json!({ "active": false, "codec": "closed", "name": name }),
        PublishOptions::retained().with_origin("host-stream"),
    );
    (StatusCode::OK, Json(json!({ "ok": true, "closed": name })))
}

#[derive(Deserialize)]
struct KeyboardReadQuery {
    path: String,
}

async fn keyboard_soc_stats_v2(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let device = state.device.clone();
    let res = tokio::task::spawn_blocking(move || device.read_device_soc_stats()).await;
    match res {
        Ok(Some(v)) => (StatusCode::OK, Json(v)),
        _ => (StatusCode::OK, Json(json!({ "offline": true }))),
    }
}

async fn keyboard_read_file(
    State(state): State<Arc<ApiState>>,
    Query(q): Query<KeyboardReadQuery>,
) -> impl IntoResponse {
    let device = state.device.clone();
    let path = q.path;
    let res = tokio::task::spawn_blocking(move || device.get_file(&path))
        .await
        .map_err(|e| e.to_string())
        .and_then(|r| r);
    match res {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            (StatusCode::OK, Json(json!({ "ok": true, "bytes": bytes.len(), "text": text })))
        }
        Err(e) => err_resp(StatusCode::BAD_GATEWAY, e),
    }
}

#[derive(Deserialize)]
struct VialXferBody {
    data: Vec<u8>,
}

async fn vial_status_api(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let s = Arc::clone(&state);
    match tokio::task::spawn_blocking(move || s.v2.vial_relay.status()).await {
        Ok(st) => (StatusCode::OK, Json(json!(st))),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn vial_xfer_api(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<VialXferBody>,
) -> impl IntoResponse {
    if body.data.len() > VIAL_RAW_EPSIZE {
        return err_resp(
            StatusCode::BAD_REQUEST,
            format!("Vial packet exceeds {VIAL_RAW_EPSIZE} bytes"),
        );
    }
    let mut cmd = [0u8; VIAL_RAW_EPSIZE];
    cmd[..body.data.len()].copy_from_slice(&body.data);
    let s = Arc::clone(&state);
    match tokio::task::spawn_blocking(move || s.v2.vial_relay.xfer(&cmd)).await {
        Ok(Ok(reply)) => (StatusCode::OK, Json(json!({ "data": reply.to_vec() }))),
        Ok(Err(e)) => err_resp(StatusCode::BAD_GATEWAY, e),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn vial_actions_api(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let mut map = state.v2.vial_actions.lock().unwrap();
    map.reconcile_from_registry(&state.v2.registry);
    let _ = map.save(&state.v2.vial_actions_path);
    let vial_json = map.build_vial_json();
    (
        StatusCode::OK,
        Json(json!({
            "slots": map.slots,
            "entries": map.entries,
            "vial_json": vial_json,
        })),
    )
}

async fn vial_sync_api(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let (xz_bytes, map_bytes, slot_count) = {
        let mut map = state.v2.vial_actions.lock().unwrap();
        map.reconcile_from_registry(&state.v2.registry);
        let _ = map.save(&state.v2.vial_actions_path);
        let xz = match map.build_vial_xz() {
            Ok(b) => b,
            Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
        };
        let mb = match serde_json::to_vec_pretty(&*map) {
            Ok(b) => b,
            Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
        };
        (xz, mb, map.entries.len())
    };

    let s = Arc::clone(&state);
    let res = tokio::task::spawn_blocking(move || {
        let mut firmware_synced = false;
        let mut soc_synced = false;
        let mut warnings = Vec::<String>::new();

        // 1. Upload XZ vial.json to RP2040 LittleFS over IF 1 bulk command (0x73).
        match crate::usb::libusb::KeyboardUsbClient::new() {
            Ok(t) => match t.put_vial_def(&xz_bytes) {
                Ok(()) => firmware_synced = true,
                Err(e) => warnings.push(format!("RP2040 vial.json upload failed: {e}")),
            },
            Err(e) => warnings.push(format!("USB transport unavailable: {e}")),
        }

        // 2. Sync module_actions.json to the Linux SoC so offline keyboard modules resolve slots.
        if let Err(e) = s
            .device
            .put_file("/home/root/openflux/v2/module_actions.json", &map_bytes)
        {
            warnings.push(format!("SoC module_actions.json sync failed: {e}"));
        } else {
            soc_synced = true;
        }

        json!({
            "ok": firmware_synced || soc_synced,
            "firmware_synced": firmware_synced,
            "soc_synced": soc_synced,
            "xz_bytes": xz_bytes.len(),
            "slot_count": slot_count,
            "warnings": warnings,
        })
    })
    .await;

    match res {
        Ok(payload) => (StatusCode::OK, Json(payload)),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn get_display_prefs(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    (StatusCode::OK, Json(state.v2.load_display_prefs()))
}

async fn put_display_parallax(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    state.v2.publish_parallax_from_settings(&body);
    (StatusCode::OK, Json(json!({ "ok": true, "parallax": state.v2.load_display_prefs()["parallax"] })))
}

/// Normalizes physical top-bar hardware attachment slots (`0..4` bays) and computes exact
/// 1920x1080 `clear_rects` (merged contiguous clear windows) and `blocked_rects` (physical knobs,
/// sliders, macro blocks, or opaque covers) so any software module (`ui-osd-toast`, `dock-top-bar`)
/// can automatically avoid opaque hardware attachments!
fn build_attachments_bus_payload(raw: Option<&Value>) -> Value {
    let default_bays = || -> Vec<Value> {
        (0..4)
            .map(|i| {
                json!({
                    "pos": i as f64,
                    "span": 1.0,
                    "kind": "clear",
                    "opaque": false,
                    "label": "Clear Window",
                })
            })
            .collect()
    };

    let bays: Vec<Value> = raw
        .and_then(|v| v.get("bays"))
        .and_then(|v| v.as_array())
        .filter(|arr| !arr.is_empty())
        .map(|arr| {
            arr.iter()
                .map(|b| {
                    let pos = b.get("pos").and_then(|v| v.as_f64()).unwrap_or(0.0).clamp(0.0, 3.5);
                    let span = b.get("span").and_then(|v| v.as_f64()).unwrap_or(1.0).clamp(0.5, 4.0);
                    let kind = b
                        .get("kind")
                        .and_then(|v| v.as_str())
                        .unwrap_or("clear")
                        .to_string();
                    let opaque = b
                        .get("opaque")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(kind != "clear");
                    let label = b
                        .get("label")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| match kind.as_str() {
                            "dial" => "Rotary Dial".into(),
                            "triple_dial" => "Triple Dial".into(),
                            "slider" => "Linear Slider".into(),
                            "macro_4" => "4-Key Macro".into(),
                            "blank" => "Opaque Cover".into(),
                            _ => "Clear Window".into(),
                        });
                    json!({
                        "pos": pos,
                        "span": span,
                        "kind": kind,
                        "opaque": opaque,
                        "label": label,
                    })
                })
                .collect()
        })
        .unwrap_or_else(default_bays);

    // Map 4 bays cleanly across header_rect (x=12..1908, y=5..261, w=1896, h=256)
    let hdr_x = 12.0_f64;
    let hdr_y = 5.0_f64;
    let hdr_w = 1896.0_f64;
    let hdr_h = 256.0_f64;
    let unit_w = hdr_w / 4.0;

    let mut blocked_rects = Vec::new();
    let mut blocked_halves = [false; 8];
    for b in &bays {
        let pos = b["pos"].as_f64().unwrap_or(0.0);
        let span = b["span"].as_f64().unwrap_or(1.0);
        let opaque = b["opaque"].as_bool().unwrap_or(false);
        if opaque {
            let x = hdr_x + pos * unit_w;
            let w = (span * unit_w).min(hdr_x + hdr_w - x);
            blocked_rects.push(json!({
                "pos": pos,
                "span": span,
                "kind": b["kind"],
                "label": b["label"],
                "x": x,
                "y": hdr_y,
                "w": w,
                "h": hdr_h,
            }));
            let h0 = ((pos * 2.0).round() as isize).clamp(0, 8) as usize;
            let h1 = (((pos + span) * 2.0).round() as isize).clamp(0, 8) as usize;
            for h in h0..h1 {
                blocked_halves[h] = true;
            }
        }
    }

    // Merge contiguous clear half-bays into maximal `clear_rects`!
    let mut clear_rects = Vec::new();
    let mut i = 0usize;
    while i < 8 {
        if blocked_halves[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < 8 && !blocked_halves[i] {
            i += 1;
        }
        let pos = start as f64 / 2.0;
        let span = (i - start) as f64 / 2.0;
        clear_rects.push(json!({
            "pos": pos,
            "span": span,
            "x": hdr_x + pos * unit_w,
            "y": hdr_y,
            "w": span * unit_w,
            "h": hdr_h,
        }));
    }

    json!({
        "bays": bays,
        "clear_rects": clear_rects,
        "blocked_rects": blocked_rects,
    })
}

async fn get_display_attachments(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let prefs = state.v2.load_display_prefs();
    let payload = build_attachments_bus_payload(prefs.get("attachments"));
    (StatusCode::OK, Json(payload))
}

async fn put_display_attachments(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let payload = state.v2.publish_attachments(&body);
    (StatusCode::OK, Json(json!({ "ok": true, "attachments": payload })))
}

async fn put_display_power(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    state.v2.publish_power_prefs(&body);
    (StatusCode::OK, Json(json!({ "ok": true, "power": state.v2.load_display_prefs()["power"] })))
}

/// Authenticates EVERY `/v2/*` HTTP and WebSocket request:
/// 1. Rejects cross-origin browser requests (`Origin` outside Studio's own localhost/tauri origins)
///    to block browser WebSocket keylogging and CSRF attacks.
/// 2. Requires a valid daemon bearer token via `Authorization: Bearer <tok>`, `X-OpenFlux-Token`,
///    or `?token=<tok>` (for WebSocket clients).
async fn v2_auth_middleware(
    State(state): State<Arc<ApiState>>,
    req: Request,
    next: Next,
) -> Response {
    if let Some(origin) = req.headers().get("origin").and_then(|v| v.to_str().ok()) {
        let allowed = origin == "tauri://localhost"
            || origin == "https://tauri.localhost"
            || origin.starts_with("http://localhost:")
            || origin.starts_with("http://127.0.0.1:")
            || origin == "http://localhost"
            || origin == "http://127.0.0.1";
        if !allowed {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({ "error": "cross-origin browser access to OpenFlux v2 API is forbidden" })),
            )
                .into_response();
        }
    }

    let auth_owned: Option<String> = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| {
            req.headers()
                .get("x-openflux-token")
                .and_then(|v| v.to_str().ok())
                .map(|t| format!("Bearer {}", t.trim()))
        })
        .or_else(|| {
            req.uri().query().and_then(|q| {
                q.split('&').find_map(|pair| {
                    pair.strip_prefix("token=")
                        .filter(|v| !v.is_empty())
                        .map(|v| format!("Bearer {v}"))
                })
            })
        });

    if let Err((code, msg)) = state.verify_bearer(auth_owned.as_deref()) {
        let status = StatusCode::from_u16(code).unwrap_or(StatusCode::UNAUTHORIZED);
        return (status, Json(json!({ "error": msg }))).into_response();
    }

    next.run(req).await
}

/// Mounts all OpenFlux v2 routes (authenticated localhost bus, module management, disclosure, lists, presets, GPU preview).
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/v2/status", get(v2_status_api))
        .route("/v2/display/prefs", get(get_display_prefs))
        .route("/v2/display/parallax", put(put_display_parallax))
        .route("/v2/display/power", put(put_display_power))
        .route("/v2/display/attachments", get(get_display_attachments).put(put_display_attachments))
        .route("/v2/logs", get(get_all_logs_api))
        .route("/v2/vial/status", get(vial_status_api))
        .route("/v2/vial/xfer", post(vial_xfer_api))
        .route("/v2/vial/actions", get(vial_actions_api))
        .route("/v2/vial/sync", post(vial_sync_api))
        .route("/v2/bus/pub", post(bus_pub))
        .route("/v2/bus/retained", get(bus_retained))
        .route("/v2/bus/events", get(bus_events_poll))
        .route("/v2/bus/ws", get(bus_ws))
        .route("/v2/modules", get(list_modules))
        .route("/v2/modules/create", post(create_module))
        .route("/v2/modules/fork", post(fork_module))
        .route("/v2/modules/inspect", post(inspect_module_api))
        .route("/v2/modules/install", post(install_module_api))
        .route("/v2/modules/:id", get(get_module_detail).delete(uninstall_module_api))
        .route("/v2/modules/:id/update", post(update_module_api))
        .route("/v2/modules/:id/state", put(update_module_state))
        .route("/v2/modules/:id/action", post(trigger_module_action))
        .route("/v2/modules/:id/reload", post(reload_module_api))
        .route("/v2/modules/:id/logs", get(get_module_logs_api))
        .route("/v2/media/library", get(list_media_library_api))
        .route("/v2/media/import", post(import_media_file_api))
        .route("/v2/streams/:name", post(ingest_media_stream).delete(close_media_stream))
        .route("/v2/lists", get(list_curated_lists).post(add_curated_list))
        .route("/v2/lists/browse", get(browse_curated_list))
        .route("/v2/lists/:id", delete(remove_curated_list))
        .route("/v2/preset/export", post(export_preset_api))
        .route("/v2/preset/import", post(import_preset_api))
        .route("/v2/simulate/key", post(simulate_key))
        .route("/v2/simulate/impulse", post(simulate_impulse))
        .route("/v2/simulate/burst", post(simulate_physics_burst))
        .route("/v2/preview/frame", post(render_preview_frame))
        .route("/v2/keyboard/sync", post(keyboard_sync_v2))
        .route("/v2/keyboard/soc-stats", get(keyboard_soc_stats_v2))
        .route("/v2/keyboard/read", get(keyboard_read_file))
        .route("/v2/keyboard/upload-media", post(keyboard_upload_media))
        .route("/v2/keyboard/sideload", post(keyboard_sideload))
        .route("/v2/keyboard/flash-uf2", post(keyboard_flash_uf2))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            v2_auth_middleware,
        ))
        .with_state(state)
}
