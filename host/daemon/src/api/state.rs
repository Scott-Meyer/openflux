use crate::api::tls::write_owner_only_file;
use crate::api::worker::UsbWorkerHandle;
use crate::bridges::{AppProfileRule, HardwareMetrics, MediaSnapshot};
use crate::codecs::keymap::decode_keymap_blob;
use crate::preview::PreviewBackend;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Semaphore};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericTileConfig {
    pub id: String,
    pub index: usize,
    pub title: String,
    #[serde(rename = "type")]
    pub tile_type: u8,
    pub length: u32,
    #[serde(rename = "subTitle")]
    pub sub_title: String,
    pub unit: String,
    #[serde(rename = "secondarySubtitle")]
    pub secondary_subtitle: String,
    #[serde(rename = "timeUnit")]
    pub time_unit: String,
    pub color: String,
    pub leading: String,
    pub ending: String,
    #[serde(rename = "currentValue")]
    pub current_value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ShortcutProfileState {
    pub id: String,
    pub name: String,
    pub theme_id: String,
    pub contexts: Vec<String>,
    pub active_context: String,
    #[serde(default)]
    pub actuation_mode_index: usize,
    #[serde(default)]
    pub default_layer: usize,
    #[serde(default)]
    pub shortcut_pack: Option<String>,
    #[serde(default)]
    pub app_matcher: Option<String>,
}

/// Who a bearer token was issued to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TokenKind {
    /// OpenFlux Studio, the tray and the daemon's own command line; may call every route.
    FirstParty,
    /// A third-party app approved by the user; may call only the third-party app API.
    ThirdParty { app: String },
}

/// The user's answer to a third-party registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationDecision {
    Approve,
    Deny,
}

/// A third-party app waiting for the user to approve or deny it.
pub struct PendingRegistration {
    pub app: String,
    pub requested_at_unix: u64,
    pub waiters: Vec<tokio::sync::oneshot::Sender<RegistrationDecision>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiBroadcastEvent {
    pub event: String,
    pub data: serde_json::Value,
}

/// The state the firmware runs with, as the firmware reports it. Function keys on the keyboard
/// change all three without the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirmwareRuntime {
    /// The active profile: its slot in the profile list saved to the keyboard.
    pub active_profile_slot: u32,
    /// The actuation mode in use: its index in the keymap's actuation modes.
    pub actuation_mode_index: u32,
    /// The platform the keys are sent for: 1 Windows, 2 macOS, 3 Linux.
    pub platform_code: u32,
}

impl FirmwareRuntime {
    /// Decodes the firmware's `GET_RUNTIME_STATE` answer: `active_profile`,
    /// `active_actuation_mode` and `platform_code`, each a little-endian `u32`.
    pub fn decode(raw: &[u8]) -> Result<Self, String> {
        let word = |i: usize| -> Result<u32, String> {
            raw.get(i * 4..i * 4 + 4)
                .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .ok_or_else(|| {
                    format!(
                        "The firmware's runtime state is {} bytes, not 12",
                        raw.len()
                    )
                })
        };
        Ok(Self {
            active_profile_slot: word(0)?,
            actuation_mode_index: word(1)?,
            platform_code: word(2)?,
        })
    }
}

pub trait DeviceSink: Send + Sync {
    fn is_connected(&self) -> (bool, bool);
    fn versions(&self) -> serde_json::Value;
    fn send_live_json(&self, payload: &serde_json::Value) -> Result<(), String>;
    /// True when the clean full-duplex priority-multiplexed `OFV2` USB link is active
    /// (bypassing legacy v1 `live_json`, SimpleHDLC, and `CMD_POLL_BUS_FRAMES` polling!).
    fn is_v2_mux_active(&self) -> bool {
        false
    }
    /// Streams a real-time `CH_MEDIA` frame (raw H.264 Annex-B or JPEG) on Priority 1 with zero disk I/O.
    fn send_media_frame(
        &self,
        _stream_name: &str,
        _codec: openflux_bus::MediaCodec,
        _data: &[u8],
        _flags: u8,
    ) -> bool {
        false
    }
    /// Sends one live `openflux_bus::WireFrame` directly to the keyboard over `LIVE_DATA_TYPE_BUS_FRAME`.
    fn send_bus_frame(&self, _frame: &openflux_bus::WireFrame) -> Result<(), String> {
        Err("Bus bridge unsupported on this sink".into())
    }
    /// Drains queued keyboard-originated `WireFrame`s from the keyboard bridge over USB.
    fn poll_bus_frames(&self) -> Result<Vec<openflux_bus::WireFrame>, String> {
        Ok(Vec::new())
    }
    fn set_active_profile(&self, profile_idx: u32) -> Result<(), String>;
    fn set_language(&self, lang_code: u32) -> Result<(), String>;
    fn set_platform(&self, platform_code: u32) -> Result<(), String>;
    /// Reads the firmware's active profile, actuation mode and platform.
    fn read_firmware_runtime(&self) -> Result<FirmwareRuntime, String> {
        Err(
            "The keyboard firmware doesn't report its active profile, actuation mode or platform"
                .into(),
        )
    }
    /// Sets the keyboard's clock to `utc_timestamp` (`YYYY-MM-DD HH:MM:SS`, UTC) and its time zone
    /// to the IANA zone `zone`.
    fn set_system_time(&self, _utc_timestamp: &str, _zone: &str) -> Result<(), String> {
        Ok(())
    }
    fn save_config_dir(&self, save_plan: &serde_json::Value) -> Result<(), String>;
    fn pull_config_dir(&self) -> Result<serde_json::Value, String> {
        Err("Device config pull not supported by this sink".into())
    }
    fn backup_device(&self, _out_dir: &str) -> Result<String, String> {
        Err("Backup not supported by this sink".into())
    }
    fn flash_uf2(&self, _uf2_path: &str) -> Result<String, String> {
        Err("RP2040 UF2 flash requires attached USB device".into())
    }
    fn sideload_ipk(&self, _ipk_path: &str) -> Result<String, String> {
        Err("Package install requires the keyboard bridge".into())
    }
    /// Reads a file on the keyboard through the bridge.
    fn get_file(&self, _path: &str) -> Result<Vec<u8>, String> {
        Err("Keyboard bridge not connected".into())
    }
    /// Writes a file on the keyboard through the bridge.
    fn put_file(&self, _path: &str, _data: &[u8]) -> Result<(), String> {
        Err("Keyboard bridge not connected".into())
    }
    /// Whether a file exists on the keyboard.
    fn stat_file(&self, _path: &str) -> Result<bool, String> {
        Err("Keyboard bridge not connected".into())
    }
    /// The regular files in a directory on the keyboard, with their sizes.
    fn list_files(&self, _dir: &str) -> Result<Vec<(String, u64)>, String> {
        Err("Keyboard bridge not connected".into())
    }
    /// Reads the keymap blob the firmware stores, through the firmware's keymap command.
    fn read_firmware_keymap(&self) -> Result<Vec<u8>, String> {
        Err("Keyboard firmware not connected".into())
    }
    /// Replaces the keymap blob the firmware stores, through the firmware's keymap command.
    fn write_firmware_keymap(&self, _blob: &[u8]) -> Result<(), String> {
        Err("Keyboard firmware not connected".into())
    }
    /// The firmware's press-refinement status (`PRESS_REFINEMENT_STATUS`).
    fn press_refinement_status(&self) -> Result<crate::usb::press_refinement::Status, String> {
        Err("Keyboard firmware not connected".into())
    }
    /// Stores a press-refinement record built by [`crate::usb::press_refinement::build_record`]
    /// (`PUT_PRESS_REFINEMENT`).
    fn put_press_refinement(&self, _record: &[u8]) -> Result<(), String> {
        Err("Keyboard firmware not connected".into())
    }
    /// The press-refinement record the firmware stores (`GET_PRESS_REFINEMENT`).
    fn get_press_refinement(&self) -> Result<Vec<u8>, String> {
        Err("Keyboard firmware not connected".into())
    }
    fn bridge_armed_sentinel_present(&self) -> Result<bool, String> {
        Ok(false)
    }
    fn confirm_bridge_install(&self) -> Result<(), String> {
        Ok(())
    }
    fn read_device_pomodoro_state(&self) -> Option<serde_json::Value> {
        None
    }
    fn read_device_soc_stats(&self) -> Option<serde_json::Value> {
        None
    }
}

