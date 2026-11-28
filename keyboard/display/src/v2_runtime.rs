//! Keyglass v2 keyboard-side display engine: bridges `keyglass-bridge` IPC lines (`key_state`,
//! `key_analog`, `live_data`, `bus_frame`) onto the keyboard's local `keyglass-bus` broker,
//! runs `keyglass-modules::ModuleRuntime` (100% standalone capable!), steps `WaveField2D`,
//! `ParticleSystem` (Key-SDF wall bounces + clear-keycap lens magnification + collisions),
//! and `Canvas2D` (cached key legends + top-bar widgets + Idle Pet), ready for native Mali-G52
//! OpenGL ES 2.0 presentation with zero-copy `rkmpp` 1080p NV12 video refraction.

use keyglass_bus::{
    apply_remote_frame, topics, AnalogKeyFrame, AnalogKeySample, Broker, FluxActionEvent,
    FrameLayout, HostConnectionState, KeyEvent, KeyEventKind, KeyboardGeometry, KeyboardState,
    MediaNowPlaying, PublishOptions, ResolvedKeyEntry, ResolvedKeysState, SystemMetrics,
    WeatherState, WireFrame, DISPLAY_HEIGHT_PX, DISPLAY_WIDTH_PX,
};
use keyglass_compositor::{
    Canvas2D, KeySdfGrid, ParticleDot, ParticleSystem, WaveField2D, DEFAULT_ANSI_LABELS, WAVE2D_H,
    WAVE2D_W,
};
use keyglass_modules::{
    KeyGlowShape, ModuleRegistry, ModuleRuntime, WaveFieldParams,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

struct OverlayRasterReq {
    under_key_draws: Vec<keyglass_modules::DrawCommand>,
    tile_draws: Vec<keyglass_modules::DrawCommand>,
    tiles_hash: u64,
    world_overlay_draws: Vec<keyglass_modules::DrawCommand>,
    has_custom_keys_renderer: bool,
    has_custom_dock: bool,
    geometry: KeyboardGeometry,
}

struct OverlayRasterResult {
    rgba: Vec<u8>,
    /// Dirty vertical strip `(y_start, height)` in pixels for fast partial `glTexSubImage2D`!
    dirty_y: i32,
    dirty_h: i32,
}

/// Coalescing off-thread 2D overlay rasterizer with zero-alloc ping-pong buffer swap,
/// top-bar band caching, dirty-row-band tracking, and bounded same-frame wait for instant
/// Fn / Shift / layer switches!
struct OverlayRasterWorker {
    req_slot: Arc<(Mutex<Option<(u64, OverlayRasterReq)>>, Condvar)>,
    ready_slot: Arc<(Mutex<Option<(u64, OverlayRasterResult)>>, Condvar)>,
    spare_slot: Arc<Mutex<Option<Vec<u8>>>>,
    next_seq: u64,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

fn draw_cmds_y_bounds(cmds: &[keyglass_modules::DrawCommand]) -> Option<(usize, usize)> {
    use keyglass_modules::DrawCommand;
    let mut y_min = usize::MAX;
    let mut y_max = 0usize;
    let mut include = |y0: f32, y1: f32| {
        let a = (y0.floor() as isize - 6).clamp(0, DISPLAY_HEIGHT_PX as isize) as usize;
        let b = (y1.ceil() as isize + 6).clamp(0, DISPLAY_HEIGHT_PX as isize) as usize;
        if a < y_min {
            y_min = a;
        }
        if b > y_max {
            y_max = b;
        }
    };
    for c in cmds {
        match c {
            DrawCommand::Rect { y, h, .. }
            | DrawCommand::RoundRect { y, h, .. }
            | DrawCommand::Image { y, h, .. } => include(*y, *y + *h),
            DrawCommand::Circle { cy, r, .. } => include(*cy - *r, *cy + *r),
            DrawCommand::Ellipse { cy, ry, .. } => include(*cy - *ry, *cy + *ry),
            DrawCommand::Arc { cy, r, width, .. } => {
                let pad = *r + *width + 2.0;
                include(*cy - pad, *cy + pad);
            }
            DrawCommand::Pie { cy, r, .. } => include(*cy - *r, *cy + *r),
            DrawCommand::Line { y1, y2, width, .. } => {
                let lo = y1.min(*y2) - *width;
                let hi = y1.max(*y2) + *width;
                include(lo, hi);
            }
            DrawCommand::Text { y, size, .. } => include(*y - *size * 1.4, *y + *size * 1.4),
            DrawCommand::Clip { .. } | DrawCommand::ClearClip => {}
        }
    }
    if y_min < y_max {
        Some((y_min, y_max))
    } else {
        None
    }
}

impl OverlayRasterWorker {
    fn spawn(font_bytes: Option<Vec<u8>>) -> Self {
        let req_slot: Arc<(Mutex<Option<(u64, OverlayRasterReq)>>, Condvar)> =
            Arc::new((Mutex::new(None), Condvar::new()));
        let px_bytes = (DISPLAY_WIDTH_PX * DISPLAY_HEIGHT_PX * 4) as usize;
        let ready_slot: Arc<(Mutex<Option<(u64, OverlayRasterResult)>>, Condvar)> =
            Arc::new((Mutex::new(None), Condvar::new()));
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
                // Cache the top-bar strip (`y = 0..266`, Clock/Weather/System/Prusa) separately
                // and track the exact dirty vertical row band (`dirty_y..dirty_y+dirty_h`) so
                // rapid Fn / Shift taps NEVER re-rasterize top-bar widgets or upload all 8.3 MB!
                const TOP_BAR_H: usize = 266;
                let row_bytes = (DISPLAY_WIDTH_PX as usize) * 4;
                let top_bar_bytes = row_bytes * TOP_BAR_H;
                let mut cached_top_bar: Vec<u8> = Vec::new();
                let mut cached_top_hash: u64 = 0;
                let mut prev_keys_band: (usize, usize) = (TOP_BAR_H, DISPLAY_HEIGHT_PX as usize);
                let mut first_frame = true;
                loop {
                    let (seq, req) = {
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
                    let can_reuse_top = !first_frame
                        && !cached_top_bar.is_empty()
                        && cached_top_hash == req.tiles_hash
                        && req.has_custom_keys_renderer
                        && req.has_custom_dock
                        && req.world_overlay_draws.is_empty();
                    let (dirty_y, dirty_h) = if can_reuse_top {
                        canvas.rgba[..top_bar_bytes].copy_from_slice(&cached_top_bar);
                        canvas.rgba[top_bar_bytes..].fill(0);
                        if !req.under_key_draws.is_empty() {
                            canvas.execute(&req.under_key_draws);
                        }
                        let cur_band = draw_cmds_y_bounds(&req.under_key_draws)
                            .map(|(a, b)| (a.max(TOP_BAR_H), b.max(TOP_BAR_H)))
                            .unwrap_or((TOP_BAR_H, TOP_BAR_H));
                        let y0 = prev_keys_band.0.min(cur_band.0).max(TOP_BAR_H);
                        let y1 = prev_keys_band.1.max(cur_band.1).min(DISPLAY_HEIGHT_PX as usize);
                        prev_keys_band = cur_band;
                        if y1 > y0 {
                            (y0 as i32, (y1 - y0) as i32)
                        } else {
                            (TOP_BAR_H as i32, 1)
                        }
                    } else {
                        first_frame = false;
                        canvas.clear();
                        if !req.tile_draws.is_empty() {
                            canvas.execute(&req.tile_draws);
                        }
                        if req.has_custom_keys_renderer && !req.has_custom_dock {
                            canvas.draw_top_bar_dividers(&req.geometry);
                        }
                        if req.has_custom_keys_renderer && req.has_custom_dock {
                            if cached_top_bar.len() != top_bar_bytes {
                                cached_top_bar.resize(top_bar_bytes, 0);
                            }
                            cached_top_bar.copy_from_slice(&canvas.rgba[..top_bar_bytes]);
                            cached_top_hash = req.tiles_hash;
                        }
                        if !req.under_key_draws.is_empty() {
                            canvas.execute(&req.under_key_draws);
                        }
                        if !req.has_custom_keys_renderer {
                            canvas.blit_cached_default_key_legends(&req.geometry, DEFAULT_ANSI_LABELS);
                        }
                        if !req.world_overlay_draws.is_empty() {
                            canvas.execute(&req.world_overlay_draws);
                        }
                        prev_keys_band = draw_cmds_y_bounds(&req.under_key_draws)
                            .map(|(a, b)| (a.max(TOP_BAR_H), b.max(TOP_BAR_H)))
                            .unwrap_or((TOP_BAR_H, DISPLAY_HEIGHT_PX as usize));
                        (0, DISPLAY_HEIGHT_PX as i32)
                    };
                    let mut out_buf = spare_bg
                        .lock()
                        .unwrap()
                        .take()
                        .filter(|v| v.len() == canvas.rgba.len())
                        .unwrap_or_else(|| vec![0u8; canvas.rgba.len()]);
                    std::mem::swap(&mut canvas.rgba, &mut out_buf);
                    let (rmtx, rcv) = &*ready_bg;
                    let prev_ready = rmtx.lock().unwrap().replace((
                        seq,
                        OverlayRasterResult {
                            rgba: out_buf,
                            dirty_y,
                            dirty_h,
                        },
                    ));
                    rcv.notify_all();
                    if let Some((_, recycled)) = prev_ready {
                        *spare_bg.lock().unwrap() = Some(recycled.rgba);
                    }
                }
            })
            .ok();

        Self {
            req_slot,
            ready_slot,
            spare_slot,
            next_seq: 0,
            stop,
            join,
        }
    }

    fn submit(&mut self, req: OverlayRasterReq) -> u64 {
        self.next_seq = self.next_seq.wrapping_add(1);
        let seq = self.next_seq;
        let (mtx, cv) = &*self.req_slot;
        *mtx.lock().unwrap() = Some((seq, req));
        cv.notify_one();
        seq
    }

    /// Takes a completed raster result; when `wait_for_seq` is `Some((target, budget))` (e.g.
    /// right after an Fn/Shift/layer change), waits up to `budget` so the new keycaps present on
    /// the VERY SAME FRAME instead of lagging 1-2 frames behind!
    fn take_ready(
        &self,
        front_buf: &mut Vec<u8>,
        wait_for_seq: Option<(u64, std::time::Duration)>,
    ) -> Option<(i32, i32)> {
        let (rmtx, rcv) = &*self.ready_slot;
        let mut g = rmtx.lock().unwrap();
        if let Some((target_seq, budget)) = wait_for_seq {
            let deadline = std::time::Instant::now() + budget;
            while g.as_ref().is_none_or(|(s, _)| *s < target_seq) {
                let rem = deadline.saturating_duration_since(std::time::Instant::now());
                if rem.is_zero() {
                    break;
                }
                let (ng, to) = rcv.wait_timeout(g, rem).unwrap();
                g = ng;
                if to.timed_out() {
                    break;
                }
            }
        }
        if let Some((_, mut finished)) = g.take() {
            drop(g);
            let band = (finished.dirty_y, finished.dirty_h);
            std::mem::swap(front_buf, &mut finished.rgba);
            *self.spare_slot.lock().unwrap() = Some(finished.rgba);
            return Some(band);
        }
        None
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

/// Packed GPU draw primitive for Mali-G52 GLES2 under-key glows.
#[derive(Clone, Copy, Debug)]
pub struct V2GlowDraw {
    pub cx: f32,
    pub cy: f32,
    pub w: f32,
    pub h: f32,
    /// `0=Glow`, `1=Ring`, `2=Ripple`, `3=Solid`
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
    /// Top-bar + world-overlay RGBA buffer (rasterized off-thread; never touched on Fn/layer taps!).
    pub overlay_rgba: Vec<u8>,
    overlay_worker: OverlayRasterWorker,
    /// Dedicated keycap Canvas2D + 8-slot GPU texture LRU tracker (`active_keys_hash` hits Mali-G52
    /// VRAM in 0.000 ms with ZERO CPU raster and ZERO `glTexSubImage2D` upload on every repeat tap!).
    pub keys_canvas: Canvas2D,
    keys_cache_lru: std::collections::VecDeque<u64>,
    pub active_keys_hash: u64,
    pub pending_keys_upload: Option<u64>,
    pub wave_enabled: bool,
    pub wave_params: WaveFieldParams,
    pub active_media_path: Option<String>,
    pub active_stream_name: Option<String>,
    pub stream_overlays: Vec<keyglass_modules::StreamOverlayCmd>,
    pub bg_dimming: f32,
    pub top_fade: f32,
    parallax_rx: keyglass_bus::Subscription,
    power_rx: keyglass_bus::Subscription,
    host_display_rx: keyglass_bus::Subscription,
    last_parallax_sig: String,
    pub brightness_pct: u32,
    pub sleep_timeout_sec: u64,
    pub follow_host_display: bool,
    pub host_display_asleep: bool,
    pub pending_brightness_pct: Option<u32>,
    pub pending_brightness_steps: Vec<i8>,
    prefs_path: PathBuf,
    resolved_keys_cache_path: PathBuf,
    /// Debounced resolved-keys cache payload written off-thread after 2.5s of stability
    /// (never stalls the display render thread with synchronous eMMC writes on Fn/layer taps!).
    pending_resolved_cache: Option<(std::time::Instant, String)>,
    pub dt_sec: f32,
    pub background_shaders: Vec<keyglass_modules::ShaderPassCmd>,
    pub distortion_shaders: Vec<keyglass_modules::ShaderPassCmd>,
    pub under_key_shaders: Vec<keyglass_modules::ShaderPassCmd>,
    pub glows: Vec<V2GlowDraw>,
    pub particle_dots: Vec<ParticleDot>,
    pub time_sec: f32,
    pub overlay_dirty: bool,
    pub overlay_dirty_band: (i32, i32),
    last_keys_hash: u64,
    cached_under_key_draws: Vec<keyglass_modules::DrawCommand>,
    last_tiles_hash: u64,
    last_world_hash: u64,
    last_tiles_ms: u64,
    last_bitmap: [u8; 32],
    last_key_seq: Option<u32>,
    last_key_press_ms: [u64; 256],
    last_input_ms: u64,
    last_input_mono: std::time::Instant,
    last_state_mtime: Option<SystemTime>,
    last_reload_check: std::time::Instant,
    pub last_lua_us: u64,
    pub last_physics_us: u64,
    pub mirror: keyglass_bus::BrokerMirror,
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
fn hash_draw_list(cmds: &[keyglass_modules::DrawCommand]) -> u64 {
    use keyglass_modules::DrawCommand;
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
            DrawCommand::Pie { cx, cy, r, start_deg, end_deg, color } => {
                10u8.hash(&mut h);
                hash_f32(&mut h, *cx);
                hash_f32(&mut h, *cy);
                hash_f32(&mut h, *r);
                hash_f32(&mut h, *start_deg);
                hash_f32(&mut h, *end_deg);
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
        let base_v2_dir = state_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("/home/root/keyglass/v2"));
        let prefs_path = base_v2_dir.join("display_prefs.json");
        let resolved_keys_cache_path = base_v2_dir.join("resolved_keys.json");
        let registry = ModuleRegistry::new(bundled_dir, installed_dir, local_dir, state_path);
        let mut runtime = ModuleRuntime::new(broker.clone(), geometry.clone());
        let discovered = registry.discover();
        let saved = registry.load_state();
        runtime.sync_modules(&discovered, &saved.modules);

        let sdf_grid = KeySdfGrid::build(&geometry);
        let overlay_rgba = vec![0u8; (DISPLAY_WIDTH_PX * DISPLAY_HEIGHT_PX * 4) as usize];
        let overlay_worker = OverlayRasterWorker::spawn(font_bytes.map(|b| b.to_vec()));
        let keys_canvas = Canvas2D::new(DISPLAY_WIDTH_PX, DISPLAY_HEIGHT_PX, font_bytes);
        let parallax_rx = broker
            .subscribe("display/parallax", keyglass_bus::SubscribeOptions::default())
            .expect("valid parallax topic");
        let power_rx = broker
            .subscribe("display/power", keyglass_bus::SubscribeOptions::default())
            .expect("valid power topic");
        let host_display_rx = broker
            .subscribe("host/display/state", keyglass_bus::SubscribeOptions::default())
            .expect("valid host display state topic");
        let mirror = keyglass_bus::BrokerMirror::new(broker.clone(), "host");

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
            keys_canvas,
            keys_cache_lru: std::collections::VecDeque::with_capacity(8),
            active_keys_hash: 0,
            pending_keys_upload: None,
            wave_enabled: false,
            wave_params: WaveFieldParams::default(),
            active_media_path: None,
            active_stream_name: None,
            stream_overlays: Vec::new(),
            bg_dimming: 0.0,
            top_fade: 0.0,
            parallax_rx,
            power_rx,
            host_display_rx,
            last_parallax_sig: String::new(),
            brightness_pct: 80,
            sleep_timeout_sec: 300,
            follow_host_display: true,
            host_display_asleep: false,
            pending_brightness_pct: None,
            pending_brightness_steps: Vec::new(),
            prefs_path,
            resolved_keys_cache_path,
            pending_resolved_cache: None,
            dt_sec: 0.016,
            background_shaders: Vec::new(),
            distortion_shaders: Vec::new(),
            under_key_shaders: Vec::new(),
            glows: Vec::with_capacity(256),
            particle_dots: Vec::with_capacity(256),
            time_sec: 0.0,
            overlay_dirty: true,
            overlay_dirty_band: (0, DISPLAY_HEIGHT_PX as i32),
            last_keys_hash: 0,
            cached_under_key_draws: Vec::new(),
            last_tiles_hash: 0,
            last_world_hash: 0,
            last_tiles_ms: 0,
            last_bitmap: [0u8; 32],
            last_key_seq: None,
            last_key_press_ms: [0u64; 256],
            last_input_ms: keyglass_bus::now_unix_ms(),
            last_input_mono: std::time::Instant::now(),
            last_state_mtime,
            last_reload_check: std::time::Instant::now(),
            last_lua_us: 0,
            last_physics_us: 0,
            mirror,
        };
        this.load_persisted_prefs();
        this.load_cached_resolved_keys();
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
                    if let Some(f) = pw.get("followHostDisplay").and_then(|v| v.as_bool()) {
                        self.follow_host_display = f;
                    }
                }
            }
        }
    }

    fn load_cached_resolved_keys(&self) {
        if let Ok(raw) = std::fs::read_to_string(&self.resolved_keys_cache_path) {
            if let Ok(state) = serde_json::from_str::<ResolvedKeysState>(&raw) {
                if !state.keys.is_empty() {
                    let _ = self.broker.publish_typed(
                        topics::KEYBOARD_RESOLVED_KEYS,
                        &state,
                        PublishOptions::retained(),
                    );
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
    pub fn build_mirror_subscribe(&mut self, force: bool) -> Option<keyglass_bus::PeerBusFrame> {
        let extra = vec![
            "host/#".to_string(),
            "modules/#".to_string(),
            "profiles/#".to_string(),
            "system/actions".to_string(),
            "keyboard/attachments".to_string(),
            "layout/#".to_string(),
            "display/parallax".to_string(),
            "display/power".to_string(),
            "ui/#".to_string(),
            "fx/surface/impulse".to_string(),
            "keys/providers/ability-deck".to_string(),
            "keys/providers/streamdeck".to_string(),
        ];
        self.mirror.build_subscribe_if_changed(&extra, force)
    }

    pub fn on_mirror_frame(
        &mut self,
        frame: keyglass_bus::PeerBusFrame,
    ) -> Vec<keyglass_bus::PeerBusFrame> {
        self.mirror.on_remote_frame(frame)
    }

    pub fn drain_mirror_outgoing(&self) -> Vec<keyglass_bus::PeerBusFrame> {
        self.mirror.drain_outgoing()
    }

    pub fn on_host_disconnect(&mut self) {
        self.mirror.on_disconnect();
        // When detached from host, stop deferring to host display sleep and fall back cleanly
        // to the keyboard's own idle timeout.
        self.host_display_asleep = false;
    }

    pub fn publish_stream_state(&self, name: &str, active: bool, codec: &str) {
        let _ = self.broker.publish(
            format!("modules/streams/{name}"),
            serde_json::json!({ "active": active, "codec": codec, "name": name }),
            PublishOptions::retained().with_origin("display-media"),
        );
    }

    /// Checks whether `modules_state.json` or any Lua module file changed on disk (e.g. after USB sync).
    /// Throttled to once every 500 ms so synchronous eMMC `stat()` calls never stall the 60 Hz frame loop!
    pub fn poll_reload(&mut self) -> bool {
        if self.last_reload_check.elapsed() < std::time::Duration::from_millis(500) {
            return false;
        }
        self.last_reload_check = std::time::Instant::now();
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

    pub fn take_module_perf_snapshot(&mut self) -> Vec<keyglass_modules::ModulePerfStats> {
        self.runtime.take_perf_snapshot()
    }

    pub fn publish_perf_snapshot(&self, payload: serde_json::Value) {
        let _ = self.broker.publish(
            "display/perf",
            payload,
            PublishOptions::retained().with_origin("keyboard"),
        );
    }

    pub fn wave_dims(&self) -> (usize, usize) {
        (WAVE2D_W, WAVE2D_H)
    }

    /// Consumes one newline-delimited JSON IPC message from `keyglass-bridge` and publishes
    /// semantic events onto the keyboard's local `keyglass-bus`.
    pub fn ingest_bridge_ipc_line(&mut self, line: &str) {
        let Ok(val) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let now_ms = keyglass_bus::now_unix_ms();
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
                    self.last_input_mono = std::time::Instant::now();
                    if let Some(key) = self.geometry.key(k as u16) {
                        let ev = KeyEvent {
                            key_index: k as u16,
                            kind: if now {
                                KeyEventKind::Press
                            } else {
                                KeyEventKind::Release
                            },
                            timestamp_ms: now_ms,
                            // Where the key appears (parallax-corrected), so effects line up with it.
                            x_px: key.visual_x,
                            y_px: key.visual_y,
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
            "keyglass.brightness_up",
            "keyglass.brightness_down",
            "keyglass.profile_next",
            "keyglass.profile_prev",
            "keyglass.actuation_next",
            "keyglass.actuation_prev",
            "keyglass.platform_linux",
            "keyglass.platform_macos",
            "keyglass.platform_windows",
            "keyglass.auto_profile_lock",
            "widget-pomodoro.toggle",
            "widget-pomodoro.reset",
            "keyglass.theme_next",
            "keyglass.theme_prev",
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
                // Actions owned by the host (modules that need the computer) are published by the
                // host from the same key press over USB; publishing here too would reach the host
                // a second time through the bus mirror and fire the action twice.
                // (`modules/streamdeck/key` is for hosts that predate the `dispatch` field.)
                if m.get("dispatch").and_then(|v| v.as_str()) == Some("host") || topic == "modules/streamdeck/key" {
                    return;
                }
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
        // Never synchronously write eMMC flash on every momentary Fn press/release!
        // Stage for debounced background persistence after 2.5s of layer stability.
        if let Ok(txt) = serde_json::to_string(&state) {
            self.pending_resolved_cache = Some((std::time::Instant::now(), txt));
        }
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
                        // Ignore tiny Hall-effect magnetic noise (<0.4 mm) so desk vibration or
                        // thermal sensor drift never causes phantom wakes while sleeping!
                        if depth_um >= 400 {
                            self.last_input_ms = now_ms;
                            self.last_input_mono = std::time::Instant::now();
                        }
                        samples.push(AnalogKeySample::from_raw(
                            key_index,
                            depth_um,
                            vel,
                            key.visual_x,
                            key.visual_y,
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
                        .map(|h| keyglass_bus::WeatherHourly {
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

    /// Drains live `display/power` and `host/display/state` messages on every main-loop tick
    /// (including while asleep!) so host monitor sleep/wake and brightness/timeout updates apply
    /// within milliseconds without waiting for a rendered frame.
    pub fn poll_power_and_host_state(&mut self) {
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
            if let Some(f) = pw.get("followHostDisplay").and_then(|v| v.as_bool()) {
                self.follow_host_display = f;
            }
            self.persist_prefs_field("power", &pw);
        }
        let mut latest_host_disp: Option<bool> = None;
        while let Some(msg) = self.host_display_rx.try_recv() {
            if let Some(asleep) = msg.payload.get("asleep").and_then(|v| v.as_bool()) {
                latest_host_disp = Some(asleep);
            }
        }
        if let Some(asleep) = latest_host_disp {
            let was_asleep = self.host_display_asleep;
            self.host_display_asleep = asleep;
            if was_asleep && !asleep {
                // Computer screen woke up -> wake the keyboard display immediately too!
                self.last_input_ms = keyglass_bus::now_unix_ms();
                self.last_input_mono = std::time::Instant::now();
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
                self.keys_cache_lru.clear();
                self.overlay_dirty = true;
            }
        }
        self.poll_power_and_host_state();
        if let Some((at, _)) = &self.pending_resolved_cache {
            if at.elapsed() >= std::time::Duration::from_millis(2500) {
                if let Some((_, txt)) = self.pending_resolved_cache.take() {
                    let path = self.resolved_keys_cache_path.clone();
                    let _ = std::thread::Builder::new()
                        .name("kg-resolved-cache".into())
                        .spawn(move || {
                            let tmp = path.with_extension("json.tmp");
                            if std::fs::write(&tmp, txt).is_ok() {
                                let _ = std::fs::rename(&tmp, &path);
                            }
                        });
                }
            }
        }
        let dt = dt_sec.clamp(0.001, 0.06);
        self.time_sec += dt;
        let t_lua = std::time::Instant::now();
        let scene = self.runtime.step_frame(dt, now_ms);
        self.last_lua_us = t_lua.elapsed().as_micros() as u64;
        let t_phys = std::time::Instant::now();

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

        // Pack distinct glow and point-sprite draws for Mali-G52.
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
        self.particle_dots.clear();
        self.particle_dots
            .extend(self.particles.particles.iter().map(|p| p.dot()));

        // 1. Keycap Layer (8-slot Mali-G52 GPU Texture Cache):
        //    - Cache HIT (every repeat Fn / Base / Shift / Cmd / Layer tap!): 0.000 ms CPU raster,
        //      0 bytes uploaded to GPU, instantaneous same-frame `glBindTexture` swap!
        //    - Cache MISS (first time seeing a new layer/mod state): rasterizes ONLY `under_key_draws`
        //      and uploads once into an LRU GPU texture slot.
        self.pending_keys_upload = None;
        if !(scene.keep_under_key_layer && self.last_keys_hash != 0) {
            let mut kh = hash_draw_list(&scene.under_key_draws);
            if !scene.has_custom_keys_renderer {
                kh ^= 0x5A5A_1234_ABCD_0001;
            }
            if kh == 0 {
                kh = 1;
            }
            if kh != self.last_keys_hash {
                self.last_keys_hash = kh;
                self.active_keys_hash = kh;
                if let Some(pos) = self.keys_cache_lru.iter().position(|&h| h == kh) {
                    // Instant GPU VRAM cache hit!
                    self.keys_cache_lru.remove(pos);
                    self.keys_cache_lru.push_back(kh);
                } else {
                    // First-time miss: rasterize key legends ONCE into `keys_canvas` for GPU upload.
                    self.keys_canvas.clear();
                    if !scene.under_key_draws.is_empty() {
                        self.keys_canvas.execute(&scene.under_key_draws);
                    }
                    if !scene.has_custom_keys_renderer {
                        self.keys_canvas
                            .blit_cached_default_key_legends(&self.geometry, DEFAULT_ANSI_LABELS);
                    }
                    if self.keys_cache_lru.len() >= 8 {
                        self.keys_cache_lru.pop_front();
                    }
                    self.keys_cache_lru.push_back(kh);
                    self.pending_keys_upload = Some(kh);
                }
            }
        }

        // 2. Top-Bar + World-Overlay Layer (off-thread; NEVER triggered by Fn / layer key taps!):
        let world_hash = hash_draw_list(&scene.world_overlay_draws);
        let tiles_hash = hash_draw_list(&scene.tile_draws);
        let world_changed = world_hash != self.last_world_hash;
        let tiles_changed = tiles_hash != self.last_tiles_hash;

        let due_for_overlay = self.last_tiles_hash == 0
            || world_changed
            || (tiles_changed && now_ms.saturating_sub(self.last_tiles_ms) >= 140);

        if due_for_overlay {
            self.last_world_hash = world_hash;
            self.last_tiles_hash = tiles_hash;
            self.last_tiles_ms = now_ms;
            let _ = self.overlay_worker.submit(OverlayRasterReq {
                under_key_draws: Vec::new(),
                tile_draws: scene.tile_draws,
                tiles_hash,
                world_overlay_draws: scene.world_overlay_draws,
                has_custom_keys_renderer: scene.has_custom_keys_renderer,
                has_custom_dock: scene.has_custom_dock,
                geometry: self.geometry.clone(),
            });
        }
        if let Some(band) = self.overlay_worker.take_ready(&mut self.overlay_rgba, None) {
            self.overlay_dirty = true;
            self.overlay_dirty_band = band;
        } else {
            self.overlay_dirty = false;
        }
        self.last_physics_us = t_phys.elapsed().as_micros() as u64;
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
            k.visual_rect = Some(keyglass_bus::RectPx {
                x: k.rect.x + dx as f32,
                y: k.rect.y + dy as f32,
                w: k.rect.w,
                h: k.rect.h,
            });
        }
        // Rebuild the shared Key-SDF grid so physics/particles/effects collide against the
        // true parallax-shifted optical keycap borders automatically!
        self.sdf_grid = KeySdfGrid::build(&self.geometry);
        self.runtime.update_geometry(self.geometry.clone());
        // Publish updated geometry on retained `keyboard/geometry` so every module subscribed to
        // generally available key positioning gets live parallax coordinates & `visual_rect`s.
        let _ = self.broker.publish_typed(
            topics::KEYBOARD_GEOMETRY,
            &self.geometry,
            PublishOptions::retained().with_origin("keyboard"),
        );
    }

    pub fn is_display_asleep(&self) -> bool {
        if self.follow_host_display && self.host_display_asleep {
            return true;
        }
        if self.sleep_timeout_sec == 0 {
            return false;
        }
        self.last_input_mono.elapsed().as_secs() >= self.sleep_timeout_sec
    }
}
