//! On-device filesystem paths the OpenFlux bridge reads, writes or sets up.

pub const FFS_FUNCTION_NAME: &str = "ffs.openflux";

// The factory bridge's USB gadget mount and function, torn down when the OpenFlux bridge sets
// up its own gadget.
pub const FACTORY_BRIDGE_FFS_MOUNT: &str = "/dev/ffs-garfield";
pub const FACTORY_FFS_FUNCTION_NAME: &str = "ffs.garfield";

pub const BRIDGE_ROLLBACK_DIR: &str = "/home/root/.openflux/rollback/bridge";
pub const DISPLAY_VERSION_FILE: &str = "/home/root/.openflux/versions/display";
// The keyboard's base OS image keeps its own version in this file; the name is the OS image's.
pub const OS_VERSION_FILE: &str = "/home/root/gazpacho_version.txt";

pub const BRIDGE_READY_STAMP_DEFAULT: &str = "/tmp/openflux_bridge_ready";
pub const DISPLAY_ACTIVE_MARKER_DEFAULT: &str = "/tmp/openflux_display_active";
pub const BRIDGE_SOCK_DEFAULT: &str = "/tmp/openflux_bridge.sock";
pub const MEDIA_STATUS_FILE: &str = "/home/root/current_active_media.txt";
pub const SHORTCUT_CONTEXT_FILE: &str = "/home/root/current_shortcut_context.txt";
