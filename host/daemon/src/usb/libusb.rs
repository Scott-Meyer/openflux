//! USB transport to the keyboard through libusb (`rusb`). On Unix each command takes a per-device
//! file lock so the daemon and other Keyglass tools never interleave commands on one interface; on
//! Windows no lock is taken.

use crate::usb::protocol::{
    build_named_payload, build_put_file_payload, build_set_time_payload, transact_io,
    UsbEndpointIo, UsbError, UsbTarget, BRIDGE_CMD_BRIDGE_VERSION, BRIDGE_CMD_DELETE_FILE,
    BRIDGE_CMD_DISPLAY_VERSION, BRIDGE_CMD_GET_FILE, BRIDGE_CMD_GET_FILE_LIST,
    BRIDGE_CMD_OS_VERSION, BRIDGE_CMD_POLL_BUS_FRAMES, BRIDGE_CMD_PUT_FILE,
    BRIDGE_CMD_SEND_LIVE_DATA,
    BRIDGE_CMD_SET_SYSTEM_TIME, BRIDGE_CMD_STAT_FILE, BRIDGE_TARGET, FIRMWARE_TARGET,
    FW_CMD_FILE_FROM_HOST, FW_CMD_FILE_TO_HOST, FW_CMD_GET_KEYMAP, FW_CMD_GET_PRESS_REFINEMENT,
    FW_CMD_PRESS_REFINEMENT_STATUS, FW_CMD_PUT_PRESS_REFINEMENT, FW_CMD_PUT_VIAL_DEF,
    FW_CMD_SEND_KEYMAP, FW_CMD_SET_GAMEPAD_CONFIG, FW_CMD_SET_LAYER_OVERRIDE,
    FW_CMD_SET_ACTIVE_PROFILE, FW_CMD_SET_LANGUAGE, FW_CMD_SET_PLATFORM, FW_CMD_VERSION,
    KEYBOARD_VID,
};
use rusb::{Context, DeviceHandle, UsbContext};
use std::time::Duration;

struct RusbEndpoint<'a, C: UsbContext> {
    handle: &'a mut DeviceHandle<C>,
}

impl<'a, C: UsbContext> UsbEndpointIo for RusbEndpoint<'a, C> {
    fn bulk_write(&mut self, ep: u8, data: &[u8], timeout_ms: u64) -> Result<(), UsbError> {
        let written = self
            .handle
            .write_bulk(ep, data, Duration::from_millis(timeout_ms))
            .map_err(|e| UsbError::Io(format!("bulk_write(0x{ep:02x}): {e}")))?;
        if written != data.len() {
            return Err(UsbError::Io(format!(
                "short bulk_write: {written} != {}",
                data.len()
            )));
        }
        Ok(())
    }

    fn bulk_read(&mut self, ep: u8, len: usize, timeout_ms: u64) -> Result<Vec<u8>, UsbError> {
        let mut buf = vec![0u8; len.max(1)];
        let n = self
            .handle
            .read_bulk(ep, &mut buf, Duration::from_millis(timeout_ms))
            .map_err(|e| UsbError::Io(format!("bulk_read(0x{ep:02x}): {e}")))?;
        buf.truncate(n);
        Ok(buf)
    }
}

#[cfg(unix)]
struct UsbLockGuard {
    _file: std::fs::File,
}

