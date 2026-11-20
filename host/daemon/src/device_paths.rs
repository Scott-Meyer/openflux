//! Paths on the keyboard that the daemon reads and writes through the bridge.
//!
//! The configuration and media directories are where the display loads its configuration from.

/// Directory holding the display configuration documents.
pub const CONFIG_DIR: &str = "/home/root/openflux/config";
pub const APPEARANCE_JSON: &str = "/home/root/openflux/config/appearance.json";
pub const SETTINGS_JSON: &str = "/home/root/openflux/config/settings.json";
pub const MOSAIC_JSON: &str = "/home/root/openflux/config/mosaic.json";
pub const KEYMAP_JSON: &str = "/home/root/openflux/config/keymap.json";
pub const PROFILES_JSON: &str = "/home/root/openflux/config/profiles.json";

/// Background images and videos.
pub const MEDIA_DIR: &str = "/home/root/openflux/media";
/// Key icons referenced by the appearance stylemaps.
pub const KEY_ICONS_DIR: &str = "/home/root/openflux/media/key_icons";

/// Pomodoro state the display publishes for the host.
pub const POMODORO_STATE_JSON: &str = "/home/root/openflux/logs/pomo_state.json";
/// Keyboard system statistics the display publishes for the host.
pub const SOC_STATS_JSON: &str = "/home/root/openflux/logs/soc_stats.json";

/// Directory an uploaded package is placed in; the keyboard installs packages found there.
pub const PACKAGE_UPLOAD_DIR: &str = "/home/root";

/// Bridge rollback state written by the bridge package. While `armed` exists, the keyboard
/// restores the previous bridge unless the host confirms the new one.
pub const BRIDGE_ROLLBACK_DIR: &str = "/home/root/.openflux/rollback/bridge";
pub const BRIDGE_ROLLBACK_ARMED: &str = "/home/root/.openflux/rollback/bridge/armed";
/// Written by the bridge package when an install starts: `attempt_id=<hex>` and
/// `version=openflux-bridge <target>` lines.
pub const BRIDGE_ROLLBACK_ATTEMPT: &str = "/home/root/.openflux/rollback/bridge/attempt";

/// Written by a newly installed bridge once its USB function is up: `version=openflux-bridge
/// <live>` and `pid=<pid>` lines. Only this proves the new bridge is the one answering.
pub fn bridge_trial_path(attempt_id: &str) -> String {
    format!("{BRIDGE_ROLLBACK_DIR}/trial-{attempt_id}")
}

/// Where the host stages the OpenFlux bridge and its launcher before the first OpenFlux bridge is
/// installed on a keyboard, with a `manifest.sha256` covering both. The bridge package refuses a
/// first install without a complete, verified pair; the keyboard falls back to it if the new
/// bridge doesn't start.
pub const RESCUE_DIR: &str = "/home/root/.openflux/rescue";
/// Where the bridge package installs the bridge and its launcher.
pub const BRIDGE_BINARY_PATH: &str = "/usr/lib/openflux/bridge/openflux-bridge";
pub const BRIDGE_LAUNCHER_PATH: &str = "/usr/lib/openflux/bridge/openflux-autolaunch.sh";

/// Staging directory for a factory restore: the factory packages and their `manifest.sha256`.
pub const FACTORY_RESTORE_DIR: &str = "/home/root/.openflux/factory-restore";

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
