use crate::device_paths::BRIDGE_SOCK_DEFAULT;
use crate::protocol::{
    DISPLAY_MAX_CLIENTS, FW_UART_MSG_ANALOG, FW_UART_MSG_FLUX_ACTION, FW_UART_MSG_QC_DATA,
    FW_UART_MSG_RESOLVED_KEYS, FW_UART_MSG_UPDATE, LIVE_DATA_TYPE_BUS_FRAME,
};
use crate::simple_hdlc::SimpleHdlcDecoder;
use crate::uart_bridge::{FirmwareUartState, UartBridge};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};

const MAX_CLIENT_RX: usize = 16384;
const MAX_HOST_OUTBOX: usize = 256;
const MAX_LIVE_LEN: usize = 1024 * 1024;

/// First byte of the 5-byte control frame that announces a live-data message length.
const LIVE_CONTROL_MARKER: u8 = 0xFF;
const LIVE_CONTROL_FRAME_LEN: usize = 5;
/// Live-data message types (the first byte of a complete message).
const LIVE_DATA_TYPE_JSON: u8 = 0;
const LIVE_DATA_TYPE_HEARTBEAT: u8 = 4;

struct ClientConn {
    stream: UnixStream,
    rx_buf: Vec<u8>,
    discard: bool,
}

/// The bridge's socket for display clients. Writes to clients don't block or buffer: a client
/// that can't take a whole line immediately is dropped, and the display reconnects.
pub struct DisplayIpc {
    listener: Option<UnixListener>,
    sock_path: String,
    clients: Vec<ClientConn>,
    live_hdlc: SimpleHdlcDecoder,
    collecting_live: bool,
    expected_live_len: usize,
    live_buf: Vec<u8>,
    host_outbox: VecDeque<serde_json::Value>,
}

impl Default for DisplayIpc {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for DisplayIpc {
    fn drop(&mut self) {
        self.close();
    }
}

impl DisplayIpc {
    pub fn new() -> Self {
        Self {
            listener: None,
            sock_path: String::new(),
            clients: Vec::new(),
            live_hdlc: SimpleHdlcDecoder::new(),
            collecting_live: false,
            expected_live_len: 0,
            live_buf: Vec::new(),
            host_outbox: VecDeque::with_capacity(64),
        }
    }

    pub fn drain_host_bus_frames(&mut self) -> Vec<u8> {
        if self.host_outbox.is_empty() {
            return b"[]".to_vec();
        }
        let items: Vec<serde_json::Value> = self.host_outbox.drain(..).collect();
        serde_json::to_vec(&items).unwrap_or_else(|_| b"[]".to_vec())
    }

