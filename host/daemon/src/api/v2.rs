//! OpenFlux v2 host runtime and open localhost HTTP/WebSocket API.
//!
//! Exposes the hierarchical pub/sub bus (`openflux-bus`), unified module management & disclosure
//! inspection (`openflux-modules`), unrestricted host process execution, curated lists, secret-free
//! presets, synthetic input simulation, and live GPU compositor previews (`openflux-compositor`).

use crate::api::state::{default_data_dir, ApiState};
use axum::extract::ws::{Message as WsMessage, WebSocket};
use axum::extract::{Path as AxumPath, Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use base64::Engine;
use openflux_bus::{
    topics, AnalogKeyFrame, AnalogKeySample, Broker, FrameLayout, HostConnectionState, KeyEvent,
    KeyEventKind, KeyboardGeometry, PublishOptions, SubscribeOptions, SurfaceImpulse, WireFrame,
};
use openflux_compositor::{Compositor, CompositorOptions};
use openflux_modules::{
    inspect_module_dir, CuratedListDocument, CuratedListSubscription, HostProcessManager,
    ModuleCategory, ModuleInstanceState, ModuleRegistry, ModuleRuntime, SetupPreset,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

pub struct V2State {
    pub broker: Broker,
    pub registry: ModuleRegistry,
    pub runtime: Mutex<ModuleRuntime>,
    pub host_procs: Mutex<HostProcessManager>,
    pub modules_rev: AtomicU64,
    last_disk_sig: Mutex<String>,
    compositor: Mutex<Option<Compositor>>,
    geometry: KeyboardGeometry,
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
        PathBuf::from("assets/bundled_fonts/Montserrat-VariableFont_wght.ttf"),
        PathBuf::from("dist/assets/fonts/Montserrat-VariableFont_wght.ttf"),
    ];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(pp) = exe.parent().and_then(|p| p.parent()) {
            candidates.push(pp.join("Resources/assets/bundled_fonts/Montserrat-VariableFont_wght.ttf"));
        }
    }
    if let Ok(home) = std::env::var("HOME") {
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
    pub fn new(data_dir: Option<PathBuf>) -> Arc<Self> {
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
        let geometry = KeyboardGeometry::build(FrameLayout::Ansi);
        let _ = broker.publish_typed(
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

        let mut runtime = ModuleRuntime::new(broker.clone(), geometry.clone());
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

        let mut host_procs = HostProcessManager::new("https://127.0.0.1:52323");
        for (id, dm) in &discovered {
            if let Some(inst) = state_file.modules.get(id) {
                if inst.enabled && dm.manifest.entrypoints.host.is_some() {
                    let _ = host_procs.start_module(dm, inst);
                }
            }
        }

        let initial_sig = Self::compute_disk_sig(&registry);
        let state = Arc::new(Self {
            broker,
            registry,
            runtime: Mutex::new(runtime),
            host_procs: Mutex::new(host_procs),
            modules_rev: AtomicU64::new(1),
            last_disk_sig: Mutex::new(initial_sig),
            compositor: Mutex::new(None),
            geometry,
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

    pub fn notify_modules_changed(&self, reason: &str, module_id: Option<&str>) {
        let rev = self.modules_rev.fetch_add(1, Ordering::SeqCst) + 1;
        *self.last_disk_sig.lock().unwrap() = Self::compute_disk_sig(&self.registry);
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

    let items: Vec<Value> = discovered
        .into_iter()
        .map(|(id, dm)| {
            let inst = saved.modules.get(&id).cloned().unwrap_or_default();
            let rt_status = status_map.get(&id);
            json!({
                "id": id,
                "manifest": dm.manifest,
                "tier": dm.tier,
                "path": dm.path,
                "state": inst,
                "healthy": rt_status.map(|s| s.healthy).unwrap_or(true),
                "last_error": rt_status.and_then(|s| s.last_error.clone()),
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
    if !discovered.contains_key(&id) {
        return err_resp(StatusCode::NOT_FOUND, format!("module {id:?} not found"));
    }
    let mut saved = state.v2.registry.load_state();
    if let Some(slot_opt) = body.slot {
        if slot_opt.is_some_and(|s| s > 3) {
            return err_resp(StatusCode::BAD_REQUEST, "slot must be 0..=3");
        }
        if let Some(target_slot) = slot_opt {
            for (other_id, other) in saved.modules.iter_mut() {
                if other_id != &id && other.slot == Some(target_slot) {
                    other.slot = None;
                }
            }
        }
        saved.modules.entry(id.clone()).or_default().slot = slot_opt;
    }
    let entry = saved.modules.entry(id.clone()).or_default();
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
    path: String,
    #[serde(default)]
    accept_disclosure: bool,
}

async fn inspect_module_api(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<InspectInstallBody>,
) -> impl IntoResponse {
    match state.v2.registry.preview_install(std::path::Path::new(&body.path)) {
        Ok(preview) => (StatusCode::OK, Json(json!({ "preview": preview }))),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
}

async fn install_module_api(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<InspectInstallBody>,
) -> impl IntoResponse {
    let p = std::path::Path::new(&body.path);
    let preview = match state.v2.registry.preview_install(p) {
        Ok(pr) => pr,
        Err(e) => return err_resp(StatusCode::BAD_REQUEST, e),
    };
    if !body.accept_disclosure {
        return (
            StatusCode::PRECONDITION_REQUIRED,
            Json(json!({
                "installed": false,
                "requires_acceptance": true,
                "preview": preview,
            })),
        );
    }
    match state.v2.registry.install_from_dir(p) {
        Ok(dm) => {
            state.v2.sync_all();
            (
                StatusCode::OK,
                Json(json!({ "installed": true, "module": dm, "preview": preview })),
            )
        }
        Err(e) => err_resp(StatusCode::BAD_REQUEST, e),
    }
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
    let text = match std::fs::read_to_string(&q.source) {
        Ok(t) => t,
        Err(e) => return err_resp(StatusCode::BAD_REQUEST, format!("read list: {e}")),
    };
    match serde_json::from_str::<CuratedListDocument>(&text) {
        Ok(doc) => (StatusCode::OK, Json(json!({ "list": doc }))),
        Err(e) => err_resp(StatusCode::BAD_REQUEST, format!("invalid list JSON: {e}")),
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
    match state.v2.registry.import_preset(&preset) {
        Ok(saved) => {
            state.v2.sync_all();
            (
                StatusCode::OK,
                Json(json!({ "imported": preset.name, "modules": saved.modules.len() })),
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
        .broker
        .publish_typed(topics::INPUT_KEYS_EVENT, &ev, PublishOptions::default());
    let sample = AnalogKeySample::from_raw(
        body.key_index,
        body.depth_um,
        body.velocity_um_s,
        k.center_x,
        k.center_y,
    );
    let _ = state.v2.broker.publish_typed(
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
    let _ = state.v2.broker.publish_typed(
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
    let device = state.device.clone();

    let res = tokio::task::spawn_blocking(move || {
        let (has_bridge, _) = device.is_connected();
        if !has_bridge {
            return Err("Keyboard USB bridge is not connected".to_string());
        }
        let mut uploaded_files = Vec::new();
        let mut uploaded_bytes = 0usize;

        // Upload non-bundled (local/installed) modules to `/home/root/openflux/v2/modules/<id>/`
        for (id, dm) in &discovered {
            if dm.tier != openflux_modules::ModuleTier::Bundled {
                let mut stack = vec![dm.path.clone()];
                while let Some(dir) = stack.pop() {
                    let Ok(rd) = std::fs::read_dir(&dir) else { continue };
                    for entry in rd.flatten() {
                        let p = entry.path();
                        let name = entry.file_name().to_string_lossy().into_owned();
                        if name == ".git" || name == ".DS_Store" {
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
        }

        // If `bg-media` points to a local video/image file, upload it to `/home/root/openflux/media/`
        // if absent on the keyboard and rewrite the keyboard state path to the keyboard filename.
        if let Some(bg_mod) = kb_state.modules.get_mut("bg-media") {
            if let Some(media_str) = bg_mod
                .config
                .get("media_path")
                .and_then(|v| v.as_str())
                .map(str::to_string)
            {
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
                        bg_mod
                            .config
                            .insert("media_path".into(), json!(fname));
                    }
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

/// Mounts all OpenFlux v2 routes (open localhost bus, module management, disclosure, lists, presets, GPU preview).
pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/v2/status", get(v2_status_api))
        .route("/v2/logs", get(get_all_logs_api))
        .route("/v2/bus/pub", post(bus_pub))
        .route("/v2/bus/retained", get(bus_retained))
        .route("/v2/bus/events", get(bus_events_poll))
        .route("/v2/bus/ws", get(bus_ws))
        .route("/v2/modules", get(list_modules))
        .route("/v2/modules/create", post(create_module))
        .route("/v2/modules/fork", post(fork_module))
        .route("/v2/modules/inspect", post(inspect_module_api))
        .route("/v2/modules/install", post(install_module_api))
        .route("/v2/modules/:id", get(get_module_detail))
        .route("/v2/modules/:id/state", put(update_module_state))
        .route("/v2/modules/:id/action", post(trigger_module_action))
        .route("/v2/modules/:id/reload", post(reload_module_api))
        .route("/v2/modules/:id/logs", get(get_module_logs_api))
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
        .route("/v2/keyboard/upload-media", post(keyboard_upload_media))
        .route("/v2/keyboard/sideload", post(keyboard_sideload))
        .route("/v2/keyboard/flash-uf2", post(keyboard_flash_uf2))
        .with_state(state)
}
