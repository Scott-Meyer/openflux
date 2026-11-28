//! Paths on the keyboard that the daemon reads and writes through the bridge.
//!
//! The configuration and media directories are where the display loads its configuration from.

/// Directory holding the display configuration documents.
pub const CONFIG_DIR: &str = "/home/root/keyglass/config";
pub const APPEARANCE_JSON: &str = "/home/root/keyglass/config/appearance.json";
pub const SETTINGS_JSON: &str = "/home/root/keyglass/config/settings.json";
pub const MOSAIC_JSON: &str = "/home/root/keyglass/config/mosaic.json";
pub const KEYMAP_JSON: &str = "/home/root/keyglass/config/keymap.json";
pub const PROFILES_JSON: &str = "/home/root/keyglass/config/profiles.json";

/// Background images and videos.
pub const MEDIA_DIR: &str = "/home/root/keyglass/media";
/// Key icons referenced by the appearance stylemaps.
pub const KEY_ICONS_DIR: &str = "/home/root/keyglass/media/key_icons";

/// Pomodoro state the display publishes for the host.
pub const POMODORO_STATE_JSON: &str = "/home/root/keyglass/logs/pomo_state.json";
/// Keyboard system statistics the display publishes for the host.
pub const SOC_STATS_JSON: &str = "/home/root/keyglass/logs/soc_stats.json";

/// Directory an uploaded package is placed in; the keyboard installs packages found there.
pub const PACKAGE_UPLOAD_DIR: &str = "/home/root";

/// Bridge rollback state written by the bridge package. While `armed` exists, the keyboard
/// restores the previous bridge unless the host confirms the new one.
pub const BRIDGE_ROLLBACK_DIR: &str = "/home/root/.keyglass/rollback/bridge";
pub const BRIDGE_ROLLBACK_ARMED: &str = "/home/root/.keyglass/rollback/bridge/armed";
/// Written by the bridge package when an install starts: `attempt_id=<hex>` and
/// `version=keyglass-bridge <target>` lines.
pub const BRIDGE_ROLLBACK_ATTEMPT: &str = "/home/root/.keyglass/rollback/bridge/attempt";

/// Written by a newly installed bridge once its USB function is up: `version=keyglass-bridge
/// <live>` and `pid=<pid>` lines. Only this proves the new bridge is the one answering.
pub fn bridge_trial_path(attempt_id: &str) -> String {
    format!("{BRIDGE_ROLLBACK_DIR}/trial-{attempt_id}")
}

/// Where the host stages the Keyglass bridge and its launcher before the first Keyglass bridge is
/// installed on a keyboard, with a `manifest.sha256` covering both. The bridge package refuses a
/// first install without a complete, verified pair; the keyboard falls back to it if the new
/// bridge doesn't start.
pub const RESCUE_DIR: &str = "/home/root/.keyglass/rescue";
/// Where the bridge package installs the bridge and its launcher.
pub const BRIDGE_BINARY_PATH: &str = "/usr/lib/keyglass/bridge/keyglass-bridge";
pub const BRIDGE_LAUNCHER_PATH: &str = "/usr/lib/keyglass/bridge/keyglass-autolaunch.sh";

/// Staging directory for a factory restore: the factory packages and their `manifest.sha256`.
pub const FACTORY_RESTORE_DIR: &str = "/home/root/.keyglass/factory-restore";

/// Every display configuration document, paired with its key in a save plan or pulled config.
pub const CONFIG_DOCUMENTS: [(&str, &str); 5] = [
    (APPEARANCE_JSON, "appearance"),
    (SETTINGS_JSON, "settings"),
    (MOSAIC_JSON, "mosaic"),
    (KEYMAP_JSON, "keymap"),
    (PROFILES_JSON, "profiles"),
];

pub fn key_icon_path(name: &str) -> String {
    format!("{KEY_ICONS_DIR}/{name}")
}

pub fn media_path(name: &str) -> String {
    format!("{MEDIA_DIR}/{name}")
}
