//! OpenFlux v2 keyboard-side display runtime: bridges `openflux-bridge` IPC lines (`key_state`,
//! `key_analog`, `live_data`, `bus_frame`) onto the keyboard's local `openflux-bus` broker,
//! runs `openflux-modules::ModuleRuntime` (100% standalone capable!), and feeds `openflux-compositor`.

use openflux_bus::{
    apply_remote_frame, topics, AnalogKeyFrame, AnalogKeySample, Broker, FrameLayout,
    HostConnectionState, KeyEvent, KeyEventKind, KeyboardGeometry, KeyboardState, MediaNowPlaying,
    PublishOptions, SystemMetrics, WeatherState, WireFrame,
};
use openflux_compositor::{Compositor, CompositorOptions};
use openflux_modules::{FrameScene, ModuleRegistry, ModuleRuntime};
use serde_json::Value;
use std::path::PathBuf;

pub struct KeyboardV2Display {
    pub broker: Broker,
    pub geometry: KeyboardGeometry,
    pub registry: ModuleRegistry,
    pub runtime: ModuleRuntime,
    pub compositor: Option<Compositor>,
    last_bitmap: [u8; 32],
    last_input_ms: u64,
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
        let registry = ModuleRegistry::new(bundled_dir, installed_dir, local_dir, state_path);
        let mut runtime = ModuleRuntime::new(broker.clone(), geometry.clone());
        let discovered = registry.discover();
        let saved = registry.load_state();
        runtime.sync_modules(&discovered, &saved.modules);

        let compositor =
            Compositor::new_headless(geometry.clone(), font_bytes, CompositorOptions::default())
                .ok();

        Self {
            broker,
            geometry,
            registry,
            runtime,
            compositor,
            last_bitmap: [0u8; 32],
            last_input_ms: openflux_bus::now_unix_ms(),
        }
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
                },
                PublishOptions::retained().with_ttl_ms(600_000),
            );
        }
    }

    /// Feeds a hardware-decoded `rkmpp` NV12 video frame straight into the v2 GPU compositor.
    pub fn upload_rkmpp_nv12(&mut self, w: u32, h: u32, y_plane: &[u8], uv_plane: &[u8]) {
        if let Some(comp) = self.compositor.as_mut() {
            comp.upload_nv12_background(w, h, y_plane, uv_plane);
        }
    }

    pub fn step_and_render(&mut self, dt_sec: f32, now_ms: u64) -> FrameScene {
        let _ = self.runtime.check_hot_reload();
        let scene = self.runtime.step_frame(dt_sec, now_ms);
        if let Some(comp) = self.compositor.as_mut() {
            comp.render_scene(&scene);
        }
        scene
    }
}
