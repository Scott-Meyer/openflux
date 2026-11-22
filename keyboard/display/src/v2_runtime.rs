//! OpenFlux v2 keyboard-side display engine: bridges `openflux-bridge` IPC lines (`key_state`,
//! `key_analog`, `live_data`, `bus_frame`) onto the keyboard's local `openflux-bus` broker,
//! runs `openflux-modules::ModuleRuntime` (100% standalone capable!), steps `WaveField2D`,
//! `ParticleSystem` (Key-SDF wall bounces + clear-keycap lens magnification + collisions),
//! and `Canvas2D` (cached key legends + top-bar widgets + Idle Pet), ready for native Mali-G52
//! OpenGL ES 2.0 presentation with zero-copy `rkmpp` 1080p NV12 video refraction.

use openflux_bus::{
    apply_remote_frame, topics, AnalogKeyFrame, AnalogKeySample, Broker, FrameLayout,
    HostConnectionState, KeyEvent, KeyEventKind, KeyboardGeometry, KeyboardState, MediaNowPlaying,
    PublishOptions, SystemMetrics, WeatherState, WireFrame, DISPLAY_HEIGHT_PX, DISPLAY_WIDTH_PX,
};
use openflux_compositor::{
    Canvas2D, KeySdfGrid, ParticleSystem, WaveField2D, DEFAULT_ANSI_LABELS, WAVE2D_H, WAVE2D_W,
};
use openflux_modules::{
    KeyGlowShape, ModuleRegistry, ModuleRuntime, WaveFieldParams,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
    pub canvas: Canvas2D,
    pub wave_enabled: bool,
    pub wave_params: WaveFieldParams,
    pub active_media_path: Option<String>,
    pub glows: Vec<V2GlowDraw>,
    pub time_sec: f32,
    pub overlay_dirty: bool,
    last_overlay_hash: u64,
    last_overlay_ms: u64,
    last_bitmap: [u8; 32],
    last_input_ms: u64,
    last_state_mtime: Option<SystemTime>,
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
        let registry = ModuleRegistry::new(bundled_dir, installed_dir, local_dir, state_path);
        let mut runtime = ModuleRuntime::new(broker.clone(), geometry.clone());
        let discovered = registry.discover();
        let saved = registry.load_state();
        runtime.sync_modules(&discovered, &saved.modules);

        let sdf_grid = KeySdfGrid::build(&geometry);
        let canvas = Canvas2D::new(DISPLAY_WIDTH_PX, DISPLAY_HEIGHT_PX, font_bytes);

        Self {
            broker,
            geometry,
            registry,
            runtime,
            sdf_grid,
            particles: ParticleSystem::default(),
            wave: WaveField2D::new(),
            canvas,
            wave_enabled: false,
            wave_params: WaveFieldParams::default(),
            active_media_path: None,
            glows: Vec::with_capacity(256),
            time_sec: 0.0,
            overlay_dirty: true,
            last_overlay_hash: 0,
            last_overlay_ms: 0,
            last_bitmap: [0u8; 32],
            last_input_ms: openflux_bus::now_unix_ms(),
            last_state_mtime,
        }
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
            Some("bus_frame") => {
                if let Some(frame_val) = val.get("frame") {
                    if let Ok(frame) = serde_json::from_value::<WireFrame>(frame_val.clone()) {
                        let _ = apply_remote_frame(&self.broker, frame, "host");
                    }
                }
            }
            Some("live_data") => self.handle_v1_live_data(&val, now_ms),
            _ => {}
        }
    }

    fn handle_key_state(&mut self, val: &Value, now_ms: u64) {
        if let Some(hex) = val.get("key_bitmap").and_then(|v| v.as_str()) {
            let mut next = [0u8; 32];
            for i in 0..32 {
                if let Some(pair) = hex.get(i * 2..i * 2 + 2) {
                    if let Ok(b) = u8::from_str_radix(pair, 16) {
                        next[i] = b;
                    }
                }
            }
            for k in 0..self.geometry.keys.len().min(256) {
                let was = (self.last_bitmap[k >> 3] & (1 << (k & 7))) != 0;
                let now = (next[k >> 3] & (1 << (k & 7))) != 0;
                if was != now {
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
        };
        let _ = self.broker.publish_typed(
            topics::KEYBOARD_STATE,
            &ks,
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
        let dt = dt_sec.clamp(0.001, 0.06);
        self.time_sec += dt;
        let scene = self.runtime.step_frame(dt, now_ms);

        self.active_media_path = scene.background_media.clone();
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

        // Only re-rasterize & re-upload the 1920x1080 2D overlay when draw commands actually change
        // (and rate-limit purely decorative animations like equalizer bars to ~10 Hz on the SoC
        // so Mali-G52 + rkmpp video + 3D water ripples + particles stay locked at 60 FPS!).
        let has_world_anim = !scene.world_overlay_draws.is_empty();
        let min_overlay_interval_ms = if has_world_anim { 45 } else { 180 };
        let overlay_hash = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            if let Ok(b) = serde_json::to_vec(&(
                &scene.under_key_draws,
                &scene.tile_draws,
                &scene.world_overlay_draws,
            )) {
                b.hash(&mut h);
            }
            h.finish()
        };
        let due_for_overlay = self.last_overlay_hash == 0
            || (overlay_hash != self.last_overlay_hash
                && now_ms.saturating_sub(self.last_overlay_ms) >= min_overlay_interval_ms);
        if due_for_overlay {
            self.last_overlay_hash = overlay_hash;
            self.last_overlay_ms = now_ms;
            self.canvas.clear();
            if !scene.under_key_draws.is_empty() {
                self.canvas.execute(&scene.under_key_draws);
            }
            if scene.has_custom_keys_renderer {
                if !scene.has_custom_dock {
                    self.canvas.draw_top_bar_dividers(&self.geometry);
                }
            } else {
                self.canvas
                    .blit_cached_default_key_legends(&self.geometry, DEFAULT_ANSI_LABELS);
            }
            if !scene.tile_draws.is_empty() {
                self.canvas.execute(&scene.tile_draws);
            }
            if !scene.world_overlay_draws.is_empty() {
                self.canvas.execute(&scene.world_overlay_draws);
            }
            self.overlay_dirty = true;
        } else {
            self.overlay_dirty = false;
        }
    }

    /// Updates every key's optical parallax offset (`visual_x`, `visual_y`, `parallax_dx`, `parallax_dy`)
    /// from the keyboard's physical viewing settings and republishes `keyboard/geometry` onto the bus.
    pub fn apply_parallax_settings(&mut self, settings: &crate::config::SettingsConfig) {
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
}