#[derive(Default)]
pub struct NoopDeviceSink;
impl DeviceSink for NoopDeviceSink {
    fn is_connected(&self) -> (bool, bool) {
        (false, false)
    }
    fn versions(&self) -> serde_json::Value {
        empty_versions()
    }
    fn send_live_json(&self, _payload: &serde_json::Value) -> Result<(), String> {
        Err("Device not connected".into())
    }
    fn set_active_profile(&self, _profile_idx: u32) -> Result<(), String> {
        Err("Keyboard firmware not connected".into())
    }
    fn set_language(&self, _lang_code: u32) -> Result<(), String> {
        Err("Keyboard firmware not connected".into())
    }
    fn set_platform(&self, _platform_code: u32) -> Result<(), String> {
        Err("Keyboard firmware not connected".into())
    }
    fn save_config_dir(&self, _save_plan: &serde_json::Value) -> Result<(), String> {
        Err("Keyboard bridge not connected".into())
    }
}

/// What the last attempt to read the keyboard's keymap found.
///
/// Whether the keymap is trusted is [`InnerState::keymap_trusted`]; this keeps why it isn't, for
/// `keymapSync` in the device status.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum KeymapPull {
    /// Nothing read since the keyboard attached or its software changed.
    #[default]
    Pending,
    /// `keymap.json` and the firmware keymap agree.
    Ok,
    /// `keymap.json` and the firmware keymap disagree, or `keymap.json` is missing.
    Mismatch(String),
    /// The keymap couldn't be read: the pull failed, or the firmware keymap is missing or
    /// unreadable.
    Error(String),
}

/// Versions object with every component unreported.
pub fn empty_versions() -> serde_json::Value {
    serde_json::json!({"firmware": null, "bridge": null, "display": null, "os": null})
}

pub struct InnerState {
    pub tokens: HashMap<String, TokenKind>,
    pub registration_secret: Option<String>,
    pub pending_registrations: BTreeMap<String, PendingRegistration>,
    pub denied_apps: BTreeSet<String>,
    pub third_party: crate::api::third_party::ThirdPartySettings,
    pub appearance: serde_json::Value,
    pub settings: serde_json::Value,
    pub mosaic: serde_json::Value,
    pub keymap: serde_json::Value,
    pub actuation_modes: serde_json::Value,
    pub appearance_dirty: bool,
    pub settings_dirty: bool,
    pub mosaic_dirty: bool,
    pub keymap_dirty: bool,
    pub profiles_dirty: bool,
    pub appearance_rev: u64,
    pub settings_rev: u64,
    pub mosaic_rev: u64,
    pub keymap_rev: u64,
    pub profiles_rev: u64,
    pub appearance_hydrated: bool,
    pub settings_hydrated: bool,
    pub mosaic_hydrated: bool,
    pub keymap_hydrated: bool,
    /// The keymap blob the keymap document is encoded against; edits re-encode it.
    pub firmware_keymap_blob: Option<Vec<u8>>,
    /// The firmware keymap as last read from (or written to) the keyboard, whether or not
    /// `keymap.json` agreed with it.
    pub device_keymap_blob: Option<Vec<u8>>,
    /// What the last keymap read found.
    pub keymap_pull: KeymapPull,
    /// The last keymap problem written to the log, so each distinct one is logged once.
    pub keymap_pull_logged: Option<String>,
    pub generic_tiles: [Option<GenericTileConfig>; 4],
    pub imported_icons: Vec<String>,
    pub imported_media: Vec<String>,
    pub app_rules: Vec<AppProfileRule>,
    pub detection_override_process: Option<String>,
    pub detection_override_processes: Option<Vec<String>>,
    pub detection_override_layout: Option<String>,
    pub last_client_activity: Option<Instant>,
    pub desired_language_code: Option<u32>,
    pub applied_language_code: Option<u32>,
    pub profiles: Vec<ShortcutProfileState>,
    pub committed_profile_ids: Option<Vec<String>>,
    pub last_effective_profile_binding: Option<(String, String)>,
    /// When the worker next reads the keyboard again while the profile map or the keymap is
    /// still unknown.
    pub next_pull_retry_at: Option<Instant>,
    pub desired_profile_index: usize,
    pub applied_profile_index: Option<usize>,
    /// The profile slot the firmware last acknowledged from the daemon while attached. An active
    /// slot the firmware reports that differs from it was chosen on the keyboard.
    pub applied_profile_slot: Option<u32>,
    /// What the firmware last reported of its runtime state; `None` while it isn't known.
    pub firmware_runtime: Option<FirmwareRuntime>,
    /// The firmware answered the last command sent to it.
    pub firmware_attached: bool,
    /// The platform code the firmware acknowledged since it attached. The host's platform is sent
    /// once per attachment, so a platform chosen on the keyboard stays until it attaches again.
    pub platform_sent: Option<u32>,
    /// AUTO_PROFILE_LOCK: app rules don't switch the profile while set.
    pub auto_profile_lock: bool,
    /// Set when the profile was chosen on the keyboard: app rules don't switch the profile until
    /// the foreground app changes.
    pub app_rules_hold: Option<AppRulesHold>,
    /// `themeRev` of live data: changes only when the host chooses the theme (see
    /// [`InnerState::choose_profile_theme`]), so the display applies `activeThemeId` once per
    /// choice and a theme picked on the keyboard stays until the host chooses again.
    pub theme_rev: u64,
    /// The profile and theme `theme_rev` was given for, as `(profile id, theme id)`.
    pub theme_rev_for: Option<(String, String)>,
    /// The appearance, settings and mosaic documents as the keyboard held them when they were last
    /// read from or written to it, by save-plan key: the base a save compares the cached
    /// documents with (see [`crate::api::keyboard_edits`]).
    pub keyboard_documents: BTreeMap<String, serde_json::Value>,
    /// A theme was picked through the API (or by a profile switch) since `appearance` was last
    /// read from or written to the keyboard: a save writes the cached `selectedId` over the
    /// keyboard's, even when the pick ended on the base's value.
    pub theme_picked: bool,
    pub versions_attempted: bool,
    pub versions_loaded: bool,
    pub cached_connected: (bool, bool),
    pub cached_versions: serde_json::Value,
    pub hardware: HardwareMetrics,
    pub media: MediaSnapshot,
    pub cached_live_data_json: Option<String>,
    pub pomo_running: bool,
    pub pomo_anchor_rem_s: u32,
    pub pomo_anchor_at: Option<Instant>,
    pub pomo_rev: u64,
    pub pomo_cmd_seq: u64,
    pub pomo_expected_cmd_seq: u64,
    pub pomo_ack_baseline_seq: u64,
    pub pomo_last_dev_rev: u64,
    pub pomo_dev_boot_id: Option<String>,
}

