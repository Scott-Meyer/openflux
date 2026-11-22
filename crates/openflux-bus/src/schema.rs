//! Shared semantic topic conventions and payload types for cross-module composition.
//!
//! Any module or external program can publish and subscribe to arbitrary topics, while these
//! well-known channels let independently authored modules (like the Idle Pet, 3D Water Ripple,
//! Analog Depth Glow, and Host Metrics) compose seamlessly without hardcoding each other.

use serde::{Deserialize, Serialize};

pub mod topics {
    pub const INPUT_KEYS_BITMAP: &str = "input/keys/bitmap";
    pub const INPUT_KEYS_EVENT: &str = "input/keys/event";
    pub const INPUT_KEYS_ANALOG: &str = "input/keys/analog";

    pub const FX_SURFACE_IMPULSE: &str = "fx/surface/impulse";
    pub const FX_KEYS_GLOW: &str = "fx/keys/glow";

    pub const KEYBOARD_GEOMETRY: &str = "keyboard/geometry";
    pub const KEYBOARD_STATE: &str = "keyboard/state";

    pub const HOST_CONNECTION: &str = "host/connection";
    pub const HOST_SYSTEM_METRICS: &str = "host/system/metrics";
    pub const HOST_MEDIA_NOW_PLAYING: &str = "host/media/now_playing";
    pub const HOST_WEATHER_CURRENT: &str = "host/weather/current";
    pub const HOST_APP_ACTIVE: &str = "host/app/active";
    pub const HOST_APP_SHORTCUTS: &str = "host/app/shortcuts";
}

/// Maximum Hall-effect switch travel in micrometers (`2.8 mm`).
pub const MAX_TRAVEL_UM: u16 = 2800;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyEventKind {
    Press,
    Release,
}

/// Discrete key press/release event enriched with physical coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyEvent {
    pub key_index: u16,
    pub kind: KeyEventKind,
    pub timestamp_ms: u64,
    pub x_px: f32,
    pub y_px: f32,
    pub w_px: f32,
    pub h_px: f32,
    pub uv_x: f32,
    pub uv_y: f32,
    #[serde(default)]
    pub row: u8,
    #[serde(default)]
    pub col: u8,
    /// Optional actuation depth at the moment of the event (`0..2800 µm`).
    #[serde(default)]
    pub depth_um: u16,
    #[serde(default)]
    pub velocity_um_s: i32,
    #[serde(default)]
    pub color: Option<[f32; 4]>,
}

/// Continuous Hall-effect analog travel + velocity reading for one active switch.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AnalogKeySample {
    pub key_index: u16,
    /// Current travel depth in micrometers (`0..2800`).
    pub depth_um: u16,
    /// Signed travel velocity in micrometers per second (`> 0` pressing down, `< 0` releasing up).
    pub velocity_um_s: i32,
    /// Normalized depth `[0.0, 1.0]`.
    pub depth_norm: f32,
    /// Normalized signed velocity `[-1.0, 1.0]` (clamped around typical fast-press speed ~18,000 µm/s).
    pub velocity_norm: f32,
    pub x_px: f32,
    pub y_px: f32,
}

impl AnalogKeySample {
    pub fn from_raw(key_index: u16, depth_um: u16, velocity_um_s: i32, x_px: f32, y_px: f32) -> Self {
        let clamped_depth = depth_um.min(MAX_TRAVEL_UM);
        let depth_norm = clamped_depth as f32 / MAX_TRAVEL_UM as f32;
        let velocity_norm = (velocity_um_s as f32 / 18_000.0).clamp(-1.0, 1.0);
        Self {
            key_index,
            depth_um: clamped_depth,
            velocity_um_s,
            depth_norm,
            velocity_norm,
            x_px,
            y_px,
        }
    }
}

/// Batch of active analog key samples emitted from the firmware/simulator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalogKeyFrame {
    pub timestamp_ms: u64,
    pub samples: Vec<AnalogKeySample>,
}

/// Shared spatial impulse contract (`fx/surface/impulse`).
///
/// Any module (key press handler, Idle Pet paw landing, Pomodoro completion, external test suite
/// notifier via `fluxctl bus pub`) can emit a surface impulse, and any visual/reactive module
/// (3D Water Ripple, Particles, Idle Pet startle reaction) can subscribe and respond.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SurfaceImpulse {
    /// Center position in display pixels (`0..1920`, `0..1080`).
    pub x: f32,
    pub y: f32,
    /// Radius of the disturbance in display pixels.
    #[serde(default = "default_impulse_radius")]
    pub radius: f32,
    /// Signed or positive intensity (`0.0..2.0`, typical key press ~`0.8`, pet paw ~`0.45`, shockwave ~`1.5`).
    #[serde(default = "default_impulse_force")]
    pub force: f32,
    /// Optional RGBA tint (`0.0..1.0`).
    #[serde(default)]
    pub color: Option<[f32; 4]>,
    /// Semantic kind hint (`"key"`, `"paw"`, `"shockwave"`, `"ripple"`, `"burst"`).
    #[serde(default = "default_impulse_kind")]
    pub kind: String,
    /// Optional source identifier (`"module:companion-idle-pet"`, `"cli"`, `"input"`).
    #[serde(default)]
    pub source: Option<String>,
    /// Optional associated key index if centered on a physical key.
    #[serde(default)]
    pub key_index: Option<u16>,
}

fn default_impulse_radius() -> f32 {
    58.0
}
fn default_impulse_force() -> f32 {
    0.85
}
fn default_impulse_kind() -> String {
    "ripple".to_string()
}

/// Retained host connectivity state (`host/connection`) so hybrid modules degrade gracefully when unplugged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostConnectionState {
    pub connected: bool,
    pub host_asleep: bool,
    pub last_seen_ms: u64,
    #[serde(default)]
    pub host_platform: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub cpu_percent: f32,
    pub memory_used_gb: f32,
    pub memory_total_gb: f32,
    pub gpu_percent: f32,
    pub gpu_unified: bool,
    pub vram_used_gb: Option<f32>,
    pub vram_total_gb: Option<f32>,
    pub net_down_kbps: f32,
    pub net_up_kbps: f32,
    #[serde(default)]
    pub net_down: String,
    #[serde(default)]
    pub net_up: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MediaNowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    pub length_sec: f32,
    pub position_sec: f32,
    #[serde(default)]
    pub track_id: String,
    #[serde(default)]
    pub art_base64: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WeatherHourly {
    pub hour: String,
    pub temp: f32,
    pub condition: String,
    pub is_day: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WeatherState {
    pub city: String,
    pub temp: f32,
    pub high: f32,
    pub low: f32,
    pub condition: String,
    pub is_day: bool,
    pub celsius: bool,
    pub ready: bool,
    #[serde(default)]
    pub aqi: Option<u16>,
    #[serde(default)]
    pub sunrise: String,
    #[serde(default)]
    pub sunset: String,
    #[serde(default)]
    pub utc_offset_sec: Option<i64>,
    #[serde(default)]
    pub hourly: Vec<WeatherHourly>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct KeyboardState {
    pub active_layer_bitfield: u32,
    pub active_profile: u32,
    pub active_actuation_mode: u32,
    pub num_lock: bool,
    pub caps_lock: bool,
    pub scroll_lock: bool,
    pub keyframe_attached: bool,
    pub idle_ms: u64,
}
