//! Vial Raw HID (`0xFF60:0x61`) relay and dynamic `vial.json` / `module_actions.json` builder.
//!
//! Keeps `openfluxd` as the sole USB/HID owner on the host so Studio's embedded `vialite` tab
//! works identically in macOS WKWebView (no WebHID), Windows WebView2, and Linux WebKitGTK.

use crate::usb::protocol::{FIRMWARE_PID, KEYBOARD_VID};
use openflux_modules::{ModuleManifest, ModuleRegistry};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

pub const VIAL_USAGE_PAGE: u16 = 0xFF60;
pub const VIAL_USAGE: u16 = 0x61;
pub const VIAL_RAW_EPSIZE: usize = 32;
pub const VIAL_KEYCODE_BASE: u16 = 0x7E00;
/// Action slots; a slot's keycode is always `0x7E00 + slot`.
/// - `0..13`  (`0x7E00..0x7E0D`, QK_KB): 14 built-in OpenFlux actions
/// - `14..38` (`0x7E0E..0x7E26`, QK_KB): 25 sticky module action slots
/// - `39..63` (`0x7E27..0x7E3F`, QK_KB): 25 fixed Hall-effect Analog Gamepad keycodes (`GP_*`)
/// - `64..94` (`0x7E40..0x7E5E`, QK_USER_0..30): 31 more module action slots (any firmware 2.x).
/// - `95` (`0x7E5F`, QK_USER_31) is the firmware's macOS Globe key, not an action.
/// - `96..511` (`0x7E60..0x7FFF`): the rest of QK_USER, 416 more module action slots, forwarded
///   by firmware 2.1.6+. Allocated last, so older firmware only misses them past 56 actions.
/// Vial only names the 64 QK_KB codes in `customKeycodes`; external Vial apps show QK_USER
/// codes as raw numbers, while Studio and the keycaps label them.
pub const VIAL_MAX_CUSTOM_SLOTS: usize = 64;
pub const VIAL_BUILTIN_SLOTS: usize = 14;
pub const VIAL_GAMEPAD_SLOT_START: usize = 39;
pub const VIAL_GAMEPAD_SLOT_COUNT: usize = 25;
pub const VIAL_USER_SLOT_START: usize = 64;
pub const VIAL_USER_SLOT_COUNT: usize = 31;
pub const VIAL_GLOBE_SLOT: u16 = 95;
pub const VIAL_EXTENDED_SLOT_START: usize = 96;
pub const VIAL_EXTENDED_SLOT_END: usize = 512; // exclusive; slot 511 = 0x7FFF = QK_USER_MAX
/// First firmware that forwards slots `96..511`.
pub const EXTENDED_SLOTS_MIN_FIRMWARE: &str = "2.1.6";

/// Every slot a module action may occupy, in allocation order.
fn module_action_slots() -> impl Iterator<Item = u16> + Clone {
    (VIAL_BUILTIN_SLOTS as u16..VIAL_GAMEPAD_SLOT_START as u16)
        .chain(VIAL_USER_SLOT_START as u16..(VIAL_USER_SLOT_START + VIAL_USER_SLOT_COUNT) as u16)
        .chain(VIAL_EXTENDED_SLOT_START as u16..VIAL_EXTENDED_SLOT_END as u16)
}

/// The firmware version a slot needs, when it's newer than every 2.x firmware.
pub fn slot_requires_firmware(slot: u16) -> Option<&'static str> {
    (slot as usize >= VIAL_EXTENDED_SLOT_START).then_some(EXTENDED_SLOTS_MIN_FIRMWARE)
}

fn is_module_action_slot(slot: u16) -> bool {
    module_action_slots().any(|s| s == slot)
}

pub const MODULE_ACTION_SLOT_COUNT: usize = (VIAL_GAMEPAD_SLOT_START - VIAL_BUILTIN_SLOTS)
    + VIAL_USER_SLOT_COUNT
    + (VIAL_EXTENDED_SLOT_END - VIAL_EXTENDED_SLOT_START);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VialStatus {
    pub connected: bool,
    pub product_name: String,
    pub vendor_id: u16,
    pub product_id: u16,
}

/// Thread-safe relay to the keyboard's Vial Raw HID interface (`0xFF60:0x61`, 32-byte IN/OUT).
pub struct VialHidRelay {
    inner: Mutex<RelayInner>,
}

struct RelayInner {
    api: Option<hidapi::HidApi>,
    dev: Option<hidapi::HidDevice>,
    product_name: String,
}

impl Default for VialHidRelay {
    fn default() -> Self {
        Self::new()
    }
}

