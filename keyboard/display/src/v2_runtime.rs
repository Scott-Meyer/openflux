//! OpenFlux v2 keyboard-side display engine: bridges `openflux-bridge` IPC lines (`key_state`,
//! `key_analog`, `live_data`, `bus_frame`) onto the keyboard's local `openflux-bus` broker,
//! runs `openflux-modules::ModuleRuntime` (100% standalone capable!), steps `WaveField2D`,
//! `ParticleSystem` (Key-SDF wall bounces + clear-keycap lens magnification + collisions),
//! and `Canvas2D` (cached key legends + top-bar widgets + Idle Pet), ready for native Mali-G52
//! OpenGL ES 2.0 presentation with zero-copy `rkmpp` 1080p NV12 video refraction.

use openflux_bus::{
    apply_remote_frame, topics, AnalogKeyFrame, AnalogKeySample, Broker, FluxActionEvent,
    FrameLayout, HostConnectionState, KeyEvent, KeyEventKind, KeyboardGeometry, KeyboardState,
    MediaNowPlaying, PublishOptions, ResolvedKeyEntry, ResolvedKeysState, SystemMetrics,
    WeatherState, WireFrame, DISPLAY_HEIGHT_PX, DISPLAY_WIDTH_PX,
};
use openflux_compositor::{
    Canvas2D, KeySdfGrid, ParticleSystem, WaveField2D, DEFAULT_ANSI_LABELS, WAVE2D_H, WAVE2D_W,
};
use openflux_modules::{
    KeyGlowShape, ModuleRegistry, ModuleRuntime, WaveFieldParams,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

struct OverlayRasterReq {
    under_key_draws: Vec<openflux_modules::DrawCommand>,
    tile_draws: Vec<openflux_modules::DrawCommand>,
    world_overlay_draws: Vec<openflux_modules::DrawCommand>,
    has_custom_keys_renderer: bool,
    has_custom_dock: bool,
    geometry: KeyboardGeometry,
}

/// Coalescing off-thread 2D overlay rasterizer with zero-alloc ping-pong buffer swap.
/// Keeps the 60 FPS Mali-G52 video + water-refraction render thread completely non-blocking even
/// when spamming Shift or rapidly switching layers/shortcuts!
struct OverlayRasterWorker {
    req_slot: Arc<(Mutex<Option<OverlayRasterReq>>, Condvar)>,
    ready_slot: Arc<Mutex<Option<Vec<u8>>>>,
    spare_slot: Arc<Mutex<Option<Vec<u8>>>>,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl OverlayRasterWorker {
    fn spawn(font_bytes: Option<Vec<u8>>) -> Self {
        let req_slot: Arc<(Mutex<Option<OverlayRasterReq>>, Condvar)> =
            Arc::new((Mutex::new(None), Condvar::new()));
        let px_bytes = (DISPLAY_WIDTH_PX * DISPLAY_HEIGHT_PX * 4) as usize;
        let ready_slot: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
        let spare_slot: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(Some(vec![0u8; px_bytes])));
        let stop = Arc::new(AtomicBool::new(false));

        let req_bg = req_slot.clone();
        let ready_bg = ready_slot.clone();
        let spare_bg = spare_slot.clone();
        let stop_bg = stop.clone();

        let join = std::thread::Builder::new()
            .name("v2-overlay-raster".into())
            .spawn(move || {
                let mut canvas = Canvas2D::new(
                    DISPLAY_WIDTH_PX,
                    DISPLAY_HEIGHT_PX,
                    font_bytes.as_deref(),
                );
                loop {
                    let req = {
                        let (mtx, cv) = &*req_bg;
                        let mut g = mtx.lock().unwrap();
                        loop {
                            if stop_bg.load(Ordering::SeqCst) {
                                return;
                            }
                            if let Some(r) = g.take() {
                                break r;
                            }
                            g = cv.wait(g).unwrap();
                        }
                    };
                    if let Some(spare) = spare_bg.lock().unwrap().take() {
                        if spare.len() == canvas.rgba.len() {
                            canvas.rgba = spare;
                        }
                    }
                    canvas.clear();
                    if !req.under_key_draws.is_empty() {
                        canvas.execute(&req.under_key_draws);
                    }
                    if req.has_custom_keys_renderer {
                        if !req.has_custom_dock {
                            canvas.draw_top_bar_dividers(&req.geometry);
                        }
                    } else {
                        canvas.blit_cached_default_key_legends(&req.geometry, DEFAULT_ANSI_LABELS);
                    }
                    if !req.tile_draws.is_empty() {
                        canvas.execute(&req.tile_draws);
                    }
                    if !req.world_overlay_draws.is_empty() {
                        canvas.execute(&req.world_overlay_draws);
                    }
                    let mut out_buf = spare_bg
                        .lock()
                        .unwrap()
                        .take()
                        .filter(|v| v.len() == canvas.rgba.len())
                        .unwrap_or_else(|| vec![0u8; canvas.rgba.len()]);
                    std::mem::swap(&mut canvas.rgba, &mut out_buf);
                    let prev_ready = ready_bg.lock().unwrap().replace(out_buf);
                    if let Some(recycled) = prev_ready {
                        *spare_bg.lock().unwrap() = Some(recycled);
                    }
                }
            })
            .ok();

        Self {
            req_slot,
            ready_slot,
            spare_slot,
            stop,
            join,
        }
    }

    fn submit(&self, req: OverlayRasterReq) {
        let (mtx, cv) = &*self.req_slot;
        *mtx.lock().unwrap() = Some(req);
        cv.notify_one();
    }

    fn take_ready(&self, front_buf: &mut Vec<u8>) -> bool {
        if let Some(mut finished) = self.ready_slot.lock().unwrap().take() {
            std::mem::swap(front_buf, &mut finished);
            *self.spare_slot.lock().unwrap() = Some(finished);
            return true;
        }
        false
    }
}

impl Drop for OverlayRasterWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.req_slot.1.notify_all();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Packed GPU draw primitive for Mali-G52 GLES2 under-key glows & SDF-refracting particles.
#[derive(Clone, Copy, Debug)]
pub struct V2GlowDraw {
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    /// `0=Glow`, `1=Ring`, `2=Ripple`, `3=Solid`, `4=Particle`
    pub shape: f32,
    pub radius_px: f32,
    pub thickness_px: f32,
    pub intensity: f32,
    pub color: [f32; 4],
}

pub struct KeyboardV2Display {
    pub broker: Broker,
    pub geometry: KeyboardGeometry,
    pub registry: ModuleRegistry,
    pub runtime: ModuleRuntime,
    pub sdf_grid: KeySdfGrid,
    pub particles: ParticleSystem,
    pub wave: WaveField2D,
    pub overlay_rgba: Vec<u8>,
    overlay_worker: OverlayRasterWorker,
    pub wave_enabled: bool,
    pub wave_params: WaveFieldParams,
    pub active_media_path: Option<String>,
    pub active_stream_name: Option<String>,
    pub stream_overlays: Vec<openflux_modules::StreamOverlayCmd>,
    pub bg_dimming: f32,
    pub top_fade: f32,
    parallax_rx: openflux_bus::Subscription,
    power_rx: openflux_bus::Subscription,
    last_parallax_sig: String,
    pub brightness_pct: u32,
    pub sleep_timeout_sec: u64,
    pub pending_brightness_pct: Option<u32>,
    pub pending_brightness_steps: Vec<i8>,
    prefs_path: PathBuf,
    pub dt_sec: f32,
    pub background_shaders: Vec<openflux_modules::ShaderPassCmd>,
    pub distortion_shaders: Vec<openflux_modules::ShaderPassCmd>,
    pub under_key_shaders: Vec<openflux_modules::ShaderPassCmd>,
    pub glows: Vec<V2GlowDraw>,
    pub time_sec: f32,
    pub overlay_dirty: bool,
    last_keys_hash: u64,
    cached_under_key_draws: Vec<openflux_modules::DrawCommand>,
    last_tiles_hash: u64,
    last_world_hash: u64,
    last_tiles_ms: u64,
    last_bitmap: [u8; 32],
    last_key_seq: Option<u32>,
    last_key_press_ms: [u64; 256],
    last_input_ms: u64,
    last_state_mtime: Option<SystemTime>,
    pub mirror: openflux_bus::BrokerMirror,
}

fn hash_f32<H: std::hash::Hasher>(h: &mut H, v: f32) {
    use std::hash::Hash;
    v.to_bits().hash(h);
}

fn hash_color<H: std::hash::Hasher>(h: &mut H, c: &[f32; 4]) {
    for &x in c {
        hash_f32(h, x);
    }
}

/// Fast zero-allocation structural hash for DrawCommand slices (replaces slow JSON serialization).
fn hash_draw_list(cmds: &[openflux_modules::DrawCommand]) -> u64 {
    use openflux_modules::DrawCommand;
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    cmds.len().hash(&mut h);
    for cmd in cmds {
        match cmd {
            DrawCommand::Clip { x, y, w, h: ch } => {
                0u8.hash(&mut h);
                hash_f32(&mut h, *x);
                hash_f32(&mut h, *y);
                hash_f32(&mut h, *w);
                hash_f32(&mut h, *ch);
            }
            DrawCommand::ClearClip => 1u8.hash(&mut h),
            DrawCommand::Rect { x, y, w, h: ch, color } => {
                2u8.hash(&mut h);
                hash_f32(&mut h, *x);
                hash_f32(&mut h, *y);
                hash_f32(&mut h, *w);
                hash_f32(&mut h, *ch);
                hash_color(&mut h, color);
            }
            DrawCommand::RoundRect { x, y, w, h: ch, radius, color, stroke_width, stroke_color } => {
                3u8.hash(&mut h);
                hash_f32(&mut h, *x);
                hash_f32(&mut h, *y);
                hash_f32(&mut h, *w);
                hash_f32(&mut h, *ch);
                hash_f32(&mut h, *radius);
                hash_color(&mut h, color);
                hash_f32(&mut h, *stroke_width);
                hash_color(&mut h, stroke_color);
            }
            DrawCommand::Circle { cx, cy, r, color, stroke_width, stroke_color } => {
                4u8.hash(&mut h);
                hash_f32(&mut h, *cx);
                hash_f32(&mut h, *cy);
                hash_f32(&mut h, *r);
                hash_color(&mut h, color);
                hash_f32(&mut h, *stroke_width);
                hash_color(&mut h, stroke_color);
            }
            DrawCommand::Ellipse { cx, cy, rx, ry, color } => {
                5u8.hash(&mut h);
                hash_f32(&mut h, *cx);
                hash_f32(&mut h, *cy);
                hash_f32(&mut h, *rx);
                hash_f32(&mut h, *ry);
                hash_color(&mut h, color);
            }
            DrawCommand::Line { x1, y1, x2, y2, width, color } => {
                6u8.hash(&mut h);
                hash_f32(&mut h, *x1);
                hash_f32(&mut h, *y1);
                hash_f32(&mut h, *x2);
                hash_f32(&mut h, *y2);
                hash_f32(&mut h, *width);
                hash_color(&mut h, color);
            }
            DrawCommand::Arc { cx, cy, r, start_deg, end_deg, width, color } => {
                7u8.hash(&mut h);
                hash_f32(&mut h, *cx);
                hash_f32(&mut h, *cy);
                hash_f32(&mut h, *r);
                hash_f32(&mut h, *start_deg);
                hash_f32(&mut h, *end_deg);
                hash_f32(&mut h, *width);
                hash_color(&mut h, color);
            }
            DrawCommand::Text { x, y, text, size, color, align } => {
                8u8.hash(&mut h);
                hash_f32(&mut h, *x);
                hash_f32(&mut h, *y);
                text.hash(&mut h);
                hash_f32(&mut h, *size);
                hash_color(&mut h, color);
                (*align as u8).hash(&mut h);
            }
            DrawCommand::Image { x, y, w, h: ch, path, tint } => {
                9u8.hash(&mut h);
                hash_f32(&mut h, *x);
                hash_f32(&mut h, *y);
                hash_f32(&mut h, *w);
                hash_f32(&mut h, *ch);
                path.hash(&mut h);
                hash_color(&mut h, tint);
            }
        }
    }
    h.finish()
}

fn file_mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl KeyboardV2Display {
    pub fn new(
        bundled_dir: PathBuf,
        installed_dir: PathBuf,
        local_dir: PathBuf,
        state_path: PathBuf,
        layout: FrameLayout,
        font_bytes: Option<&[u8]>,
    ) -> Self {
        let broker = Broker::new("keyboard");
        let geometry = KeyboardGeometry::build(layout);
        let _ = broker.publish_typed(
            topics::KEYBOARD_GEOMETRY,
            &geometry,
            PublishOptions::retained(),
        );
        let last_state_mtime = file_mtime(&state_path);
        let prefs_path = state_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("/home/root/openflux/v2"))
            .join("display_prefs.json");
        let registry = ModuleRegistry::new(bundled_dir, installed_dir, local_dir, state_path);
        let mut runtime = ModuleRuntime::new(broker.clone(), geometry.clone());
        let discovered = registry.discover();
        let saved = registry.load_state();
        runtime.sync_modules(&discovered, &saved.modules);

        let sdf_grid = KeySdfGrid::build(&geometry);
        let overlay_rgba = vec![0u8; (DISPLAY_WIDTH_PX * DISPLAY_HEIGHT_PX * 4) as usize];
        let overlay_worker = OverlayRasterWorker::spawn(font_bytes.map(|b| b.to_vec()));
        let parallax_rx = broker
            .subscribe("display/parallax", openflux_bus::SubscribeOptions::default())
            .expect("valid parallax topic");
        let power_rx = broker
            .subscribe("display/power", openflux_bus::SubscribeOptions::default())
            .expect("valid power topic");
        let mirror = openflux_bus::BrokerMirror::new(broker.clone(), "host");

        let mut this = Self {
            broker,
            geometry,
            registry,
            runtime,
            sdf_grid,
            particles: ParticleSystem::default(),
            wave: WaveField2D::new(),
            overlay_rgba,
            overlay_worker,
            wave_enabled: false,
            wave_params: WaveFieldParams::default(),
            active_media_path: None,
            active_stream_name: None,
            stream_overlays: Vec::new(),
            bg_dimming: 0.0,
            top_fade: 0.0,
            parallax_rx,
            power_rx,
            last_parallax_sig: String::new(),
            brightness_pct: 80,
            sleep_timeout_sec: 300,
            pending_brightness_pct: None,
            pending_brightness_steps: Vec::new(),
            prefs_path,
            dt_sec: 0.016,
            background_shaders: Vec::new(),
            distortion_shaders: Vec::new(),
            under_key_shaders: Vec::new(),
            glows: Vec::with_capacity(256),
            time_sec: 0.0,
            overlay_dirty: true,
            last_keys_hash: 0,
            cached_under_key_draws: Vec::new(),
            last_tiles_hash: 0,
            last_world_hash: 0,
            last_tiles_ms: 0,
            last_bitmap: [0u8; 32],
            last_key_seq: None,
            last_key_press_ms: [0u64; 256],
            last_input_ms: openflux_bus::now_unix_ms(),
            last_state_mtime,
            mirror,
        };
        this.load_persisted_prefs();
        this
    }

    fn load_persisted_prefs(&mut self) {
        if let Ok(raw) = std::fs::read_to_string(&self.prefs_path) {
            if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(p) = doc.get("parallax") {
                    let s = crate::layout::ParallaxConfig::from_json(p);
                    self.apply_parallax_settings(&s);
                    self.last_parallax_sig = p.to_string();
                }
                if let Some(pw) = doc.get("power") {
                    if let Some(b) = pw.get("brightness").and_then(|v| v.as_u64()) {
                        self.brightness_pct = (b as u32).clamp(5, 100);
                        self.pending_brightness_pct = Some(self.brightness_pct);
                    }
                    if let Some(t) = pw.get("timeoutSec").and_then(|v| v.as_u64()) {
                        self.sleep_timeout_sec = t;
                    }
                }
            }
        }
    }

    fn persist_prefs_field(&self, key: &str, val: &serde_json::Value) {
        let mut doc = std::fs::read_to_string(&self.prefs_path)
            .ok()
            .and_then(|r| serde_json::from_str::<serde_json::Value>(&r).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(obj) = doc.as_object_mut() {
            obj.insert(key.to_string(), val.clone());
        }
        if let Ok(txt) = serde_json::to_string_pretty(&doc) {
            let _ = std::fs::write(&self.prefs_path, txt);
        }
    }

    /// Builds a `PeerBusFrame::Subscribe` advertising all patterns active keyboard modules care about.
    pub fn build_mirror_subscribe(&mut self, force: bool) -> Option<openflux_bus::PeerBusFrame> {
        let extra = vec![
            "host/#".to_string(),
            "modules/#".to_string(),
            "system/actions".to_string(),
            "display/parallax".to_string(),
            "display/power".to_string(),
            "fx/surface/impulse".to_string(),
            "keys/providers/ability-deck".to_string(),
            "keys/providers/streamdeck".to_string(),
        ];
        self.mirror.build_subscribe_if_changed(&extra, force)
    }

    pub fn on_mirror_frame(
        &mut self,
        frame: openflux_bus::PeerBusFrame,
    ) -> Vec<openflux_bus::PeerBusFrame> {
        self.mirror.on_remote_frame(frame)
    }

    pub fn drain_mirror_outgoing(&self) -> Vec<openflux_bus::PeerBusFrame> {
        self.mirror.drain_outgoing()
    }

    pub fn on_host_disconnect(&mut self) {
        self.mirror.on_disconnect();
    }

    pub fn publish_stream_state(&self, name: &str, active: bool, codec: &str) {
        let _ = self.broker.publish(
            format!("modules/streams/{name}"),
            serde_json::json!({ "active": active, "codec": codec, "name": name }),
            PublishOptions::retained().with_origin("display-media"),
        );
    }

    /// Checks whether `modules_state.json` or any Lua module file changed on disk (e.g. after USB sync).
    pub fn poll_reload(&mut self) -> bool {
        let cur_mtime = file_mtime(&self.registry.state_path);
        if cur_mtime != self.last_state_mtime {
            self.last_state_mtime = cur_mtime;
            let discovered = self.registry.discover();
            let saved = self.registry.load_state();
            self.runtime.sync_modules(&discovered, &saved.modules);
            return true;
        }
        !self.runtime.check_hot_reload().is_empty()
    }

    pub fn wave_dims(&self) -> (usize, usize) {
        (WAVE2D_W, WAVE2D_H)
    }

    /// Consumes one newline-delimited JSON IPC message from `openflux-bridge` and publishes
    /// semantic events onto the keyboard's local `openflux-bus`.
    pub fn ingest_bridge_ipc_line(&mut self, line: &str) {
        let Ok(val) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let now_ms = openflux_bus::now_unix_ms();
        match val.get("type").and_then(|v| v.as_str()) {
            Some("key_state") => self.handle_key_state(&val, now_ms),
            Some("key_analog") => self.handle_key_analog(&val, now_ms),
            Some("flux_action") => self.handle_flux_action(&val),
            Some("resolved_keys") => self.handle_resolved_keys(&val),
            Some("bus_frame") => {
                if let Some(frame_val) = val.get("frame") {
                    if let Ok(frame) = serde_json::from_value::<WireFrame>(frame_val.clone()) {
                        let _ = apply_remote_frame(&self.broker, frame, "bridge:host");
                    }
                }
            }
            Some("live_data") => self.handle_v1_live_data(&val, now_ms),
            _ => {}
        }
    }

    fn handle_key_state(&mut self, val: &Value, now_ms: u64) {
        // Ignore stale or duplicate UART key_state packets (prevents phantom double-triggers!)
        if let Some(seq) = val.get("seq").and_then(|v| v.as_u64()).map(|v| v as u32) {
            if let Some(prev_seq) = self.last_key_seq {
                if seq == prev_seq {
                    return;
                }
            }
            self.last_key_seq = Some(seq);
        }
        if let Some(hex) = val.get("key_bitmap").and_then(|v| v.as_str()) {
            // Strict 64-hex-char validation so a truncated/malformed line NEVER zeroes key_bitmap!
            if hex.len() != 64 {
                return;
            }
            let mut next = [0u8; 32];
            for i in 0..32 {
                let Some(pair) = hex.get(i * 2..i * 2 + 2) else {
                    return;
                };
                let Ok(b) = u8::from_str_radix(pair, 16) else {
                    return;
                };
                next[i] = b;
            }
            for k in 0..self.geometry.keys.len().min(256) {
                let was = (self.last_bitmap[k >> 3] & (1 << (k & 7))) != 0;
                let now = (next[k >> 3] & (1 << (k & 7))) != 0;
                if was != now {
                    // 35ms per-key re-press refractory guard prevents micro-bounce double/triple rings
                    // when a Hall-effect key hovers near the actuation edge.
                    if now && now_ms.saturating_sub(self.last_key_press_ms[k]) < 35 {
                        continue;
                    }
                    if now {
                        self.last_key_press_ms[k] = now_ms;
                    }
                    self.last_input_ms = now_ms;
                    if let Some(key) = self.geometry.key(k as u16) {
                        let ev = KeyEvent {
                            key_index: k as u16,
                            kind: if now {
                                KeyEventKind::Press
                            } else {
                                KeyEventKind::Release
                            },
                            timestamp_ms: now_ms,
                            x_px: key.center_x,
                            y_px: key.center_y,
                            w_px: key.rect.w,
                            h_px: key.rect.h,
                            uv_x: key.uv_x,
                            uv_y: key.uv_y,
                            row: key.row,
                            col: key.col,
                            depth_um: if now { 2200 } else { 0 },
                            velocity_um_s: if now { 14000 } else { -12000 },
                            color: None,
                        };
                        let _ = self.broker.publish_typed(
                            topics::INPUT_KEYS_EVENT,
                            &ev,
                            PublishOptions::default(),
                        );
                    }
                }
            }
            self.last_bitmap = next;
        }

        let ks = KeyboardState {
            active_layer_bitfield: val
                .get("active_layer_bitfield")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32,
            active_profile: val
                .get("active_profile")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            active_actuation_mode: val
                .get("active_actuation_mode")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            num_lock: val
                .get("num_lock")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            caps_lock: val
                .get("caps_lock")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            scroll_lock: val
                .get("scroll_lock")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            keyframe_attached: val
                .get("keyframe_attached")
                .and_then(|v| v.as_bool())
                .unwrap_or(true),
            idle_ms: now_ms.saturating_sub(self.last_input_ms),
            qmk_state_valid: val
                .get("qmk_state_valid")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            effective_mods: val
                .get("effective_mods")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u8,
            oneshot_mods: val
                .get("oneshot_mods")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u8,
            oneshot_layer: val
                .get("oneshot_layer")
                .and_then(|v| v.as_u64())
                .unwrap_or(0xFF) as u8,
            caps_word_active: val
                .get("caps_word_active")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            default_layer_bitfield: val
                .get("default_layer_bitfield")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32,
            keymap_revision: val
                .get("keymap_revision")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
        };
        let _ = self.broker.publish_typed(
            topics::KEYBOARD_STATE,
            &ks,
            PublishOptions::retained(),
        );
    }

    fn resolve_action_id(&self, slot: u16) -> Option<String> {
        if let Some(dir) = self.registry.state_path.parent() {
            let map_path = dir.join("module_actions.json");
            if let Ok(raw) = std::fs::read_to_string(&map_path) {
                if let Ok(json) = serde_json::from_str::<Value>(&raw) {
                    if let Some(id) = json
                        .get("slots")
                        .and_then(|s| s.get(&slot.to_string()))
                        .and_then(|v| v.as_str())
                    {
                        return Some(id.to_string());
                    }
                }
            }
        }
        const BUILTINS: [&str; 14] = [
            "openflux.brightness_up",
            "openflux.brightness_down",
            "openflux.profile_next",
            "openflux.profile_prev",
            "openflux.actuation_next",
            "openflux.actuation_prev",
            "openflux.platform_linux",
            "openflux.platform_macos",
            "openflux.platform_windows",
            "openflux.auto_profile_lock",
            "widget-pomodoro.toggle",
            "widget-pomodoro.reset",
            "openflux.theme_next",
            "openflux.theme_prev",
        ];
        BUILTINS.get(slot as usize).map(|s| (*s).to_string())
    }

    fn handle_flux_action(&mut self, val: &Value) {
        let slot = val.get("slot").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
        let pressed = val.get("pressed").and_then(|v| v.as_bool()).unwrap_or(false);
        let mods = val.get("mods").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
        let layer = val.get("layer").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
        let profile = val.get("profile").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
        let action_id = self.resolve_action_id(slot);
        let ev = FluxActionEvent {
            slot,
            action_id: action_id.clone(),
            pressed,
            mods,
            layer,
            profile,
        };
        let _ = self
            .broker
            .publish_typed(topics::INPUT_FLUX_ACTION, &ev, PublishOptions::default());

        // Dispatch standalone on the keyboard bus using retained `system/actions` metadata
        // (supports both `mode == "tap"` and `mode == "hold"` with zero host round-trip!)
        let meta = self
            .broker
            .get_retained("system/actions")
            .and_then(|m| m.payload.get("slots").and_then(|s| s.get(&slot.to_string())).cloned());
        if let Some(m) = meta {
            if let Some(aid) = m.get("action_id").and_then(|v| v.as_str()) {
                let sem_topic = format!("actions/{}", aid.replace('.', "/").replace(':', "/"));
                let _ = self.broker.publish(
                    &sem_topic,
                    serde_json::json!({ "pressed": pressed, "slot": slot, "mods": mods, "layer": layer }),
                    PublishOptions::default(),
                );
            }
            if let Some(topic) = m.get("topic").and_then(|v| v.as_str()) {
                let retain = m.get("retain").and_then(|v| v.as_bool()).unwrap_or(false);
                let mode = m.get("mode").and_then(|v| v.as_str()).unwrap_or("tap");
                let opts = if retain {
                    PublishOptions::retained()
                } else {
                    PublishOptions::default()
                };
                if pressed {
                    let payload = m.get("payload").cloned().unwrap_or_else(|| serde_json::json!({}));
                    let _ = self.broker.publish(topic, payload, opts);
                } else if mode == "hold" {
                    let rel = m
                        .get("release_payload")
                        .filter(|v| !v.is_null())
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({ "active": false }));
                    let _ = self.broker.publish(topic, rel, opts);
                }
            }
        }
    }

    fn handle_resolved_keys(&mut self, val: &Value) {
        let Some(arr) = val.get("keys").and_then(|v| v.as_array()) else {
            return;
        };
        let mut keys = Vec::with_capacity(arr.len());
        for item in arr {
            if let Some(pair) = item.as_array() {
                if pair.len() >= 2 {
                    keys.push(ResolvedKeyEntry {
                        keycode: pair[0].as_u64().unwrap_or(0) as u16,
                        source_layer: pair[1].as_u64().unwrap_or(0) as u8,
                    });
                }
            }
        }
        let state = ResolvedKeysState {
            keymap_revision: val
                .get("keymap_revision")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            keys,
        };
        let _ = self.broker.publish_typed(
            topics::KEYBOARD_RESOLVED_KEYS,
            &state,
            PublishOptions::retained(),
        );
    }

    fn handle_key_analog(&mut self, val: &Value, now_ms: u64) {
        let Some(arr) = val.get("samples").and_then(|v| v.as_array()) else {
            return;
        };
        let mut samples = Vec::with_capacity(arr.len());
        for item in arr {
            if let Some(triple) = item.as_array() {
                if triple.len() >= 3 {
                    let key_index = triple[0].as_u64().unwrap_or(0) as u16;
                    let depth_um = triple[1].as_u64().unwrap_or(0) as u16;
                    let vel = triple[2].as_i64().unwrap_or(0) as i32;
                    if let Some(key) = self.geometry.key(key_index) {
                        if depth_um > 0 {
                            self.last_input_ms = now_ms;
                        }
                        samples.push(AnalogKeySample::from_raw(
                            key_index,
                            depth_um,
                            vel,
                            key.center_x,
                            key.center_y,
                        ));
                    }
                }
            }
        }
        if !samples.is_empty() {
            let _ = self.broker.publish_typed(
                topics::INPUT_KEYS_ANALOG,
                &AnalogKeyFrame {
                    timestamp_ms: now_ms,
                    samples,
                },
                PublishOptions::default(),
            );
        }
    }

    fn handle_v1_live_data(&mut self, val: &Value, now_ms: u64) {
        let Some(p) = val.get("payload") else {
            return;
        };
        let _ = self.broker.publish_typed(
            topics::HOST_CONNECTION,
            &HostConnectionState {
                connected: true,
                host_asleep: p
                    .get("hostDisplayAsleep")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                last_seen_ms: now_ms,
                host_platform: None,
            },
            PublishOptions::retained().with_ttl_ms(15_000),
        );
        if let Some(hw) = p.get("hardwareData") {
            let _ = self.broker.publish_typed(
                topics::HOST_SYSTEM_METRICS,
                &SystemMetrics {
                    cpu_percent: hw.get("cpu").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                    memory_used_gb: hw.get("memory").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                    memory_total_gb: hw
                        .get("totalMemory")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(16.0) as f32,
                    gpu_percent: hw.get("gpuUsage").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                    gpu_unified: hw
                        .get("gpuUnified")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false),
                    vram_used_gb: hw.get("vramUsed").and_then(|v| v.as_f64()).map(|v| v as f32),
                    vram_total_gb: hw
                        .get("totalVram")
                        .and_then(|v| v.as_f64())
                        .map(|v| v as f32),
                    net_down_kbps: 0.0,
                    net_up_kbps: 0.0,
                    net_down: hw
                        .get("netDown")
                        .and_then(|v| v.as_str())
                        .unwrap_or("0 KB/s")
                        .to_string(),
                    net_up: hw
                        .get("netUp")
                        .and_then(|v| v.as_str())
                        .unwrap_or("0 KB/s")
                        .to_string(),
                },
                PublishOptions::retained().with_ttl_ms(15_000),
            );
        }
        if let Some(m) = p.get("mediaFile") {
            let _ = self.broker.publish_typed(
                topics::HOST_MEDIA_NOW_PLAYING,
                &MediaNowPlaying {
                    title: m
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    artist: m
                        .get("artists")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    album: m
                        .get("album")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    playing: m.get("playing").and_then(|v| v.as_bool()).unwrap_or(false),
                    length_sec: m.get("length").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                    position_sec: m.get("played").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                    track_id: m
                        .get("trackId")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    art_base64: m
                        .get("base64Image")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                },
                PublishOptions::retained().with_ttl_ms(15_000),
            );
        }
        if let Some(w) = p.get("weather") {
            let hourly = w
                .get("hourly")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .take(3)
                        .map(|h| openflux_bus::WeatherHourly {
                            hour: h
                                .get("hour")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string(),
                            temp: h.get("temp").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
                            condition: h
                                .get("condition")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string(),
                            is_day: h.get("isDay").and_then(|x| x.as_bool()).unwrap_or(true),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let _ = self.broker.publish_typed(
                topics::HOST_WEATHER_CURRENT,
                &WeatherState {
                    city: w
                        .get("city")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    temp: w.get("temp").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                    high: w.get("high").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                    low: w.get("low").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                    condition: w
                        .get("condition")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    is_day: w.get("isDay").and_then(|v| v.as_bool()).unwrap_or(true),
                    celsius: w.get("celsius").and_then(|v| v.as_bool()).unwrap_or(false),
                    ready: w.get("ready").and_then(|v| v.as_bool()).unwrap_or(false),
                    aqi: w.get("aqi").and_then(|v| v.as_u64()).map(|v| v as u16),
                    sunrise: w
                        .get("sunrise")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    sunset: w
                        .get("sunset")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    utc_offset_sec: w.get("utcOffsetSec").and_then(|v| v.as_i64()),
                    hourly,
                },
                PublishOptions::retained().with_ttl_ms(600_000),
            );
        }
        if let Some(sc) = p.get("shortcutOverlays") {
            let _ = self.broker.publish(
                topics::HOST_APP_SHORTCUTS,
                serde_json::json!({ "overlays": sc }),
                PublishOptions::retained().with_origin("host"),
            );
        }
        if let Some(v2_ret) = p.get("v2Retained").and_then(|v| v.as_object()) {
            for (topic, payload) in v2_ret {
                let _ = self.broker.publish(
                    topic.clone(),
                    payload.clone(),
                    PublishOptions::retained().with_origin("host"),
                );
            }
        }
    }

    /// Steps Lua modules, 2D wave simulation, Key-SDF colliding/refracting particles, and 2D overlay.
    pub fn step_frame(&mut self, dt_sec: f32, now_ms: u64) {
        // Apply any live `display/parallax` updates pushed from Studio over OFV2 CH_BUS!
        let mut latest_parallax: Option<serde_json::Value> = None;
        while let Some(msg) = self.parallax_rx.try_recv() {
            latest_parallax = Some(msg.payload);
        }
        if let Some(val) = latest_parallax {
            let sig = val.to_string();
            if sig != self.last_parallax_sig {
                self.last_parallax_sig = sig;
                let s = crate::layout::ParallaxConfig::from_json(&val);
                self.apply_parallax_settings(&s);
                self.persist_prefs_field("parallax", &val);
                self.last_keys_hash = 0;
                self.overlay_dirty = true;
            }
        }
        // Apply any live `display/power` updates (brightness / sleep timeout) pushed over OFV2 CH_BUS!
        let mut latest_power: Option<serde_json::Value> = None;
        while let Some(msg) = self.power_rx.try_recv() {
            latest_power = Some(msg.payload);
        }
        if let Some(pw) = latest_power {
            if let Some(b) = pw.get("brightness").and_then(|v| v.as_u64()) {
                let pct = (b as u32).clamp(5, 100);
                if pct != self.brightness_pct {
                    self.brightness_pct = pct;
                    self.pending_brightness_pct = Some(pct);
                }
            }
            if let Some(t) = pw.get("timeoutSec").and_then(|v| v.as_u64()) {
                self.sleep_timeout_sec = t;
            }
            self.persist_prefs_field("power", &pw);
        }
        let dt = dt_sec.clamp(0.001, 0.06);
        self.time_sec += dt;
        let scene = self.runtime.step_frame(dt, now_ms);

        self.active_media_path = scene.background_media.clone();
        self.active_stream_name = scene.background_stream.clone();
        self.stream_overlays = scene.stream_overlays;
        self.bg_dimming = scene.bg_dimming;
        self.top_fade = scene.top_fade;
        self.dt_sec = dt;
        self.background_shaders = scene.background_shaders;
        self.distortion_shaders = scene.distortion_shaders;
        self.under_key_shaders = scene.under_key_shaders;
        self.wave_enabled = scene.distortion_enabled;
        self.wave_params = scene.wave_params.clone();

        if self.wave_enabled {
            for imp in &scene.wave_impulses {
                self.wave.apply_impulse(imp);
            }
            self.wave.step_dt(dt, &self.wave_params);
        }

        for sp in &scene.particle_spawns {
            self.particles.spawn(sp, &self.sdf_grid);
        }
        self.particles.step(dt, &self.sdf_grid, &self.geometry);

        // Pack glows + particles for Mali-G52 bounded-quad additive pass
        self.glows.clear();
        for g in &scene.key_glows {
            let shape = match g.shape {
                KeyGlowShape::Glow => 0.0,
                KeyGlowShape::Ring => 1.0,
                KeyGlowShape::Ripple => 2.0,
                KeyGlowShape::Solid => 3.0,
            };
            self.glows.push(V2GlowDraw {
                cx: g.cx,
                cy: g.cy,
                w: g.w,
                h: g.h,
                shape,
                radius_px: g.radius_px,
                thickness_px: g.thickness_px,
                intensity: g.intensity,
                color: g.color,
            });
        }
        for p in &self.particles.particles {
            let life_t = (p.age_ms / p.life_ms).clamp(0.0, 1.0);
            let fade = (1.0 - life_t).powf(1.2);
            let lens_boost = if p.inside_key { 1.35 } else { 1.0 };
            self.glows.push(V2GlowDraw {
                cx: p.x,
                cy: p.y,
                w: 2.0,
                h: 2.0,
                shape: 4.0,
                radius_px: p.radius_px * p.current_scale,
                thickness_px: 2.0,
                intensity: fade * lens_boost,
                color: p.color,
            });
        }

        // Fast zero-allocation hash split by stage:
        // When `keys-renderer` signals `keep_under_key_layer`, reuse cached key commands without
        // re-hashing or re-rasterizing 273+ key legends on every 60 FPS video frame!
        let keys_changed = if scene.keep_under_key_layer && self.last_keys_hash != 0 {
            false
        } else {
            let kh = hash_draw_list(&scene.under_key_draws);
            if kh != self.last_keys_hash {
                self.last_keys_hash = kh;
                self.cached_under_key_draws = scene.under_key_draws.clone();
                true
            } else {
                false
            }
        };
        let world_hash = hash_draw_list(&scene.world_overlay_draws);
        let tiles_hash = hash_draw_list(&scene.tile_draws);

        let world_changed = world_hash != self.last_world_hash;
        let tiles_changed = tiles_hash != self.last_tiles_hash;

        let due_for_overlay = self.last_keys_hash == 0
            || keys_changed
            || world_changed
            || (tiles_changed && now_ms.saturating_sub(self.last_tiles_ms) >= 140);

        if due_for_overlay {
            self.last_world_hash = world_hash;
            self.last_tiles_hash = tiles_hash;
            self.last_tiles_ms = now_ms;
            let active_under = if scene.keep_under_key_layer && !self.cached_under_key_draws.is_empty() {
                self.cached_under_key_draws.clone()
            } else {
                scene.under_key_draws
            };
            self.overlay_worker.submit(OverlayRasterReq {
                under_key_draws: active_under,
                tile_draws: scene.tile_draws,
                world_overlay_draws: scene.world_overlay_draws,
                has_custom_keys_renderer: scene.has_custom_keys_renderer,
                has_custom_dock: scene.has_custom_dock,
                geometry: self.geometry.clone(),
            });
        }
        // Swap in the latest off-thread rasterized RGBA buffer in O(1) whenever ready!
        self.overlay_dirty = self.overlay_worker.take_ready(&mut self.overlay_rgba);
    }

    /// Updates every key's optical parallax offset (`visual_x`, `visual_y`, `parallax_dx`, `parallax_dy`)
    /// from the keyboard's physical viewing settings and republishes `keyboard/geometry` onto the bus.
    pub fn apply_parallax_settings(&mut self, settings: &crate::layout::ParallaxConfig) {
        for k in &mut self.geometry.keys {
            let (dx, dy) = crate::layout::compute_key_parallax(
                settings,
                k.center_x as f64,
                k.center_y as f64,
            );
            k.parallax_dx = dx as f32;
            k.parallax_dy = dy as f32;
            k.visual_x = k.center_x + dx as f32;
            k.visual_y = k.center_y + dy as f32;
        }
        self.runtime.update_geometry(self.geometry.clone());
    }

    pub fn is_display_asleep(&self, now_ms: u64) -> bool {
        if self.sleep_timeout_sec == 0 {
            return false;
        }
        now_ms.saturating_sub(self.last_input_ms) >= self.sleep_timeout_sec.saturating_mul(1000)
    }
}
