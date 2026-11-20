use crate::protocol::{
    FW_MSG_QC_DATA_LEN, FW_MSG_UPDATE_LEN, FW_UART_DEFAULT_DEV, FW_UART_MSG_QC_DATA,
    FW_UART_MSG_UPDATE,
};
use crate::simple_hdlc::{simple_hdlc_encode, SimpleHdlcDecoder};
use std::os::unix::io::RawFd;

// Key-state message (type 2), laid out in docs/protocol/uart.md. Offsets are within the state
// block that starts at byte 5 of the message.
const MSG_STATE_START: usize = 5;
const ST_KEY_BITMAP: usize = 0x00;
const ST_UPTIME_MS: usize = 0x20;
const ST_KEYFRAME_LAYOUT: usize = 0x24;
const ST_KEYFRAME_ATTACHED: usize = 0x25;
const ST_SWITCH_FEEL: usize = 0x26;
const ST_ACTIVE_LAYERS: usize = 0x27;
const ST_ACTUATION_MODE: usize = 0x2B;
const ST_OF_EVENT_VER: usize = 0x2F;
const ST_OF_EVENT_SEQ: usize = 0x30;
const ST_OF_EVENT_RING: usize = 0x32;
const ST_OF_EVENT_PROFILES: usize = 0x42;
const ST_HOST_LEDS: usize = 0x5C;
const ST_LANGUAGE: usize = 0x5D;
const ST_PLATFORM: usize = 0x61;
const ST_ACTIVE_PROFILE: usize = 0x65;
const ST_KEYFRAME_ATTACH_STATE: usize = 0x6A;
const ST_MODULE_TYPES: usize = 0x6B;
const ST_MODULE_BUTTONS: usize = 0x84;

const QC_SENSOR_COUNT: usize = 198;
/// Bytes of responses waiting for the firmware beyond which new responses are dropped.
const MAX_TX_BACKLOG: usize = 1024;

fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The firmware's latest key state and sensor readings, decoded from its UART messages.
#[derive(Clone)]
pub struct FirmwareUartState {
    pub valid: bool,
    pub seq: u32,
    pub timestamp_ms: u32,
    pub keyframe_layout: u8,
    pub keyframe_attached: bool,
    pub switch_feel: u8,
    pub active_layer_bitfield: u32,
    pub active_actuation_mode: u32,
    pub of_events_valid: bool,
    pub of_events_seq: u16,
    pub of_events_ring: [u8; 16],
    pub of_events_profiles: [u8; 16],
    pub language_code: u32,
    pub platform_code: u32,
    pub active_profile: u32,
    pub key_bitmap: [u8; 32],
    pub host_state_status: u8,
    pub keyframe_attach_phase: u8,
    pub module_types: u8,
    pub module_buttons: u16,
    pub qc_sensors: [u16; QC_SENSOR_COUNT],
}

impl Default for FirmwareUartState {
    fn default() -> Self {
        Self {
            valid: false,
            seq: 0,
            timestamp_ms: 0,
            keyframe_layout: 0,
            keyframe_attached: false,
            switch_feel: 0,
            active_layer_bitfield: 0,
            active_actuation_mode: 0,
            of_events_valid: false,
            of_events_seq: 0,
            of_events_ring: [0; 16],
            of_events_profiles: [0; 16],
            language_code: 0,
            platform_code: 0,
            active_profile: 0,
            key_bitmap: [0u8; 32],
            host_state_status: 0,
            keyframe_attach_phase: 0,
            module_types: 0,
            module_buttons: 0,
            qc_sensors: [0u16; QC_SENSOR_COUNT],
        }
    }
}

pub struct UartBridge {
    pub rx_fd: RawFd,
    pub tx_fd: RawFd,
    owns_fds: bool,
    pub qc_mode: bool,
    pub stress_test_enabled: bool,
    pub tx_buf: Vec<u8>,
    pub state: FirmwareUartState,
    decoder: SimpleHdlcDecoder,
}

impl Default for UartBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for UartBridge {
    fn drop(&mut self) {
        self.close();
    }
}