/// App rules wait while the foreground app is still the one that was in front when the profile
/// was chosen on the keyboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRulesHold {
    /// The foreground app then, as [`foreground_app`] names it; `None` when none was known.
    pub foreground: Option<String>,
}

/// Names the foreground app for telling app changes apart: the detection override's process when
/// one is set, otherwise the active window's app and process. Window titles don't count, so a
/// new tab or document in the same app isn't a change.
pub fn foreground_app(
    override_process: Option<&str>,
    window: Option<&crate::bridges::ActiveWindowInfo>,
) -> Option<String> {
    if let Some(p) = override_process.filter(|p| !p.is_empty()) {
        return Some(format!("override:{}", p.to_lowercase()));
    }
    window.map(|w| format!("app:{}\u{0}{}", w.app_id, w.process_name))
}

fn unix_micros() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(1)
}

/// The `themeRev` after `rev`: at least the current time in microseconds, so it doesn't repeat a
/// rev a display kept from an earlier daemon run whose store is gone.
fn next_theme_rev(rev: u64) -> u64 {
    rev.wrapping_add(1).max(unix_micros())
}

impl InnerState {
    /// The firmware acknowledged profile slot `slot`, which is the local profile `local_idx` when
    /// the profile list still has it.
    pub fn record_applied_profile(&mut self, local_idx: Option<usize>, slot: u32) {
        self.applied_profile_index = local_idx;
        self.applied_profile_slot = Some(slot);
        self.firmware_attached = true;
    }

    /// A command to the firmware failed: the profile and language are sent again once it
    /// answers. The platform isn't; it was acknowledged for this attachment.
    pub fn record_firmware_unanswered(&mut self) {
        self.firmware_attached = false;
        self.applied_profile_index = None;
        self.applied_language_code = None;
    }

    /// The firmware is gone; everything is sent again when it attaches.
    pub fn record_firmware_detached(&mut self) {
        self.record_firmware_unanswered();
        self.platform_sent = None;
        self.applied_profile_slot = None;
        self.firmware_runtime = None;
    }

    /// The host chose profile `idx`'s theme. An explicit selection (`explicit`: a profile picked in
    /// Studio or through the API, even the one already in effect) always gets a new `theme_rev`;
    /// a profile switch or a profile edit gets one only when the theme differs from the one
    /// `theme_rev` was given for. Returns whether `theme_rev` changed.
    pub fn choose_profile_theme(&mut self, idx: usize, explicit: bool) -> bool {
        let Some(p) = self.profiles.get(idx) else {
            return false;
        };
        let choice = (p.id.clone(), p.theme_id.clone());
        let bump = explicit
            || self
                .theme_rev_for
                .as_ref()
                .is_some_and(|(_, theme)| *theme != choice.1);
        if bump {
            self.theme_rev = next_theme_rev(self.theme_rev);
        }
        self.theme_rev_for = Some(choice);
        bump
    }

    /// The keyboard put profile `idx` in effect: its theme is taken without a new `theme_rev`.
    pub fn adopt_profile_theme(&mut self, idx: usize) {
        if let Some(p) = self.profiles.get(idx) {
            self.theme_rev_for = Some((p.id.clone(), p.theme_id.clone()));
        }
    }

    /// Profiles read from the keyboard replaced the list: the theme `theme_rev` was given for
    /// follows its profile without a new `theme_rev`.
    pub fn adopt_pulled_profile_themes(&mut self) {
        if let Some((pid, theme)) = self.theme_rev_for.as_mut() {
            if let Some(p) = self.profiles.iter().find(|p| p.id == *pid) {
                theme.clone_from(&p.theme_id);
            }
        }
    }

    /// Whether app rules may switch the profile now.
    pub fn app_rules_switch_profiles(&self) -> bool {
        !self.auto_profile_lock && self.app_rules_hold.is_none()
    }

    /// Ends the hold on app rules once the foreground app differs from the one it was set for.
    /// An unknown foreground app changes nothing.
    pub fn observe_foreground_app(&mut self, foreground: Option<&str>) {
        let Some(fg) = foreground else { return };
        if self
            .app_rules_hold
            .as_ref()
            .is_some_and(|h| h.foreground.as_deref() != Some(fg))
        {
            self.app_rules_hold = None;
        }
    }

    /// `autoProfileSwitch` of the device status: `{"locked", "heldUntilAppChange"}`. App rules
    /// switch the profile only while both are false.
    pub fn auto_profile_switch_status(&self) -> serde_json::Value {
        serde_json::json!({
            "locked": self.auto_profile_lock,
            "heldUntilAppChange": self.app_rules_hold.is_some(),
        })
    }

    /// `actuationMode` of the device status: `{"index", "name", "source"}`. `source` is
    /// `keyboard` when the firmware reported the mode, and `profile` when it is the mode of the
    /// applied profile, which the firmware uses until a key on the keyboard changes it. Null when
    /// neither is known.
    pub fn actuation_mode_status(&self) -> serde_json::Value {
        let (index, source) = if let Some(rt) = self.firmware_runtime {
            (rt.actuation_mode_index as usize, "keyboard")
        } else if let Some(p) = self
            .applied_profile_index
            .and_then(|i| self.profiles.get(i))
        {
            (p.actuation_mode_index, "profile")
        } else {
            return serde_json::Value::Null;
        };
        let name = self
            .actuation_modes
            .get(index)
            .and_then(|m| m.get("name"))
            .and_then(|n| n.as_str());
        serde_json::json!({ "index": index, "name": name, "source": source })
    }

    /// `platform` of the device status: `{"code", "source"}`. `source` is `keyboard` when the
    /// firmware reported the platform, and `host` when it is the host's platform the firmware
    /// acknowledged on attaching (a key on the keyboard may have changed it since). Null when
    /// neither is known.
    pub fn platform_status(&self) -> serde_json::Value {
        match (self.firmware_runtime, self.platform_sent) {
            (Some(rt), _) => serde_json::json!({ "code": rt.platform_code, "source": "keyboard" }),
            (None, Some(code)) => serde_json::json!({ "code": code, "source": "host" }),
            (None, None) => serde_json::Value::Null,
        }
    }

    /// The keymap document and the firmware keymap it is encoded against were read from the
    /// keyboard (or written to it) and agree, so the keymap may be edited and saved.
    pub fn keymap_trusted(&self) -> bool {
        self.keymap_hydrated && self.firmware_keymap_blob.is_some()
    }

    /// Forgets what was read of the keyboard's keymap; the next pull reads it again.
    pub fn forget_keymap(&mut self) {
        self.keymap_hydrated = false;
        self.firmware_keymap_blob = None;
        self.device_keymap_blob = None;
        self.keymap_pull = KeymapPull::Pending;
    }

    /// Records what a keymap read found. Each distinct problem is logged once, until a read
    /// succeeds again.
    pub fn record_keymap_pull(&mut self, outcome: KeymapPull) {
        match &outcome {
            KeymapPull::Mismatch(reason) | KeymapPull::Error(reason) => {
                if self.keymap_pull_logged.as_deref() != Some(reason.as_str()) {
                    eprintln!("openfluxd: the keyboard's keymap isn't loaded: {reason}");
                    self.keymap_pull_logged = Some(reason.clone());
                }
            }
            KeymapPull::Ok => self.keymap_pull_logged = None,
            KeymapPull::Pending => {}
        }
        self.keymap_pull = outcome;
    }