impl VialHidRelay {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RelayInner {
                api: None,
                dev: None,
                product_name: "OpenFlux Keyboard".to_string(),
            }),
        }
    }

    fn ensure_api(inner: &mut RelayInner) -> Result<&mut hidapi::HidApi, String> {
        if inner.api.is_none() {
            let api = hidapi::HidApi::new()
                .map_err(|e| format!("HID library unavailable: {e}"))?;
            inner.api = Some(api);
        }
        Ok(inner.api.as_mut().expect("just initialized"))
    }

    fn find_and_open(inner: &mut RelayInner) -> Result<(), String> {
        let api = Self::ensure_api(inner)?;
        api.reset_devices()
            .and_then(|()| api.add_devices(KEYBOARD_VID, FIRMWARE_PID))
            .map_err(|e| format!("HID enumerate failed: {e}"))?;

        let mut target = None;
        for info in api.device_list() {
            if info.vendor_id() == KEYBOARD_VID
                && info.product_id() == FIRMWARE_PID
                && info.usage_page() == VIAL_USAGE_PAGE
                && info.usage() == VIAL_USAGE
            {
                target = Some(info);
                break;
            }
        }
        let Some(info) = target else {
            inner.dev = None;
            return Err("Vial Raw HID interface (0xFF60:0x61) not found".to_string());
        };
        let prod = info
            .product_string()
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "OpenFlux Keyboard".to_string());
        let dev = info
            .open_device(api)
            .map_err(|e| format!("Failed to open Vial Raw HID: {e}"))?;
        inner.product_name = prod;
        inner.dev = Some(dev);
        Ok(())
    }

    pub fn status(&self) -> VialStatus {
        let Ok(mut inner) = self.inner.lock() else {
            return VialStatus {
                connected: false,
                product_name: "OpenFlux Keyboard".to_string(),
                vendor_id: KEYBOARD_VID,
                product_id: FIRMWARE_PID,
            };
        };
        if inner.dev.is_none() {
            let _ = Self::find_and_open(&mut inner);
        }
        VialStatus {
            connected: inner.dev.is_some(),
            product_name: inner.product_name.clone(),
            vendor_id: KEYBOARD_VID,
            product_id: FIRMWARE_PID,
        }
    }

    /// Exchange one 32-byte Vial Raw HID command/response packet with the keyboard.
    pub fn xfer(&self, cmd: &[u8; VIAL_RAW_EPSIZE]) -> Result<[u8; VIAL_RAW_EPSIZE], String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "Vial HID mutex poisoned".to_string())?;

        for attempt in 0..2 {
            if inner.dev.is_none() {
                Self::find_and_open(&mut inner)?;
            }
            let dev = inner.dev.as_ref().expect("opened above");
            // hidapi requires a leading 0x00 report-ID byte for collections without Report IDs.
            let mut tx = [0u8; VIAL_RAW_EPSIZE + 1];
            tx[0] = 0x00;
            tx[1..].copy_from_slice(cmd);
            if let Err(e) = dev.write(&tx) {
                inner.dev = None;
                if attempt == 0 {
                    continue;
                }
                return Err(format!("Vial HID write failed: {e}"));
            }
            let mut rx = [0u8; VIAL_RAW_EPSIZE];
            match dev.read_timeout(&mut rx, 500) {
                Ok(n) if n > 0 => return Ok(rx),
                Ok(_) => {
                    inner.dev = None;
                    if attempt == 0 {
                        continue;
                    }
                    return Err("Vial HID read timed out".to_string());
                }
                Err(e) => {
                    inner.dev = None;
                    if attempt == 0 {
                        continue;
                    }
                    return Err(format!("Vial HID read failed: {e}"));
                }
            }
        }
        Err("Vial HID transfer failed".to_string())
    }
}

/// Sticky slot assignment entry persisted in `module_actions.json` (both host and Linux SoC)
/// and published on the retained `system/actions` bus topic.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionSlotEntry {
    pub action_id: String,
    pub module_id: String,
    #[serde(default)]
    pub keycode: u16,
    pub name: String,
    pub short_name: String,
    #[serde(default)]
    pub short_label: Option<String>,
    pub title: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub accent: Option<String>,
    #[serde(default = "default_tap_mode")]
    pub mode: String,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub release_payload: Option<Value>,
    #[serde(default)]
    pub retain: bool,
    #[serde(default = "default_true")]
    pub active: bool,
}

fn default_true() -> bool {
    true
}

