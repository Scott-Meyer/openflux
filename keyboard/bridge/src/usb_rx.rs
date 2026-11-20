use crate::protocol::{
    BRIDGE_MAX_CONTROL_PAYLOAD_BYTES, BRIDGE_MAX_FILE_UPLOAD_BYTES, BRIDGE_PACKET_SIZE,
    CMD_SEND_FILE_FROM_HOST, RESP_ACK, RESP_NAK,
};
use crate::usb_commands::{
    bridge_handle_command, bridge_send_response, CommandHooks, PutFileStream,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RxMode {
    Idle,
    Receiving,
    Discarding,
}

pub struct BridgeRxState {
    pub mode: RxMode,
    active_cmd: u8,
    expected_packets: u32,
    expected_bytes: u32,
    expected_wire_bytes: u64,
    received_wire_bytes: u64,
    payload: Vec<u8>,
    file_hdr: Vec<u8>,
    file_stream: Option<PutFileStream>,
}

impl Default for BridgeRxState {
    fn default() -> Self {
        Self::new()
    }
}

impl BridgeRxState {
    pub fn new() -> Self {
        Self {
            mode: RxMode::Idle,
            active_cmd: 0,
            expected_packets: 0,
            expected_bytes: 0,
            expected_wire_bytes: 0,
            received_wire_bytes: 0,
            payload: Vec::new(),
            file_hdr: Vec::new(),
            file_stream: None,
        }
    }

    pub fn reset(&mut self) {
        self.mode = RxMode::Idle;
        self.active_cmd = 0;
        self.expected_packets = 0;
        self.expected_bytes = 0;
        self.expected_wire_bytes = 0;
        self.received_wire_bytes = 0;
        self.payload.clear();
        self.payload.shrink_to_fit();
        self.file_hdr.clear();
        self.file_hdr.shrink_to_fit();
        self.file_stream = None;
    }

    pub fn abort(&mut self, hooks: &mut dyn CommandHooks) {
        let should_nak = self.mode == RxMode::Receiving || self.mode == RxMode::Discarding;
        self.reset();
        if should_nak {
            bridge_send_response(hooks, RESP_NAK, &[]);
        }
    }

    fn reject_transfer(&mut self, terminated: bool, hooks: &mut dyn CommandHooks) {
        self.reset();
        if terminated {
            bridge_send_response(hooks, RESP_NAK, &[]);
        } else {
            self.mode = RxMode::Discarding;
        }
    }

    fn feed_file_bytes(&mut self, data: &[u8]) -> Result<(), ()> {
        if data.is_empty() {
            return Ok(());
        }
        if let Some(ref mut stream) = self.file_stream {
            return stream.write_chunk(data);
        }
        let max_hdr = 256usize;
        let take = data.len().min(max_hdr.saturating_sub(self.file_hdr.len()));
        self.file_hdr.extend_from_slice(&data[..take]);
        if let Some(nul) = self.file_hdr.iter().take(255).position(|&b| b == 0) {
            let raw_path = String::from_utf8_lossy(&self.file_hdr[..nul]).to_string();
            let rest_in_hdr = &self.file_hdr[nul + 1..];
            let hdr_prefix_len = (nul + 1) as u64;
            let file_len = (self.expected_bytes as u64).saturating_sub(hdr_prefix_len);
            let mut stream = PutFileStream::open(&raw_path, file_len)?;
            if !rest_in_hdr.is_empty() {
                stream.write_chunk(rest_in_hdr)?;
            }
            if take < data.len() {
                stream.write_chunk(&data[take..])?;
            }
            self.file_stream = Some(stream);
            self.file_hdr.clear();
            self.file_hdr.shrink_to_fit();
            Ok(())
        } else if self.file_hdr.len() >= 255 {
            Err(())
        } else {
            Ok(())
        }
    }

    pub fn feed(&mut self, buf: &[u8], terminated: bool, hooks: &mut dyn CommandHooks) {
        if buf.is_empty() {
            if self.mode == RxMode::Receiving || self.mode == RxMode::Discarding {
                self.abort(hooks);
            }
            return;
        }
        if self.mode == RxMode::Discarding {
            if terminated {
                self.reset();
                bridge_send_response(hooks, RESP_NAK, &[]);
            }
            return;
        }

        let mut offset = 0usize;
        while offset < buf.len() {
            if self.mode == RxMode::Idle {
                if buf.len() - offset < BRIDGE_PACKET_SIZE {
                    self.reject_transfer(terminated, hooks);
                    return;
                }
                let cmd = buf[offset];
                let expected_packets =
                    u32::from_le_bytes(buf[offset + 1..offset + 5].try_into().unwrap());
                let expected_bytes =
                    u32::from_le_bytes(buf[offset + 5..offset + 9].try_into().unwrap());
                offset += BRIDGE_PACKET_SIZE;
                let at_end = offset == buf.len();

                if expected_packets == 0 {
                    if expected_bytes != 0 {
                        self.reject_transfer(terminated, hooks);
                        return;
                    }
                    bridge_handle_command(cmd, &[], hooks);
                    continue;
                }

                let max_bytes = if cmd == CMD_SEND_FILE_FROM_HOST {
                    BRIDGE_MAX_FILE_UPLOAD_BYTES
                } else {
                    BRIDGE_MAX_CONTROL_PAYLOAD_BYTES
                };
                let expected_wire = (expected_packets as u64) * (BRIDGE_PACKET_SIZE as u64);
                if expected_bytes == 0
                    || (expected_bytes as usize) > max_bytes
                    || (expected_bytes as u64) > expected_wire
                    || (expected_wire - expected_bytes as u64) >= (BRIDGE_PACKET_SIZE as u64)
                {
                    self.reject_transfer(terminated, hooks);
                    return;
                }

                let mut payload = Vec::new();
                if cmd != CMD_SEND_FILE_FROM_HOST
                    && payload.try_reserve_exact(expected_bytes as usize).is_err()
                {
                    self.reject_transfer(terminated, hooks);
                    return;
                }

                self.mode = RxMode::Receiving;
                self.active_cmd = cmd;
                self.expected_packets = expected_packets;
                self.expected_bytes = expected_bytes;
                self.expected_wire_bytes = expected_wire;
                self.received_wire_bytes = 0;
                self.payload = payload;
                self.file_hdr.clear();
                self.file_stream = None;
                if at_end && terminated {
                    self.abort(hooks);
                    return;
                }
            } else {
                let wire_rem = self.expected_wire_bytes - self.received_wire_bytes;
                let avail = buf.len() - offset;
                let take = avail.min(wire_rem as usize);

                if self.received_wire_bytes < self.expected_bytes as u64 {
                    let payload_rem =
                        (self.expected_bytes as u64 - self.received_wire_bytes) as usize;
                    let copy_len = take.min(payload_rem);
                    if self.active_cmd == CMD_SEND_FILE_FROM_HOST {
                        if self
                            .feed_file_bytes(&buf[offset..offset + copy_len])
                            .is_err()
                        {
                            self.received_wire_bytes += take as u64;
                            offset += take;
                            let at_end = offset == buf.len();
                            if self.received_wire_bytes >= self.expected_wire_bytes {
                                self.reset();
                                bridge_send_response(hooks, RESP_NAK, &[]);
                            } else {
                                self.reject_transfer(at_end && terminated, hooks);
                            }
                            return;
                        }
                    } else {
                        self.payload
                            .extend_from_slice(&buf[offset..offset + copy_len]);
                    }
                }
                self.received_wire_bytes += take as u64;
                offset += take;
                let at_end = offset == buf.len();

                if self.received_wire_bytes >= self.expected_wire_bytes {
                    if self.active_cmd == CMD_SEND_FILE_FROM_HOST {
                        let stream = self.file_stream.take();
                        self.reset();
                        let ok = stream.map(|s| s.finish().is_ok()).unwrap_or(false);
                        bridge_send_response(hooks, if ok { RESP_ACK } else { RESP_NAK }, &[]);
                    } else {
                        let cmd = self.active_cmd;
                        let payload = std::mem::take(&mut self.payload);
                        self.reset();
                        bridge_handle_command(cmd, &payload, hooks);
                    }
                } else if at_end && terminated {
                    self.abort(hooks);
                    return;
                }
            }
        }
    }
}