    /// `keymapSync` of `GET /v1/device/status`: `{"state", "reason"}`, where `state` is
    /// `disconnected` unless both the bridge and the firmware are attached, `ok` while the keymap
    /// is trusted, `mismatch` or `error` (with `reason`) after a read that failed, and `pulling`
    /// before the first read finishes.
    pub fn keymap_sync(&self) -> serde_json::Value {
        let (state, reason) = if !(self.cached_connected.0 && self.cached_connected.1) {
            ("disconnected", None)
        } else if self.keymap_trusted() {
            ("ok", None)
        } else {
            match &self.keymap_pull {
                KeymapPull::Mismatch(r) => ("mismatch", Some(r.as_str())),
                KeymapPull::Error(r) => ("error", Some(r.as_str())),
                KeymapPull::Pending | KeymapPull::Ok => ("pulling", None),
            }
        };
        serde_json::json!({ "state": state, "reason": reason })
    }
}

/// A pause point for tests: the paused code notifies the first value, then waits until the flag in
/// the second is set.
pub type TestBarrier = (
    Arc<tokio::sync::Notify>,
    Arc<(Mutex<bool>, std::sync::Condvar)>,
);

pub struct ApiState {
    pub inner: Mutex<InnerState>,
    pub events: broadcast::Sender<ApiBroadcastEvent>,
    pub device: Arc<dyn DeviceSink>,
    pub override_ttl: Duration,
    pub icon_store_dir: PathBuf,
    pub token_store_path: Option<PathBuf>,
    pub worker: Mutex<Option<UsbWorkerHandle>>,
    pub ws_sem: Arc<Semaphore>,
    pub preview: Arc<Mutex<Option<PreviewBackend>>>,
    pub preview_sem: Arc<Semaphore>,
    pub preview_epoch: Arc<std::sync::atomic::AtomicU64>,
    pub preview_child: Arc<Mutex<Option<std::process::Child>>>,
    pub preview_last_used: Mutex<Option<Instant>>,
    /// Notified when a preview stream's client falls behind and a frame waits for room. Only the
    /// integration tests listen; they use it to check that a slow client is dropped.
    pub preview_stream_full: Arc<tokio::sync::Notify>,
    /// Test pause points, unset in normal use. The integration tests in `tests/` drive the daemon
    /// through its public API, so these can't be `#[cfg(test)]`: `preview_barrier` holds a
    /// preview render, and `import_barrier` holds a bundle import between staging and commit.
    pub preview_barrier: Mutex<Option<TestBarrier>>,
    pub import_barrier: Mutex<Option<TestBarrier>>,
    pub persist_mutex: Mutex<()>,
    pub weather_bridge: crate::bridges::weather::WeatherBridge,
    pub lifecycle: Arc<crate::lifecycle::Lifecycle>,
    /// Random per process; lets a client tell a restarted daemon from the one it asked to exit.
    pub instance_id: String,
    pub third_party_discovery: Mutex<Option<crate::api::third_party::Discovery>>,
    /// Bumped whenever third-party access may have changed (settings, denials, revoked tokens), so
    /// open third-party connections re-check it.
    pub third_party_access: tokio::sync::watch::Sender<u64>,
    /// How long a third-party registration waits for the user before answering 408, in ms.
    pub registration_timeout_ms: std::sync::atomic::AtomicU64,
    /// Update discovery, keyboard updates and conversion, once configured.
    pub updates: Mutex<Option<Arc<crate::updates::UpdateService>>>,
    /// OpenFlux v2 pub/sub bus, module system, and unified GPU compositor runtime.
    pub v2: Arc<crate::api::v2::V2State>,
}

impl ApiState {
    pub fn new(device: Arc<dyn DeviceSink>, preseeded_token: Option<String>) -> Arc<Self> {
        Self::with_override_ttl(device, preseeded_token, Duration::from_secs(20))
    }