fn default_tap_mode() -> String {
    "tap".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModuleActionsMap {
    /// Simple `slot_str -> action_id` view (fast lookup on keyboard SoC).
    #[serde(default)]
    pub slots: BTreeMap<String, String>,
    /// Rich per-slot metadata including target bus topic/payload and Vial labels.
    #[serde(default)]
    pub entries: BTreeMap<u16, ActionSlotEntry>,
    /// Actions from the last reconcile that did not get a slot, and why.
    #[serde(skip)]
    pub unassigned: Vec<UnassignedAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnassignedAction {
    pub action_id: String,
    pub module_id: String,
    pub label: String,
    /// `"no_free_slot"` or `"invalid"`.
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Runtime actions declared by enabled `dynamic_actions` modules, keyed by module id.
pub type DeclaredActions = BTreeMap<String, Vec<openflux_modules::ModuleActionField>>;

const BUILTINS: [(&str, &str, &str, &str); VIAL_BUILTIN_SLOTS] = [
    ("openflux.brightness_up", "Screen\nBri+", "OF_BRI+", "Increase keyboard display brightness"),
    ("openflux.brightness_down", "Screen\nBri-", "OF_BRI-", "Decrease keyboard display brightness"),
    ("openflux.profile_next", "Profile\nNext", "KG_PRF+", "Switch to next Keyglass profile"),
    ("openflux.profile_prev", "Profile\nPrev", "KG_PRF-", "Switch to previous Keyglass profile"),
    ("openflux.actuation_next", "Act\nNext", "OF_ACT+", "Cycle to next actuation mode"),
    ("openflux.actuation_prev", "Act\nPrev", "OF_ACT-", "Cycle to previous actuation mode"),
    ("openflux.platform_linux", "Linux", "OF_LNX", "Switch platform mode to Linux"),
    ("openflux.platform_macos", "macOS", "OF_MAC", "Switch platform mode to macOS"),
    ("openflux.platform_windows", "Windows", "OF_WIN", "Switch platform mode to Windows"),
    ("openflux.auto_profile_lock", "Profile\nLock", "OF_LOCK", "Toggle automatic profile switching lock"),
    ("widget-pomodoro.toggle", "Pomo\nStart", "OF_POMO", "Start or pause Pomodoro timer"),
    ("widget-pomodoro.reset", "Pomo\nReset", "OF_PRST", "Reset Pomodoro timer"),
    ("openflux.theme_next", "Theme\nNext", "OF_THM+", "Switch to next display theme"),
    ("openflux.theme_prev", "Theme\nPrev", "OF_THM-", "Switch to previous display theme"),
];

const GAMEPAD_SLOTS: [(&str, &str, &str, &str); VIAL_GAMEPAD_SLOT_COUNT] = [
    ("gamepad.ls_up", "LS\nUp", "GP_LS↑", "Analog Gamepad — Left Stick Up"),
    ("gamepad.ls_down", "LS\nDown", "GP_LS↓", "Analog Gamepad — Left Stick Down"),
    ("gamepad.ls_left", "LS\nLeft", "GP_LS←", "Analog Gamepad — Left Stick Left"),
    ("gamepad.ls_right", "LS\nRight", "GP_LS→", "Analog Gamepad — Left Stick Right"),
    ("gamepad.rs_up", "RS\nUp", "GP_RS↑", "Analog Gamepad — Right Stick Up"),
    ("gamepad.rs_down", "RS\nDown", "GP_RS↓", "Analog Gamepad — Right Stick Down"),
    ("gamepad.rs_left", "RS\nLeft", "GP_RS←", "Analog Gamepad — Right Stick Left"),
    ("gamepad.rs_right", "RS\nRight", "GP_RS→", "Analog Gamepad — Right Stick Right"),
    ("gamepad.lt", "Trigger\nLT", "GP_LT", "Analog Gamepad — Left Trigger (Analog)"),
    ("gamepad.rt", "Trigger\nRT", "GP_RT", "Analog Gamepad — Right Trigger (Analog)"),
    ("gamepad.a", "GP\nA", "GP_A", "Analog Gamepad — A Button"),
    ("gamepad.b", "GP\nB", "GP_B", "Analog Gamepad — B Button"),
    ("gamepad.x", "GP\nX", "GP_X", "Analog Gamepad — X Button"),
    ("gamepad.y", "GP\nY", "GP_Y", "Analog Gamepad — Y Button"),
    ("gamepad.lb", "GP\nLB", "GP_LB", "Analog Gamepad — Left Bumper"),
    ("gamepad.rb", "GP\nRB", "GP_RB", "Analog Gamepad — Right Bumper"),
    ("gamepad.l3", "GP\nL3", "GP_L3", "Analog Gamepad — Left Stick Click"),
    ("gamepad.r3", "GP\nR3", "GP_R3", "Analog Gamepad — Right Stick Click"),
    ("gamepad.dpad_up", "D-Pad\nUp", "GP_D↑", "Analog Gamepad — D-Pad Up"),
    ("gamepad.dpad_down", "D-Pad\nDown", "GP_D↓", "Analog Gamepad — D-Pad Down"),
    ("gamepad.dpad_left", "D-Pad\nLeft", "GP_D←", "Analog Gamepad — D-Pad Left"),
    ("gamepad.dpad_right", "D-Pad\nRight", "GP_D→", "Analog Gamepad — D-Pad Right"),
    ("gamepad.start", "GP\nStart", "GP_STA", "Analog Gamepad — Start / Menu"),
    ("gamepad.back", "GP\nBack", "GP_BCK", "Analog Gamepad — Back / View"),
    ("gamepad.guide", "GP\nGuide", "GP_GUI", "Analog Gamepad — Guide / Home"),
];

fn placeholder_entry(action_id: &str, module_id: &str, slot: u16) -> ActionSlotEntry {
    ActionSlotEntry {
        action_id: action_id.to_string(),
        module_id: module_id.to_string(),
        keycode: VIAL_KEYCODE_BASE + slot,
        name: String::new(),
        short_name: String::new(),
        short_label: None,
        title: String::new(),
        icon: None,
        accent: None,
        mode: default_tap_mode(),
        topic: None,
        payload: Value::Null,
        release_payload: None,
        retain: false,
        active: true,
    }
}

fn validate_action(act: &openflux_modules::ModuleActionField) -> Result<(), String> {
    if act.id.is_empty()
        || act.id.len() > 64
        || !act.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("action id {:?} must be 1-64 chars of A-Z, a-z, 0-9, '-' or '_'", act.id));
    }
    openflux_bus::validate_topic(&act.topic).map_err(|e| format!("action topic {:?}: {e}", act.topic))
}

fn make_short_name(module_id: &str, action_id: &str) -> String {
    let mut base = module_id;
    for prefix in ["widget-", "ui-", "fx-", "bg-", "companion-", "dock-", "keys-"] {
        if let Some(rest) = base.strip_prefix(prefix) {
            base = rest;
            break;
        }
    }
    let raw = format!(
        "{}_{}",
        base.chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(4)
            .collect::<String>(),
        action_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(4)
            .collect::<String>()
    );
    raw.to_ascii_uppercase()
}

fn format_keycap_name(label: &str) -> String {
    let words: Vec<&str> = label.split_whitespace().collect();
    if words.len() >= 2 {
        format!(
            "{}\n{}",
            words[0].chars().take(8).collect::<String>(),
            words[1].chars().take(8).collect::<String>()
        )
    } else {
        label.chars().take(10).collect()
    }
}

impl ModuleActionsMap {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Module action slots currently held by an enabled module's action.
    pub fn module_slots_used(&self) -> usize {
        self.entries
            .iter()
            .filter(|(&s, e)| is_module_action_slot(s) && e.active)
            .count()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())
    }

    /// Reconcile enabled modules' `[[actions]]`, plus the runtime actions declared by
    /// `dynamic_actions` modules, into sticky module action slots without shifting previously
    /// assigned slots. Actions that don't fit are listed in `unassigned`.
    pub fn reconcile_from_manifests(&mut self, enabled_manifests: &[ModuleManifest], declared: &DeclaredActions) {
        // Ensure built-ins 0..13 always occupy their fixed slots.
        for (i, (id, name, short_name, title)) in BUILTINS.iter().enumerate() {
            let slot = i as u16;
            self.slots.insert(slot.to_string(), (*id).to_string());
            let mut topic = None;
            let mut payload = Value::Null;
            let mut icon = None;
            if *id == "widget-pomodoro.toggle" {
                topic = Some("modules/widget-pomodoro/command".to_string());
                payload = json!({ "action": "toggle" });
                icon = Some("play".to_string());
            } else if *id == "widget-pomodoro.reset" {
                topic = Some("modules/widget-pomodoro/command".to_string());
                payload = json!({ "action": "reset" });
                icon = Some("refresh".to_string());
            } else if *id == "openflux.brightness_up" || *id == "openflux.brightness_down" {
                icon = Some("brightness".to_string());
            }
            self.entries.insert(
                slot,
                ActionSlotEntry {
                    action_id: (*id).to_string(),
                    module_id: "openflux".to_string(),
                    keycode: VIAL_KEYCODE_BASE + slot,
                    name: (*name).to_string(),
                    short_name: (*short_name).to_string(),
                    short_label: Some((*short_name).to_string()),
                    title: (*title).to_string(),
                    icon,
                    accent: None,
                    mode: "tap".to_string(),
                    topic,
                    payload,
                    release_payload: None,
                    retain: false,
                    active: true,
                },
            );
        }

        // Mark existing module action slots inactive first; re-activate any still present.
        for (&slot, entry) in self.entries.iter_mut() {
            if is_module_action_slot(slot) {
                entry.active = false;
            }
        }
        self.unassigned.clear();

        let gamepad_enabled = enabled_manifests.iter().any(|m| m.id == "analog-gamepad");

        // Every enabled action that should hold a slot, in declaration order.
        let mut wanted: Vec<(&ModuleManifest, &openflux_modules::ModuleActionField, String)> = Vec::new();
        for manifest in enabled_manifests {
            let declared = declared.get(&manifest.id).filter(|_| manifest.dynamic_actions);
            for act in manifest.actions.iter().chain(declared.into_iter().flatten()) {
                let full_id = format!("{}.{}", manifest.id, act.id);
                // Built-in slots already cover pomodoro toggle/reset.
                if BUILTINS.iter().any(|(bid, _, _, _)| *bid == full_id) {
                    continue;
                }
                if let Err(detail) = validate_action(act) {
                    self.unassigned.push(UnassignedAction {
                        action_id: full_id,
                        module_id: manifest.id.clone(),
                        label: act.label.clone(),
                        reason: "invalid".into(),
                        detail: Some(detail),
                    });
                    continue;
                }
                // Declared twice (e.g. manifest and runtime): the first one wins.
                if wanted.iter().any(|(_, _, id)| *id == full_id) {
                    continue;
                }
                wanted.push((manifest, act, full_id));
            }
        }

        // Pass 1: actions that already own a slot keep it. Pass 2: new actions take the lowest
        // never-used slot, else a slot whose action is gone (its old bindings then run this one).
        // Doing every keeper first means a newcomer can never take a slot still in use.
        let mut placed: Vec<(u16, usize)> = Vec::new();
        let mut newcomers: Vec<usize> = Vec::new();
        for (i, (_, _, full_id)) in wanted.iter().enumerate() {
            let existing = self
                .entries
                .iter()
                .find(|(&s, e)| is_module_action_slot(s) && e.action_id == *full_id)
                .map(|(&s, _)| s);
            match existing {
                Some(slot) => {
                    self.entries.get_mut(&slot).unwrap().active = true;
                    placed.push((slot, i));
                }
                None => newcomers.push(i),
            }
        }
        for i in newcomers {
            let slot = module_action_slots()
                .find(|s| !self.entries.contains_key(s))
                .or_else(|| module_action_slots().find(|s| self.entries.get(s).is_some_and(|e| !e.active)));
            let (manifest, act, full_id) = &wanted[i];
            let Some(slot) = slot else {
                self.unassigned.push(UnassignedAction {
                    action_id: full_id.clone(),
                    module_id: manifest.id.clone(),
                    label: act.label.clone(),
                    reason: "no_free_slot".into(),
                    detail: Some(format!("all {MODULE_ACTION_SLOT_COUNT} module action slots are in use")),
                });
                continue;
            };
            // Claim it now so the next newcomer doesn't pick the same slot.
            self.entries.insert(slot, placeholder_entry(full_id, &manifest.id, slot));
            placed.push((slot, i));
        }

        for (slot, i) in placed {
            let (manifest, act, full_id) = &wanted[i];
            let short_name = act
                .short_label
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().chars().take(10).collect::<String>())
                .unwrap_or_else(|| make_short_name(&manifest.id, &act.id));

            self.slots.insert(slot.to_string(), full_id.clone());
            self.entries.insert(
                slot,
                ActionSlotEntry {
                    action_id: full_id.clone(),
                    module_id: manifest.id.clone(),
                    keycode: VIAL_KEYCODE_BASE + slot,
                    name: format_keycap_name(act.short_label.as_deref().unwrap_or(&act.label)),
                    short_name,
                    short_label: act.short_label.clone(),
                    title: format!("{} — {}", manifest.name, act.label),
                    icon: act.icon.clone(),
                    accent: act.accent.clone(),
                    mode: if act.mode.eq_ignore_ascii_case("hold") {
                        "hold".to_string()
                    } else {
                        "tap".to_string()
                    },
                    topic: Some(act.topic.clone()),
                    payload: act.payload.clone(),
                    release_payload: act.release_payload.clone(),
                    retain: act.retain,
                    active: true,
                },
            );
        }

        // Fixed 25-slot Analog Gamepad block (slots 39..63 = 0x7E27..0x7E3F): populated in
        // vial.json whenever the `analog-gamepad` module is enabled (marked inactive otherwise).
        if gamepad_enabled {
            for (i, (id, name, short_name, title)) in GAMEPAD_SLOTS.iter().enumerate() {
                let slot = (VIAL_GAMEPAD_SLOT_START + i) as u16;
                self.slots.insert(slot.to_string(), (*id).to_string());
                self.entries.insert(
                    slot,
                    ActionSlotEntry {
                        action_id: (*id).to_string(),
                        module_id: "analog-gamepad".to_string(),
                        keycode: VIAL_KEYCODE_BASE + slot,
                        name: (*name).to_string(),
                        short_name: (*short_name).to_string(),
                        short_label: Some((*short_name).to_string()),
                        title: (*title).to_string(),
                        icon: None,
                        accent: Some("#58f29c".to_string()),
                        mode: "hold".to_string(),
                        topic: None,
                        payload: Value::Null,
                        release_payload: None,
                        retain: false,
                        active: true,
                    },
                );
            }
        } else {
            for slot in VIAL_GAMEPAD_SLOT_START as u16..VIAL_MAX_CUSTOM_SLOTS as u16 {
                if let Some(e) = self.entries.get_mut(&slot) {
                    e.active = false;
                }
            }
        }
    }

    pub fn reconcile_from_registry(&mut self, registry: &ModuleRegistry, declared: &DeclaredActions) {
        let discovered = registry.discover();
        let state_file = registry.load_state();
        let enabled: Vec<ModuleManifest> = discovered
            .into_iter()
            .filter(|(id, _)| state_file.modules.get(id).map(|s| s.enabled).unwrap_or(false))
            .map(|(_, dm)| dm.manifest)
            .collect();
        self.reconcile_from_manifests(&enabled, declared);
    }

    /// Build the dynamic `vial.json` document with `"customKeycodes"` populated from slots `0..max`.
    pub fn build_vial_json(&self) -> Value {
        let max_slot = self
            .entries
            .keys()
            .copied()
            .max()
            .unwrap_or((VIAL_BUILTIN_SLOTS - 1) as u16)
            .min((VIAL_MAX_CUSTOM_SLOTS - 1) as u16);

        let mut custom_keycodes = Vec::with_capacity(max_slot as usize + 1);
        for slot in 0..=max_slot {
            if let Some(entry) = self.entries.get(&slot) {
                let title = if entry.active {
                    entry.title.clone()
                } else {
                    format!("{} (module disabled)", entry.title)
                };
                custom_keycodes.push(json!({
                    "name": entry.name,
                    "title": title,
                    "shortName": entry.short_name,
                }));
            } else {
                custom_keycodes.push(json!({
                    "name": format!("Slot\n{slot}"),
                    "title": format!("Unused Keyglass action slot {slot}"),
                    "shortName": format!("OF_{slot}"),
                }));
            }
        }

        json!({
            "name": "Keyglass Keyboard",
            "vendorId": "0x3761",
            "productId": "0x4015",
            "matrix": { "rows": 6, "cols": 16 },
            "layouts": {
                "labels": [["Physical Layout", "ANSI", "ISO", "JIS"]],
                "keymap": [
                    ["0,0","0,1","0,2","0,3","0,4","0,5","0,6","0,7","0,8","0,9","0,10","0,11","0,12","0,13","0,14","0,15"],
                    [
                        "1,0","1,1","1,2","1,3","1,4","1,5","1,6","1,7","1,8","1,9","1,10","1,11","1,12",
                        {"w":2},"1,13\n\n\n0,0","1,14\n\n\n0,0",
                        {"x":-3,"w":2},"1,13\n\n\n0,1","1,14\n\n\n0,1",
                        {"x":-3},"1,13\n\n\n0,2","1,14\n\n\n0,2","1,15\n\n\n0,2"
                    ],
                    [{"w":1.5},"2,0","2,1","2,2","2,3","2,4","2,5","2,6","2,7","2,8","2,9","2,10","2,11","2,12",{"w":1.5},"2,13","2,14"],
                    [
                        {"w":1.75},"3,0","3,1","3,2","3,3","3,4","3,5","3,6","3,7","3,8","3,9","3,10","3,11",
                        {"w":2.25},"3,12\n\n\n0,0","3,13\n\n\n0,0",
                        {"x":-3.25},"3,12\n\n\n0,1",{"w":1.25},"3,13\n\n\n0,1","3,14\n\n\n0,1",
                        {"x":-3.25},"3,12\n\n\n0,2",{"w":1.25},"3,13\n\n\n0,2","3,14\n\n\n0,2"
                    ],
                    [
                        {"w":2.25},"4,0\n\n\n0,0","4,1\n\n\n0,0","4,2\n\n\n0,0","4,3\n\n\n0,0","4,4\n\n\n0,0",
                        "4,5\n\n\n0,0","4,6\n\n\n0,0","4,7\n\n\n0,0","4,8\n\n\n0,0","4,9\n\n\n0,0","4,10\n\n\n0,0",
                        {"w":1.75},"4,11\n\n\n0,0","4,12\n\n\n0,0","4,13\n\n\n0,0",
                        {"x":-16,"w":1.25},"4,0\n\n\n0,1","4,1\n\n\n0,1","4,2\n\n\n0,1","4,3\n\n\n0,1","4,4\n\n\n0,1",
                        "4,5\n\n\n0,1","4,6\n\n\n0,1","4,7\n\n\n0,1","4,8\n\n\n0,1","4,9\n\n\n0,1","4,10\n\n\n0,1",
                        "4,11\n\n\n0,1",{"w":1.75},"4,12\n\n\n0,1","4,13\n\n\n0,1","4,14\n\n\n0,1",
                        {"x":-16,"w":2.25},"4,0\n\n\n0,2","4,1\n\n\n0,2","4,2\n\n\n0,2","4,3\n\n\n0,2","4,4\n\n\n0,2",
                        "4,5\n\n\n0,2","4,6\n\n\n0,2","4,7\n\n\n0,2","4,8\n\n\n0,2","4,9\n\n\n0,2","4,10\n\n\n0,2",
                        "4,11\n\n\n0,2",{"w":0.75},"4,12\n\n\n0,2","4,13\n\n\n0,2","4,14\n\n\n0,2"
                    ],
                    [
                        {"w":1.25},"5,0\n\n\n0,0",{"w":1.25},"5,1\n\n\n0,0",{"w":1.25},"5,2\n\n\n0,0",{"w":1.25},"5,3\n\n\n0,0",
                        {"w":5},"5,4\n\n\n0,0","5,5\n\n\n0,0","5,6\n\n\n0,0","5,7\n\n\n0,0","5,8\n\n\n0,0","5,9\n\n\n0,0","5,10\n\n\n0,0",
                        {"x":-16,"w":1.25},"5,0\n\n\n0,1",{"w":1.25},"5,1\n\n\n0,1",{"w":1.25},"5,2\n\n\n0,1",{"w":1.25},"5,3\n\n\n0,1",
                        {"w":5},"5,4\n\n\n0,1","5,5\n\n\n0,1","5,6\n\n\n0,1","5,7\n\n\n0,1","5,8\n\n\n0,1","5,9\n\n\n0,1","5,10\n\n\n0,1",
                        {"x":-16},"5,0\n\n\n0,2","5,1\n\n\n0,2","5,2\n\n\n0,2","5,3\n\n\n0,2","5,4\n\n\n0,2",
                        {"w":3},"5,5\n\n\n0,2","5,6\n\n\n0,2","5,7\n\n\n0,2","5,8\n\n\n0,2","5,9\n\n\n0,2","5,10\n\n\n0,2","5,11\n\n\n0,2","5,12\n\n\n0,2"
                    ]
                ]
            },
            "customKeycodes": custom_keycodes
        })
    }

    /// Serialize and wrap `vial.json` into a spec-compliant XZ stream ready for `FW_CMD_PUT_VIAL_DEF`.
    pub fn build_vial_xz(&self) -> Result<Vec<u8>, String> {
        let raw = serde_json::to_vec(&self.build_vial_json()).map_err(|e| e.to_string())?;
        Ok(wrap_xz_uncompressed(&raw))
    }
}