#[cfg(unix)]
impl UsbLockGuard {
    fn acquire(pid: u16) -> Result<Self, UsbError> {
        use std::os::unix::io::AsRawFd;
        let path = format!("/tmp/keyglass_usb_{pid:04x}.lock");
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;
        // The lock is a fixed name shared with other tools; a link planted there isn't followed.
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|e| UsbError::Io(format!("lock open {path}: {e}")))?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if rc != 0 {
            return Err(UsbError::Io(format!(
                "flock {path}: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(Self { _file: file })
    }
}

#[cfg(not(unix))]
struct UsbLockGuard;
#[cfg(not(unix))]
impl UsbLockGuard {
    fn acquire(_pid: u16) -> Result<Self, UsbError> {
        Ok(Self)
    }
}

pub struct KeyboardUsbClient {
    ctx: Context,
}

impl KeyboardUsbClient {
    pub fn new() -> Result<Self, UsbError> {
        let ctx = Context::new().map_err(|e| UsbError::Io(format!("libusb init: {e}")))?;
        Ok(Self { ctx })
    }

    pub fn probe_attached(&self) -> (bool, bool) {
        let Ok(devs) = self.ctx.devices() else {
            return (false, false);
        };
        let mut has_bridge = false;
        let mut has_firmware = false;
        for d in devs.iter() {
            if let Ok(desc) = d.device_descriptor() {
                if desc.vendor_id() == KEYBOARD_VID {
                    if desc.product_id() == BRIDGE_TARGET.pid {
                        has_bridge = true;
                    } else if desc.product_id() == FIRMWARE_TARGET.pid {
                        has_firmware = true;
                    }
                }
            }
        }
        (has_bridge, has_firmware)
    }

    pub fn transact(
        &self,
        target: UsbTarget,
        cmd: u8,
        payload: &[u8],
        timeout_ms: u64,
    ) -> Result<Vec<u8>, UsbError> {
        let _lock = UsbLockGuard::acquire(target.pid)?;
        let devs = self
            .ctx
            .devices()
            .map_err(|e| UsbError::Io(format!("device_list: {e}")))?;
        let mut handle = None;
        for d in devs.iter() {
            if let Ok(desc) = d.device_descriptor() {
                if desc.vendor_id() == KEYBOARD_VID && desc.product_id() == target.pid {
                    let h = d
                        .open()
                        .map_err(|e| UsbError::Io(format!("open 0x{:04x}: {e}", target.pid)))?;
                    handle = Some(h);
                    break;
                }
            }
        }
        let mut handle = handle.ok_or_else(|| {
            UsbError::NotFound(format!("VID=0x{KEYBOARD_VID:04x} PID=0x{:04x}", target.pid))
        })?;

        let _ = handle.set_auto_detach_kernel_driver(true);
        handle
            .claim_interface(target.interface)
            .map_err(|e| UsbError::Io(format!("claim_interface({}): {e}", target.interface)))?;

        let res = {
            let mut ep = RusbEndpoint {
                handle: &mut handle,
            };
            transact_io(&mut ep, target, cmd, payload, timeout_ms)
        };
        let _ = handle.release_interface(target.interface);
        res
    }

    /// Version strings each component reports, keyed `firmware`, `bridge`, `display` and `os`.
    pub fn get_firmware_version(&self) -> Result<String, UsbError> {
        let raw = self.transact(FIRMWARE_TARGET, FW_CMD_VERSION, &[], 2000)?;
        Ok(String::from_utf8_lossy(&raw).trim_matches('\0').to_string())
    }

    pub fn get_versions(&self) -> serde_json::Value {
        let ask = |target: UsbTarget, cmd: u8| {
            self.transact(target, cmd, &[], 2000)
                .ok()
                .map(|b| String::from_utf8_lossy(&b).trim_matches('\0').to_string())
        };
        serde_json::json!({
            "firmware": ask(FIRMWARE_TARGET, FW_CMD_VERSION),
            "bridge": ask(BRIDGE_TARGET, BRIDGE_CMD_BRIDGE_VERSION),
            "display": ask(BRIDGE_TARGET, BRIDGE_CMD_DISPLAY_VERSION),
            "os": ask(BRIDGE_TARGET, BRIDGE_CMD_OS_VERSION),
        })
    }

    /// Sets the keyboard's clock to `utc_timestamp` (`YYYY-MM-DD HH:MM:SS`, UTC) and its time zone
    /// to the IANA zone `timezone_name`.
    pub fn set_system_time(
        &self,
        utc_timestamp: &str,
        timezone_name: &str,
    ) -> Result<(), UsbError> {
        let p = build_set_time_payload(utc_timestamp, timezone_name)?;
        self.transact(BRIDGE_TARGET, BRIDGE_CMD_SET_SYSTEM_TIME, &p, 3000)?;
        Ok(())
    }

    pub fn send_live_data(&self, hdlc_wire: &[u8]) -> Result<(), UsbError> {
        self.transact(BRIDGE_TARGET, BRIDGE_CMD_SEND_LIVE_DATA, hdlc_wire, 3000)?;
        Ok(())
    }

    pub fn poll_bus_frames(&self) -> Result<Vec<u8>, UsbError> {
        self.transact(BRIDGE_TARGET, BRIDGE_CMD_POLL_BUS_FRAMES, &[], 1500)
    }

    pub fn get_file_list(&self, remote_dir: &str) -> Result<String, UsbError> {
        let p = build_named_payload(remote_dir)?;
        let raw = self.transact(BRIDGE_TARGET, BRIDGE_CMD_GET_FILE_LIST, &p, 4000)?;
        Ok(String::from_utf8_lossy(&raw).to_string())
    }

    pub fn get_file(&self, remote_name: &str) -> Result<Vec<u8>, UsbError> {
        let p = build_named_payload(remote_name)?;
        self.transact(BRIDGE_TARGET, BRIDGE_CMD_GET_FILE, &p, 8000)
    }

    pub fn put_file(&self, remote_name: &str, data: &[u8]) -> Result<(), UsbError> {
        let p = build_put_file_payload(remote_name, data)?;
        self.transact(BRIDGE_TARGET, BRIDGE_CMD_PUT_FILE, &p, 15000)?;
        Ok(())
    }

    pub fn delete_file(&self, remote_name: &str) -> Result<(), UsbError> {
        let p = build_named_payload(remote_name)?;
        self.transact(BRIDGE_TARGET, BRIDGE_CMD_DELETE_FILE, &p, 4000)?;
        Ok(())
    }

    pub fn stat_file(&self, remote_name: &str) -> Result<bool, UsbError> {
        let p = build_named_payload(remote_name)?;
        let raw = self.transact(BRIDGE_TARGET, BRIDGE_CMD_STAT_FILE, &p, 3000)?;
        let s = String::from_utf8_lossy(&raw);
        let tok = s.trim_matches('\0');
        if tok.starts_with("exists") {
            Ok(true)
        } else if tok == "absent" {
            Ok(false)
        } else {
            Err(UsbError::Io(format!(
                "unexpected stat_file response: {tok}"
            )))
        }
    }

    pub fn get_keymap(&self) -> Result<Vec<u8>, UsbError> {
        self.transact(FIRMWARE_TARGET, FW_CMD_GET_KEYMAP, &[], 5000)
    }

    pub fn enter_firmware_bootloader(&self) -> Result<(u8, Vec<u8>), UsbError> {
        let devs = self
            .ctx
            .devices()
            .map_err(|e| UsbError::Io(format!("device_list: {e}")))?;
        let mut fw_paths: Vec<(u8, Vec<u8>)> = Vec::new();
        for d in devs.iter() {
            if let Ok(desc) = d.device_descriptor() {
                if desc.vendor_id() == KEYBOARD_VID && desc.product_id() == FIRMWARE_TARGET.pid {
                    let ports = d.port_numbers().map_err(|e| {
                        UsbError::Io(format!(
                            "USB port path of the keyboard firmware is unavailable: {e}"
                        ))
                    })?;
                    if ports.is_empty() {
                        return Err(UsbError::Io(
                            "USB port path of the keyboard firmware is empty; refusing to flash"
                                .into(),
                        ));
                    }
                    fw_paths.push((d.bus_number(), ports));
                }
            }
        }
        if fw_paths.is_empty() {
            return Err(UsbError::NotFound(format!(
                "Keyboard firmware (VID 0x{KEYBOARD_VID:04x}, PID 0x{:04x}) is not attached; refusing to flash",
                FIRMWARE_TARGET.pid
            )));
        }
        if fw_paths.len() > 1 {
            return Err(UsbError::Io(
                "More than one keyboard is attached; refusing to flash".into(),
            ));
        }
        let target_path = fw_paths.into_iter().next().unwrap();
        // The firmware resets into its boot loader and may not answer first, so the reply is ignored.
        let _ = self.transact(
            FIRMWARE_TARGET,
            crate::usb::protocol::FW_CMD_ENTER_BOOTLOADER,
            &[],
            2000,
        );
        Ok(target_path)
    }

    pub fn send_keymap(&self, blob: &[u8]) -> Result<(), UsbError> {
        self.transact(FIRMWARE_TARGET, FW_CMD_SEND_KEYMAP, blob, 8000)?;
        Ok(())
    }

    pub fn set_language(&self, lang: u32) -> Result<(), UsbError> {
        self.transact(
            FIRMWARE_TARGET,
            FW_CMD_SET_LANGUAGE,
            &lang.to_le_bytes(),
            3000,
        )?;
        Ok(())
    }

    pub fn set_platform(&self, platform: u32) -> Result<(), UsbError> {
        self.transact(
            FIRMWARE_TARGET,
            FW_CMD_SET_PLATFORM,
            &platform.to_le_bytes(),
            3000,
        )?;
        Ok(())
    }

    pub fn set_active_profile(&self, profile: u32) -> Result<(), UsbError> {
        self.transact(
            FIRMWARE_TARGET,
            FW_CMD_SET_ACTIVE_PROFILE,
            &profile.to_le_bytes(),
            3000,
        )?;
        Ok(())
    }

    /// `GET_RUNTIME_STATE`: the firmware's active profile, actuation mode and platform, 12 bytes.
    /// It is read every poll, so it gets a short timeout.
    pub fn get_runtime_state(&self) -> Result<Vec<u8>, UsbError> {
        self.transact(
            FIRMWARE_TARGET,
            crate::usb::protocol::FW_CMD_GET_RUNTIME_STATE,
            &[],
            1000,
        )
    }

    pub fn firmware_file_to_host(&self, file_type: u32) -> Result<Vec<u8>, UsbError> {
        self.transact(
            FIRMWARE_TARGET,
            FW_CMD_FILE_TO_HOST,
            &file_type.to_le_bytes(),
            5000,
        )
    }

    pub fn firmware_file_from_host(&self, transfer_bytes: &[u8]) -> Result<(), UsbError> {
        self.transact(FIRMWARE_TARGET, FW_CMD_FILE_FROM_HOST, transfer_bytes, 5000)?;
        Ok(())
    }

    /// `PRESS_REFINEMENT_STATUS`: the firmware's 16-byte press-refinement status.
    pub fn press_refinement_status(&self) -> Result<Vec<u8>, UsbError> {
        self.transact(FIRMWARE_TARGET, FW_CMD_PRESS_REFINEMENT_STATUS, &[], 3000)
    }

    /// `PUT_PRESS_REFINEMENT`: stores a press-refinement record; the firmware writes it whole and
    /// verifies it before it answers.
    pub fn put_press_refinement(&self, record: &[u8]) -> Result<(), UsbError> {
        self.transact(FIRMWARE_TARGET, FW_CMD_PUT_PRESS_REFINEMENT, record, 8000)?;
        Ok(())
    }

    /// `GET_PRESS_REFINEMENT`: the stored press-refinement record.
    pub fn get_press_refinement(&self) -> Result<Vec<u8>, UsbError> {
        self.transact(FIRMWARE_TARGET, FW_CMD_GET_PRESS_REFINEMENT, &[], 3000)
    }

    /// `PUT_VIAL_DEF`: uploads a dynamic XZ-compressed `vial.json` definition to RP2040 LittleFS.
    pub fn put_vial_def(&self, xz_bytes: &[u8]) -> Result<(), UsbError> {
        self.transact(FIRMWARE_TARGET, FW_CMD_PUT_VIAL_DEF, xz_bytes, 5000)?;
        Ok(())
    }

    /// `SET_GAMEPAD_CONFIG` (0x74): configures the RP2040 Hall-effect Analog Gamepad engine.
    /// `mode`: 0=Off (no USB controller), 1=Classic HID Gamepad, 2=XInput.
    /// `curve`: 0=Linear, 1=SCurve, 2=Snappy, 3=Precision.
    pub fn set_gamepad_config(
        &self,
        mode: u8,
        deadzone_um: u16,
        saturation_um: u16,
        curve: u8,
        radial_clamp: bool,
    ) -> Result<(), UsbError> {
        let mut payload = [0u8; 7];
        payload[0] = mode;
        payload[1..3].copy_from_slice(&deadzone_um.to_le_bytes());
        payload[3..5].copy_from_slice(&saturation_um.to_le_bytes());
        payload[5] = curve;
        payload[6] = u8::from(radial_clamp);
        self.transact(FIRMWARE_TARGET, FW_CMD_SET_GAMEPAD_CONFIG, &payload, 3000)?;
        Ok(())
    }

    /// `SET_LAYER_OVERRIDE` (0x75): switches QMK's default layer for App Linking (`Some(0..7)`),
    /// or restores the active profile's default layer (`None` -> `0xFF`).
    pub fn set_layer_override(&self, layer: Option<u8>) -> Result<(), UsbError> {
        let byte = layer.unwrap_or(0xFF);
        self.transact(FIRMWARE_TARGET, FW_CMD_SET_LAYER_OVERRIDE, &[byte], 3000)?;
        Ok(())
    }
}