impl UartBridge {
    pub fn new() -> Self {
        Self {
            rx_fd: -1,
            tx_fd: -1,
            owns_fds: false,
            qc_mode: false,
            stress_test_enabled: false,
            tx_buf: Vec::new(),
            state: FirmwareUartState::default(),
            decoder: SimpleHdlcDecoder::new(),
        }
    }

    pub fn attach_fds(&mut self, rx_fd: RawFd, tx_fd: RawFd) {
        self.close();
        self.rx_fd = rx_fd;
        self.tx_fd = tx_fd;
        self.owns_fds = false;
    }

    pub fn open(&mut self, dev_path: &str) -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            use std::ffi::CString;
            if dev_path == FW_UART_DEFAULT_DEV {
                let _ = std::fs::write("/sys/class/tty/ttyS3/rx_trig_bytes", b"1\n");
            }
            let cdev = CString::new(dev_path).map_err(|e| e.to_string())?;
            let fd = unsafe {
                libc::open(
                    cdev.as_ptr(),
                    libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK,
                )
            };
            if fd < 0 {
                return Err(format!(
                    "open {dev_path} failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            unsafe {
                let mut tio: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(fd, &mut tio) == 0 {
                    libc::cfmakeraw(&mut tio);
                    libc::cfsetispeed(&mut tio, libc::B921600);
                    libc::cfsetospeed(&mut tio, libc::B921600);
                    tio.c_cflag |= libc::CLOCAL | libc::CREAD | libc::CS8;
                    tio.c_cflag &= !(libc::PARENB | libc::CSTOPB | libc::CRTSCTS);
                    tio.c_cc[libc::VMIN] = 0;
                    tio.c_cc[libc::VTIME] = 0;
                    libc::tcsetattr(fd, libc::TCSANOW, &tio);
                }
            }
            self.close();
            self.rx_fd = fd;
            self.tx_fd = fd;
            self.owns_fds = true;
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (dev_path, FW_UART_DEFAULT_DEV);
            Err("UART device open only supported on Linux".into())
        }
    }

    pub fn close(&mut self) {
        if self.owns_fds {
            if self.rx_fd >= 0 {
                unsafe { libc::close(self.rx_fd) };
            }
            if self.tx_fd >= 0 && self.tx_fd != self.rx_fd {
                unsafe { libc::close(self.tx_fd) };
            }
        }
        self.rx_fd = -1;
        self.tx_fd = -1;
        self.owns_fds = false;
    }

    pub fn has_pending_tx(&self) -> bool {
        self.tx_fd >= 0 && !self.tx_buf.is_empty()
    }

    pub fn flush_tx(&mut self) -> i32 {
        if self.tx_fd < 0 {
            return 0;
        }
        while !self.tx_buf.is_empty() {
            let w = unsafe {
                libc::write(
                    self.tx_fd,
                    self.tx_buf.as_ptr() as *const libc::c_void,
                    self.tx_buf.len(),
                )
            };
            if w > 0 {
                self.tx_buf.drain(..w as usize);
                continue;
            }
            if w == 0 {
                return 1;
            }
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if e.kind() == std::io::ErrorKind::WouldBlock {
                return 1;
            }
            return -1;
        }
        0
    }

    fn send_ack(&mut self, msg_type: u8, err: u8) -> i32 {
        if self.tx_fd < 0 {
            return 0;
        }
        let resp = [
            msg_type,
            err,
            u8::from(self.qc_mode),
            u8::from(self.stress_test_enabled),
        ];
        let Ok(wire) = simple_hdlc_encode(&resp) else {
            return -1;
        };
        if self.tx_buf.len() + wire.len() > MAX_TX_BACKLOG {
            return -1;
        }
        self.tx_buf.extend_from_slice(&wire);
        if self.flush_tx() < 0 {
            -1
        } else {
            0
        }
    }

