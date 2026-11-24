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
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextTopBarSlot {
    pub module_id: String,
    pub pos: f32,
    #[serde(default = "default_ctx_span")]
    pub span: f32,
}

fn default_ctx_span() -> f32 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamDeckButton {
    pub id: String,
    pub title: String,
    pub plugin_id: String,
    pub action_uuid: String,
    #[serde(default)]
    pub icon_media: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub settings: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StreamDeckButtonsFile {
    #[serde(default)]
    pub buttons: Vec<StreamDeckButton>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostIdentity {
    pub host_id: String,
    pub host_name: String,
    pub os: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default = "default_ctx_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub match_apps: Vec<String>,
    #[serde(default)]
    pub match_hosts: Vec<String>,
    #[serde(default)]
    pub match_os: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub qmk_layer: Option<u8>,
    #[serde(default)]
    pub top_bar_slots: Option<Vec<ContextTopBarSlot>>,
    #[serde(default)]
    pub module_overrides: BTreeMap<String, BTreeMap<String, Value>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProfilesFile {
    #[serde(default)]
    pub manual_profile_id: Option<String>,
    #[serde(default)]
    pub profiles: Vec<ProfileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRule {
    pub id: String,
    pub name: String,
    #[serde(default = "default_ctx_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub match_apps: Vec<String>,
    #[serde(default)]
    pub qmk_layer: Option<u8>,
    #[serde(default)]
    pub top_bar_slots: Option<Vec<ContextTopBarSlot>>,
}

fn default_ctx_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextRulesFile {
    #[serde(default)]
    pub rules: Vec<ContextRule>,
}

struct StagedInstallTicket {
    staged: openflux_modules::StagedModuleSource,
    preview: openflux_modules::InstallPreview,
    created_at: std::time::Instant,
}

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
    staged_installs: Mutex<BTreeMap<String, StagedInstallTicket>>,
    last_context_sig: Mutex<String>,
    profile_cmd_rx: Mutex<openflux_bus::Subscription>,
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
        let profile_cmd_rx = broker
            .subscribe("profiles/command/#", SubscribeOptions::default())
            .expect("valid profile command sub");
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
            staged_installs: Mutex::new(BTreeMap::new()),
            last_context_sig: Mutex::new(String::new()),
            profile_cmd_rx: Mutex::new(profile_cmd_rx),
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

        state.publish_host_identity();
        state.evaluate_profiles();
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

    pub fn host_identity_path(&self) -> PathBuf {
        self.registry
            .state_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("host_identity.json")
    }

    pub fn load_host_identity(&self) -> HostIdentity {
        let p = self.host_identity_path();
        if let Ok(raw) = std::fs::read_to_string(&p) {
            if let Ok(id) = serde_json::from_str::<HostIdentity>(&raw) {
                if !id.host_id.trim().is_empty() {
                    return id;
                }
            }
        }
        let os = std::env::consts::OS.to_string();
        let raw_host = std::process::Command::new("hostname")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().trim_end_matches(".local").to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{os}-host"));
        let slug: String = raw_host
            .to_ascii_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .trim_matches('-')
            .to_string();
        let identity = HostIdentity {
            host_id: format!("{}-{}", os, if slug.is_empty() { "host" } else { &slug }),
            host_name: raw_host,
            os,
        };
        let _ = self.save_host_identity(&identity);
        identity
    }

    pub fn save_host_identity(&self, id: &HostIdentity) -> Result<(), String> {
        let p = self.host_identity_path();
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let txt = serde_json::to_string_pretty(id).map_err(|e| e.to_string())?;
        std::fs::write(&p, txt).map_err(|e| e.to_string())?;
        let _ = self
            .broker
            .publish("host/identity", json!(id), PublishOptions::retained());
        Ok(())
    }

    pub fn publish_host_identity(&self) {
        let id = self.load_host_identity();
        let _ = self
            .broker
            .publish("host/identity", json!(id), PublishOptions::retained());
    }

    pub fn profiles_path(&self) -> PathBuf {
        self.registry
            .state_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("profiles.json")
    }

    pub fn load_profiles(&self) -> ProfilesFile {
        let mut doc: ProfilesFile = std::fs::read_to_string(self.profiles_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_else(|| {
                let legacy = self.load_context_rules();
                let mut profiles = vec![ProfileEntry {
                    id: "default".into(),
                    name: "Default".into(),
                    parent: None,
                    enabled: true,
                    match_apps: vec![],
                    match_hosts: vec![],
                    match_os: vec![],
                    tags: vec!["base".into()],
                    qmk_layer: None,
                    top_bar_slots: None,
                    module_overrides: BTreeMap::new(),
                }];
                for r in legacy.rules {
                    profiles.push(ProfileEntry {
                        id: r.id,
                        name: r.name,
                        parent: Some("default".into()),
                        enabled: r.enabled,
                        match_apps: r.match_apps,
                        match_hosts: vec![],
                        match_os: vec![],
                        tags: vec![],
                        qmk_layer: r.qmk_layer,
                        top_bar_slots: r.top_bar_slots,
                        module_overrides: BTreeMap::new(),
                    });
                }
                ProfilesFile {
                    manual_profile_id: None,
                    profiles,
                }
            });
        if !doc.profiles.iter().any(|p| p.id == "default") {
            doc.profiles.insert(
                0,
                ProfileEntry {
                    id: "default".into(),
                    name: "Default".into(),
                    parent: None,
                    enabled: true,
                    match_apps: vec![],
                    match_hosts: vec![],
                    match_os: vec![],
                    tags: vec!["base".into()],
                    qmk_layer: None,
                    top_bar_slots: None,
                    module_overrides: BTreeMap::new(),
                },
            );
        }
        doc
    }

    pub fn save_profiles(&self, doc: &ProfilesFile) -> Result<(), String> {
        let p = self.profiles_path();
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let txt = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?;
        std::fs::write(&p, txt).map_err(|e| e.to_string())?;
        *self.last_context_sig.lock().unwrap() = String::new();
        self.evaluate_profiles();
        Ok(())
    }

    fn process_profile_commands(&self) {
        let msgs: Vec<openflux_bus::BusMessage> = self.profile_cmd_rx.lock().unwrap().drain();
        if msgs.is_empty() {
            return;
        }
        let mut doc = self.load_profiles();
        let enabled_ids: Vec<String> = doc
            .profiles
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.id.clone())
            .collect();
        if enabled_ids.is_empty() {
            return;
        }
        let mut changed = false;
        let mut toast_name: Option<String> = None;
        for msg in msgs {
            if msg.topic == "profiles/command/auto" {
                doc.manual_profile_id = None;
                changed = true;
                toast_name = Some("Auto Profile Mode".into());
            } else if msg.topic == "profiles/command/cycle" {
                let dir = msg.payload.get("dir").and_then(|v| v.as_i64()).unwrap_or(1);
                let active_id = self
                    .broker
                    .get_retained("profiles/active")
                    .and_then(|m| {
                        m.payload
                            .get("profile_id")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                    })
                    .unwrap_or_else(|| "default".into());
                let cur = doc.manual_profile_id.as_deref().unwrap_or(&active_id);
                let cur_idx = enabled_ids.iter().position(|id| id == cur).unwrap_or(0) as isize;
                let n = enabled_ids.len() as isize;
                let next_idx = ((cur_idx + (dir as isize)).rem_euclid(n)) as usize;
                let next_id = enabled_ids[next_idx].clone();
                let next_name = doc
                    .profiles
                    .iter()
                    .find(|p| p.id == next_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| next_id.clone());
                doc.manual_profile_id = Some(next_id);
                changed = true;
                toast_name = Some(format!("Profile: {next_name}"));
            }
        }
        if changed {
            let _ = self.save_profiles(&doc);
            if let Some(title) = toast_name {
                let _ = self.broker.publish(
                    "ui/toast",
                    json!({
                        "id": "profile-switch",
                        "icon": "star",
                        "title": title,
                        "color": "#a78bfa",
                        "duration_ms": 2000,
                    }),
                    PublishOptions::default(),
                );
            }
        }
    }

    pub fn evaluate_profiles(&self) {
        let host = self.load_host_identity();
        let doc = self.load_profiles();
        let app_msg = self.broker.get_retained("host/app/active");
        let app_hay = app_msg
            .as_ref()
            .map(|m| {
                format!(
                    "{} {} {}",
                    m.payload.get("process_name").and_then(|v| v.as_str()).unwrap_or(""),
                    m.payload.get("app_id").and_then(|v| v.as_str()).unwrap_or(""),
                    m.payload.get("window_title").and_then(|v| v.as_str()).unwrap_or(""),
                )
                .to_ascii_lowercase()
            })
            .unwrap_or_default();
        let app_display = app_msg
            .as_ref()
            .and_then(|m| m.payload.get("process_name").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();

        let by_id: BTreeMap<&str, &ProfileEntry> =
            doc.profiles.iter().map(|p| (p.id.as_str(), p)).collect();

        let host_matches = |p: &ProfileEntry| -> bool {
            let ok_os = p.match_os.is_empty()
                || p.match_os.iter().any(|o| o.eq_ignore_ascii_case(&host.os));
            let ok_host = p.match_hosts.is_empty()
                || p.match_hosts.iter().any(|h| {
                    h.eq_ignore_ascii_case(&host.host_id) || h.eq_ignore_ascii_case(&host.host_name)
                });
            ok_os && ok_host
        };

        let (selected_id, source, manual_override): (&str, String, bool) =
            if let Some(ref mid) = doc.manual_profile_id {
                if by_id.contains_key(mid.as_str()) {
                    (mid.as_str(), "manual".to_string(), true)
                } else {
                    ("default", "default".to_string(), false)
                }
            } else if let Some(app_hit) = doc.profiles.iter().find(|p| {
                p.enabled
                    && p.id != "default"
                    && !p.match_apps.is_empty()
                    && host_matches(p)
                    && p.match_apps.iter().any(|tok| {
                        let t = tok.trim().to_ascii_lowercase();
                        !t.is_empty() && app_hay.contains(&t)
                    })
            }) {
                (app_hit.id.as_str(), format!("app:{app_display}"), false)
            } else if let Some(host_hit) = doc.profiles.iter().find(|p| {
                p.enabled
                    && p.id != "default"
                    && p.match_apps.is_empty()
                    && (!p.match_hosts.is_empty() || !p.match_os.is_empty())
                    && host_matches(p)
            }) {
                (host_hit.id.as_str(), format!("host:{}", host.host_name), false)
            } else {
                ("default", "default".to_string(), false)
            };

        // Cycle-safe inheritance fall-through chain: child -> parent -> ... -> default
        let mut chain_entries: Vec<&ProfileEntry> = Vec::new();
        let mut cur_opt = by_id.get(selected_id).copied();
        let mut visited = std::collections::HashSet::new();
        while let Some(entry) = cur_opt {
            if !visited.insert(entry.id.as_str()) || chain_entries.len() >= 16 {
                break;
            }
            chain_entries.push(entry);
            cur_opt = entry
                .parent
                .as_deref()
                .and_then(|pid| by_id.get(pid).copied());
        }
        if !visited.contains("default") {
            if let Some(def) = by_id.get("default").copied() {
                chain_entries.push(def);
            }
        }

        let chain_ids: Vec<String> = chain_entries.iter().map(|e| e.id.clone()).collect();
        let mut tags: Vec<String> = Vec::new();
        let mut qmk_layer: Option<u8> = None;
        let mut top_bar_slots: Option<Vec<ContextTopBarSlot>> = None;
        let mut merged_overrides: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();

        for e in &chain_entries {
            for t in &e.tags {
                if !tags.iter().any(|x| x.eq_ignore_ascii_case(t)) {
                    tags.push(t.clone());
                }
            }
            if qmk_layer.is_none() && e.qmk_layer.is_some() {
                qmk_layer = e.qmk_layer;
            }
            if top_bar_slots.is_none() && e.top_bar_slots.is_some() {
                top_bar_slots = e.top_bar_slots.clone();
            }
        }
        for e in chain_entries.iter().rev() {
            for (mod_id, kv) in &e.module_overrides {
                let target = merged_overrides.entry(mod_id.clone()).or_default();
                for (k, v) in kv {
                    target.insert(k.clone(), v.clone());
                }
            }
        }

        let profile_name = chain_entries
            .first()
            .map(|e| e.name.clone())
            .unwrap_or_else(|| "Default".into());

        let payload = json!({
            "profile_id": selected_id,
            "profile_name": profile_name,
            "chain": chain_ids,
            "source": source,
            "manual_override": manual_override,
            "host": host,
            "app_name": app_display,
            "tags": tags,
            "qmk_layer": qmk_layer,
            "top_bar_slots": top_bar_slots,
            "module_overrides": merged_overrides,
        });

        let sig = payload.to_string();
        {
            let mut prev = self.last_context_sig.lock().unwrap();
            if *prev == sig {
                return;
            }
            *prev = sig;
        }

        let _ = self
            .broker
            .publish("profiles/active", payload, PublishOptions::retained());
        let _ = self.broker.publish(
            "keyboard/layer_override",
            json!({ "layer": qmk_layer, "reason": profile_name }),
            PublishOptions::retained(),
        );
        if let Some(slots) = top_bar_slots {
            let _ = self.broker.publish(
                "layout/top_bar/context_override",
                json!({ "active": true, "rule_name": profile_name, "slots": slots }),
                PublishOptions::retained(),
            );
        } else {
            let _ = self.broker.publish(
                "layout/top_bar/context_override",
                json!({ "active": false }),
                PublishOptions::retained(),
            );
        }
    }

    pub fn context_rules_path(&self) -> PathBuf {
        self.registry
            .state_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("context_rules.json")
    }

    pub fn load_context_rules(&self) -> ContextRulesFile {
        std::fs::read_to_string(self.context_rules_path())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save_context_rules(&self, doc: &ContextRulesFile) -> Result<(), String> {
        let p = self.context_rules_path();
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let txt = serde_json::to_string_pretty(doc).map_err(|e| e.to_string())?;
        std::fs::write(&p, txt).map_err(|e| e.to_string())?;
        *self.last_context_sig.lock().unwrap() = String::new();
        self.evaluate_context_rules();
        Ok(())
    }

    /// Evaluates active `host/app/active` against saved Per-App Context Rules and atomically
    /// publishes `system/context/active`, `keyboard/layer_override` (triggering RP2040 `0x75`),
    /// and `layout/top_bar/context_override` (Apple Touch Bar per-app widget swaps!).
    pub fn evaluate_context_rules(&self) {
        let saved_mods = self.registry.load_state();
        let ctx_enabled = saved_mods
            .modules
            .get("app-context")
            .map(|m| m.enabled)
            .unwrap_or(true);
        let rules_doc = self.load_context_rules();
        let app_msg = self.broker.get_retained("host/app/active");
        let hay = app_msg
            .as_ref()
            .map(|m| {
                format!(
                    "{} {} {}",
                    m.payload.get("process_name").and_then(|v| v.as_str()).unwrap_or(""),
                    m.payload.get("app_id").and_then(|v| v.as_str()).unwrap_or(""),
                    m.payload.get("window_title").and_then(|v| v.as_str()).unwrap_or(""),
                )
                .to_ascii_lowercase()
            })
            .unwrap_or_default();
        let app_display = app_msg
            .as_ref()
            .and_then(|m| m.payload.get("process_name").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();

        let matched = if ctx_enabled && !hay.trim().is_empty() {
            rules_doc.rules.iter().find(|r| {
                r.enabled
                    && r.match_apps.iter().any(|tok| {
                        let t = tok.trim().to_ascii_lowercase();
                        !t.is_empty() && hay.contains(&t)
                    })
            })
        } else {
            None
        };

        let sig = match matched {
            Some(r) => format!(
                "match:{}:{}:{:?}:{}",
                r.id,
                app_display,
                r.qmk_layer,
                serde_json::to_string(&r.top_bar_slots).unwrap_or_default()
            ),
            None => format!("none:{app_display}"),
        };
        {
            let mut prev = self.last_context_sig.lock().unwrap();
            if *prev == sig {
                return;
            }
            *prev = sig;
        }

        if let Some(rule) = matched {
            let _ = self.broker.publish(
                "system/context/active",
                json!({
                    "active": true,
                    "rule_id": rule.id,
                    "rule_name": rule.name,
                    "app_name": app_display,
                    "qmk_layer": rule.qmk_layer,
                    "has_top_bar_override": rule.top_bar_slots.is_some(),
                }),
                PublishOptions::retained(),
            );
            if let Some(lyr) = rule.qmk_layer {
                let _ = self.broker.publish(
                    "keyboard/layer_override",
                    json!({ "layer": lyr, "reason": rule.name }),
                    PublishOptions::retained(),
                );
            } else {
                let _ = self.broker.publish(
                    "keyboard/layer_override",
                    json!({ "layer": Value::Null }),
                    PublishOptions::retained(),
                );
            }
            if let Some(ref slots) = rule.top_bar_slots {
                let _ = self.broker.publish(
                    "layout/top_bar/context_override",
                    json!({ "active": true, "rule_name": rule.name, "slots": slots }),
                    PublishOptions::retained(),
                );
            } else {
                let _ = self.broker.publish(
                    "layout/top_bar/context_override",
                    json!({ "active": false }),
                    PublishOptions::retained(),
                );
            }
        } else {
            let _ = self.broker.publish(
                "system/context/active",
                json!({ "active": false, "app_name": app_display }),
                PublishOptions::retained(),
            );
            // Clear context overrides cleanly only when rules existed
            if !rules_doc.rules.is_empty() {
                let _ = self.broker.publish(
                    "keyboard/layer_override",
                    json!({ "layer": Value::Null }),
                    PublishOptions::retained(),
                );
                let _ = self.broker.publish(
                    "layout/top_bar/context_override",
                    json!({ "active": false }),
                    PublishOptions::retained(),
                );
            }
        }
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
                        "label": if !entry.title.is_empty() { entry.title.clone() } else { entry.name.clone() },
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
        // Evaluate Profiles & Multi-Computer Context (`profiles/active`, Touch Bar swaps, QMK layers!)
        self.process_profile_commands();
        self.evaluate_profiles();
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
    let readme = std::fs::read_to_string(dm.path.join("README.md"))
        .ok()
        .map(|s| s.chars().take(32_768).collect::<String>());
    let host_status = state
        .v2
        .host_procs
        .lock()
        .unwrap()
        .status(&id, dm.manifest.entrypoints.host.is_some());
    (
        StatusCode::OK,
        Json(json!({
            "id": id,
            "manifest": dm.manifest,
            "tier": dm.tier,
            "path": dm.path,
            "state": saved.modules.get(&id).cloned().unwrap_or_default(),
            "host_status": host_status,
            "disclosure": disclosure,
            "readme": readme,
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
    inspect_token: Option<String>,
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
    let state_clone = state.clone();
    let res = tokio::task::spawn_blocking(move || {
        let saved = state_clone.v2.registry.load_state();
        let staged = openflux_modules::stage_module_source(
            &src,
            &saved.curated_lists,
            &state_clone.v2.registry.bundled_dir,
        )?;
        let preview = state_clone.v2.registry.preview_install(&staged.module_dir)?;
        let token = format!("stg_{}", openflux_bus::now_unix_ms());
        let mut map = state_clone.v2.staged_installs.lock().unwrap();
        // Prune expired inspection tickets (> 10 minutes old)
        map.retain(|_, t| t.created_at.elapsed() < std::time::Duration::from_secs(600));
        map.insert(
            token.clone(),
            StagedInstallTicket {
                staged,
                preview: preview.clone(),
                created_at: std::time::Instant::now(),
            },
        );
        Ok::<_, String>((token, preview))
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    match res {
        Ok((inspect_token, preview)) => (
            StatusCode::OK,
            Json(json!({ "preview": preview, "inspect_token": inspect_token })),
        ),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

async fn install_module_api(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<InspectInstallBody>,
) -> impl IntoResponse {
    let accept = body.accept_disclosure;
    let token_opt = body.inspect_token.clone().filter(|t| !t.trim().is_empty());
    let src_opt = body.effective_source().ok().map(str::to_string);
    if token_opt.is_none() && src_opt.is_none() {
        return err_resp(StatusCode::BAD_REQUEST, "missing `inspect_token`, `source`, or `path`");
    }
    let state_clone = state.clone();
    let res = tokio::task::spawn_blocking(move || {
        // Prefer TOCTOU-free staged ticket from `/v2/modules/inspect` (installs the exact bytes disclosed!)
        if let Some(tok) = token_opt {
            let ticket = state_clone.v2.staged_installs.lock().unwrap().remove(&tok);
            let Some(ticket) = ticket else {
                return Err("inspect_token expired or invalid; please re-inspect before installing".into());
            };
            if !accept {
                return Ok((false, None, ticket.preview));
            }
            let dm = state_clone.v2.registry.install_from_staged(&ticket.staged)?;
            return Ok::<_, String>((true, Some(dm), ticket.preview));
        }
        let src = src_opt.unwrap();
        let saved = state_clone.v2.registry.load_state();
        let staged = openflux_modules::stage_module_source(
            &src,
            &saved.curated_lists,
            &state_clone.v2.registry.bundled_dir,
        )?;
        let preview = state_clone.v2.registry.preview_install(&staged.module_dir)?;
        if !accept {
            return Ok((false, None, preview));
        }
        let dm = state_clone.v2.registry.install_from_staged(&staged)?;
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

#[derive(Deserialize)]
struct MediaFileQuery {
    name: String,
}

/// Streams a media file (MP4/MOV/PNG/JPG) from `~/.local/share/openflux/media` or bundled
/// `modules/bg-media` with HTTP 206 byte-range support for silky 60 FPS WKWebView `<video>` playback!
async fn serve_media_file_api(
    State(state): State<Arc<ApiState>>,
    Query(q): Query<MediaFileQuery>,
    headers: axum::http::HeaderMap,
) -> Response {
    let fname = q.name.trim();
    if fname.is_empty()
        || fname.contains('/')
        || fname.contains('\\')
        || fname.contains("..")
        || fname.contains('\0')
    {
        return err_resp(StatusCode::BAD_REQUEST, "invalid media filename").into_response();
    }
    let media_dir = state
        .v2
        .registry
        .state_path
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("media"))
        .unwrap_or_else(|| default_data_dir().join("media"));
    let candidates = [
        media_dir.join(fname),
        state.v2.registry.bundled_dir.join("bg-media").join(fname),
    ];
    let Some(path) = candidates.into_iter().find(|p| p.is_file()) else {
        return err_resp(StatusCode::NOT_FOUND, format!("media not found: {fname}")).into_response();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, "failed to read media").into_response();
    };
    let lower = fname.to_ascii_lowercase();
    let mime = if lower.ends_with(".mp4") {
        "video/mp4"
    } else if lower.ends_with(".mov") {
        "video/quicktime"
    } else if lower.ends_with(".webm") {
        "video/webm"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        "application/octet-stream"
    };
    let total = bytes.len();
    let allow_origin = headers
        .get("origin")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("tauri://localhost");

    if let Some(range_hdr) = headers.get("range").and_then(|v| v.to_str().ok()) {
        if let Some(spec) = range_hdr.strip_prefix("bytes=") {
            let mut parts = spec.split('-');
            let start = parts
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0)
                .min(total.saturating_sub(1));
            let end = parts
                .next()
                .and_then(|s| s.parse::<usize>().ok())
                .map(|e| e.min(total.saturating_sub(1)))
                .unwrap_or_else(|| total.saturating_sub(1));
            if start <= end && total > 0 {
                let slice = bytes[start..=end].to_vec();
                return axum::http::Response::builder()
                    .status(StatusCode::PARTIAL_CONTENT)
                    .header("Content-Type", mime)
                    .header("Accept-Ranges", "bytes")
                    .header("Content-Length", slice.len().to_string())
                    .header("Content-Range", format!("bytes {start}-{end}/{total}"))
                    .header("Access-Control-Allow-Origin", allow_origin)
                    .body(axum::body::Body::from(slice))
                    .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
            }
        }
    }

    axum::http::Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", mime)
        .header("Accept-Ranges", "bytes")
        .header("Content-Length", total.to_string())
        .header("Access-Control-Allow-Origin", allow_origin)
        .body(axum::body::Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
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
    #[serde(default)]
    overlay_only: bool,
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
    let mut last_bg_name: Option<String> = None;
    let mut last_bg_dim: f32 = 0.0;
    let mut last_top_fade: f32 = 0.0;
    for s in 0..steps {
        let t_lua = std::time::Instant::now();
        let scene = rt.step_frame(dt_sec, now_ms + (s as f32 * body.step_ms) as u64);
        lua_ms = t_lua.elapsed().as_secs_f32() * 1000.0;
        last_bg_name = scene
            .background_media
            .as_deref()
            .and_then(|p| std::path::Path::new(p).file_name().and_then(|n| n.to_str()))
            .map(str::to_string);
        last_bg_dim = scene.bg_dimming;
        last_top_fade = scene.top_fade;
        last_timings = comp.render_scene(&scene);
    }
    let mut png_bytes = Vec::new();
    let (out_w, out_h) = if body.overlay_only {
        let (ow, oh, slice) = comp.overlay_rgba();
        if let Some(img) = image::RgbaImage::from_raw(ow, oh, slice.to_vec()) {
            let mut cursor = std::io::Cursor::new(&mut png_bytes);
            if let Err(e) = img.write_to(&mut cursor, image::ImageFormat::Png) {
                return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
            }
        }
        (ow, oh)
    } else {
        let rgba = match comp.readback_rgba() {
            Ok(b) => b,
            Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
        };
        let w = body.width.clamp(320, 1920);
        let h = body.height.clamp(180, 1080);
        if let Some(img) = image::RgbaImage::from_raw(w, h, rgba) {
            let mut cursor = std::io::Cursor::new(&mut png_bytes);
            if let Err(e) = img.write_to(&mut cursor, image::ImageFormat::Png) {
                return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
            }
        }
        (w, h)
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    let statuses = rt.statuses();
    let adapter = comp.adapter_summary();
    (
        StatusCode::OK,
        Json(json!({
            "width": out_w,
            "height": out_h,
            "gpu_adapter": adapter,
            "modules_rev": state.v2.modules_rev.load(Ordering::SeqCst),
            "png_base64": b64,
            "background_media_name": last_bg_name,
            "bg_dimming": last_bg_dim,
            "top_fade": last_top_fade,
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
    let host_id_for_sync = state.v2.load_host_identity().host_id;
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

        // Sync `profiles.json` + per-host backup `/home/root/openflux/v2/hosts/<host_id>/profiles.json`
        let prof_local = kb_state_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("profiles.json");
        if let Ok(bytes) = std::fs::read(&prof_local) {
            for remote in [
                "/home/root/openflux/v2/profiles.json".to_string(),
                format!("/home/root/openflux/v2/hosts/{host_id_for_sync}/profiles.json"),
            ] {
                uploaded_bytes += bytes.len();
                device.put_file(&remote, &bytes)?;
                uploaded_files.push(remote);
            }
        }

        // Sync `modules/streamdeck` custom button icons (from `streamdeck/buttons.json`) to keyboard!
        let sd_btns_path = kb_state_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .join("streamdeck/buttons.json");
        if let Ok(raw) = std::fs::read_to_string(&sd_btns_path) {
            if let Ok(doc) = serde_json::from_str::<StreamDeckButtonsFile>(&raw) {
                let media_dir = default_data_dir().join("media");
                for b in &doc.buttons {
                    if let Some(ref fname) = b.icon_media {
                        let clean = std::path::Path::new(fname)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or(fname);
                        let lp = media_dir.join(clean);
                        if lp.is_file() {
                            let remote = format!("/home/root/openflux/media/{clean}");
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

fn streamdeck_root(state: &ApiState) -> PathBuf {
    state
        .v2
        .registry
        .state_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("streamdeck")
}

fn copy_dir_recursive_safe(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
        let p = entry.path();
        let name = entry.file_name();
        let target = dst.join(&name);
        if p.is_dir() {
            copy_dir_recursive_safe(&p, &target)?;
        } else if p.is_file() {
            std::fs::copy(&p, &target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn find_sd_icon_file(plugin_dir: &std::path::Path, icon_stem: &str) -> Option<PathBuf> {
    if icon_stem.is_empty() || icon_stem.contains("..") {
        return None;
    }
    for suffix in ["@2x.png", ".png", ".svg", ""] {
        let cand = plugin_dir.join(format!("{icon_stem}{suffix}"));
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

fn parse_sd_plugin_dir(plugin_dir: &std::path::Path) -> Option<Value> {
    let manifest_path = plugin_dir.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path).ok()?;
    let m: Value = serde_json::from_str(&raw).ok()?;
    let dir_name = plugin_dir.file_name()?.to_string_lossy().into_owned();
    let plugin_id = dir_name
        .strip_suffix(".sdPlugin")
        .unwrap_or(&dir_name)
        .to_string();
    let name = m
        .get("Name")
        .and_then(|v| v.as_str())
        .unwrap_or(&plugin_id)
        .to_string();
    let author = m.get("Author").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let version = m.get("Version").and_then(|v| v.as_str()).unwrap_or("1.0").to_string();
    let description = m.get("Description").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let code_path = m
        .get("CodePathMac")
        .or_else(|| m.get("CodePath"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let property_inspector = m
        .get("PropertyInspectorPath")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let actions: Vec<Value> = m
        .get("Actions")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    let uuid = a.get("UUID").and_then(|v| v.as_str())?;
                    let aname = a.get("Name").and_then(|v| v.as_str()).unwrap_or(uuid);
                    let tooltip = a.get("Tooltip").and_then(|v| v.as_str()).unwrap_or("");
                    let icon_stem = a
                        .get("Icon")
                        .and_then(|v| v.as_str())
                        .or_else(|| {
                            a.get("States")
                                .and_then(|s| s.as_array())
                                .and_then(|arr| arr.first())
                                .and_then(|st| st.get("Image"))
                                .and_then(|v| v.as_str())
                        })
                        .unwrap_or("");
                    let icon_file = find_sd_icon_file(plugin_dir, icon_stem)
                        .and_then(|p| p.strip_prefix(plugin_dir).ok().map(|r| r.to_string_lossy().into_owned()));
                    let states: Vec<Value> = a
                        .get("States")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    Some(json!({
                        "uuid": uuid,
                        "name": aname,
                        "tooltip": tooltip,
                        "icon": icon_file,
                        "states": states,
                    }))
                })
                .collect()
        })
        .unwrap_or_default();

    Some(json!({
        "id": plugin_id,
        "name": name,
        "author": author,
        "version": version,
        "description": description,
        "code_path": code_path,
        "property_inspector": property_inspector,
        "path": plugin_dir.to_string_lossy(),
        "actions": actions,
    }))
}

async fn get_streamdeck_state_api(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let root = streamdeck_root(&state);
    let plugins_dir = root.join("plugins");
    let _ = std::fs::create_dir_all(&plugins_dir);

    let mut plugins = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&plugins_dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if let Some(parsed) = parse_sd_plugin_dir(&p) {
                    plugins.push(parsed);
                }
            }
        }
    }

    // Also scan standard macOS Elgato Stream Deck Plugins folder so users can import with 1 click!
    let mut elgato_detected = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let elgato_dir = PathBuf::from(home).join("Library/Application Support/com.elgato.StreamDeck/Plugins");
        if let Ok(rd) = std::fs::read_dir(&elgato_dir) {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_dir() && p.extension().and_then(|e| e.to_str()) == Some("sdPlugin") {
                    if let Some(parsed) = parse_sd_plugin_dir(&p) {
                        elgato_detected.push(parsed);
                    }
                }
            }
        }
    }

    let buttons_doc: StreamDeckButtonsFile = std::fs::read_to_string(root.join("buttons.json"))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();

    let faces = state
        .v2
        .broker
        .get_retained("modules/streamdeck/faces")
        .map(|m| m.payload)
        .unwrap_or_else(|| json!({}));
    let ws_port = state
        .v2
        .broker
        .get_retained("modules/streamdeck/host_info")
        .and_then(|m| m.payload.get("ws_port").and_then(|v| v.as_u64()))
        .unwrap_or(0);

    (
        StatusCode::OK,
        Json(json!({
            "plugins": plugins,
            "elgato_detected": elgato_detected,
            "buttons": buttons_doc.buttons,
            "faces": faces,
            "ws_port": ws_port,
        })),
    )
}

#[derive(Deserialize)]
struct ImportStreamDeckBody {
    path: String,
}

async fn import_streamdeck_plugin_api(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<ImportStreamDeckBody>,
) -> impl IntoResponse {
    let src = PathBuf::from(body.path.trim());
    if !src.exists() {
        return err_resp(StatusCode::BAD_REQUEST, format!("path not found: {}", src.display()));
    }
    let root = streamdeck_root(&state);
    let plugins_dir = root.join("plugins");
    let media_dir = state
        .v2
        .registry
        .state_path
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("media"))
        .unwrap_or_else(|| default_data_dir().join("media"));
    let _ = std::fs::create_dir_all(&plugins_dir);
    let _ = std::fs::create_dir_all(&media_dir);

    let installed_dir: PathBuf = if src.is_dir() {
        if !src.join("manifest.json").is_file() {
            return err_resp(
                StatusCode::BAD_REQUEST,
                "folder is not a valid .sdPlugin (missing manifest.json)",
            );
        }
        let name = src
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("imported.sdPlugin");
        let safe_name: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '_')
            .collect();
        let dest = plugins_dir.join(if safe_name.ends_with(".sdPlugin") {
            safe_name
        } else {
            format!("{safe_name}.sdPlugin")
        });
        if dest.exists() {
            let _ = std::fs::remove_dir_all(&dest);
        }
        if let Err(e) = copy_dir_recursive_safe(&src, &dest) {
            return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
        dest
    } else {
        // `.streamDeckPlugin` ZIP archive: extract safely into temp dir with zip-slip checks!
        let file = match std::fs::File::open(&src) {
            Ok(f) => f,
            Err(e) => return err_resp(StatusCode::BAD_REQUEST, format!("open failed: {e}")),
        };
        let mut archive = match zip::ZipArchive::new(file) {
            Ok(a) => a,
            Err(e) => return err_resp(StatusCode::BAD_REQUEST, format!("not a valid .streamDeckPlugin zip: {e}")),
        };
        let tmp_extract = std::env::temp_dir().join(format!("keyglass-sd-{}", openflux_bus::now_unix_ms()));
        let _ = std::fs::create_dir_all(&tmp_extract);
        for i in 0..archive.len() {
            let Ok(mut entry) = archive.by_index(i) else { continue };
            let Some(enclosed) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
                continue;
            };
            let out_path = tmp_extract.join(&enclosed);
            if entry.is_dir() {
                let _ = std::fs::create_dir_all(&out_path);
            } else {
                if let Some(parent) = out_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Ok(mut out_f) = std::fs::File::create(&out_path) {
                    let _ = std::io::copy(&mut entry, &mut out_f);
                }
            }
        }
        // Locate the `.sdPlugin` directory inside the extracted archive
        let found_sd = std::fs::read_dir(&tmp_extract)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.is_dir() && p.join("manifest.json").is_file())
            .or_else(|| {
                if tmp_extract.join("manifest.json").is_file() {
                    Some(tmp_extract.clone())
                } else {
                    None
                }
            });
        let Some(sd_src) = found_sd else {
            let _ = std::fs::remove_dir_all(&tmp_extract);
            return err_resp(StatusCode::BAD_REQUEST, "archive does not contain a valid .sdPlugin/manifest.json");
        };
        let raw_name = sd_src
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("plugin.sdPlugin");
        let safe_name: String = raw_name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '_')
            .collect();
        let dest = plugins_dir.join(if safe_name.ends_with(".sdPlugin") {
            safe_name
        } else {
            format!("{safe_name}.sdPlugin")
        });
        if dest.exists() {
            let _ = std::fs::remove_dir_all(&dest);
        }
        let copy_res = copy_dir_recursive_safe(&sd_src, &dest);
        let _ = std::fs::remove_dir_all(&tmp_extract);
        if let Err(e) = copy_res {
            return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
        dest
    };

    // Copy any action PNG icons from the imported `.sdPlugin` into `~/.local/share/openflux/media/`
    let parsed = parse_sd_plugin_dir(&installed_dir);
    if let Some(ref pdoc) = parsed {
        let pid = pdoc["id"].as_str().unwrap_or("sd");
        if let Some(acts) = pdoc["actions"].as_array() {
            for a in acts {
                if let Some(rel) = a["icon"].as_str() {
                    let src_img = installed_dir.join(rel);
                    if src_img.is_file() && rel.to_ascii_lowercase().ends_with(".png") {
                        let uuid_slug: String = a["uuid"]
                            .as_str()
                            .unwrap_or("act")
                            .chars()
                            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                            .collect();
                        let dst_name = format!("sd_{pid}_{uuid_slug}.png");
                        let _ = std::fs::copy(&src_img, media_dir.join(&dst_name));
                    }
                }
            }
        }
    }

    (StatusCode::OK, Json(json!({ "ok": true, "plugin": parsed })))
}

async fn delete_streamdeck_plugin_api(
    State(state): State<Arc<ApiState>>,
    AxumPath(id): AxumPath<String>,
) -> impl IntoResponse {
    let clean: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '_')
        .collect();
    if clean.is_empty() || clean.contains("..") {
        return err_resp(StatusCode::BAD_REQUEST, "invalid plugin id");
    }
    let plugins_dir = streamdeck_root(&state).join("plugins");
    let target = plugins_dir.join(if clean.ends_with(".sdPlugin") {
        clean
    } else {
        format!("{clean}.sdPlugin")
    });
    if !target.is_dir() {
        return err_resp(StatusCode::NOT_FOUND, "plugin not found");
    }
    if let Err(e) = std::fs::remove_dir_all(&target) {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    (StatusCode::OK, Json(json!({ "ok": true, "deleted": id })))
}

async fn put_streamdeck_buttons_api(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<StreamDeckButtonsFile>,
) -> impl IntoResponse {
    let root = streamdeck_root(&state);
    let _ = std::fs::create_dir_all(&root);
    let txt = match serde_json::to_string_pretty(&body) {
        Ok(t) => t,
        Err(e) => return err_resp(StatusCode::BAD_REQUEST, e.to_string()),
    };
    if let Err(e) = std::fs::write(root.join("buttons.json"), txt) {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    // Reconcile QMK custom slots + publish `system/actions` + broadcast `modules/streamdeck/buttons`!
    state.v2.publish_system_actions();
    let _ = state.v2.broker.publish(
        "modules/streamdeck/buttons",
        json!({ "buttons": body.buttons }),
        PublishOptions::retained(),
    );
    // Immediately upload any button PNG icons to `/home/root/openflux/media/` on the connected
    // keyboard so newly added Stream Deck keys show their real icon right away!
    let icon_files: Vec<String> = body
        .buttons
        .iter()
        .filter_map(|b| b.icon_media.clone())
        .collect();
    if !icon_files.is_empty() {
        let dev = state.device.clone();
        let media_dir = default_data_dir().join("media");
        tokio::task::spawn_blocking(move || {
            for fname in icon_files {
                let clean = std::path::Path::new(&fname)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&fname);
                let lp = media_dir.join(clean);
                if lp.is_file() {
                    let remote = format!("/home/root/openflux/media/{clean}");
                    if !dev.stat_file(&remote).unwrap_or(false) {
                        if let Ok(bytes) = std::fs::read(&lp) {
                            let _ = dev.put_file(&remote, &bytes);
                        }
                    }
                }
            }
        });
    }
    (StatusCode::OK, Json(json!({ "ok": true, "buttons": body.buttons })))
}

#[derive(Deserialize)]
struct SdPluginAssetQuery {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    ws_port: Option<u16>,
    #[serde(default)]
    btn_id: Option<String>,
    #[serde(default)]
    action_uuid: Option<String>,
    #[serde(default)]
    pi_boot: Option<u8>,
    #[serde(default)]
    plugin_boot: Option<u8>,
}

/// Serves static files inside an imported `.sdPlugin` directory (HTML/CSS/JS/PNG/SVG) so KeyGlass
/// can render every Elgato plugin's native `PropertyInspectorPath` settings UI (e.g. Audio Device
/// picker, OBS Scene picker, Hue Light picker) connected live to `sd_host.py`'s WebSocket server!
async fn serve_streamdeck_plugin_asset_api(
    State(state): State<Arc<ApiState>>,
    AxumPath((plugin_id, rel)): AxumPath<(String, String)>,
    Query(q): Query<SdPluginAssetQuery>,
) -> Response {
    let clean_id: String = plugin_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-' || *c == '_')
        .collect();
    if clean_id.is_empty() || clean_id.contains("..") || rel.contains("..") || rel.contains('\0') {
        return err_resp(StatusCode::BAD_REQUEST, "invalid plugin asset path").into_response();
    }
    let plugins_dir = streamdeck_root(&state).join("plugins");
    let pdir = plugins_dir.join(if clean_id.ends_with(".sdPlugin") {
        clean_id.clone()
    } else {
        format!("{clean_id}.sdPlugin")
    });
    let file_path = pdir.join(rel.trim_start_matches('/'));
    if !file_path.is_file() {
        return err_resp(StatusCode::NOT_FOUND, "plugin asset not found").into_response();
    }
    let Ok(bytes) = std::fs::read(&file_path) else {
        return err_resp(StatusCode::INTERNAL_SERVER_ERROR, "failed to read asset").into_response();
    };
    let lower = rel.to_ascii_lowercase();
    let mime = if lower.ends_with(".html") || lower.ends_with(".htm") {
        "text/html; charset=utf-8"
    } else if lower.ends_with(".js") || lower.ends_with(".mjs") {
        "application/javascript; charset=utf-8"
    } else if lower.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".json") {
        "application/json"
    } else {
        "application/octet-stream"
    };

    // Inject Elgato's official `connectElgatoStreamDeckSocket(...)` bootstrap into BOTH
    // Property Inspector HTML (`pi_boot=1`) AND HTML-based `.sdPlugin` runtimes (`plugin_boot=1`,
    // like Task Timer's `index.html`)!
    if (lower.ends_with(".html") || lower.ends_with(".htm")) && (q.pi_boot == Some(1) || q.plugin_boot == Some(1)) {
        if let Some(ws_port) = q.ws_port {
            let mut html = String::from_utf8_lossy(&bytes).into_owned();
            let info_json = json!({
                "application": { "language": "en", "platform": "mac", "platformVersion": "14.0", "version": "6.5.0" },
                "plugin": { "uuid": clean_id, "version": "1.0.0" },
                "devicePixelRatio": 2,
                "devices": [{ "id": "keyglass-main", "name": "KeyGlass", "size": { "columns": 15, "rows": 6 }, "type": 0 }]
            });
            let boot_script = if q.plugin_boot == Some(1) {
                format!(
                    r#"<script>
window.addEventListener('load', function() {{
  try {{
    if (typeof window.connectElgatoStreamDeckSocket === 'function') {{
      window.connectElgatoStreamDeckSocket(
        {ws_port},
        {pid_js},
        'registerPlugin',
        {info_js}
      );
    }}
  }} catch (e) {{ console.error('KeyGlass HTML plugin boot error:', e); }}
}});
</script>"#,
                    ws_port = ws_port,
                    pid_js = serde_json::to_string(&clean_id).unwrap_or_else(|_| "\"\"".into()),
                    info_js = serde_json::to_string(&info_json.to_string()).unwrap_or_else(|_| "\"{}\"".into()),
                )
            } else {
                let btn_id = q.btn_id.unwrap_or_default();
                let action_uuid = q.action_uuid.unwrap_or_default();
                let buttons_doc: StreamDeckButtonsFile =
                    std::fs::read_to_string(streamdeck_root(&state).join("buttons.json"))
                        .ok()
                        .and_then(|r| serde_json::from_str(&r).ok())
                        .unwrap_or_default();
                let cur_settings = buttons_doc
                    .buttons
                    .iter()
                    .find(|b| b.id == btn_id)
                    .map(|b| b.settings.clone())
                    .unwrap_or_else(|| json!({}));
                let action_info_json = json!({
                    "action": action_uuid,
                    "context": btn_id,
                    "device": "keyglass-main",
                    "payload": {
                        "settings": cur_settings,
                        "coordinates": { "column": 0, "row": 0 }
                    }
                });
                format!(
                    r#"<script>
window.addEventListener('load', function() {{
  try {{
    if (typeof window.connectElgatoStreamDeckSocket === 'function') {{
      window.connectElgatoStreamDeckSocket(
        {ws_port},
        {btn_id_js},
        'registerPropertyInspector',
        {info_js},
        {action_info_js}
      );
    }}
  }} catch (e) {{ console.error('KeyGlass PI boot error:', e); }}
}});
</script>"#,
                    ws_port = ws_port,
                    btn_id_js = serde_json::to_string(&btn_id).unwrap_or_else(|_| "\"\"".into()),
                    info_js = serde_json::to_string(&info_json.to_string()).unwrap_or_else(|_| "\"{}\"".into()),
                    action_info_js = serde_json::to_string(&action_info_json.to_string()).unwrap_or_else(|_| "\"{}\"".into()),
                )
            };
            if let Some(pos) = html.rfind("</body>") {
                html.insert_str(pos, &boot_script);
            } else {
                html.push_str(&boot_script);
            }
            let mut builder = axum::http::Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", mime);
            if let Some(tok) = q.token.as_deref().filter(|t| !t.is_empty()) {
                builder = builder.header(
                    "Set-Cookie",
                    format!("openflux_sd_token={tok}; Path=/v2/streamdeck/plugin-asset/; HttpOnly; SameSite=Lax"),
                );
            }
            return builder
                .body(axum::body::Body::from(html))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response());
        }
    }

    axum::http::Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", mime)
        .body(axum::body::Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn get_host_identity_api(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    (StatusCode::OK, Json(json!(state.v2.load_host_identity())))
}

async fn put_host_identity_api(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<HostIdentity>,
) -> impl IntoResponse {
    match state.v2.save_host_identity(&body) {
        Ok(()) => {
            *state.v2.last_context_sig.lock().unwrap() = String::new();
            state.v2.evaluate_profiles();
            (StatusCode::OK, Json(json!({ "ok": true, "host": body })))
        }
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn get_profiles_api(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let doc = state.v2.load_profiles();
    let active = state
        .v2
        .broker
        .get_retained("profiles/active")
        .map(|m| m.payload)
        .unwrap_or_else(|| json!({ "profile_id": "default", "profile_name": "Default", "chain": ["default"] }));
    let host = state.v2.load_host_identity();
    let current_app = state
        .v2
        .broker
        .get_retained("host/app/active")
        .map(|m| m.payload);
    (
        StatusCode::OK,
        Json(json!({
            "manual_profile_id": doc.manual_profile_id,
            "profiles": doc.profiles,
            "active": active,
            "host": host,
            "current_app": current_app,
        })),
    )
}

async fn put_profiles_api(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<ProfilesFile>,
) -> impl IntoResponse {
    match state.v2.save_profiles(&body) {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true, "profiles": body.profiles }))),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Deserialize)]
struct SelectProfileBody {
    profile_id: Option<String>,
}

async fn select_profile_api(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<SelectProfileBody>,
) -> impl IntoResponse {
    let mut doc = state.v2.load_profiles();
    doc.manual_profile_id = body.profile_id.filter(|s| !s.trim().is_empty());
    match state.v2.save_profiles(&doc) {
        Ok(()) => {
            let active = state
                .v2
                .broker
                .get_retained("profiles/active")
                .map(|m| m.payload);
            (StatusCode::OK, Json(json!({ "ok": true, "active": active })))
        }
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn get_context_rules(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let doc = state.v2.load_context_rules();
    let active = state
        .v2
        .broker
        .get_retained("system/context/active")
        .map(|m| m.payload)
        .unwrap_or_else(|| json!({ "active": false }));
    let current_app = state
        .v2
        .broker
        .get_retained("host/app/active")
        .map(|m| m.payload);
    (
        StatusCode::OK,
        Json(json!({
            "rules": doc.rules,
            "active_context": active,
            "current_app": current_app,
        })),
    )
}

async fn put_context_rules(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<ContextRulesFile>,
) -> impl IntoResponse {
    match state.v2.save_context_rules(&body) {
        Ok(()) => (StatusCode::OK, Json(json!({ "ok": true, "rules": body.rules }))),
        Err(e) => err_resp(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn get_keyboard_software_status(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let (has_bridge, has_firmware) = state.device.is_connected();
    let is_v2 = state.device.is_v2_mux_active();
    let versions = state.device.versions();
    let bridge_ver = versions
        .get("bridge")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let display_ver = versions
        .get("display")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let fw_ver = versions
        .get("firmware")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mode = if !has_bridge && !has_firmware {
        "offline"
    } else if is_v2
        || bridge_ver.contains("openflux")
        || display_ver.contains("openflux")
        || fw_ver.contains("openflux")
    {
        "keyglass"
    } else {
        "stock"
    };
    (
        StatusCode::OK,
        Json(json!({
            "mode": mode,
            "connected": has_bridge || has_firmware,
            "ofv2": is_v2,
            "versions": versions,
        })),
    )
}

async fn return_keyboard_to_stock(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let svc_opt = state.updates.lock().unwrap().clone();
    let Some(svc) = svc_opt else {
        return err_resp(
            StatusCode::SERVICE_UNAVAILABLE,
            "Factory restore index service is not initialized on this daemon.",
        );
    };
    let res = tokio::task::spawn_blocking(move || svc.restore(true)).await;
    match res {
        Ok(Ok(job)) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "message": "Signed factory software restore started — keep the keyboard plugged in while it returns to Stock.",
                "job": job,
            })),
        ),
        Ok(Err(e)) => err_resp(StatusCode::CONFLICT, format!("{e:?}")),
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
        })
        .or_else(|| {
            // Allow scoped HttpOnly cookie ONLY for static sub-resources (JS/CSS/audio) loaded
            // inside an authenticated `.sdPlugin` Property Inspector or HTML plugin iframe!
            if req.uri().path().starts_with("/v2/streamdeck/plugin-asset/") {
                req.headers()
                    .get("cookie")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|c| {
                        c.split(';').find_map(|part| {
                            part.trim()
                                .strip_prefix("openflux_sd_token=")
                                .filter(|v| !v.is_empty())
                                .map(|v| format!("Bearer {v}"))
                        })
                    })
            } else {
                None
            }
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
        .route("/v2/host/identity", get(get_host_identity_api).put(put_host_identity_api))
        .route("/v2/profiles", get(get_profiles_api).put(put_profiles_api))
        .route("/v2/profiles/select", post(select_profile_api))
        .route("/v2/streamdeck/state", get(get_streamdeck_state_api))
        .route("/v2/streamdeck/import", post(import_streamdeck_plugin_api))
        .route("/v2/streamdeck/plugins/:id", delete(delete_streamdeck_plugin_api))
        .route("/v2/streamdeck/plugin-asset/:plugin_id/*rel", get(serve_streamdeck_plugin_asset_api))
        .route("/v2/streamdeck/buttons", put(put_streamdeck_buttons_api))
        .route("/v2/context/rules", get(get_context_rules).put(put_context_rules))
        .route("/v2/keyboard/software-status", get(get_keyboard_software_status))
        .route("/v2/keyboard/return-to-stock", post(return_keyboard_to_stock))
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
        .route("/v2/media/file", get(serve_media_file_api))
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
