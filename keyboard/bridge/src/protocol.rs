/// `openflux-bridge <version>`, where the version is `OPENFLUX_VERSION` at build time or the
/// crate version (see build.rs).
pub const BRIDGE_VERSION: &str = env!("OPENFLUX_BRIDGE_VERSION_STR");
pub const DISPLAY_VERSION_FALLBACK: &str = "unknown";
pub const OS_VERSION_FALLBACK: &str = "unknown";

pub const USB_VENDOR_ID_STR: &str = "0x3761";
pub const USB_PRODUCT_ID_STR: &str = "0x0104";
/// The same on every keyboard; the host finds the bridge by vendor and product ID, not by serial.
pub const USB_SERIAL_STR: &str = "fedcba0123456789";
pub const USB_MFR_STR: &str = "OpenFlux";
pub const USB_PROD_STR: &str = "OpenFlux Keyboard";
pub const USB_CONFIG_STR: &str = "OpenFlux Bridge";
pub const USB_UDC_NAME: &str = "fcc00000.usb";

pub const BRIDGE_PACKET_SIZE: usize = 512;
pub const BRIDGE_MAX_CONTROL_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;
pub const BRIDGE_MAX_FILE_UPLOAD_BYTES: usize = 128 * 1024 * 1024;
pub const BRIDGE_DISK_RESERVE_BYTES: u64 = 64 * 1024 * 1024;

pub const CMD_REQUEST_BRIDGE_VERSION: u8 = 1;
pub const CMD_REQUEST_DISPLAY_VERSION: u8 = 2;
pub const CMD_REQUEST_OS_VERSION: u8 = 3;
pub const CMD_SET_SYSTEM_TIME: u8 = 4;
pub const CMD_GET_LIST_OF_FILES: u8 = 100;
pub const CMD_SEND_FILE_TO_HOST: u8 = 101;
pub const CMD_SEND_FILE_FROM_HOST: u8 = 102;
pub const CMD_DELETE_FILE: u8 = 103;
pub const CMD_GET_FREE_DISK_SPACE: u8 = 104;
pub const CMD_STAT_FILE: u8 = 105;
pub const CMD_SEND_LIVE_DATA: u8 = 200;
pub const CMD_GET_MEDIA_STATUS: u8 = 201;
pub const CMD_GET_SHORTCUT_CONTEXT: u8 = 202;
pub const CMD_POLL_BUS_FRAMES: u8 = 205;

pub const RESP_NAK: u8 = 0;
pub const RESP_ACK: u8 = 1;

pub const FW_UART_DEFAULT_DEV: &str = "/dev/ttyS3";
pub const FW_UART_MSG_UPDATE: u8 = 2;
pub const FW_UART_MSG_QC_DATA: u8 = 3;
pub const FW_UART_MSG_ANALOG: u8 = 4;
pub const FW_MSG_UPDATE_LEN: usize = 139;
pub const FW_MSG_QC_DATA_LEN: usize = 401;
pub const FW_UART_FLAG_QC_MODE: u8 = 0x01;
pub const FW_UART_FLAG_ANALOG_MODE: u8 = 0x02;

pub const LIVE_DATA_TYPE_BUS_FRAME: u8 = 10;

pub const DISPLAY_MAX_CLIENTS: usize = 8;
