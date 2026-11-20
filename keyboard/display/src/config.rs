use crate::device_paths::{BRIDGE_UPDATING_FLAG_PATH, DISPLAY_UPDATING_FLAG_PATH};
pub use crate::layout::MAX_KEYS;
use crate::layout::{keycode_to_legend, FrameLayoutType};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

pub const MAX_TILES: usize = 4;
pub const HW_HIST_LEN: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEffect {
    None,
    GlowingRings,
    SolidHighlight,
    Ripple,
    AutofocusLock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TileType {
    None = 0,
    DigitalClock = 2,
    DebugTelemetry = 3,
    DeviceInfo = 4,
    QcSensor = 5,
    DisplayPrefs = 6,
    HwGraph = 7,
    HwStatus = 8,
    KbdState = 9,
    AnalogClock = 10,
    FlipClock = 11,
    MinimalClock = 12,
    Pomodoro = 13,
    WorldClock = 14,
    Weather = 15,
    ActiveMedia = 16,
    VirtualPet = 17,
    Generic = 18,
}

impl TileType {
    pub fn from_u32(v: u32) -> Self {
        match v {
            2 => Self::DigitalClock,
            3 => Self::DebugTelemetry,
            4 => Self::DeviceInfo,
            5 => Self::QcSensor,
            6 => Self::DisplayPrefs,
            7 => Self::HwGraph,
            8 => Self::HwStatus,
            9 => Self::KbdState,
            10 => Self::AnalogClock,
            11 => Self::FlipClock,
            12 => Self::MinimalClock,
            13 => Self::Pomodoro,
            14 => Self::WorldClock,
            15 => Self::Weather,
            16 => Self::ActiveMedia,
            17 => Self::VirtualPet,
            18 => Self::Generic,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct KeyStyleOverride {
    pub label: Option<String>,
    pub icon: Option<String>,
    pub legend_argb: Option<u32>,
    pub ring_argb: Option<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct StylemapLayer {
    pub default_style: Option<KeyStyleOverride>,
    pub keys: HashMap<usize, KeyStyleOverride>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockIndicatorStyle {
    Off,
    Dot,
    Highlight,
}

#[derive(Debug, Clone)]
pub struct AppearanceConfig {
    pub theme_name: String,
    pub font_family: String,
    pub font_weight: f64,
    pub font_italic: bool,
    pub bg_path: String,
    pub shader_hue_deg: f64,
    pub shader_speed: f64,
    pub bg_blur: f64,
    pub bg_dimming: f64,
    pub module_dimming: f64,
    pub key_effect: KeyEffect,
    pub ring_glow_argb: u32,
    pub legend_argb: u32,
    pub lock_indicator_style: LockIndicatorStyle,
    pub lock_indicator_argb: Option<u32>,
    pub drop_shadow: bool,
    pub retint_icons: bool,
    pub retint_modules: bool,
    pub stylemaps: HashMap<usize, StylemapLayer>,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            theme_name: "Aurora".into(),
            font_family: "Montserrat".into(),
            font_weight: 300.0,
            font_italic: false,
            bg_path: "aurora_drift.frag".into(),
            shader_hue_deg: 0.0,
            shader_speed: 1.0,
            bg_blur: 0.0,
            bg_dimming: 20.0,
            module_dimming: 40.0,
            key_effect: KeyEffect::GlowingRings,
            ring_glow_argb: 0xFF34D399,
            legend_argb: 0xFFFFFFFF,
            lock_indicator_style: LockIndicatorStyle::Dot,
            lock_indicator_argb: None,
            drop_shadow: true,
            retint_icons: true,
            retint_modules: true,
            stylemaps: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SettingsConfig {
    pub timeout_sec: f64,
    pub parallax_active: bool,
    pub eye_height_mm: f64,
    pub eye_dist_front_mm: f64,
    pub eye_x_offset_mm: f64,
    pub foot_stand_deg: f64,
    pub display_offset_x_px: f64,
    pub display_offset_y_px: f64,
    pub show_alignment_grid: bool,
}

impl Default for SettingsConfig {
    fn default() -> Self {
        Self {
            timeout_sec: 900.0,
            parallax_active: true,
            eye_height_mm: 425.0,
            eye_dist_front_mm: 225.0,
            eye_x_offset_mm: 0.0,
            foot_stand_deg: 0.0,
            display_offset_x_px: 0.0,
            display_offset_y_px: 0.0,
            show_alignment_grid: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct KeyAction {
    pub action_type: u8,
    pub target_layer: u8,
    pub is_momentary: bool,
    pub key_code: String,
    pub legend: String,
}

#[derive(Debug, Clone, Default)]
pub struct KeymapLayer {
    pub name: String,
    pub actions: Vec<KeyAction>,
}

#[derive(Debug, Clone)]
pub struct KeymapConfig {
    pub selected_layout: FrameLayoutType,
    pub default_layer: u8,
    pub layers: Vec<KeymapLayer>,
}

impl Default for KeymapConfig {
    fn default() -> Self {
        Self {
            selected_layout: FrameLayoutType::Ansi,
            default_layer: 0,
            layers: vec![KeymapLayer {
                name: "Base".into(),
                actions: vec![KeyAction::default(); 85],
            }],
        }
    }
}

#[derive(Debug, Clone)]
pub struct MosaicTileConfig {
    pub tile_type: TileType,
    pub city: String,
    pub celsius: bool,
    pub pomo_main_min: u32,
    pub utc_offset_min: i32,
    pub tz_label: String,
    pub pet_name: String,
    pub generic_title: String,
    pub generic_subtitle: String,
    pub generic_value: String,
    pub generic_progress: f64,
    pub graph_metric: String,
}

impl Default for MosaicTileConfig {
    fn default() -> Self {
        Self {
            tile_type: TileType::None,
            city: String::new(),
            celsius: false,
            pomo_main_min: 25,
            utc_offset_min: 0,
            tz_label: String::new(),
            pet_name: "Mochi".into(),
            generic_title: String::new(),
            generic_subtitle: String::new(),
            generic_value: String::new(),
            generic_progress: 0.0,
            graph_metric: "cpu".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MosaicConfig {
    pub tiles: [MosaicTileConfig; MAX_TILES],
}

impl Default for MosaicConfig {
    fn default() -> Self {
        let mut tiles: [MosaicTileConfig; MAX_TILES] = Default::default();
        tiles[0].tile_type = TileType::MinimalClock;
        tiles[1].tile_type = TileType::HwStatus;
        tiles[2].tile_type = TileType::Weather;
        tiles[2].city = "Local".into();
        tiles[3].tile_type = TileType::Pomodoro;
        Self { tiles }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ThemeOverrideState {
    pub selected_id: String,
    pub profile: u32,
    pub host_theme_id: Option<String>,
    pub host_profile_slot: Option<Option<u32>>,
    pub host_theme_rev: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct IpcLineOutcome {
    pub applied: bool,
    pub is_live_data_payload: bool,
    pub triggered_action: bool,
    pub theme_cycled: bool,
    pub theme_reconciled: bool,
}

#[derive(Debug, Clone)]
pub struct DisplayConfig {
    pub appearance: AppearanceConfig,
    pub settings: SettingsConfig,
    pub keymap: KeymapConfig,
    pub mosaic: MosaicConfig,
    pub appearance_selected_id: String,
    pub active_theme_id: String,
    pub profile_theme_ids: Vec<String>,
    pub appearance_raw: Option<Value>,
    pub loaded_appearance_mtime_ns: u128,
    pub theme_override: Option<ThemeOverrideState>,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            appearance: AppearanceConfig::default(),
            settings: SettingsConfig::default(),
            keymap: KeymapConfig::default(),
            mosaic: MosaicConfig::default(),
            appearance_selected_id: "default".into(),
            active_theme_id: "default".into(),
            profile_theme_ids: Vec::new(),
            appearance_raw: None,
            loaded_appearance_mtime_ns: 0,
            theme_override: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GenericSlotLive {
    pub title: String,
    pub subtitle: String,
    pub value: String,
    pub progress: f64,
    pub active: bool,
}

impl Default for GenericSlotLive {
    fn default() -> Self {
        Self {
            title: String::new(),
            subtitle: String::new(),
            value: String::new(),
            progress: 0.0,
            active: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutOverlayEntry {
    pub mods: u8, // 1=Ctrl, 2=Shift, 4=Opt, 8=Cmd
    pub key_code: String,
    pub label: String,
    pub icon: String,
}

pub fn default_macos_shortcut_overlays() -> Vec<ShortcutOverlayEntry> {
    [
        (8, "KC_C", "Copy"),
        (8, "KC_V", "Paste"),
        (8, "KC_X", "Cut"),
        (8, "KC_Z", "Undo"),
        (10, "KC_Z", "Redo"),
        (8, "KC_S", "Save"),
        (8, "KC_F", "Find"),
        (8, "KC_A", "All"),
        (8, "KC_W", "Close"),
        (8, "KC_N", "New"),
        (8, "KC_T", "Tab"),
        (8, "KC_Q", "Quit"),
        (8, "KC_R", "Reload"),
    ]
    .into_iter()
    .map(|(mods, key_code, label)| ShortcutOverlayEntry {
        mods,
        key_code: key_code.to_string(),
        label: label.to_string(),
        icon: String::new(),
    })
    .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct WeatherHourLive {
    pub hour_label: String,
    pub temp: f64,
    pub condition: String,
    pub is_day: bool,
}

#[derive(Debug, Clone)]
pub struct PendingPressEdge {
    pub key_index: usize,
    pub key_bitmap: [u8; 32],
    pub has_firmware_layer: bool,
    pub layer_bitfield: u32,
}

#[derive(Debug, Clone)]
pub struct LiveState {
    pub key_seq: u32,
    pub keyframe_layout: u8,
    pub has_firmware_layer: bool,
    pub active_layer_bitfield: u32,
    pub active_layer: u8,
    pub active_actuation_mode: u32,
    pub language_code: u32,
    pub platform_code: u32,
    pub active_profile: u32,
    pub active_theme_id: String,
    pub active_theme_for_profile: u32,
    pub has_active_theme_for_profile: bool,
    pub local_theme_override: Option<(String, u32, u128)>,
    pub last_host_theme_id: Option<String>,
    pub last_host_theme_profile_slot: Option<Option<u32>>,
    pub last_host_theme_rev: Option<u64>,
    pub key_bitmap: [u8; 32],
    pub prev_key_bitmap: [u8; 32],
    pub key_press_ms: [u64; MAX_KEYS],
    pub key_release_ms: [u64; MAX_KEYS],
    pub shortcut_overlays: Vec<ShortcutOverlayEntry>,
    pub caps_lock: bool,
    pub num_lock: bool,
    pub scroll_lock: bool,
    pub qc_mode: bool,
    pub stress_test: bool,
    pub cpu_str: String,
    pub mem_str: String,
    pub net_down_str: String,
    pub net_up_str: String,
    pub gpu_str: String,
    pub vram_used_gb: Option<f32>,
    pub vram_total_gb: Option<f32>,
    pub gpu_unified: bool,
    pub cpu_pct: f32,
    pub gpu_pct: f32,
    pub mem_pct: f32,
    pub cpu_hist: Vec<f32>,
    pub gpu_hist: Vec<f32>,
    pub mem_hist: Vec<f32>,
    pub media_title: String,
    pub media_artists: String,
    pub media_album: String,
    pub media_length_s: u32,
    pub media_played_s: u32,
    pub media_playing: bool,
    pub weather_city: String,
    pub weather_condition: String,
    pub weather_temp: f64,
    pub weather_high: f64,
    pub weather_low: f64,
    pub weather_ready: bool,
    pub weather_is_day: bool,
    pub weather_celsius: bool,
    pub has_live_weather_unit: bool,
    pub weather_sunrise: String,
    pub weather_sunset: String,
    pub weather_aqi: Option<u32>,
    pub weather_utc_offset_sec: Option<i64>,
    pub weather_hourly: Vec<WeatherHourLive>,
    pub pet_mood: u8,
    pub pet_happiness: u8,
    pub pet_steps: u32,
    pub generic_title: String,
    pub generic_subtitle: String,
    pub generic_value: String,
    pub generic_progress: f64,
    pub has_generic_live: bool,
    pub generic_slots: [GenericSlotLive; MAX_TILES],
    pub pomo_running: bool,
    pub pomo_remaining_s: u32,
    pub has_live_pomo: bool,
    pub pomo_anchor_ms: u64,
    pub pomo_anchor_rem_s: u32,
    pub pomo_anchor_set: bool,
    pub last_pomo_rev: u64,
    pub dev_pomo_rev: u64,
    pub prev_of_events_seq: u16,
    pub has_of_events: bool,
    pub last_fw_uptime_ms: Option<u32>,
    pub last_fw_key_seq: Option<u32>,
    pub pending_of_events: Vec<(u8, Option<u32>)>,
    pub pending_press_edges: Vec<PendingPressEdge>,
    pub pending_brightness_steps: Vec<i8>,
    pub pending_theme_delta: i32,
    pub last_key_activity_ms: u64,
    pub host_display_asleep: bool,
    pub ripple_pulses: Vec<(usize, u64)>,
}

impl LiveState {
    pub fn from_config(cfg: &DisplayConfig) -> Self {
        let mut st = Self {
            key_seq: 0,
            keyframe_layout: 0,
            has_firmware_layer: false,
            active_layer_bitfield: 0,
            active_layer: cfg.keymap.default_layer,
            active_actuation_mode: 0,
            language_code: 0,
            platform_code: 0,
            active_profile: 0,
            active_theme_id: String::new(),
            active_theme_for_profile: 0,
            has_active_theme_for_profile: false,
            local_theme_override: None,
            last_host_theme_id: None,
            last_host_theme_profile_slot: None,
            last_host_theme_rev: None,
            key_bitmap: [0; 32],
            prev_key_bitmap: [0; 32],
            key_press_ms: [0; MAX_KEYS],
            key_release_ms: [0; MAX_KEYS],
            shortcut_overlays: default_macos_shortcut_overlays(),
            caps_lock: false,
            num_lock: false,
            scroll_lock: false,
            qc_mode: false,
            stress_test: false,
            cpu_str: "0%".into(),
            mem_str: "--".into(),
            net_down_str: "0 KB/s".into(),
            net_up_str: "0 KB/s".into(),
            gpu_str: "0%".into(),
            vram_used_gb: None,
            vram_total_gb: None,
            gpu_unified: false,
            cpu_pct: 0.0,
            gpu_pct: 0.0,
            mem_pct: 0.0,
            cpu_hist: Vec::new(),
            gpu_hist: Vec::new(),
            mem_hist: Vec::new(),
            media_title: String::new(),
            media_artists: String::new(),
            media_album: String::new(),
            media_length_s: 0,
            media_played_s: 0,
            media_playing: false,
            weather_city: "Local".into(),
            weather_condition: "Clear".into(),
            weather_temp: 0.0,
            weather_high: 0.0,
            weather_low: 0.0,
            weather_ready: false,
            weather_is_day: true,
            weather_celsius: false,
            has_live_weather_unit: false,
            weather_sunrise: String::new(),
            weather_sunset: String::new(),
            weather_aqi: None,
            weather_utc_offset_sec: None,
            weather_hourly: Vec::new(),
            pet_mood: 0,
            pet_happiness: 85,
            pet_steps: 0,
            generic_title: String::new(),
            generic_subtitle: String::new(),
            generic_value: String::new(),
            generic_progress: 0.0,
            has_generic_live: false,
            generic_slots: Default::default(),
            pomo_running: false,
            pomo_remaining_s: 25 * 60,
            has_live_pomo: false,
            pomo_anchor_ms: 0,
            pomo_anchor_rem_s: 0,
            pomo_anchor_set: false,
            last_pomo_rev: 0,
            dev_pomo_rev: 0,
            prev_of_events_seq: 0,
            has_of_events: false,
            last_fw_uptime_ms: None,
            last_fw_key_seq: None,
            pending_of_events: Vec::new(),
            pending_press_edges: Vec::new(),
            pending_brightness_steps: Vec::new(),
            pending_theme_delta: 0,
            last_key_activity_ms: 0,
            host_display_asleep: false,
            ripple_pulses: Vec::new(),
        };
        if let Some(ref ov) = cfg.theme_override {
            st.local_theme_override = Some((
                ov.selected_id.clone(),
                ov.profile,
                cfg.loaded_appearance_mtime_ns,
            ));
            st.active_profile = ov.profile;
            st.last_host_theme_id = ov.host_theme_id.clone();
            st.last_host_theme_profile_slot = ov.host_profile_slot;
            st.last_host_theme_rev = ov.host_theme_rev;
        }
        for t in &cfg.mosaic.tiles {
            if t.tile_type == TileType::Weather && !t.city.is_empty() {
                st.weather_city = t.city.clone();
                st.weather_celsius = t.celsius;
            } else if t.tile_type == TileType::Pomodoro && t.pomo_main_min > 0 {
                st.pomo_remaining_s = t.pomo_main_min * 60;
            }
        }
        st
    }

    pub fn is_key_pressed(&self, key_index: usize) -> bool {
        if key_index >= 256 {
            return false;
        }
        ((self.key_bitmap[key_index >> 3] >> (key_index & 7)) & 1) != 0
    }

    pub fn effective_layer_for(
        keymap: &KeymapConfig,
        key_bitmap: &[u8; 32],
        has_firmware_layer: bool,
        active_layer: u8,
    ) -> usize {
        if keymap.layers.is_empty() {
            return 0;
        }
        let mut layer = if has_firmware_layer && (active_layer as usize) < keymap.layers.len() {
            active_layer as usize
        } else {
            (keymap.default_layer as usize).min(keymap.layers.len() - 1)
        };
        if !has_firmware_layer {
            let base_idx = (keymap.default_layer as usize).min(keymap.layers.len() - 1);
            for (i, ka) in keymap.layers[base_idx]
                .actions
                .iter()
                .enumerate()
                .take(MAX_KEYS)
            {
                let pressed = ((key_bitmap[i >> 3] >> (i & 7)) & 1) != 0;
                if ka.action_type == 2 && ka.is_momentary && pressed {
                    let tl = ka.target_layer as usize;
                    if tl < keymap.layers.len() {
                        layer = tl;
                        break;
                    }
                }
            }
        }
        layer
    }

    pub fn effective_layer(&self, keymap: &KeymapConfig) -> usize {
        Self::effective_layer_for(
            keymap,
            &self.key_bitmap,
            self.has_firmware_layer,
            self.active_layer,
        )
    }

    pub fn is_display_asleep(&self, cfg: &DisplayConfig, mono_ms: u64) -> bool {
        if std::path::Path::new(crate::device_paths::SUSPEND_FLAG_PATH).exists()
            || self.host_display_asleep
        {
            return true;
        }
        if cfg.settings.timeout_sec > 0.0 {
            let limit_ms = (cfg.settings.timeout_sec * 1000.0).round() as u64;
            if limit_ms > 0 && mono_ms.saturating_sub(self.last_key_activity_ms) >= limit_ms {
                return true;
            }
        }
        false
    }

    /// Resolves the active key action for a press edge, matching firmware layer traversal:
    /// checks active layers highest-first (including held momentary layers) and falls through only
    /// on `KC_TRANSPARENT`, stopping on `KC_NO` or any concrete action.
    pub fn resolve_action_for_edge<'a>(
        keymap: &'a KeymapConfig,
        key_bitmap: &[u8; 32],
        has_firmware_layer: bool,
        fw_layer_bitfield: u32,
        key_index: usize,
    ) -> Option<&'a KeyAction> {
        if keymap.layers.is_empty() {
            return None;
        }
        let mut lookup_mask: u32 = if has_firmware_layer {
            fw_layer_bitfield | 1
        } else {
            let base = (keymap.default_layer as usize).min(keymap.layers.len() - 1);
            let mut mask = (1u32 << base.min(31)) | 1;
            for _ in 0..keymap.layers.len().min(8) {
                let mut changed = false;
                for i in 0..MAX_KEYS {
                    let pressed = ((key_bitmap[i >> 3] >> (i & 7)) & 1) != 0;
                    if !pressed {
                        continue;
                    }
                    for l in (0..keymap.layers.len().min(32)).rev() {
                        if (mask >> l) & 1 == 0 {
                            continue;
                        }
                        let Some(ka) = keymap.layers[l].actions.get(i) else {
                            continue;
                        };
                        if ka.key_code == "KC_TRANSPARENT" {
                            continue;
                        }
                        if ka.action_type == 2 && ka.is_momentary {
                            let tl = ka.target_layer as usize;
                            if tl < keymap.layers.len() && tl < 32 && ((mask >> tl) & 1) == 0 {
                                mask |= 1u32 << tl;
                                changed = true;
                            }
                        }
                        break;
                    }
                }
                if !changed {
                    break;
                }
            }
            mask
        };
        lookup_mask |= 1;
        for l in (0..keymap.layers.len().min(32)).rev() {
            if (lookup_mask >> l) & 1 == 0 {
                continue;
            }
            let Some(ka) = keymap.layers[l].actions.get(key_index) else {
                continue;
            };
            if ka.key_code == "KC_TRANSPARENT" {
                continue;
            }
            return Some(ka);
        }
        None
    }

    fn apply_of_func_code(&mut self, kc: &str, default_pomo_secs: u32, now_ms: u64) -> bool {
        if kc == "KC_OF_START_PAUSE_POMODORO" || kc == "OF_START_PAUSE_POMODORO" {
            self.tick_pomo(now_ms);
            self.pomo_running = !self.pomo_running;
            if self.pomo_running {
                if self.pomo_remaining_s == 0 {
                    self.pomo_remaining_s = default_pomo_secs;
                }
                self.pomo_anchor_rem_s = self.pomo_remaining_s;
                self.pomo_anchor_ms = now_ms;
                self.pomo_anchor_set = true;
            } else {
                self.pomo_anchor_set = false;
            }
            self.has_live_pomo = true;
            self.dev_pomo_rev = self.dev_pomo_rev.saturating_add(1);
            return true;
        }
        if kc == "KC_OF_RESET_POMODORO" || kc == "OF_RESET_POMODORO" {
            self.pomo_running = false;
            self.pomo_remaining_s = default_pomo_secs;
            self.pomo_anchor_rem_s = default_pomo_secs;
            self.pomo_anchor_set = false;
            self.has_live_pomo = true;
            self.dev_pomo_rev = self.dev_pomo_rev.saturating_add(1);
            return true;
        }
        if kc == "KC_OF_BRIGHTNESS_UP" || kc == "OF_BRIGHTNESS_UP" {
            self.pending_brightness_steps.push(1);
            return true;
        }
        if kc == "KC_OF_BRIGHTNESS_DOWN" || kc == "OF_BRIGHTNESS_DOWN" {
            self.pending_brightness_steps.push(-1);
            return true;
        }
        if kc == "KC_OF_THEME_CYCLE_NEXT" || kc == "OF_THEME_CYCLE_NEXT" {
            self.pending_theme_delta += 1;
            return true;
        }
        if kc == "KC_OF_THEME_CYCLE_PREV" || kc == "OF_THEME_CYCLE_PREV" {
            self.pending_theme_delta -= 1;
            return true;
        }
        false
    }

    pub fn handle_key_press_actions(&mut self, cfg: &DisplayConfig, now_ms: u64) -> bool {
        let of_events = std::mem::take(&mut self.pending_of_events);
        let edges = std::mem::take(&mut self.pending_press_edges);
        if of_events.is_empty() && edges.is_empty() {
            return false;
        }
        let mut changed = false;
        let default_pomo_secs = cfg
            .mosaic
            .tiles
            .iter()
            .find(|t| t.tile_type == TileType::Pomodoro && t.pomo_main_min > 0)
            .map(|t| t.pomo_main_min * 60)
            .unwrap_or(25 * 60);

        if self.has_of_events {
            for (code, _prof_opt) in of_events {
                let kc = match code {
                    2 => "KC_OF_BRIGHTNESS_UP",
                    3 => "KC_OF_BRIGHTNESS_DOWN",
                    12 => "KC_OF_START_PAUSE_POMODORO",
                    13 => "KC_OF_RESET_POMODORO",
                    14 => "KC_OF_THEME_CYCLE_NEXT",
                    15 => "KC_OF_THEME_CYCLE_PREV",
                    _ => continue,
                };
                if self.apply_of_func_code(kc, default_pomo_secs, now_ms) {
                    changed = true;
                }
            }
            return changed;
        }

        for edge in edges {
            let Some(a) = Self::resolve_action_for_edge(
                &cfg.keymap,
                &edge.key_bitmap,
                edge.has_firmware_layer,
                edge.layer_bitfield,
                edge.key_index,
            ) else {
                continue;
            };
            if self.apply_of_func_code(a.key_code.as_str(), default_pomo_secs, now_ms) {
                changed = true;
            }
        }
        changed
    }

    pub fn held_modifier_mask(&self, keymap: &KeymapConfig) -> u8 {
        let eff_layer = self.effective_layer(keymap);
        let fallback_layer =
            (keymap.default_layer as usize).min(keymap.layers.len().saturating_sub(1));
        let actions = keymap
            .layers
            .get(eff_layer)
            .map(|l| l.actions.as_slice())
            .unwrap_or(&[]);
        let fallback_actions = keymap
            .layers
            .get(fallback_layer)
            .map(|l| l.actions.as_slice())
            .unwrap_or(&[]);
        let mut mask = 0u8;
        for k in 0..MAX_KEYS {
            if !self.is_key_pressed(k) {
                continue;
            }
            let act = actions
                .get(k)
                .filter(|a| a.key_code != "KC_TRANSPARENT")
                .or_else(|| fallback_actions.get(k));
            if let Some(a) = act {
                if a.action_type == 0 {
                    match a.key_code.as_str() {
                        "KC_LEFT_CTRL" | "KC_LCTL" | "KC_RIGHT_CTRL" | "KC_RCTL" => mask |= 1,
                        "KC_LEFT_SHIFT" | "KC_LSHIFT" | "KC_RIGHT_SHIFT" | "KC_RSHIFT" => mask |= 2,
                        "KC_LEFT_ALT" | "KC_LALT" | "KC_RIGHT_ALT" | "KC_RALT" => mask |= 4,
                        "KC_LEFT_GUI" | "KC_LGUI" | "KC_RIGHT_GUI" | "KC_RGUI" => mask |= 8,
                        _ => {}
                    }
                }
            }
        }
        mask
    }

    pub fn tick_pomo(&mut self, now_ms: u64) {
        if !self.pomo_running {
            self.pomo_anchor_set = false;
            return;
        }
        if !self.pomo_anchor_set {
            self.pomo_anchor_ms = now_ms;
            self.pomo_anchor_rem_s = self.pomo_remaining_s;
            self.pomo_anchor_set = true;
            return;
        }
        if now_ms > self.pomo_anchor_ms {
            let elapsed_s = (now_ms - self.pomo_anchor_ms) / 1000;
            if elapsed_s >= self.pomo_anchor_rem_s as u64 {
                self.pomo_remaining_s = 0;
                self.pomo_running = false;
                self.pomo_anchor_set = false;
            } else {
                self.pomo_remaining_s = self.pomo_anchor_rem_s - elapsed_s as u32;
            }
        }
    }

    pub fn apply_live_json(&mut self, json_str: &str, now_ms: u64) -> Result<(), String> {
        let mut val: Value = serde_json::from_str(json_str).map_err(|e| e.to_string())?;
        if val.get("type").and_then(|v| v.as_str()) == Some("key_state") {
            let prev_fw_uptime = self.last_fw_uptime_ms;
            let prev_fw_kseq = self.last_fw_key_seq;
            if let Some(seq) = val.get("seq").and_then(|v| v.as_u64()) {
                let s32 = seq as u32;
                self.key_seq = s32;
                self.last_fw_key_seq = Some(s32);
            }
            if let Some(ts) = val.get("timestamp_ms").and_then(|v| v.as_u64()) {
                self.last_fw_uptime_ms = Some(ts as u32);
            }
            let mcu_rebooted = matches!((prev_fw_uptime, self.last_fw_uptime_ms), (Some(p), Some(c)) if p > 0 && c < p)
                || matches!((prev_fw_kseq, self.last_fw_key_seq), (Some(p), Some(c)) if p > 0 && c < p);
            if let Some(kl) = val.get("keyframe_layout").and_then(|v| v.as_u64()) {
                self.keyframe_layout = kl as u8;
            }
            if let Some(bf) = val.get("active_layer_bitfield").and_then(|v| v.as_u64()) {
                let bf32 = bf as u32;
                self.has_firmware_layer = true;
                self.active_layer_bitfield = bf32;
                let mut highest = 0u8;
                for b in 0..32u8 {
                    if ((bf32 >> b) & 1) != 0 {
                        highest = b;
                    }
                }
                self.active_layer = highest;
            }
            if let Some(m) = val.get("active_actuation_mode").and_then(|v| v.as_u64()) {
                self.active_actuation_mode = m as u32;
            }
            if let Some(lc) = val.get("language_code").and_then(|v| v.as_u64()) {
                self.language_code = lc as u32;
            }
            if let Some(pc) = val.get("platform_code").and_then(|v| v.as_u64()) {
                self.platform_code = pc as u32;
            }
            if let Some(ap) = val.get("active_profile").and_then(|v| v.as_u64()) {
                let ap32 = ap as u32;
                if ap32 != self.active_profile {
                    self.active_profile = ap32;
                    self.active_theme_id.clear();
                    self.has_active_theme_for_profile = false;
                    self.local_theme_override = None;
                }
            }
            if let Some(b) = val.get("caps_lock").and_then(|v| v.as_bool()) {
                self.caps_lock = b;
            }
            if let Some(b) = val.get("num_lock").and_then(|v| v.as_bool()) {
                self.num_lock = b;
            }
            if let Some(b) = val.get("scroll_lock").and_then(|v| v.as_bool()) {
                self.scroll_lock = b;
            }
            if let Some(b) = val.get("qc_mode").and_then(|v| v.as_bool()) {
                self.qc_mode = b;
            }
            if let Some(b) = val.get("stress_test").and_then(|v| v.as_bool()) {
                self.stress_test = b;
            }
            if let (Some(raw_seq), Some(arr)) = (
                val.get("of_events_seq").and_then(|v| v.as_u64()),
                val.get("of_events_ring").and_then(|v| v.as_array()),
            ) {
                if arr.len() == 16 {
                    let prof_arr = val
                        .get("of_events_profiles")
                        .and_then(|v| v.as_array())
                        .filter(|p| p.len() == 16);
                    let seq = (raw_seq & 0xFFFF) as u16;
                    if !self.has_of_events || mcu_rebooted {
                        self.prev_of_events_seq = seq;
                        self.has_of_events = true;
                    } else {
                        let delta = seq.wrapping_sub(self.prev_of_events_seq).min(16);
                        let start = seq.wrapping_sub(delta);
                        for offset in 0..delta {
                            let idx = (start.wrapping_add(offset) & 15) as usize;
                            if let Some(code) = arr[idx].as_u64() {
                                let p_opt =
                                    prof_arr.and_then(|pa| pa[idx].as_u64()).map(|p| p as u32);
                                self.pending_of_events.push(((code & 0xFF) as u8, p_opt));
                            }
                        }
                        self.prev_of_events_seq = seq;
                    }
                }
            }
            if let Some(hex) = val.get("key_bitmap").and_then(|v| v.as_str()) {
                if hex.len() == 64 {
                    let prev = self.key_bitmap;
                    let mut parsed = [0u8; 32];
                    let bytes = hex.as_bytes();
                    let mut ok = true;
                    for i in 0..32 {
                        let hi = (bytes[i * 2] as char).to_digit(16);
                        let lo = (bytes[i * 2 + 1] as char).to_digit(16);
                        if let (Some(h), Some(l)) = (hi, lo) {
                            parsed[i] = ((h << 4) | l) as u8;
                        } else {
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        if parsed != prev {
                            self.last_key_activity_ms = now_ms;
                        }
                        self.prev_key_bitmap = prev;
                        self.key_bitmap = parsed;
                        for k in 0..MAX_KEYS {
                            let was_p = ((prev[k >> 3] >> (k & 7)) & 1) != 0;
                            let is_p = ((self.key_bitmap[k >> 3] >> (k & 7)) & 1) != 0;
                            if is_p && !was_p {
                                self.key_press_ms[k] = now_ms.max(1);
                                let t_pulse = now_ms.max(1);
                                self.ripple_pulses
                                    .retain(|&(_, ts)| t_pulse.saturating_sub(ts) < 2150);
                                if self.ripple_pulses.len() >= 32 {
                                    self.ripple_pulses.remove(0);
                                }
                                self.ripple_pulses.push((k, t_pulse));
                                if !self.has_of_events {
                                    self.pending_press_edges.push(PendingPressEdge {
                                        key_index: k,
                                        key_bitmap: self.key_bitmap,
                                        has_firmware_layer: self.has_firmware_layer,
                                        layer_bitfield: self.active_layer_bitfield,
                                    });
                                }
                                self.pet_steps = self.pet_steps.saturating_add(1);
                                self.pet_mood = 1;
                                if self.pet_happiness < 100 {
                                    self.pet_happiness += 1;
                                }
                            } else if !is_p && was_p {
                                self.key_release_ms[k] = now_ms.max(1);
                            }
                        }
                    }
                }
            }
            return Ok(());
        }

        if val.get("type").and_then(|v| v.as_str()) == Some("live_data") {
            if let Some(payload) = val.get("payload").cloned() {
                val = payload;
            } else {
                return Ok(());
            }
        }

        let mut applied = false;
        if let Some(at) = val
            .get("activeThemeId")
            .or_else(|| val.get("active_theme_id"))
            .and_then(|v| v.as_str())
        {
            let slot_opt = val
                .get("activeThemeProfileSlot")
                .and_then(|v| v.as_u64())
                .map(|s| s as u32);
            let rev_opt = val.get("themeRev").and_then(|v| v.as_u64());
            let theme_changed = if let Some(rev) = rev_opt {
                self.last_host_theme_rev != Some(rev)
            } else {
                self.last_host_theme_id.as_deref() != Some(at)
                    || self.last_host_theme_profile_slot != Some(slot_opt)
            };
            if theme_changed {
                self.last_host_theme_id = Some(at.to_string());
                self.last_host_theme_profile_slot = Some(slot_opt);
                if let Some(rev) = rev_opt {
                    self.last_host_theme_rev = Some(rev);
                }
                self.local_theme_override = None;
                self.active_theme_id = at.to_string();
                if let Some(slot) = slot_opt {
                    self.active_theme_for_profile = slot;
                    self.has_active_theme_for_profile = !self.active_theme_id.is_empty();
                } else {
                    self.has_active_theme_for_profile = false;
                }
            }
            applied = true;
        }

        if let Some(hw) = val.get("hardwareData") {
            if let Some(c) = hw.get("cpu") {
                if let Some(s) = c.as_str() {
                    self.cpu_str = s.to_string();
                    self.cpu_pct = s.trim_end_matches('%').parse().unwrap_or(0.0);
                } else if let Some(d) = c.as_f64() {
                    self.cpu_pct = d as f32;
                    self.cpu_str = format!("{d:.0}%");
                }
            }
            if let Some(mem) = hw.get("memory").and_then(|v| v.as_f64()) {
                if let Some(tot) = hw
                    .get("totalMemory")
                    .and_then(|v| v.as_f64())
                    .filter(|&t| t > 0.0)
                {
                    self.mem_pct = ((mem / tot) * 100.0) as f32;
                    self.mem_str = format!("{mem:.1}/{tot:.0}G");
                } else {
                    self.mem_str = format!("{mem:.1}G");
                }
            }
            if let Some(nd) = hw.get("netDown").and_then(|v| v.as_str()) {
                self.net_down_str = nd.to_string();
            }
            if let Some(nu) = hw.get("netUp").and_then(|v| v.as_str()) {
                self.net_up_str = nu.to_string();
            }
            if let Some(g) = hw.get("gpuUsage") {
                if let Some(s) = g.as_str() {
                    self.gpu_str = s.to_string();
                    self.gpu_pct = s.trim_end_matches('%').parse().unwrap_or(0.0);
                } else if let Some(d) = g.as_f64() {
                    self.gpu_pct = d as f32;
                    self.gpu_str = format!("{d:.0}%");
                }
            }
            // Missing or null VRAM fields explicitly clear any previously known VRAM sample
            self.vram_used_gb = hw
                .get("vramUsed")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32);
            self.vram_total_gb = hw
                .get("totalVram")
                .and_then(|v| v.as_f64())
                .filter(|&t| t > 0.0)
                .map(|v| v as f32);
            self.gpu_unified = hw
                .get("gpuUnified")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            push_hist(&mut self.cpu_hist, self.cpu_pct);
            push_hist(&mut self.gpu_hist, self.gpu_pct);
            push_hist(&mut self.mem_hist, self.mem_pct);
            applied = true;
        }

        if let Some(mf) = val.get("mediaFile") {
            if let Some(s) = mf.get("title").and_then(|v| v.as_str()) {
                self.media_title = s.to_string();
            }
            if let Some(s) = mf.get("artists").and_then(|v| v.as_str()) {
                self.media_artists = s.to_string();
            }
            if let Some(s) = mf.get("album").and_then(|v| v.as_str()) {
                self.media_album = s.to_string();
            }
            if let Some(n) = mf.get("length").and_then(|v| v.as_u64()) {
                self.media_length_s = n as u32;
            }
            if let Some(n) = mf.get("played").and_then(|v| v.as_u64()) {
                self.media_played_s = n as u32;
            }
            if let Some(b) = mf.get("playing").and_then(|v| v.as_bool()) {
                self.media_playing = b;
            }
            applied = true;
        }

        if let Some(wt) = val.get("weather") {
            if let Some(s) = wt.get("city").and_then(|v| v.as_str()) {
                self.weather_city = s.to_string();
            }
            if let Some(s) = wt.get("condition").and_then(|v| v.as_str()) {
                self.weather_condition = s.to_string();
            }
            if let Some(d) = wt.get("temp").and_then(|v| v.as_f64()) {
                self.weather_temp = d;
            }
            if let Some(d) = wt.get("high").and_then(|v| v.as_f64()) {
                self.weather_high = d;
            }
            if let Some(d) = wt.get("low").and_then(|v| v.as_f64()) {
                self.weather_low = d;
            }
            if let Some(b) = wt.get("ready").and_then(|v| v.as_bool()) {
                self.weather_ready = b;
            }
            if let Some(b) = wt.get("isDay").and_then(|v| v.as_bool()) {
                self.weather_is_day = b;
            }
            // "celcius" is the established wire spelling; "celsius" is also accepted.
            if let Some(b) = wt
                .get("celcius")
                .or_else(|| wt.get("celsius"))
                .and_then(|v| v.as_bool())
            {
                self.weather_celsius = b;
                self.has_live_weather_unit = true;
            }
            if !self.weather_ready {
                self.weather_sunrise.clear();
                self.weather_sunset.clear();
                self.weather_aqi = None;
                self.weather_utc_offset_sec = None;
                self.weather_hourly.clear();
            } else {
                self.weather_sunrise = wt
                    .get("sunrise")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                self.weather_sunset = wt
                    .get("sunset")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                self.weather_aqi = wt.get("aqi").and_then(|v| v.as_u64()).map(|n| n as u32);
                self.weather_utc_offset_sec = wt.get("utcOffsetSec").and_then(|v| v.as_i64());
                self.weather_hourly = wt
                    .get("hourly")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .take(3)
                            .filter_map(|item| {
                                Some(WeatherHourLive {
                                    hour_label: item.get("hour")?.as_str()?.to_string(),
                                    temp: item.get("temp")?.as_f64()?,
                                    condition: item
                                        .get("condition")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("Cloudy")
                                        .to_string(),
                                    is_day: item
                                        .get("isDay")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(true),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            }
            applied = true;
        }

        if let Some(cg) = val
            .get("customGraphData")
            .or_else(|| val.get("genericModule"))
        {
            let tbuf = cg.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let sbuf = cg.get("subtitle").and_then(|v| v.as_str()).unwrap_or("");
            let vbuf = cg.get("value").and_then(|v| v.as_str()).unwrap_or("");
            let prog = cg.get("progress").and_then(|v| v.as_f64());
            let mut slot_opt = cg
                .get("index")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .filter(|&i| i < MAX_TILES);
            if slot_opt.is_none() && !tbuf.is_empty() {
                slot_opt = self
                    .generic_slots
                    .iter()
                    .position(|s| s.active && s.title == tbuf);
            }
            if let Some(si) = slot_opt {
                if !tbuf.is_empty() {
                    self.generic_slots[si].title = tbuf.to_string();
                }
                if !sbuf.is_empty() {
                    self.generic_slots[si].subtitle = sbuf.to_string();
                }
                if !vbuf.is_empty() {
                    self.generic_slots[si].value = vbuf.to_string();
                }
                if let Some(p) = prog {
                    self.generic_slots[si].progress = p;
                }
                self.generic_slots[si].active = true;
            } else {
                if !tbuf.is_empty() {
                    self.generic_title = tbuf.to_string();
                }
                if !sbuf.is_empty() {
                    self.generic_subtitle = sbuf.to_string();
                }
                if !vbuf.is_empty() {
                    self.generic_value = vbuf.to_string();
                }
                if let Some(p) = prog {
                    self.generic_progress = p;
                }
                self.has_generic_live = true;
            }
            applied = true;
        }

        if let Some(pet) = val.get("pet") {
            if let Some(m) = pet.get("mood").and_then(|v| v.as_u64()) {
                self.pet_mood = m as u8;
            }
            if let Some(h) = pet.get("happiness").and_then(|v| v.as_u64()) {
                self.pet_happiness = h as u8;
            }
            applied = true;
        }

        if let Some(pomo) = val.get("pomodoro") {
            let cmd_seq = pomo.get("cmdSeq").and_then(|v| v.as_u64());
            let skip_same_cmd = cmd_seq.is_some() && cmd_seq == Some(self.last_pomo_rev);
            if !skip_same_cmd {
                if let Some(seq) = cmd_seq {
                    self.last_pomo_rev = seq;
                }
                if self.pomo_running && self.pomo_anchor_set {
                    self.tick_pomo(now_ms);
                }
                if let Some(act) = pomo.get("action").and_then(|v| v.as_str()) {
                    let def_s = pomo
                        .get("defaultS")
                        .and_then(|v| v.as_u64())
                        .map(|v| v as u32)
                        .unwrap_or(25 * 60);
                    if act == "toggle" {
                        if !self.has_live_pomo && !self.pomo_running {
                            self.pomo_remaining_s = def_s;
                        }
                        self.pomo_running = !self.pomo_running;
                        self.has_live_pomo = true;
                        if self.pomo_running {
                            if self.pomo_remaining_s == 0 {
                                self.pomo_remaining_s = def_s;
                            }
                            self.pomo_anchor_rem_s = self.pomo_remaining_s;
                            self.pomo_anchor_ms = now_ms;
                            self.pomo_anchor_set = true;
                        } else {
                            self.pomo_anchor_set = false;
                        }
                        self.dev_pomo_rev = self.dev_pomo_rev.saturating_add(1);
                        applied = true;
                    } else if act == "reset" {
                        self.pomo_running = false;
                        self.pomo_remaining_s = def_s;
                        self.pomo_anchor_rem_s = def_s;
                        self.pomo_anchor_set = false;
                        self.has_live_pomo = true;
                        self.dev_pomo_rev = self.dev_pomo_rev.saturating_add(1);
                        applied = true;
                    }
                }
                let rem_v = pomo
                    .get("remainingS")
                    .or_else(|| pomo.get("remaining"))
                    .and_then(|v| v.as_u64());
                if let Some(rem) = rem_v {
                    self.pomo_remaining_s = rem as u32;
                    self.pomo_anchor_rem_s = rem as u32;
                    self.pomo_anchor_ms = now_ms;
                    self.has_live_pomo = true;
                    applied = true;
                }
                if let Some(run) = pomo.get("running").and_then(|v| v.as_bool()) {
                    let was_running = self.pomo_running;
                    self.pomo_running = run;
                    self.has_live_pomo = true;
                    if run {
                        if !was_running || rem_v.is_some() || !self.pomo_anchor_set {
                            self.pomo_anchor_rem_s = self.pomo_remaining_s;
                            self.pomo_anchor_ms = now_ms;
                            self.pomo_anchor_set = true;
                        }
                    } else {
                        self.pomo_anchor_set = false;
                    }
                    applied = true;
                } else if rem_v.is_some() {
                    self.pomo_anchor_set = self.pomo_running;
                }
            } else {
                applied = true;
            }
        }
        if let Some(asleep) = val.get("hostDisplayAsleep").and_then(|v| v.as_bool()) {
            if self.host_display_asleep && !asleep {
                // Host display woke: restart the inactivity timer so the keyboard display wakes with it.
                self.last_key_activity_ms = now_ms;
            }
            self.host_display_asleep = asleep;
            applied = true;
        }
        if let Some(rem) = val.get("pomoRemainingS").and_then(|v| v.as_u64()) {
            self.pomo_remaining_s = rem as u32;
            self.pomo_anchor_rem_s = rem as u32;
            self.pomo_anchor_ms = now_ms;
            self.pomo_anchor_set = self.pomo_running;
            self.has_live_pomo = true;
            applied = true;
        }
        if let Some(run) = val.get("pomoRunning").and_then(|v| v.as_bool()) {
            if self.pomo_running && self.pomo_anchor_set {
                self.tick_pomo(now_ms);
            }
            let was_running = self.pomo_running;
            self.pomo_running = run;
            self.has_live_pomo = true;
            if run {
                if !was_running || !self.pomo_anchor_set {
                    self.pomo_anchor_rem_s = self.pomo_remaining_s;
                    self.pomo_anchor_ms = now_ms;
                    self.pomo_anchor_set = true;
                }
            } else {
                self.pomo_anchor_set = false;
            }
            applied = true;
        }

        if let Some(arr) = val.get("shortcutOverlays").and_then(|v| v.as_array()) {
            let mut next = Vec::new();
            for item in arr {
                let mods = item.get("mods").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
                let Some(kc) = item.get("keyCode").and_then(|v| v.as_str()) else {
                    continue;
                };
                let Some(lbl) = item.get("label").and_then(|v| v.as_str()) else {
                    continue;
                };
                let icon = item
                    .get("icon")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                next.push(ShortcutOverlayEntry {
                    mods,
                    key_code: kc.to_string(),
                    label: lbl.to_string(),
                    icon,
                });
            }
            self.shortcut_overlays = if next.is_empty() {
                default_macos_shortcut_overlays()
            } else {
                next
            };
            applied = true;
        }

        if applied {
            Ok(())
        } else {
            Err("No supported live_data fields".into())
        }
    }
}

fn push_hist(hist: &mut Vec<f32>, val: f32) {
    let clamped = val.clamp(0.0, 100.0);
    if hist.len() < HW_HIST_LEN {
        hist.push(clamped);
    } else {
        hist.remove(0);
        hist.push(clamped);
    }
}

fn parse_style_entry(dst: &mut KeyStyleOverride, obj: &Value) {
    if let Some(s) = obj
        .get("label")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        dst.label = Some(s.to_string());
    }
    if let Some(s) = obj
        .get("iconAssetPath")
        .or_else(|| obj.get("icon"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        dst.icon = Some(s.to_string());
    }
    if let Some(c) = obj.get("legendColor").and_then(|v| v.as_u64()) {
        dst.legend_argb = Some(c as u32);
    }
    if let Some(c) = obj.get("ringGlowColor").and_then(|v| v.as_u64()) {
        dst.ring_argb = Some(c as u32);
    }
}

fn parse_hex_argb(clean: &str) -> Option<u32> {
    let v = u32::from_str_radix(clean, 16).ok()?;
    match clean.len() {
        3 => {
            let r = ((v >> 8) & 0xF) * 0x11;
            let g = ((v >> 4) & 0xF) * 0x11;
            let b = (v & 0xF) * 0x11;
            Some(0xFF00_0000 | (r << 16) | (g << 8) | b)
        }
        4 => {
            let r = ((v >> 12) & 0xF) * 0x11;
            let g = ((v >> 8) & 0xF) * 0x11;
            let b = ((v >> 4) & 0xF) * 0x11;
            let a = (v & 0xF) * 0x11;
            Some((a << 24) | (r << 16) | (g << 8) | b)
        }
        6 => Some(0xFF00_0000 | v),
        8 => Some(v),
        _ => None,
    }
}

fn apply_theme_object(app: &mut AppearanceConfig, theme: &Value) {
    if let Some(s) = theme.get("name").and_then(|v| v.as_str()) {
        app.theme_name = s.to_string();
    }
    if let Some(s) = theme
        .get("fontFamily")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    {
        app.font_family = s.to_string();
    }
    if let Some(d) = theme.get("fontWeight").and_then(|v| v.as_f64()) {
        app.font_weight = d;
    }
    if let Some(b) = theme.get("fontItalic").and_then(|v| v.as_bool()) {
        app.font_italic = b;
    }
    if let Some(s) = theme.get("backgroundImagePath").and_then(|v| v.as_str()) {
        app.bg_path = s.to_string();
    }
    if let Some(sp) = theme
        .get("backgroundShaderParams")
        .and_then(|v| v.as_object())
    {
        if let Some(h) = sp.get("hue").and_then(|v| v.as_f64()) {
            app.shader_hue_deg = h.clamp(-180.0, 180.0);
        }
        if let Some(spd) = sp.get("speed").and_then(|v| v.as_f64()) {
            app.shader_speed = spd.clamp(0.25, 4.0);
        }
    }
    if let Some(ls) = theme.get("lockIndicatorStyle").and_then(|v| v.as_str()) {
        app.lock_indicator_style = match ls {
            "off" | "Off" => LockIndicatorStyle::Off,
            "highlight" | "Highlight" => LockIndicatorStyle::Highlight,
            _ => LockIndicatorStyle::Dot,
        };
    }
    if let Some(lc) = theme.get("lockIndicatorColor") {
        if lc.is_null() {
            app.lock_indicator_argb = None;
        } else if let Some(n) = lc.as_u64() {
            app.lock_indicator_argb = Some(n as u32);
        } else if let Some(s) = lc.as_str() {
            let clean = s.trim_start_matches('#');
            app.lock_indicator_argb = parse_hex_argb(clean);
        }
    }
    if let Some(d) = theme
        .get("backgroundImageBlurAmount")
        .and_then(|v| v.as_f64())
    {
        app.bg_blur = d;
    }
    if let Some(d) = theme.get("backgroundImageDimming").and_then(|v| v.as_f64()) {
        app.bg_dimming = d;
    }
    if let Some(d) = theme
        .get("moduleBackgroundDimming")
        .and_then(|v| v.as_f64())
    {
        app.module_dimming = d;
    }
    if let Some(c) = theme.get("ringGlowColor").and_then(|v| v.as_u64()) {
        app.ring_glow_argb = c as u32;
    }
    if let Some(c) = theme.get("legendColor").and_then(|v| v.as_u64()) {
        app.legend_argb = c as u32;
    }
    if let Some(b) = theme.get("retintIcons").and_then(|v| v.as_bool()) {
        app.retint_icons = b;
    }
    if let Some(b) = theme.get("retintModules").and_then(|v| v.as_bool()) {
        app.retint_modules = b;
    }
    if let Some(b) = theme.get("dropShadow").and_then(|v| v.as_bool()) {
        app.drop_shadow = b;
    }
    if let Some(bm) = theme.get("blendingModePreset").and_then(|v| v.as_str()) {
        app.drop_shadow = bm != "none" && bm != "off";
    }
    if let Some(eff) = theme.get("selectedKeyEffect").and_then(|v| v.as_str()) {
        app.key_effect = match eff {
            "none" | "None" => KeyEffect::None,
            "solid_highlight" | "solidHighlight" | "solid" => KeyEffect::SolidHighlight,
            "ripple" | "Ripple" => KeyEffect::Ripple,
            "autofocus_lock" | "autofocusLock" | "autofocus" => KeyEffect::AutofocusLock,
            "glowing_rings" | "glowingRings" | "rings" => KeyEffect::GlowingRings,
            _ => KeyEffect::None,
        };
    }
    if let Some(sm) = theme.get("stylemaps").and_then(|v| v.as_object()) {
        for (lkey, lval) in sm {
            let Ok(lidx) = lkey.parse::<usize>() else {
                continue;
            };
            let mut layer = app.stylemaps.remove(&lidx).unwrap_or_default();
            if let Some(ds) = lval.get("defaultStyle").filter(|v| v.is_object()) {
                let mut dso = layer.default_style.take().unwrap_or_default();
                parse_style_entry(&mut dso, ds);
                layer.default_style = Some(dso);
            }
            if let Some(styles) = lval.get("styles").and_then(|v| v.as_object()) {
                for (kkey, kval) in styles {
                    let Ok(kidx) = kkey.parse::<usize>() else {
                        continue;
                    };
                    if kval.is_object() {
                        let mut ov = layer.keys.remove(&kidx).unwrap_or_default();
                        parse_style_entry(&mut ov, kval);
                        layer.keys.insert(kidx, ov);
                    }
                }
            }
            app.stylemaps.insert(lidx, layer);
        }
    }
}

impl DisplayConfig {
    pub fn apply_appearance_theme_id(&mut self, target_id: &str) {
        let Some(raw) = self.appearance_raw.clone() else {
            return;
        };
        self.appearance = AppearanceConfig::default();
        if let Some(def) = raw.get("defaultTheme").filter(|v| v.is_object()) {
            apply_theme_object(&mut self.appearance, def);
        }
        let eff = if target_id.is_empty() {
            "default"
        } else {
            target_id
        };
        if eff != "default" {
            if let Some(ct) = raw.get("customThemes").and_then(|v| v.as_array()) {
                if let Some(found) = ct
                    .iter()
                    .find(|t| t.get("id").and_then(|x| x.as_str()) == Some(eff))
                {
                    apply_theme_object(&mut self.appearance, found);
                }
            }
        }
        self.active_theme_id = eff.to_string();
    }

    pub fn load_appearance_json(&mut self, txt: &str) {
        let Ok(val) = serde_json::from_str::<Value>(txt) else {
            return;
        };
        let sel = val
            .get("selectedId")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or("default")
            .to_string();
        self.appearance_selected_id = sel.clone();
        self.appearance_raw = Some(val);
        self.apply_appearance_theme_id(&sel);
    }

    pub fn load_settings_json(&mut self, txt: &str) {
        let Ok(val) = serde_json::from_str::<Value>(txt) else {
            return;
        };
        if let Some(d) = val.get("timeout").and_then(|v| v.as_f64()) {
            self.settings.timeout_sec = d;
        }
        if let Some(b) = val.get("parallaxActive").and_then(|v| v.as_bool()) {
            self.settings.parallax_active = b;
        }
        if let Some(d) = val.get("eyeHeight").and_then(|v| v.as_f64()) {
            self.settings.eye_height_mm = d;
        }
        if let Some(d) = val.get("eyeDistanceFromFront").and_then(|v| v.as_f64()) {
            self.settings.eye_dist_front_mm = d;
        }
        if let Some(d) = val.get("eyeXOffset").and_then(|v| v.as_f64()) {
            self.settings.eye_x_offset_mm = d;
        }
        if let Some(d) = val.get("displayOffsetX").and_then(|v| v.as_f64()) {
            self.settings.display_offset_x_px = d;
        }
        if let Some(d) = val.get("displayOffsetY").and_then(|v| v.as_f64()) {
            self.settings.display_offset_y_px = d;
        }
        if let Some(b) = val.get("showAlignmentGrid").and_then(|v| v.as_bool()) {
            self.settings.show_alignment_grid = b;
        }
        if let Some(mode) = val.get("footStandMode").and_then(|v| v.as_str()) {
            self.settings.foot_stand_deg = match mode {
                "low" => 4.0,
                "high" => 8.0,
                _ => 0.0,
            };
        }
    }

    pub fn load_keymap_json(&mut self, txt: &str) {
        let Ok(val) = serde_json::from_str::<Value>(txt) else {
            return;
        };
        let lay_str = val
            .get("selectedFrameLayoutType")
            .and_then(|v| v.as_str())
            .unwrap_or("ansi");
        self.keymap.selected_layout = if lay_str == "iso" {
            FrameLayoutType::Iso
        } else {
            FrameLayoutType::Ansi
        };
        let key = if self.keymap.selected_layout == FrameLayoutType::Iso {
            "iso"
        } else {
            "ansi"
        };
        if let Some(km_obj) = val
            .get("keymaps")
            .and_then(|k| k.get(key).or_else(|| k.get("ansi")))
        {
            if let Some(dl) = km_obj.get("defaultLayer").and_then(|v| v.as_u64()) {
                self.keymap.default_layer = dl as u8;
            }
            if let Some(layers) = km_obj.get("layers").and_then(|v| v.as_array()) {
                self.keymap.layers.clear();
                for l in layers {
                    let name = l
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Layer")
                        .to_string();
                    let mut actions = Vec::new();
                    if let Some(ka_arr) = l.get("keyActions").and_then(|v| v.as_array()) {
                        for ka in ka_arr {
                            let action_type =
                                ka.get("type").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
                            let target_layer =
                                ka.get("targetLayer").and_then(|v| v.as_u64()).unwrap_or(0) as u8;
                            let key_code = ka
                                .get("keyCode")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let is_momentary = action_type == 2
                                && ka.get("layerControlType").and_then(|v| v.as_str())
                                    == Some("momentaryLayer");
                            let legend = keycode_to_legend(&key_code, action_type, target_layer);
                            actions.push(KeyAction {
                                action_type,
                                target_layer,
                                is_momentary,
                                key_code,
                                legend,
                            });
                        }
                    }
                    self.keymap.layers.push(KeymapLayer { name, actions });
                }
            }
        }
        if self.keymap.layers.is_empty() {
            self.keymap.layers.push(KeymapLayer {
                name: "Base".into(),
                actions: vec![KeyAction::default(); 85],
            });
        }
    }

    pub fn load_mosaic_json(&mut self, txt: &str) {
        let Ok(val) = serde_json::from_str::<Value>(txt) else {
            return;
        };
        if let Some(tt) = val.get("tileTypes").and_then(|v| v.as_array()) {
            for (i, v) in tt.iter().take(MAX_TILES).enumerate() {
                self.mosaic.tiles[i].tile_type = TileType::from_u32(v.as_u64().unwrap_or(0) as u32);
            }
        }
        if let Some(tc) = val.get("tileConfigs").and_then(|v| v.as_array()) {
            for (i, cfg) in tc.iter().take(MAX_TILES).enumerate() {
                let t = &mut self.mosaic.tiles[i];
                if let Some(s) = cfg.get("city").and_then(|v| v.as_str()) {
                    t.city = s.to_string();
                }
                // "celcius" is the established wire spelling; "celsius" is also accepted.
                if let Some(b) = cfg
                    .get("celcius")
                    .or_else(|| cfg.get("celsius"))
                    .and_then(|v| v.as_bool())
                {
                    t.celsius = b;
                }
                if let Some(s) = cfg
                    .get("main")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok())
                {
                    t.pomo_main_min = s;
                } else if let Some(n) = cfg.get("main").and_then(|v| v.as_u64()) {
                    t.pomo_main_min = n as u32;
                }
                if let Some(d) = cfg.get("utcOffsetMinutes").and_then(|v| v.as_f64()) {
                    t.utc_offset_min = d as i32;
                } else if let Some(d) = cfg.get("utcOffsetHours").and_then(|v| v.as_f64()) {
                    t.utc_offset_min = (d * 60.0) as i32;
                }
                if let Some(s) = cfg.get("timezone").and_then(|v| v.as_str()) {
                    t.tz_label = s.to_string();
                }
                if let Some(s) = cfg.get("petName").and_then(|v| v.as_str()) {
                    t.pet_name = s.to_string();
                }
                if let Some(s) = cfg.get("title").and_then(|v| v.as_str()) {
                    t.generic_title = s.to_string();
                }
                if let Some(s) = cfg.get("subtitle").and_then(|v| v.as_str()) {
                    t.generic_subtitle = s.to_string();
                }
                if let Some(s) = cfg.get("value").and_then(|v| v.as_str()) {
                    t.generic_value = s.to_string();
                }
                if let Some(d) = cfg.get("progress").and_then(|v| v.as_f64()) {
                    t.generic_progress = d;
                }
                if let Some(s) = cfg.get("metric").and_then(|v| v.as_str()) {
                    t.graph_metric = s.to_string();
                }
            }
        }
    }

    pub fn load_profiles_json(&mut self, txt: &str) {
        let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(txt) else {
            return;
        };
        self.profile_theme_ids = arr
            .iter()
            .take(64)
            .map(|p| {
                p.get("theme_id")
                    .or_else(|| p.get("themeId"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("default")
                    .to_string()
            })
            .collect();
    }

    fn appearance_mtime_ns(dir: &Path) -> u128 {
        std::fs::metadata(dir.join("appearance.json"))
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|mt| mt.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }

    pub fn load_dir(dir: &Path) -> Self {
        let mut cfg = Self::default();
        if let Ok(s) = std::fs::read_to_string(dir.join("appearance.json")) {
            cfg.load_appearance_json(&s);
            cfg.loaded_appearance_mtime_ns = Self::appearance_mtime_ns(dir);
        }
        if let Ok(ts) = std::fs::read_to_string(dir.join("theme_state.json")) {
            if let Ok(v) = serde_json::from_str::<Value>(&ts) {
                let recorded_ns = v
                    .get("appearanceMtimeNs")
                    .and_then(|x| x.as_str())
                    .and_then(|x| x.parse::<u128>().ok())
                    .unwrap_or(0);
                let disk_ns = Self::appearance_mtime_ns(dir);
                if disk_ns > 0 && recorded_ns == disk_ns {
                    if let Some(sel) = v
                        .get("selectedId")
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        let prof = v.get("profile").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                        let host_theme_id = v
                            .get("hostThemeId")
                            .and_then(|x| x.as_str())
                            .map(|s| s.to_string());
                        let host_profile_slot = v
                            .get("hostProfileSlot")
                            .map(|x| x.as_u64().map(|s| s as u32));
                        let host_theme_rev = v.get("hostThemeRev").and_then(|x| x.as_u64());
                        cfg.appearance_selected_id = sel.to_string();
                        cfg.theme_override = Some(ThemeOverrideState {
                            selected_id: sel.to_string(),
                            profile: prof,
                            host_theme_id,
                            host_profile_slot,
                            host_theme_rev,
                        });
                        cfg.apply_appearance_theme_id(sel);
                    }
                }
            }
        }
        if let Ok(s) = std::fs::read_to_string(dir.join("settings.json")) {
            cfg.load_settings_json(&s);
        }
        if let Ok(s) = std::fs::read_to_string(dir.join("keymap.json")) {
            cfg.load_keymap_json(&s);
        }
        if let Ok(s) = std::fs::read_to_string(dir.join("mosaic.json")) {
            cfg.load_mosaic_json(&s);
        }
        if let Ok(s) = std::fs::read_to_string(dir.join("profiles.json")) {
            cfg.load_profiles_json(&s);
        }
        cfg
    }

    pub fn reconcile_live_theme(&mut self, st: &LiveState) -> bool {
        if self.appearance_raw.is_none() {
            return false;
        }
        let desired = if let Some((ref tid, prof, gen)) = st.local_theme_override {
            if prof == st.active_profile && gen == self.loaded_appearance_mtime_ns {
                tid.clone()
            } else if !st.active_theme_id.is_empty()
                && (!st.has_active_theme_for_profile
                    || st.active_theme_for_profile == st.active_profile)
            {
                st.active_theme_id.clone()
            } else if let Some(tid) = self
                .profile_theme_ids
                .get(st.active_profile as usize)
                .filter(|s| !s.is_empty())
            {
                tid.clone()
            } else if !self.appearance_selected_id.is_empty() {
                self.appearance_selected_id.clone()
            } else {
                "default".into()
            }
        } else if !st.active_theme_id.is_empty()
            && (!st.has_active_theme_for_profile
                || st.active_theme_for_profile == st.active_profile)
        {
            st.active_theme_id.clone()
        } else if let Some(tid) = self
            .profile_theme_ids
            .get(st.active_profile as usize)
            .filter(|s| !s.is_empty())
        {
            tid.clone()
        } else if !self.appearance_selected_id.is_empty() {
            self.appearance_selected_id.clone()
        } else {
            "default".into()
        };
        if desired != self.active_theme_id {
            self.apply_appearance_theme_id(&desired);
            return true;
        }
        false
    }

    pub fn theme_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        if let Some(ref raw) = self.appearance_raw {
            let def_id = raw
                .get("defaultTheme")
                .and_then(|t| t.get("id"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("default")
                .to_string();
            ids.push(def_id);
            if let Some(ct) = raw.get("customThemes").and_then(|v| v.as_array()) {
                for t in ct {
                    if let Some(id) = t
                        .get("id")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        if !ids.iter().any(|existing| existing == id) {
                            ids.push(id.to_string());
                        }
                    }
                }
            }
        }
        if ids.is_empty() {
            ids.push("default".to_string());
        }
        ids
    }

    pub fn cycle_theme(&mut self, delta: i32, st: &mut LiveState, cfg_dir: Option<&Path>) -> bool {
        if delta == 0 {
            return false;
        }
        let ids = self.theme_ids();
        if ids.is_empty() {
            return false;
        }
        let cur = if !self.active_theme_id.is_empty() {
            self.active_theme_id.as_str()
        } else if !self.appearance_selected_id.is_empty() {
            self.appearance_selected_id.as_str()
        } else {
            ids[0].as_str()
        };
        let cur_idx = ids.iter().position(|id| id == cur).unwrap_or(0);
        let len_i = ids.len() as isize;
        let next_idx = ((cur_idx as isize + delta as isize).rem_euclid(len_i)) as usize;
        let next_id = ids[next_idx].clone();
        if next_id == cur && ids.len() <= 1 {
            return false;
        }
        self.apply_appearance_theme_id(&next_id);
        self.appearance_selected_id = next_id.clone();
        self.theme_override = Some(ThemeOverrideState {
            selected_id: next_id.clone(),
            profile: st.active_profile,
            host_theme_id: st.last_host_theme_id.clone(),
            host_profile_slot: st.last_host_theme_profile_slot,
            host_theme_rev: st.last_host_theme_rev,
        });
        st.local_theme_override = Some((
            next_id.clone(),
            st.active_profile,
            self.loaded_appearance_mtime_ns,
        ));
        if let Some(dir) = cfg_dir {
            let updating = dir.join("config.updating").exists()
                || dir.join("config.settings.updating").exists()
                || Path::new(DISPLAY_UPDATING_FLAG_PATH).exists()
                || Path::new(BRIDGE_UPDATING_FLAG_PATH).exists();
            if !updating {
                let mtime_ns = self.loaded_appearance_mtime_ns;
                let host_slot_json = match st.last_host_theme_profile_slot {
                    Some(Some(s)) => Value::from(s),
                    Some(None) => Value::Null,
                    None => Value::Null,
                };
                let mut state_obj = serde_json::Map::new();
                state_obj.insert("selectedId".into(), Value::String(next_id));
                state_obj.insert("profile".into(), Value::from(st.active_profile));
                state_obj.insert(
                    "appearanceMtimeNs".into(),
                    Value::String(mtime_ns.to_string()),
                );
                if let Some(ref hid) = st.last_host_theme_id {
                    state_obj.insert("hostThemeId".into(), Value::String(hid.clone()));
                    if st.last_host_theme_profile_slot.is_some() {
                        state_obj.insert("hostProfileSlot".into(), host_slot_json);
                    }
                }
                if let Some(rev) = st.last_host_theme_rev {
                    state_obj.insert("hostThemeRev".into(), Value::from(rev));
                }
                let state_doc = Value::Object(state_obj);
                if let Ok(serialized) = serde_json::to_string(&state_doc) {
                    let target = dir.join("theme_state.json");
                    let tmp = dir.join("theme_state.json.tmp");
                    if std::fs::write(&tmp, serialized).is_ok()
                        && std::fs::rename(&tmp, &target).is_err()
                    {
                        let _ = std::fs::remove_file(&tmp);
                    }
                }
            }
        }
        true
    }

    pub fn apply_ipc_line(
        &mut self,
        st: &mut LiveState,
        line: &str,
        mono_ms: u64,
        cfg_dir: Option<&Path>,
    ) -> IpcLineOutcome {
        let mut out = IpcLineOutcome::default();
        let pre_profile = st.active_profile;
        if st.apply_live_json(line, mono_ms).is_err() {
            return out;
        }
        out.applied = true;
        if !line.contains("\"key_state\"") && !line.contains("\"data_type\":4") {
            out.is_live_data_payload = true;
        }

        let has_event_profiles = st
            .pending_of_events
            .iter()
            .any(|(_, prof_opt)| prof_opt.is_some());
        if st.has_of_events && has_event_profiles {
            let final_profile = st.active_profile;
            let events = std::mem::take(&mut st.pending_of_events);
            let default_pomo_secs = self
                .mosaic
                .tiles
                .iter()
                .find(|t| t.tile_type == TileType::Pomodoro && t.pomo_main_min > 0)
                .map(|t| t.pomo_main_min * 60)
                .unwrap_or(25 * 60);
            let mut cur_profile = pre_profile;
            st.active_profile = cur_profile;
            for (code, prof_opt) in events {
                if let Some(ep) = prof_opt {
                    if ep != cur_profile {
                        cur_profile = ep;
                        st.active_profile = ep;
                        st.active_theme_id.clear();
                        st.has_active_theme_for_profile = false;
                        st.local_theme_override = None;
                        if self.reconcile_live_theme(st) {
                            out.theme_reconciled = true;
                        }
                    }
                }
                match code {
                    2 => {
                        st.pending_brightness_steps.push(1);
                        out.triggered_action = true;
                    }
                    3 => {
                        st.pending_brightness_steps.push(-1);
                        out.triggered_action = true;
                    }
                    12 => {
                        if st.apply_of_func_code(
                            "KC_OF_START_PAUSE_POMODORO",
                            default_pomo_secs,
                            mono_ms,
                        ) {
                            out.triggered_action = true;
                        }
                    }
                    13 => {
                        if st.apply_of_func_code("KC_OF_RESET_POMODORO", default_pomo_secs, mono_ms)
                        {
                            out.triggered_action = true;
                        }
                    }
                    14 | 15 => {
                        let delta = if code == 14 { 1 } else { -1 };
                        if self.cycle_theme(delta, st, cfg_dir) {
                            out.theme_cycled = true;
                            out.triggered_action = true;
                        }
                    }
                    _ => {}
                }
            }
            if st.active_profile != final_profile {
                st.active_profile = final_profile;
                st.active_theme_id.clear();
                st.has_active_theme_for_profile = false;
                st.local_theme_override = None;
            }
            if self.reconcile_live_theme(st) {
                out.theme_reconciled = true;
            }
            return out;
        }

        if st.handle_key_press_actions(self, mono_ms) {
            out.triggered_action = true;
        }
        if st.pending_theme_delta != 0 {
            let delta = std::mem::take(&mut st.pending_theme_delta);
            if self.cycle_theme(delta, st, cfg_dir) {
                out.theme_cycled = true;
            }
        }
        if self.reconcile_live_theme(st) {
            out.theme_reconciled = true;
        }
        out
    }
}

pub struct ConfigReloader {
    dir: std::path::PathBuf,
    last_stamp: Vec<(bool, u128, u64, u64)>,
    was_updating: bool,
}

impl ConfigReloader {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            last_stamp: Self::stamp(dir),
            was_updating: false,
        }
    }

    fn stamp(dir: &Path) -> Vec<(bool, u128, u64, u64)> {
        let files = [
            "appearance.json",
            "settings.json",
            "keymap.json",
            "mosaic.json",
            "profiles.json",
        ];
        files
            .iter()
            .map(|f| {
                if let Ok(meta) = std::fs::metadata(dir.join(f)) {
                    let ns = meta
                        .modified()
                        .ok()
                        .and_then(|mt| mt.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_nanos())
                        .unwrap_or(0);
                    #[cfg(unix)]
                    let ino = {
                        use std::os::unix::fs::MetadataExt;
                        meta.ino()
                    };
                    #[cfg(not(unix))]
                    let ino = 0u64;
                    (true, ns, meta.len(), ino)
                } else {
                    (false, 0, 0, 0)
                }
            })
            .collect()
    }

    pub fn poll_reload(&mut self, cfg: &mut DisplayConfig) -> bool {
        let updating = self.dir.join("config.updating").exists()
            || self.dir.join("config.settings.updating").exists()
            || Path::new(DISPLAY_UPDATING_FLAG_PATH).exists()
            || Path::new(BRIDGE_UPDATING_FLAG_PATH).exists();
        if updating {
            self.was_updating = true;
            return false;
        }
        let cur_stamp = Self::stamp(&self.dir);
        let should_reload = self.was_updating || cur_stamp != self.last_stamp;
        self.was_updating = false;
        if should_reload {
            *cfg = DisplayConfig::load_dir(&self.dir);
            self.last_stamp = cur_stamp;
            return true;
        }
        false
    }
}