    pub fn with_override_ttl(
        device: Arc<dyn DeviceSink>,
        preseeded_token: Option<String>,
        override_ttl: Duration,
    ) -> Arc<Self> {
        let icon_dir = std::env::var("OPENFLUX_ICON_DIR")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| default_data_dir().join("assets/key_icons"));
        Self::with_dirs(device, preseeded_token, override_ttl, icon_dir)
    }

    pub fn with_dirs(
        device: Arc<dyn DeviceSink>,
        preseeded_token: Option<String>,
        override_ttl: Duration,
        icon_store_dir: PathBuf,
    ) -> Arc<Self> {
        let (tx, _rx) = broadcast::channel(64);
        let mut tokens = HashMap::new();
        if let Some(t) = preseeded_token {
            tokens.insert(t, TokenKind::FirstParty);
        }
        let DefaultDocuments {
            appearance,
            settings,
            mosaic,
            keymap,
            actuation_modes,
        } = default_documents();
        let cached_connected = device.is_connected();

        Arc::new(Self {
            inner: Mutex::new(InnerState {
                tokens,
                registration_secret: None,
                pending_registrations: BTreeMap::new(),
                denied_apps: BTreeSet::new(),
                third_party: Default::default(),
                appearance,
                settings,
                mosaic,
                keymap,
                actuation_modes,
                appearance_dirty: false,
                settings_dirty: false,
                mosaic_dirty: false,
                keymap_dirty: false,
                profiles_dirty: false,
                appearance_rev: 0,
                settings_rev: 0,
                mosaic_rev: 0,
                keymap_rev: 0,
                profiles_rev: 0,
                appearance_hydrated: false,
                settings_hydrated: false,
                mosaic_hydrated: false,
                keymap_hydrated: false,
                firmware_keymap_blob: None,
                device_keymap_blob: None,
                keymap_pull: KeymapPull::Pending,
                keymap_pull_logged: None,
                generic_tiles: [None, None, None, None],
                imported_icons: Vec::new(),
                imported_media: Vec::new(),
                app_rules: Vec::new(),
                detection_override_process: None,
                detection_override_processes: None,
                detection_override_layout: None,
                last_client_activity: None,
                desired_language_code: None,
                applied_language_code: None,
                profiles: default_profiles(),
                committed_profile_ids: None,
                last_effective_profile_binding: Some(("default".into(), "default".into())),
                next_pull_retry_at: None,
                desired_profile_index: 0,
                applied_profile_index: None,
                applied_profile_slot: None,
                firmware_runtime: None,
                firmware_attached: false,
                platform_sent: None,
                auto_profile_lock: false,
                app_rules_hold: None,
                theme_rev: unix_micros(),
                theme_rev_for: None,
                keyboard_documents: BTreeMap::new(),
                theme_picked: false,
                versions_attempted: false,
                versions_loaded: false,
                cached_connected,
                cached_versions: empty_versions(),
                hardware: HardwareMetrics {
                    cpu_pct: 0.0,
                    mem_used_gb: 0.0,
                    mem_total_gb: 0.0,
                    gpu_pct: 0.0,
                    vram_used_gb: None,
                    vram_total_gb: None,
                    gpu_unified_mem: false,
                    net_down: "0 KB/s".into(),
                    net_up: "0 KB/s".into(),
                },
                media: MediaSnapshot::default(),
                cached_live_data_json: None,
                pomo_running: false,
                pomo_anchor_rem_s: 25 * 60,
                pomo_anchor_at: None,
                pomo_rev: 0,
                pomo_cmd_seq: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_micros() as u64)
                    .unwrap_or(1_000_000),
                pomo_expected_cmd_seq: 0,
                pomo_ack_baseline_seq: 0,
                pomo_last_dev_rev: 0,
                pomo_dev_boot_id: None,
            }),
            events: tx,
            device,
            override_ttl,
            icon_store_dir,
            token_store_path: None,
            worker: Mutex::new(None),
            ws_sem: Arc::new(Semaphore::new(16)),
            preview: Arc::new(Mutex::new(None)),
            preview_sem: Arc::new(Semaphore::new(1)),
            preview_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            preview_child: Arc::new(Mutex::new(None)),
            preview_last_used: Mutex::new(None),
            preview_stream_full: Arc::new(tokio::sync::Notify::new()),
            preview_barrier: Mutex::new(None),
            import_barrier: Mutex::new(None),
            persist_mutex: Mutex::new(()),
            weather_bridge: crate::bridges::weather::WeatherBridge::new(),
            lifecycle: crate::lifecycle::Lifecycle::new(),
            instance_id: Self::generate_csprng_token()
                .unwrap_or_default()
                .chars()
                .rev()
                .take(16)
                .collect(),
            third_party_discovery: Mutex::new(None),
            third_party_access: tokio::sync::watch::channel(0).0,
            registration_timeout_ms: std::sync::atomic::AtomicU64::new(120_000),
            updates: Mutex::new(None),
            v2: crate::api::v2::V2State::new(None),
        })
    }

    pub fn with_persistent_stores(
        device: Arc<dyn DeviceSink>,
        preseeded_token: Option<String>,
        override_ttl: Duration,
        icon_store_dir: PathBuf,
        token_store_path: PathBuf,
        registration_secret: String,
    ) -> Arc<Self> {
        let mut st = Self::with_dirs(device, preseeded_token, override_ttl, icon_store_dir);
        let cfg_dir = token_store_path.parent().map(|p| p.to_path_buf());
        {
            let mut g = st.inner.lock().unwrap();
            for (token, kind) in read_token_store(&token_store_path) {
                g.tokens.entry(token).or_insert(kind);
            }
            g.registration_secret = Some(registration_secret);
            if let Some(ref cfg_dir) = cfg_dir {
                if let Ok(raw) = std::fs::read_to_string(cfg_dir.join(DENIED_APPS_FILE)) {
                    if let Ok(apps) = serde_json::from_str::<Vec<String>>(&raw) {
                        g.denied_apps = apps.into_iter().collect();
                    }
                }
                if let Ok(raw_store) = std::fs::read_to_string(cfg_dir.join("profiles_store.json"))
                {
                    if let Ok(doc) = serde_json::from_str::<serde_json::Value>(&raw_store) {
                        if let Some(p_val) = doc.get("profiles") {
                            if let Ok(profs) =
                                serde_json::from_value::<Vec<ShortcutProfileState>>(p_val.clone())
                            {
                                if !profs.is_empty() {
                                    g.profiles = profs;
                                }
                            }
                        }
                        if let Some(r_val) = doc.get("appRules") {
                            if let Ok(rules) =
                                serde_json::from_value::<Vec<AppProfileRule>>(r_val.clone())
                            {
                                g.app_rules = rules;
                            }
                        }
                        if let Some(d) = doc.get("dirty").and_then(|v| v.as_bool()) {
                            g.profiles_dirty = d;
                        }
                        if let Some(di) = doc.get("desiredProfileIndex").and_then(|v| v.as_u64()) {
                            g.desired_profile_index =
                                (di as usize).min(g.profiles.len().saturating_sub(1));
                        }
                        if let Some(l) = doc.get("autoProfileLock").and_then(|v| v.as_bool()) {
                            g.auto_profile_lock = l;
                        }
                        // The stored rev is sent again, so a restart doesn't put the host's
                        // theme back over one picked on the keyboard.
                        if let Some(rev) = doc.get("themeRev").and_then(|v| v.as_u64()) {
                            g.theme_rev = rev;
                            g.theme_rev_for = doc
                                .get("themeRevFor")
                                .and_then(|v| serde_json::from_value(v.clone()).ok());
                        }
                        // committedIds on disk are not trusted: which profiles the keyboard holds is
                        // learned from the keyboard itself once it is attached.
                    }
                }
            }
        }
        Arc::get_mut(&mut st)
            .expect("ApiState is not shared yet")
            .token_store_path = Some(token_store_path);
        st
    }

    /// Adds third-party tokens adopted from another token store, keeping existing entries.
    /// `tokens` pairs each token with the app it was issued to.
    pub fn import_third_party_tokens(
        &self,
        tokens: Vec<(String, String)>,
    ) -> Result<usize, String> {
        let added = {
            let mut g = self.inner.lock().unwrap();
            let mut added = 0;
            for (t, app) in tokens {
                let t = t.trim().to_string();
                if t.is_empty() {
                    continue;
                }
                if let std::collections::hash_map::Entry::Vacant(e) = g.tokens.entry(t) {
                    e.insert(TokenKind::ThirdParty { app });
                    added += 1;
                }
            }
            added
        };
        if added > 0 {
            self.persist_tokens()?;
        }
        Ok(added)
    }

    pub fn registration_timeout(&self) -> Duration {
        Duration::from_millis(
            self.registration_timeout_ms
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    pub fn set_registration_timeout(&self, timeout: Duration) {
        self.registration_timeout_ms.store(
            timeout.as_millis() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    /// Tells open third-party connections to re-check their access.
    pub fn third_party_access_changed(&self) {
        self.third_party_access
            .send_modify(|v| *v = v.wrapping_add(1));
    }

    /// Directory of the persistent stores, when the state has one.
    pub fn config_dir(&self) -> Option<PathBuf> {
        self.token_store_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
    }

    pub fn persist_denied_apps(&self) -> Result<(), String> {
        let Some(cfg_dir) = self.token_store_path.as_ref().and_then(|p| p.parent()) else {
            return Ok(());
        };
        std::fs::create_dir_all(cfg_dir).map_err(|e| e.to_string())?;
        let _persist_guard = self.persist_mutex.lock().unwrap();
        let apps: Vec<String> = self
            .inner
            .lock()
            .unwrap()
            .denied_apps
            .iter()
            .cloned()
            .collect();
        let raw = serde_json::to_vec_pretty(&apps).map_err(|e| e.to_string())?;
        write_owner_only_file(&cfg_dir.join(DENIED_APPS_FILE), &raw, false)
    }

    pub fn persist_profiles(&self) -> Result<(), String> {
        let Some(ref path) = self.token_store_path else {
            return Ok(());
        };
        let Some(cfg_dir) = path.parent() else {
            return Ok(());
        };
        std::fs::create_dir_all(cfg_dir).map_err(|e| e.to_string())?;
        let _persist_guard = self.persist_mutex.lock().unwrap();
        let (profs, rules, dirty, committed_ids, rev, desired_idx, lock, theme_rev, theme_for) = {
            let g = self.inner.lock().unwrap();
            (
                g.profiles.clone(),
                g.app_rules.clone(),
                g.profiles_dirty,
                g.committed_profile_ids.clone(),
                g.profiles_rev,
                g.desired_profile_index,
                g.auto_profile_lock,
                g.theme_rev,
                g.theme_rev_for.clone(),
            )
        };
        let store_doc = serde_json::json!({
            "version": 1,
            "rev": rev,
            "dirty": dirty,
            "desiredProfileIndex": desired_idx,
            "autoProfileLock": lock,
            "themeRev": theme_rev,
            "themeRevFor": theme_for,
            // Diagnostic only: the loader learns which profiles the keyboard holds from the keyboard.
            "committedIds": committed_ids,
            "profiles": profs,
            "appRules": rules,
        });
        let raw_store = serde_json::to_vec_pretty(&store_doc).map_err(|e| e.to_string())?;
        write_owner_only_file(&cfg_dir.join("profiles_store.json"), &raw_store, true)
    }

    pub fn hardware_profile_slot(g: &InnerState, local_idx: usize) -> Option<usize> {
        let pid = g.profiles.get(local_idx)?.id.as_str();
        g.committed_profile_ids
            .as_ref()?
            .iter()
            .position(|cid| cid == pid)
    }

    pub fn record_committed_profiles_from_snapshot(
        g: &mut InnerState,
        snapshot: &serde_json::Value,
    ) {
        if let Some(arr) = snapshot.get("profiles").and_then(|v| v.as_array()) {
            let ids: Vec<String> = arr
                .iter()
                .filter_map(|p| p.get("id").and_then(|x| x.as_str()).map(|s| s.to_string()))
                .collect();
            if !ids.is_empty() {
                g.committed_profile_ids = Some(ids);
                g.applied_profile_index = None;
            }
        }
    }

    /// After a save that wrote `snapshot`, the firmware holds the keymap blob it carried.
    pub fn record_saved_keymap_from_snapshot(g: &mut InnerState, snapshot: &serde_json::Value) {
        if let Some(bytes) = snapshot
            .get("keymapBlobHex")
            .and_then(|v| v.as_str())
            .and_then(|hex| Self::decode_hex_bytes(hex).ok())
        {
            g.device_keymap_blob = Some(bytes);
        }
    }

    pub fn validate_profile_list(profs: &[ShortcutProfileState]) -> Result<(), String> {
        if profs.is_empty() || profs.len() > crate::codecs::keymap::MAX_PROFILES {
            return Err(format!(
                "Profiles must contain 1 to {} profiles",
                crate::codecs::keymap::MAX_PROFILES
            ));
        }
        let mut seen_ids = std::collections::HashSet::new();
        for p in profs {
            if p.id.trim().is_empty() || p.name.trim().is_empty() || !seen_ids.insert(p.id.as_str())
            {
                return Err("Each profile must have a unique non-empty id and name".into());
            }
            if p.default_layer > 31 || u32::try_from(p.actuation_mode_index).is_err() {
                return Err(
                    "Profile default_layer (0..=31) or actuation_mode_index out of range".into(),
                );
            }
        }
        Ok(())
    }

    pub fn verify_profiles_match_settings(
        profs: &[ShortcutProfileState],
        ps_bytes: &[u8],
    ) -> Result<(), String> {
        let dec =
            crate::codecs::keymap::decode_profile_settings(ps_bytes, true).map_err(|e| e.0)?;
        if dec.settings.len() != profs.len() {
            return Err(format!(
                "Profile count mismatch between profiles.json ({}) and the firmware profile settings ({})",
                profs.len(),
                dec.settings.len()
            ));
        }
        for (i, p) in profs.iter().enumerate() {
            let expected_bf = 1u32.checked_shl(p.default_layer as u32).unwrap_or(0);
            if dec.settings[i].actuation_mode_index != p.actuation_mode_index as u32
                || dec.settings[i].default_layer_bitfield != expected_bf
            {
                return Err(format!(
                    "Profile slot {i} ({}) differs between profiles.json and the firmware profile settings",
                    p.id
                ));
            }
        }
        Ok(())
    }

    pub fn invalidate_committed_profiles_on_uncertain_save(
        g: &mut InnerState,
        snapshot: &serde_json::Value,
        err_msg: &str,
    ) {
        if snapshot.get("profiles").is_some()
            && (err_msg.contains("PARTIAL_SAVE_UNKNOWN")
                || err_msg.contains("PARTIAL_SAVE_MISMATCH"))
        {
            g.committed_profile_ids = None;
            g.next_pull_retry_at = None;
            g.applied_profile_index = None;
            g.profiles_dirty = true;
            g.profiles_rev = g.profiles_rev.wrapping_add(1);
        }
    }

    pub fn replace_profiles_and_remap(
        g: &mut InnerState,
        new_profiles: Vec<ShortcutProfileState>,
        mark_dirty: bool,
        sync_matchers: bool,
    ) {
        let old_profs = std::mem::take(&mut g.profiles);
        let old_selected_id = old_profs.get(g.desired_profile_index).map(|p| p.id.clone());
        g.profiles = new_profiles;
        if let Some(ref sid) = old_selected_id {
            if let Some(new_idx) = g.profiles.iter().position(|p| p.id == *sid) {
                g.desired_profile_index = new_idx;
            } else {
                g.desired_profile_index = 0;
            }
        } else {
            g.desired_profile_index = 0;
        }
        g.applied_profile_index = None;

        // App rules follow their profile by stable ID, whether the list came from an edit, an import or a pull.
        let mut remapped_rules: Vec<AppProfileRule> = Vec::new();
        for r in &g.app_rules {
            if let Some(old_p) = old_profs.get(r.profile_index as usize) {
                if let Some((new_idx, new_p)) = g
                    .profiles
                    .iter()
                    .enumerate()
                    .find(|(_, np)| np.id == old_p.id)
                {
                    if sync_matchers {
                        let old_m = old_p.app_matcher.as_deref().map(str::trim).unwrap_or("");
                        let new_m = new_p.app_matcher.as_deref().map(str::trim).unwrap_or("");
                        let was_profile_generated =
                            !old_m.is_empty() && r.matcher == old_m && r.language_code.is_none();
                        if was_profile_generated && new_m != old_m {
                            continue;
                        }
                    }
                    let mut nr = r.clone();
                    nr.profile_index = new_idx as u32;
                    remapped_rules.push(nr);
                }
            }
        }
        if sync_matchers {
            for (idx, p) in g.profiles.iter().enumerate() {
                if let Some(ref m) = p.app_matcher {
                    let mtrim = m.trim();
                    if !mtrim.is_empty()
                        && !remapped_rules
                            .iter()
                            .any(|r| r.profile_index == idx as u32 && r.matcher == mtrim)
                    {
                        remapped_rules.push(AppProfileRule {
                            matcher: mtrim.to_string(),
                            profile_index: idx as u32,
                            language_code: None,
                        });
                    }
                }
            }
        }
        if mark_dirty {
            g.profiles_dirty = true;
            g.profiles_rev = g.profiles_rev.wrapping_add(1);
        }
        g.app_rules = remapped_rules;
    }

    pub fn apply_profile_theme(g: &mut InnerState, idx: usize) {
        let Some(prof) = g.profiles.get(idx).cloned() else {
            return;
        };
        g.last_effective_profile_binding = Some((prof.id.clone(), prof.theme_id.clone()));
        let valid_theme = prof.theme_id == "default"
            || g.appearance
                .get("customThemes")
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter().any(|t| {
                        t.get("id").and_then(|x| x.as_str()) == Some(prof.theme_id.as_str())
                    })
                })
                .unwrap_or(false);
        if valid_theme {
            let cur_sid = g
                .appearance
                .get("selectedId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if cur_sid != prof.theme_id {
                if let Some(aobj) = g.appearance.as_object_mut() {
                    aobj.insert(
                        "selectedId".into(),
                        serde_json::Value::String(prof.theme_id.clone()),
                    );
                    g.theme_picked = true;
                    if g.appearance_hydrated {
                        g.appearance_dirty = true;
                        g.appearance_rev = g.appearance_rev.wrapping_add(1);
                    }
                }
            }
        }
    }

    /// AUTO_PROFILE_LOCK: switches app-rule profile switching off, or back on, and returns whether
    /// it is now locked. See [`Self::set_auto_profile_lock`].
    pub fn toggle_auto_profile_lock(&self) -> bool {
        let locked = self.inner.lock().unwrap().auto_profile_lock;
        self.set_auto_profile_lock(!locked)
    }

    /// Locks or unlocks app-rule profile switching and returns the new state. Locking keeps the
    /// profile the keyboard is on: it becomes the desired profile. Unlocking lets app rules switch
    /// again at once, including after a profile chosen on the keyboard. The lock is kept across
    /// restarts and announced as an `autoProfileLock` event (`{"locked"}`).
    pub fn set_auto_profile_lock(&self, locked: bool) -> bool {
        let changed = {
            let mut g = self.inner.lock().unwrap();
            let changed = g.auto_profile_lock != locked;
            g.auto_profile_lock = locked;
            if locked {
                if let Some(applied) = g.applied_profile_index {
                    Self::apply_profile_selection(&mut g, applied);
                }
            } else {
                g.app_rules_hold = None;
            }
            changed
        };
        if changed {
            if let Err(e) = self.persist_profiles() {
                eprintln!("openfluxd: the profile lock couldn't be stored: {e}");
            }
            self.emit("autoProfileLock", serde_json::json!({ "locked": locked }));
        }
        locked
    }

    pub fn apply_profile_selection(g: &mut InnerState, idx: usize) {
        let clamped = idx.min(g.profiles.len().saturating_sub(1));
        if g.desired_profile_index != clamped {
            g.desired_profile_index = clamped;
            g.profiles_rev = g.profiles_rev.wrapping_add(1);
        }
        Self::apply_profile_theme(g, clamped);
    }

    pub fn persist_tokens(&self) -> Result<(), String> {
        let Some(ref path) = self.token_store_path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let _persist_guard = self.persist_mutex.lock().unwrap();
        let entries: Vec<serde_json::Value> = {
            let g = self.inner.lock().unwrap();
            let mut v: Vec<_> = g
                .tokens
                .iter()
                .map(|(t, k)| {
                    let mut e = serde_json::to_value(k).unwrap_or_else(|_| serde_json::json!({}));
                    e["token"] = serde_json::Value::String(t.clone());
                    e
                })
                .collect();
            v.sort_by(|a, b| a["token"].as_str().cmp(&b["token"].as_str()));
            v
        };
        let doc = serde_json::json!({"version": 1, "tokens": entries});
        let raw = serde_json::to_vec_pretty(&doc).map_err(|e| e.to_string())?;
        write_owner_only_file(path, &raw, true)
    }

    /// Lowercase hex, as save plans and the API carry binary files.
    pub fn encode_hex_bytes(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn decode_hex_bytes(hex: &str) -> Result<Vec<u8>, String> {
        if !hex.is_ascii() || !hex.len().is_multiple_of(2) {
            return Err("Invalid hex string (must be even-length ASCII hex)".into());
        }
        let mut out = Vec::with_capacity(hex.len() / 2);
        for chunk in hex.as_bytes().chunks_exact(2) {
            let s = std::str::from_utf8(chunk).map_err(|_| "Invalid UTF-8 hex".to_string())?;
            let b = u8::from_str_radix(s, 16).map_err(|e| format!("Invalid hex digit: {e}"))?;
            out.push(b);
        }
        Ok(out)
    }

    pub fn validate_firmware_blob(raw: &[u8]) -> Result<(), String> {
        decode_keymap_blob(raw)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn set_worker(&self, handle: UsbWorkerHandle) {
        *self.worker.lock().unwrap() = Some(handle);
    }

    pub fn set_updates(&self, updates: Arc<crate::updates::UpdateService>) {
        *self.updates.lock().unwrap() = Some(updates);
    }

    pub fn updates(&self) -> Option<Arc<crate::updates::UpdateService>> {
        self.updates.lock().unwrap().clone()
    }

    /// True while a job has the keyboard to itself.
    pub fn keyboard_reserved(&self) -> bool {
        self.updates().is_some_and(|u| u.keyboard_reserved())
    }

    /// Whether new keyboard work may start: refused once the daemon is about to exit and while a
    /// job has the keyboard.
    pub fn check_keyboard_free(&self) -> Result<(), crate::updates::WriteRefusal> {
        if let Some(kind) = self.lifecycle.exit_requested() {
            return Err(crate::updates::WriteRefusal {
                code: "busy",
                message: crate::lifecycle::ExitPending(kind).to_string(),
            });
        }
        self.check_keyboard_unreserved()
    }

    /// Refused while a job has the keyboard to itself.
    pub fn check_keyboard_unreserved(&self) -> Result<(), crate::updates::WriteRefusal> {
        if self.keyboard_reserved() {
            return Err(crate::updates::WriteRefusal {
                code: "busy",
                message: "The keyboard is being updated; try again when it finishes".into(),
            });
        }
        Ok(())
    }

    /// Whether ordinary work may write the given keyboard areas now. Refused while a job has the
    /// keyboard, while the keyboard needs conversion, and for areas the keyboard stores in a format
    /// this OpenFlux doesn't fully understand.
    pub fn check_keyboard_write(
        &self,
        areas: &[crate::updates::Area],
    ) -> Result<(), crate::updates::WriteRefusal> {
        self.check_keyboard_free()?;
        match self.updates() {
            Some(u) => u.check_write(areas),
            None => Ok(()),
        }
    }

    pub fn get_worker(&self) -> Option<UsbWorkerHandle> {
        self.worker.lock().unwrap().clone()
    }

    pub fn set_registration_secret(&self, secret: &str) {
        self.inner.lock().unwrap().registration_secret = Some(secret.to_string());
    }

    pub fn generate_csprng_token() -> Result<String, String> {
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes).map_err(|e| format!("CSPRNG error: {e}"))?;
        let mut hex = String::with_capacity(73);
        hex.push_str("openflux_");
        for b in bytes {
            hex.push_str(&format!("{b:02x}"));
        }
        Ok(hex)
    }

    pub fn emit(&self, event: &str, data: serde_json::Value) {
        let _ = self.events.send(ApiBroadcastEvent {
            event: event.to_string(),
            data,
        });
    }

    pub fn prune_expired_overrides_locked(guard: &mut InnerState, override_ttl: Duration) {
        if let Some(last) = guard.last_client_activity {
            if last.elapsed() >= override_ttl {
                guard.detection_override_process = None;
                guard.detection_override_processes = None;
                guard.detection_override_layout = None;
                guard.last_client_activity = None;
            }
        }
    }

    pub fn prune_expired_overrides(&self) {
        let mut g = self.inner.lock().unwrap();
        Self::prune_expired_overrides_locked(&mut g, self.override_ttl);
    }

    pub fn is_trusted_origin(origin: Option<&str>) -> bool {
        let Some(orig) = origin else {
            return true;
        };
        let orig = orig.trim();
        if orig.is_empty() || orig.eq_ignore_ascii_case("null") {
            return false;
        }
        let Some((scheme, rest)) = orig.split_once("://") else {
            return false;
        };
        let scheme_low = scheme.to_ascii_lowercase();
        if scheme_low != "http" && scheme_low != "https" && scheme_low != "tauri" {
            return false;
        }
        if rest.is_empty()
            || rest.contains('@')
            || rest.contains('/')
            || rest.contains('?')
            || rest.contains('#')
            || rest.chars().any(char::is_whitespace)
        {
            return false;
        }
        let (host, port_opt) = if let Some(after_open) = rest.strip_prefix('[') {
            let Some((h_inner, tail)) = after_open.split_once(']') else {
                return false;
            };
            let port_opt = if tail.is_empty() {
                None
            } else if let Some(p) = tail.strip_prefix(':') {
                Some(p)
            } else {
                return false;
            };
            (format!("[{h_inner}]"), port_opt)
        } else if let Some((h, p)) = rest.split_once(':') {
            (h.to_string(), Some(p))
        } else {
            (rest.to_string(), None)
        };
        if let Some(p) = port_opt {
            if p.is_empty() || p.parse::<u16>().is_err() {
                return false;
            }
        }
        let host_low = host.to_ascii_lowercase();
        match scheme_low.as_str() {
            "tauri" => host_low == "localhost",
            "http" | "https" => matches!(
                host_low.as_str(),
                "localhost" | "127.0.0.1" | "[::1]" | "tauri.localhost"
            ),
            _ => false,
        }
    }

    /// Issues and persists a new token.
    pub fn issue_token(&self, kind: TokenKind) -> Result<String, String> {
        let tok = Self::generate_csprng_token()?;
        self.inner.lock().unwrap().tokens.insert(tok.clone(), kind);
        self.persist_tokens()?;
        Ok(tok)
    }

    /// Checks a bearer token and returns who it was issued to.
    pub fn verify_bearer(
        &self,
        auth_header: Option<&str>,
    ) -> Result<TokenKind, (u16, &'static str)> {
        let Some(raw) = auth_header else {
            return Err((401, "Missing or invalid Authorization header"));
        };
        let Some(tok) = raw.strip_prefix("Bearer ") else {
            return Err((401, "Missing or invalid Authorization header"));
        };
        let tok = tok.trim();
        if tok.is_empty() {
            return Err((401, "Empty bearer token"));
        }
        let mut guard = self.inner.lock().unwrap();
        let Some(kind) = guard.tokens.get(tok).cloned() else {
            return Err((401, "Invalid token"));
        };
        Self::prune_expired_overrides_locked(&mut guard, self.override_ttl);
        if guard.last_client_activity.is_some() {
            guard.last_client_activity = Some(Instant::now());
        }
        Ok(kind)
    }
}

const DENIED_APPS_FILE: &str = "third_party_denied.json";

/// The daemon's data directory: `OPENFLUX_DATA_DIR`, else `%LOCALAPPDATA%\openflux` on Windows and
/// `~/.local/share/openflux` elsewhere.
pub fn default_data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("OPENFLUX_DATA_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    #[cfg(windows)]
    {
        if let Some(d) = std::env::var_os("LOCALAPPDATA").filter(|d| !d.is_empty()) {
            return PathBuf::from(d).join("openflux");
        }
        if let Some(d) = std::env::var_os("USERPROFILE").filter(|d| !d.is_empty()) {
            return PathBuf::from(d)
                .join("AppData")
                .join("Local")
                .join("openflux");
        }
    }
    match std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        Some(home) => PathBuf::from(home).join(".local/share/openflux"),
        #[cfg(unix)]
        None => std::env::temp_dir().join(format!("openflux_{}", unsafe { libc::geteuid() })),
        #[cfg(not(unix))]
        None => std::env::temp_dir().join("openflux"),
    }
}

/// Reads the token store: `{"version":1,"tokens":[{"token":..,"kind":..}]}`.
fn read_token_store(path: &std::path::Path) -> Vec<(String, TokenKind)> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Vec::new();
    };
    doc.get("tokens")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    let tok = e.get("token")?.as_str()?.to_string();
                    let kind = serde_json::from_value::<TokenKind>(e.clone()).ok()?;
                    Some((tok, kind))
                })
                .collect()
        })
        .unwrap_or_default()
}