/// Pure-Rust IEEE CRC-32 used by the XZ container framing.
fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn encode_vli(mut n: u64, out: &mut Vec<u8>) {
    loop {
        let mut b = (n & 0x7F) as u8;
        n >>= 7;
        if n != 0 {
            b |= 0x80;
            out.push(b);
        } else {
            out.push(b);
            break;
        }
    }
}

/// Wraps `data` in a valid XZ stream using LZMA2 uncompressed chunks (zero external C/crate deps).
/// Compatible with `xz-decompress` (JS/vialite/vial.rocks), Python `lzma` (vial-gui), and `xz`.
pub fn wrap_xz_uncompressed(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 64);
    let flags = [0x00u8, 0x01u8]; // Check type 0x01 = CRC32
    // Stream Header
    out.extend_from_slice(&[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]);
    out.extend_from_slice(&flags);
    out.extend_from_slice(&crc32_ieee(&flags).to_le_bytes());

    // Block Header (12 bytes total: encoded_size=2 => 12 bytes, 1 filter = LZMA2 dict=4KiB)
    let bh_body = [0x02u8, 0x00, 0x21, 0x01, 0x00, 0x00, 0x00, 0x00];
    out.extend_from_slice(&bh_body);
    out.extend_from_slice(&crc32_ieee(&bh_body).to_le_bytes());

    // LZMA2 uncompressed chunk(s)
    let mut chunk_bytes = 0usize;
    let mut off = 0usize;
    while off < data.len() {
        let len = (data.len() - off).min(65536);
        out.push(0x01); // uncompressed chunk, reset dict
        let len_minus_1 = (len - 1) as u16;
        out.extend_from_slice(&len_minus_1.to_be_bytes());
        out.extend_from_slice(&data[off..off + len]);
        chunk_bytes += 3 + len;
        off += len;
    }
    out.push(0x00); // LZMA2 end-of-payload marker
    chunk_bytes += 1;

    let pad_len = (4 - (chunk_bytes % 4)) % 4;
    for _ in 0..pad_len {
        out.push(0x00);
    }
    out.extend_from_slice(&crc32_ieee(data).to_le_bytes());

    // Index
    let unpadded_size = (12 + chunk_bytes + 4) as u64;
    let mut idx_body = Vec::with_capacity(32);
    idx_body.push(0x00); // index indicator
    encode_vli(1, &mut idx_body); // 1 record
    encode_vli(unpadded_size, &mut idx_body);
    encode_vli(data.len() as u64, &mut idx_body);
    while idx_body.len() % 4 != 0 {
        idx_body.push(0x00);
    }
    let idx_crc = crc32_ieee(&idx_body);
    let idx_total_len = idx_body.len() + 4;
    out.extend_from_slice(&idx_body);
    out.extend_from_slice(&idx_crc.to_le_bytes());

    // Stream Footer
    let back_size = ((idx_total_len / 4) - 1) as u32;
    let mut ftr_body = [0u8; 6];
    ftr_body[0..4].copy_from_slice(&back_size.to_le_bytes());
    ftr_body[4..6].copy_from_slice(&flags);
    out.extend_from_slice(&crc32_ieee(&ftr_body).to_le_bytes());
    out.extend_from_slice(&ftr_body);
    out.extend_from_slice(b"YZ");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use openflux_modules::{ModuleActionField, ModuleCategory};

    fn fake_manifest(id: &str, actions: Vec<(&str, &str)>) -> ModuleManifest {
        ModuleManifest {
            id: id.to_string(),
            name: id.to_string(),
            version: "1.0.0".to_string(),
            description: String::new(),
            author: String::new(),
            license: None,
            homepage: None,
            repository: None,
            issues_url: None,
            support_url: None,
            discord_url: None,
            changelog: None,
            changelog_url: None,
            maturity: openflux_modules::ModuleMaturity::Stable,
            deprecated: false,
            deprecation_message: None,
            moved_to: None,
            replaced_by: None,
            category: ModuleCategory::Widget,
            tags: vec![],
            provides: vec![],
            depends: vec![],
            stages: vec![],
            host_requirement: Default::default(),
            slot_capable: false,
            top_bar: None,
            stackable: true,
            forked_from: None,
            ui: Default::default(),
            entrypoints: Default::default(),
            channels: Default::default(),
            settings: vec![],
            dynamic_actions: false,
            actions: actions
                .into_iter()
                .map(|(aid, label)| ModuleActionField {
                    id: aid.to_string(),
                    label: label.to_string(),
                    topic: format!("modules/{id}/command"),
                    payload: json!({ "action": aid }),
                    retain: false,
                    short_label: Some(aid.to_string()),
                    icon: Some("prusa-cam.png".to_string()),
                    accent: Some("#fa6831".to_string()),
                    mode: "hold".to_string(),
                    release_payload: Some(json!({ "active": false })),
                })
                .collect(),
        }
    }

    #[test]
    fn sticky_slots_never_shift_and_support_64_slots() {
        let mut map = ModuleActionsMap::default();
        let m1 = fake_manifest("pet", vec![("feed", "Feed Pet")]);
        let m2 = fake_manifest("stream", vec![("mute", "Mute Stream")]);
        map.reconcile_from_manifests(&[m1.clone(), m2.clone()], &DeclaredActions::new());
        assert_eq!(map.slots.get("14").map(String::as_str), Some("pet.feed"));
        assert_eq!(map.slots.get("15").map(String::as_str), Some("stream.mute"));
        assert_eq!(map.entries.get(&14).unwrap().keycode, 0x7E0E);
        assert_eq!(map.entries.get(&14).unwrap().mode, "hold");
        assert_eq!(map.entries.get(&14).unwrap().icon.as_deref(), Some("prusa-cam.png"));

        // Disable m1: m2 MUST stay at slot 15!
        map.reconcile_from_manifests(&[m2.clone()], &DeclaredActions::new());
        assert_eq!(map.slots.get("15").map(String::as_str), Some("stream.mute"));
        assert!(!map.entries.get(&14).unwrap().active);
        assert!(map.entries.get(&15).unwrap().active);

        // Verify 25 dynamic module slots (14..38) + 25 fixed Analog Gamepad slots (39..63) = 64 total,
        // and confirm full 64-slot vial.json XZ fits inside FW_QMK_MAX_VIAL_DEF_BYTES (10240).
        let many_acts: Vec<(&str, &str)> = (0..23)
            .map(|i| {
                let s: &'static str = Box::leak(format!("act_{i}").into_boxed_str());
                (s, s)
            })
            .collect();
        let m_many = fake_manifest("many", many_acts);
        let m_gp = fake_manifest("analog-gamepad", vec![]);
        map.reconcile_from_manifests(&[m1, m2, m_many, m_gp], &DeclaredActions::new());
        assert_eq!(map.entries.len(), 64);
        assert_eq!(map.entries.get(&38).unwrap().keycode, 0x7E26);
        assert_eq!(map.entries.get(&39).unwrap().action_id, "gamepad.ls_up");
        assert_eq!(map.entries.get(&39).unwrap().keycode, 0x7E27);
        assert_eq!(map.entries.get(&63).unwrap().action_id, "gamepad.guide");
        assert_eq!(map.entries.get(&63).unwrap().keycode, 0x7E3F);

        let xz = map.build_vial_xz().unwrap();
        assert_eq!(&xz[0..6], &[0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00]);
        assert_eq!(&xz[xz.len() - 2..], b"YZ");
        assert!(xz.len() <= 10240, "64-slot vial.json XZ ({} B) exceeds 10240 B cap", xz.len());
    }

    fn declared(module: &str, ids: &[&str]) -> DeclaredActions {
        let mut d = DeclaredActions::new();
        d.insert(
            module.to_string(),
            ids.iter()
                .map(|id| ModuleActionField {
                    id: id.to_string(),
                    label: format!("Button {id}"),
                    topic: format!("modules/{module}/key"),
                    payload: json!({ "button_id": id, "pressed": true }),
                    retain: false,
                    short_label: None,
                    icon: None,
                    accent: None,
                    mode: "hold".to_string(),
                    release_payload: Some(json!({ "button_id": id, "pressed": false })),
                })
                .collect(),
        );
        d
    }

    #[test]
    fn runtime_declared_actions_get_sticky_slots_past_the_qk_kb_range_and_report_overflow() {
        let mut mqtt = fake_manifest("mqtt-bridge", vec![]);
        mqtt.dynamic_actions = true;
        let ids: Vec<String> = (0..60).map(|i| format!("b{i}")).collect();
        let many: Vec<String> = (0..480).map(|i| format!("x{i}")).collect();
        let many: Vec<&str> = many.iter().map(String::as_str).collect();
        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();

        let mut map = ModuleActionsMap::default();
        map.reconcile_from_manifests(&[mqtt.clone()], &declared("mqtt-bridge", &ids));

        // The first 56 fill the slots every firmware forwards (25 QK_KB + 31 QK_USER); the rest
        // go on past the Globe key into the range that needs firmware 2.1.6.
        assert_eq!(map.module_slots_used(), 60);
        assert!(map.unassigned.is_empty());
        assert_eq!(map.entries[&14].action_id, "mqtt-bridge.b0");
        assert_eq!(map.entries[&38].action_id, "mqtt-bridge.b24");
        assert_eq!(map.entries[&64].action_id, "mqtt-bridge.b25");
        assert_eq!(map.entries[&64].keycode, 0x7E40);
        assert_eq!(map.entries[&94].keycode, 0x7E5E);
        assert!(!map.entries.contains_key(&95), "0x7E5F is the firmware Globe key");
        assert_eq!(map.entries[&96].action_id, "mqtt-bridge.b56");
        assert_eq!(slot_requires_firmware(94), None);
        assert_eq!(slot_requires_firmware(96), Some("2.1.6"));
        assert_eq!(map.entries[&20].topic.as_deref(), Some("modules/mqtt-bridge/key"));

        // Redeclaring in a different order keeps every existing binding where it was.
        let mut reversed = ids.clone();
        reversed.reverse();
        map.reconcile_from_manifests(&[mqtt.clone()], &declared("mqtt-bridge", &reversed));
        assert_eq!(map.entries[&14].action_id, "mqtt-bridge.b0");
        assert_eq!(map.entries[&64].action_id, "mqtt-bridge.b25");

        // Past every slot (472), the leftovers are reported, not dropped.
        let mut full = ModuleActionsMap::default();
        full.reconcile_from_manifests(&[mqtt.clone()], &declared("mqtt-bridge", &many));
        assert_eq!(full.module_slots_used(), MODULE_ACTION_SLOT_COUNT);
        assert_eq!(MODULE_ACTION_SLOT_COUNT, 472);
        assert_eq!(full.entries[&511].keycode, 0x7FFF);
        assert_eq!(full.unassigned.len(), 480 - 472);
        assert!(full.unassigned.iter().all(|u| u.reason == "no_free_slot"));

        // A module that hasn't opted in can't declare actions.
        let plain = fake_manifest("plain", vec![]);
        let mut fresh = ModuleActionsMap::default();
        fresh.reconcile_from_manifests(&[plain], &declared("plain", &["x"]));
        assert_eq!(fresh.module_slots_used(), 0);

        // Invalid declarations are reported rather than silently dropped.
        let mut bad = ModuleActionsMap::default();
        bad.reconcile_from_manifests(&[mqtt], &declared("mqtt-bridge", &["has space"]));
        assert_eq!(bad.unassigned[0].reason, "invalid");
    }
}