    pub fn init(&mut self, sock_path: Option<&str>) -> Result<(), String> {
        self.close();
        let path = sock_path
            .filter(|s| !s.is_empty())
            .unwrap_or(BRIDGE_SOCK_DEFAULT);
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path).map_err(|e| format!("bind {path}: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        self.sock_path = path.to_string();
        self.listener = Some(listener);
        Ok(())
    }

    pub fn close(&mut self) {
        self.clients.clear();
        self.listener = None;
        if !self.sock_path.is_empty() {
            let _ = std::fs::remove_file(&self.sock_path);
            self.sock_path.clear();
        }
        self.live_buf.clear();
        self.collecting_live = false;
        self.expected_live_len = 0;
    }

    pub fn listen_fd(&self) -> RawFd {
        self.listener.as_ref().map(|l| l.as_raw_fd()).unwrap_or(-1)
    }

    pub fn client_fds(&self) -> Vec<RawFd> {
        self.clients.iter().map(|c| c.stream.as_raw_fd()).collect()
    }

    pub fn accept(&mut self) {
        let Some(ref l) = self.listener else { return };
        loop {
            match l.accept() {
                Ok((stream, _)) => {
                    if self.clients.len() >= DISPLAY_MAX_CLIENTS {
                        drop(stream);
                        continue;
                    }
                    let _ = stream.set_nonblocking(true);
                    self.clients.push(ClientConn {
                        stream,
                        rx_buf: Vec::with_capacity(256),
                        discard: false,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }

    fn send_line_to_idx(&mut self, idx: usize, line: &[u8]) -> bool {
        if idx >= self.clients.len() || line.is_empty() {
            return true;
        }
        let res = self.clients[idx].stream.write_all(line);
        if res.is_err() {
            self.clients.remove(idx);
            return false;
        }
        true
    }

    pub fn broadcast_line(&mut self, line: &str) {
        let bytes = line.as_bytes();
        let mut idx = 0;
        while idx < self.clients.len() {
            if self.clients[idx].stream.write_all(bytes).is_err() {
                self.clients.remove(idx);
            } else {
                idx += 1;
            }
        }
    }

    pub fn read_client_by_fd(&mut self, fd: RawFd, uart: &mut UartBridge) {
        let Some(idx) = self.clients.iter().position(|c| c.stream.as_raw_fd() == fd) else {
            return;
        };
        let mut buf = [0u8; 256];
        let n = match self.clients[idx].stream.read(&mut buf) {
            Ok(0) => {
                self.clients.remove(idx);
                return;
            }
            Ok(n) => n,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock
                    || e.kind() == io::ErrorKind::Interrupted =>
            {
                return
            }
            Err(_) => {
                self.clients.remove(idx);
                return;
            }
        };

        let mut completed_lines = Vec::new();
        {
            let c = &mut self.clients[idx];
            for &ch in &buf[..n] {
                if ch == b'\n' || ch == b'\r' {
                    if !c.discard && !c.rx_buf.is_empty() {
                        if let Ok(s) = std::str::from_utf8(&c.rx_buf) {
                            completed_lines.push(s.to_string());
                        }
                    }
                    c.rx_buf.clear();
                    c.discard = false;
                } else if !c.discard {
                    if c.rx_buf.len() + 1 >= MAX_CLIENT_RX {
                        c.rx_buf.clear();
                        c.discard = true;
                    } else {
                        c.rx_buf.push(ch);
                    }
                }
            }
        }

        for line in completed_lines {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
                match val.get("cmd").and_then(|v| v.as_str()) {
                    Some("set_qc") => {
                        if let Some(on) = val.get("enabled").and_then(|v| v.as_bool()) {
                            uart.qc_mode = on;
                        }
                    }
                    Some("set_analog") => {
                        if let Some(on) = val.get("enabled").and_then(|v| v.as_bool()) {
                            uart.analog_mode = on;
                        }
                    }
                    Some("set_stress") => {
                        if let Some(on) = val.get("enabled").and_then(|v| v.as_bool()) {
                            uart.stress_test_enabled = on;
                        }
                    }
                    Some("bus_frame") => {
                        if let Some(frame) = val.get("frame") {
                            // Forward to any OTHER local IPC clients (never echo back to sender!),
                            // and enqueue for host polling over USB (`CMD_POLL_BUS_FRAMES`).
                            let out_line = format!("{{\"type\":\"bus_frame\",\"frame\":{frame}}}\n");
                            let bytes = out_line.as_bytes();
                            let mut c_idx = 0;
                            while c_idx < self.clients.len() {
                                if self.clients[c_idx].stream.as_raw_fd() == fd {
                                    c_idx += 1;
                                    continue;
                                }
                                if self.clients[c_idx].stream.write_all(bytes).is_err() {
                                    self.clients.remove(c_idx);
                                } else {
                                    c_idx += 1;
                                }
                            }
                            if self.host_outbox.len() >= MAX_HOST_OUTBOX {
                                self.host_outbox.pop_front();
                            }
                            self.host_outbox.push_back(frame.clone());
                        }
                    }
                    Some("get_state") => {
                        if uart.state.valid {
                            let s = format_uart_json(&uart.state, FW_UART_MSG_UPDATE);
                            if !s.is_empty() {
                                let Some(cur_idx) =
                                    self.clients.iter().position(|c| c.stream.as_raw_fd() == fd)
                                else {
                                    break;
                                };
                                if !self.send_line_to_idx(cur_idx, s.as_bytes()) {
                                    break;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn publish_uart(&mut self, state: &FirmwareUartState, msg_type: u8) {
        let line = format_uart_json(state, msg_type);
        if !line.is_empty() {
            self.broadcast_line(&line);
        }
    }

    pub fn feed_live_data(&mut self, data: &[u8]) {
        let mut frames: Vec<Vec<u8>> = Vec::new();
        self.live_hdlc.feed(data, |p| frames.push(p.to_vec()));
        for payload in frames {
            if payload.is_empty() {
                continue;
            }
            if payload.len() == LIVE_CONTROL_FRAME_LEN && payload[0] == LIVE_CONTROL_MARKER {
                let total =
                    u32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]) as usize;
                if total == 0 || total > MAX_LIVE_LEN {
                    self.collecting_live = false;
                    self.live_buf.clear();
                    continue;
                }
                self.expected_live_len = total;
                self.live_buf.clear();
                self.collecting_live = true;
                continue;
            }
            if !self.collecting_live {
                continue;
            }
            if self.live_buf.len() + payload.len() > self.expected_live_len {
                self.collecting_live = false;
                self.live_buf.clear();
                continue;
            }
            self.live_buf.extend_from_slice(&payload);
            if self.live_buf.len() < self.expected_live_len {
                continue;
            }
            self.collecting_live = false;
            if let Some(line) = format_live_data_line(&self.live_buf) {
                self.broadcast_line(&line);
            }
            self.live_buf.clear();
        }
    }
}

/// Builds the `live_data` line for one complete live-data message, or `None`
/// for an unknown type or a JSON message whose data isn't a JSON object.
/// The object is re-serialized, so the line is always a single valid JSON line.
fn format_live_data_line(message: &[u8]) -> Option<String> {
    let (&data_type, body) = message.split_first()?;
    match data_type {
        LIVE_DATA_TYPE_HEARTBEAT => Some(format!(
            "{{\"type\":\"live_data\",\"data_type\":{LIVE_DATA_TYPE_HEARTBEAT}}}\n"
        )),
        LIVE_DATA_TYPE_JSON => {
            let value = serde_json::from_slice::<serde_json::Value>(body).ok()?;
            if !value.is_object() {
                return None;
            }
            Some(format!(
                "{{\"type\":\"live_data\",\"data_type\":{LIVE_DATA_TYPE_JSON},\"payload\":{value}}}\n"
            ))
        }
        LIVE_DATA_TYPE_BUS_FRAME => {
            let value = serde_json::from_slice::<serde_json::Value>(body).ok()?;
            if !value.is_object() {
                return None;
            }
            Some(format!(
                "{{\"type\":\"bus_frame\",\"frame\":{value}}}\n"
            ))
        }
        _ => None,
    }
}

pub fn format_uart_json(state: &FirmwareUartState, msg_type: u8) -> String {
    if msg_type == FW_UART_MSG_UPDATE {
        let mut hex = String::with_capacity(64);
        for b in &state.key_bitmap {
            use std::fmt::Write;
            let _ = write!(&mut hex, "{b:02x}");
        }
        let mut ev_field = String::new();
        if state.of_events_valid {
            use std::fmt::Write;
            let _ = write!(
                &mut ev_field,
                ",\"of_events_seq\":{},\"of_events_ring\":[",
                state.of_events_seq
            );
            for (i, c) in state.of_events_ring.iter().enumerate() {
                if i > 0 {
                    ev_field.push(',');
                }
                let _ = write!(&mut ev_field, "{c}");
            }
            ev_field.push_str("],\"of_events_profiles\":[");
            for (i, p) in state.of_events_profiles.iter().enumerate() {
                if i > 0 {
                    ev_field.push(',');
                }
                let _ = write!(&mut ev_field, "{p}");
            }
            ev_field.push(']');
        }
        let mut qmk_field = String::new();
        if state.qmk_state_valid {
            use std::fmt::Write;
            let _ = write!(
                &mut qmk_field,
                ",\"qmk_state_valid\":true,\"effective_mods\":{},\"oneshot_mods\":{},\"oneshot_layer\":{},\"caps_word_active\":{},\"default_layer_bitfield\":{},\"keymap_revision\":{}",
                state.qmk_effective_mods,
                state.qmk_oneshot_mods,
                state.qmk_oneshot_layer,
                state.qmk_caps_word_active,
                state.qmk_default_layer_bitfield,
                state.qmk_keymap_revision
            );
        }
        return format!(
            "{{\"type\":\"key_state\",\"seq\":{},\"timestamp_ms\":{},\"keyframe_layout\":{},\"keyframe_attached\":{},\"switch_feel\":{},\"active_layer_bitfield\":{},\"active_actuation_mode\":{},\"language_code\":{},\"platform_code\":{},\"active_profile\":{},\"num_lock\":{},\"caps_lock\":{},\"scroll_lock\":{}{}{},\"key_bitmap\":\"{}\"}}\n",
            state.seq,
            state.timestamp_ms,
            state.keyframe_layout,
            state.keyframe_attached,
            state.switch_feel,
            state.active_layer_bitfield,
            state.active_actuation_mode,
            state.language_code,
            state.platform_code,
            state.active_profile,
            (state.host_state_status & 0x01) != 0,
            (state.host_state_status & 0x02) != 0,
            (state.host_state_status & 0x04) != 0,
            ev_field,
            qmk_field,
            hex
        );
    }
    if msg_type == FW_UART_MSG_QC_DATA {
        let mut out = format!("{{\"type\":\"key_qc\",\"seq\":{},\"sensors\":[", state.seq);
        for (i, s) in state.qc_sensors.iter().enumerate() {
            use std::fmt::Write;
            if i > 0 {
                out.push(',');
            }
            let _ = write!(&mut out, "{s}");
        }
        out.push_str("]}\n");
        return out;
    }
    if msg_type == FW_UART_MSG_ANALOG {
        let mut out = format!(
            "{{\"type\":\"key_analog\",\"seq\":{},\"timestamp_ms\":{},\"samples\":[",
            state.seq, state.analog_timestamp_ms
        );
        for (i, s) in state.analog_samples.iter().enumerate() {
            use std::fmt::Write;
            if i > 0 {
                out.push(',');
            }
            let _ = write!(&mut out, "[{},{},{}]", s.key_index, s.depth_um, s.velocity_um_s);
        }
        out.push_str("]}\n");
        return out;
    }
    if msg_type == FW_UART_MSG_FLUX_ACTION {
        let a = state.last_flux_action;
        return format!(
            "{{\"type\":\"flux_action\",\"seq\":{},\"slot\":{},\"pressed\":{},\"mods\":{},\"layer\":{},\"profile\":{}}}\n",
            state.seq, a.slot, a.pressed, a.mods, a.layer, a.profile
        );
    }
    if msg_type == FW_UART_MSG_RESOLVED_KEYS {
        let mut out = format!(
            "{{\"type\":\"resolved_keys\",\"seq\":{},\"keymap_revision\":{},\"keys\":[",
            state.seq, state.resolved_keymap_revision
        );
        for (i, k) in state.resolved_keys.iter().enumerate() {
            use std::fmt::Write;
            if i > 0 {
                out.push(',');
            }
            let _ = write!(&mut out, "[{},{}]", k.keycode, k.source_layer);
        }
        out.push_str("]}\n");
        return out;
    }
    String::new()
}