struct DefaultDocuments {
    appearance: serde_json::Value,
    settings: serde_json::Value,
    mosaic: serde_json::Value,
    keymap: serde_json::Value,
    actuation_modes: serde_json::Value,
}

fn default_documents() -> DefaultDocuments {
    let defaults = crate::defaults::default_config();
    let mut appearance = defaults
        .get("appearance")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = appearance.as_object_mut() {
        if !obj
            .get("customThemes")
            .map(|v| v.is_array())
            .unwrap_or(false)
        {
            obj.insert("customThemes".into(), serde_json::json!([]));
        }
    }
    DefaultDocuments {
        appearance,
        settings: defaults
            .get("settings")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
        mosaic: defaults
            .get("mosaic")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({})),
        keymap: crate::defaults::default_keymap(),
        actuation_modes: defaults
            .get("actuationModes")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    }
}

/// A general profile, and one for VS Code with its own theme and shortcut pack.
fn default_profiles() -> Vec<ShortcutProfileState> {
    vec![
        ShortcutProfileState {
            id: "default".into(),
            name: "Default".into(),
            theme_id: "default".into(),
            contexts: vec!["Main".into()],
            active_context: "Main".into(),
            actuation_mode_index: 0,
            default_layer: 0,
            shortcut_pack: None,
            app_matcher: None,
        },
        ShortcutProfileState {
            id: "coding".into(),
            name: "VS Code".into(),
            theme_id: "theme_coding".into(),
            contexts: vec!["Editor".into(), "Debug".into(), "Terminal".into()],
            active_context: "Editor".into(),
            actuation_mode_index: 0,
            default_layer: 0,
            shortcut_pack: Some("Visual%20Studio%20Code.csv".into()),
            app_matcher: Some("com.microsoft.VSCode".into()),
        },
    ]
}

/// The default configuration in the shape of `GET /v1/config`.
pub fn default_config_snapshot() -> serde_json::Value {
    let d = default_documents();
    serde_json::json!({
        "appearance": d.appearance,
        "settings": d.settings,
        "mosaic": d.mosaic,
        "keymap": d.keymap,
        "actuationModes": d.actuation_modes,
        "profiles": default_profiles(),
        "appRules": Vec::<AppProfileRule>::new(),
        "desiredProfileIndex": 0,
    })
}

#[cfg(test)]
mod tests {
    use super::FirmwareRuntime;

    #[test]
    fn firmware_runtime_decodes_three_little_endian_words() {
        let mut raw = Vec::new();
        for w in [2u32, 1, 3] {
            raw.extend_from_slice(&w.to_le_bytes());
        }
        assert_eq!(
            FirmwareRuntime::decode(&raw),
            Ok(FirmwareRuntime {
                active_profile_slot: 2,
                actuation_mode_index: 1,
                platform_code: 3,
            })
        );
        assert!(FirmwareRuntime::decode(&raw[..8]).is_err());
    }
}
