//! On-device filesystem paths for Keyglass display and hardware control.

pub const DISPLAY_INSTALL_DIR: &str = "/usr/lib/keyglass/display";
pub const DISPLAY_FFMPEG_PATH: &str = "/usr/lib/keyglass/display/keyglass-ffmpeg";

pub const DISPLAY_CONFIG_DIR_DEFAULT: &str = "/home/root/keyglass/config";
pub const DISPLAY_MEDIA_DIR_DEFAULT: &str = "/home/root/keyglass/media";
pub const DISPLAY_LOG_DIR_DEFAULT: &str = "/home/root/keyglass/logs";
pub const DISPLAY_ACTIVE_MARKER_DEFAULT: &str = "/tmp/keyglass_display_active";
pub const DISPLAY_UPDATING_FLAG_PATH: &str = "/tmp/keyglass_updating_display";
pub const BRIDGE_UPDATING_FLAG_PATH: &str = "/tmp/keyglass_updating_bridge";
pub const BRIDGE_SOCK_DEFAULT: &str = "/tmp/keyglass_bridge.sock";
pub const SAVED_BRIGHTNESS_PATH: &str = "/tmp/keyglass_saved_brightness";
/// While this file exists, the display stays asleep.
pub const SUSPEND_FLAG_PATH: &str = "/home/root/suspended.txt";

pub const BACKLIGHT_BL_POWER_PATH: &str = "/sys/class/backlight/backlight/bl_power";
pub const BACKLIGHT_BRIGHTNESS_PATH: &str = "/sys/class/backlight/backlight/brightness";
pub const BACKLIGHT_MAX_BRIGHTNESS_PATH: &str = "/sys/class/backlight/backlight/max_brightness";