    pub fn handle_frame<F: FnMut(&FirmwareUartState, u8)>(
        &mut self,
        payload: &[u8],
        on_update: &mut F,
    ) -> i32 {
        if payload.is_empty() {
            return -1;
        }
        let msg_type = payload[0];
        if msg_type == FW_UART_MSG_UPDATE {
            if payload.len() != FW_MSG_UPDATE_LEN {
                let _ = self.send_ack(msg_type, 1);
                return -1;
            }
            let st = &payload[MSG_STATE_START..];
            let s = &mut self.state;
            s.valid = true;
            s.seq = le_u32(payload, 1);
            s.key_bitmap
                .copy_from_slice(&st[ST_KEY_BITMAP..ST_KEY_BITMAP + 32]);
            s.timestamp_ms = le_u32(st, ST_UPTIME_MS);
            s.keyframe_layout = st[ST_KEYFRAME_LAYOUT];
            s.keyframe_attached = st[ST_KEYFRAME_ATTACHED] != 0;
            s.switch_feel = st[ST_SWITCH_FEEL];
            s.active_layer_bitfield = le_u32(st, ST_ACTIVE_LAYERS);
            s.active_actuation_mode = le_u32(st, ST_ACTUATION_MODE);
            s.of_events_valid = st[ST_OF_EVENT_VER] != 0;
            if s.of_events_valid {
                s.of_events_seq =
                    u16::from_le_bytes([st[ST_OF_EVENT_SEQ], st[ST_OF_EVENT_SEQ + 1]]);
                s.of_events_ring
                    .copy_from_slice(&st[ST_OF_EVENT_RING..ST_OF_EVENT_RING + 16]);
                s.of_events_profiles
                    .copy_from_slice(&st[ST_OF_EVENT_PROFILES..ST_OF_EVENT_PROFILES + 16]);
            } else {
                s.of_events_seq = 0;
                s.of_events_ring = [0; 16];
                s.of_events_profiles = [0; 16];
            }
            s.host_state_status = st[ST_HOST_LEDS];
            s.language_code = le_u32(st, ST_LANGUAGE);
            s.platform_code = le_u32(st, ST_PLATFORM);
            s.active_profile = le_u32(st, ST_ACTIVE_PROFILE);
            s.keyframe_attach_phase = st[ST_KEYFRAME_ATTACH_STATE];
            s.module_types = st[ST_MODULE_TYPES];
            s.module_buttons =
                u16::from_le_bytes([st[ST_MODULE_BUTTONS], st[ST_MODULE_BUTTONS + 1]]);
            let _ = self.send_ack(msg_type, 0);
            on_update(&self.state, msg_type);
            return 0;
        }
        if msg_type == FW_UART_MSG_QC_DATA {
            if payload.len() != FW_MSG_QC_DATA_LEN {
                let _ = self.send_ack(msg_type, 1);
                return -1;
            }
            self.state.seq = le_u32(payload, 1);
            let readings = &payload[MSG_STATE_START..];
            for (i, v) in self.state.qc_sensors.iter_mut().enumerate() {
                *v = u16::from_le_bytes([readings[i * 2], readings[i * 2 + 1]]);
            }
            let _ = self.send_ack(msg_type, 0);
            on_update(&self.state, msg_type);
            return 0;
        }
        -1
    }

    pub fn feed_bytes<F: FnMut(&FirmwareUartState, u8)>(&mut self, data: &[u8], mut on_update: F) {
        let mut frames: Vec<Vec<u8>> = Vec::new();
        self.decoder.feed(data, |p| frames.push(p.to_vec()));
        for f in frames {
            let _ = self.handle_frame(&f, &mut on_update);
        }
    }

    pub fn poll_read<F: FnMut(&FirmwareUartState, u8)>(&mut self, mut on_update: F) -> i32 {
        if self.rx_fd < 0 {
            return -1;
        }
        let mut buf = [0u8; 1024];
        let mut total = 0i32;
        loop {
            let n =
                unsafe { libc::read(self.rx_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n > 0 {
                self.feed_bytes(&buf[..n as usize], &mut on_update);
                total += n as i32;
                continue;
            }
            if n == 0 {
                return if total > 0 { total } else { -1 };
            }
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if e.kind() == std::io::ErrorKind::WouldBlock {
                return total;
            }
            return -1;
        }
    }
}
