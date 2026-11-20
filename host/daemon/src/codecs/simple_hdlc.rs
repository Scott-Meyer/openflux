//! SimpleHDLC framing and the live data packets the host sends to the display through the bridge.

use std::fmt;

pub const HDLC_FLAG: u8 = 0x7E;
pub const HDLC_ESCAPE: u8 = 0x7D;
pub const HDLC_ESCAPE_XOR: u8 = 0x20;
pub const HDLC_MAX_PAYLOAD: usize = 8192;
pub const LIVE_DATA_MAX_CHUNK: usize = 8000;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataType {
    Json = 0,
    Heartbeat = 4,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HdlcError(pub String);

impl fmt::Display for HdlcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for HdlcError {}

const fn build_reverse_table() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut rev = 0u8;
        let mut bit = 0u8;
        while bit < 8 {
            rev = (rev << 1) | (((i as u8) >> bit) & 1);
            bit += 1;
        }
        t[i] = rev;
        i += 1;
    }
    t
}

const fn build_crc32_table() -> [u32; 256] {
    let poly: u32 = 0x04C1_1DB7;
    let mut t = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut crc = (i as u32) << 24;
        let mut b = 0;
        while b < 8 {
            if (crc & 0x8000_0000) != 0 {
                crc = (crc << 1) ^ poly;
            } else {
                crc <<= 1;
            }
            b += 1;
        }
        t[i] = crc;
        i += 1;
    }
    t
}

static REVERSE_TABLE: [u8; 256] = build_reverse_table();
static CRC32_TABLE: [u32; 256] = build_crc32_table();

/// SimpleHDLC CRC-32: polynomial 0x04C11DB7, reflected input bytes, initial value and final XOR
/// 0xFFFFFFFF.
pub fn compute_crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in data {
        let idx = (((crc >> 24) as u8) ^ REVERSE_TABLE[b as usize]) as usize;
        crc = CRC32_TABLE[idx] ^ (crc << 8);
    }
    !crc
}

fn push_escaped(out: &mut Vec<u8>, byte: u8) {
    if byte == HDLC_FLAG || byte == HDLC_ESCAPE {
        out.push(HDLC_ESCAPE);
        out.push(byte ^ HDLC_ESCAPE_XOR);
    } else {
        out.push(byte);
    }
}

pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, HdlcError> {
    if payload.len() > u16::MAX as usize {
        return Err(HdlcError(format!(
            "Payload too large for SimpleHDLC: {}",
            payload.len()
        )));
    }
    let mut out = Vec::with_capacity(payload.len() + 16);
    out.push(HDLC_FLAG);
    let len = payload.len() as u16;
    push_escaped(&mut out, (len >> 8) as u8);
    push_escaped(&mut out, (len & 0xFF) as u8);
    for &b in payload {
        push_escaped(&mut out, b);
    }
    let crc = compute_crc32(payload);
    push_escaped(&mut out, ((crc >> 24) & 0xFF) as u8);
    push_escaped(&mut out, ((crc >> 16) & 0xFF) as u8);
    push_escaped(&mut out, ((crc >> 8) & 0xFF) as u8);
    push_escaped(&mut out, (crc & 0xFF) as u8);
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct SimpleHdlcDecoder {
    max_payload: usize,
    state: u8,
    escaped: bool,
    expected_len: usize,
    buf: Vec<u8>,
}

impl Default for SimpleHdlcDecoder {
    fn default() -> Self {
        Self::new(HDLC_MAX_PAYLOAD)
    }
}

impl SimpleHdlcDecoder {
    pub fn new(max_payload: usize) -> Self {
        Self {
            max_payload,
            state: 0,
            escaped: false,
            expected_len: 0,
            buf: Vec::new(),
        }
    }

    pub fn reset(&mut self) {
        self.state = 0;
        self.escaped = false;
        self.expected_len = 0;
        self.buf.clear();
    }

    pub fn feed(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        let mut frames = Vec::new();
        for &raw in data {
            if raw == HDLC_FLAG {
                self.state = 1;
                self.escaped = false;
                self.expected_len = 0;
                self.buf.clear();
                continue;
            }
            if self.state == 0 {
                continue;
            }
            let b = if self.escaped {
                self.escaped = false;
                raw ^ HDLC_ESCAPE_XOR
            } else if raw == HDLC_ESCAPE {
                self.escaped = true;
                continue;
            } else {
                raw
            };

            match self.state {
                1 => {
                    self.expected_len = (b as usize) << 8;
                    self.state = 2;
                }
                2 => {
                    self.expected_len |= b as usize;
                    if self.expected_len > self.max_payload {
                        self.reset();
                    } else {
                        self.buf.clear();
                        self.state = 3;
                    }
                }
                3 => {
                    self.buf.push(b);
                    if self.buf.len() == self.expected_len + 4 {
                        let payload = &self.buf[..self.expected_len];
                        let rx_crc = u32::from_be_bytes([
                            self.buf[self.expected_len],
                            self.buf[self.expected_len + 1],
                            self.buf[self.expected_len + 2],
                            self.buf[self.expected_len + 3],
                        ]);
                        if compute_crc32(payload) == rx_crc {
                            frames.push(payload.to_vec());
                        }
                        self.reset();
                    }
                }
                _ => self.reset(),
            }
        }
        frames
    }
}

pub fn encode_live_data(data_type: DataType, data: &[u8]) -> Result<Vec<u8>, HdlcError> {
    let mut full = Vec::with_capacity(1 + data.len());
    full.push(data_type as u8);
    full.extend_from_slice(data);

    let mut out = Vec::with_capacity(full.len() + 32);
    let total_len =
        u32::try_from(full.len()).map_err(|_| HdlcError("Live data payload exceeds u32".into()))?;
    let mut ctrl = [0u8; 5];
    ctrl[0] = 0xFF;
    ctrl[1..5].copy_from_slice(&total_len.to_be_bytes());
    out.extend_from_slice(&encode_frame(&ctrl)?);

    for chunk in full.chunks(LIVE_DATA_MAX_CHUNK) {
        out.extend_from_slice(&encode_frame(chunk)?);
    }
    Ok(out)
}

pub fn encode_live_json(payload: &serde_json::Value) -> Result<Vec<u8>, HdlcError> {
    let s = serde_json::to_string(payload).map_err(|e| HdlcError(e.to_string()))?;
    encode_live_data(DataType::Json, s.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_simple_hdlc_standard_vector() {
        assert_eq!(compute_crc32(b"123456789"), 0x649c_2fd3);
    }

    #[test]
    fn encode_decode_escape_bytes_and_chunking() {
        let payload = vec![0x7E, 0x7D, 0x00, 0xFF, 0x7E, 0x42];
        let frame = encode_frame(&payload).unwrap();
        let mut dec = SimpleHdlcDecoder::default();
        let mut got = Vec::new();
        for ch in frame.chunks(2) {
            got.extend(dec.feed(ch));
        }
        assert_eq!(got, vec![payload]);
    }

    #[test]
    fn rejects_corrupted_crc() {
        let mut frame = encode_frame(b"hello flux").unwrap();
        let last = frame.len() - 1;
        frame[last] ^= 0x01;
        let mut dec = SimpleHdlcDecoder::default();
        assert!(dec.feed(&frame).is_empty());
    }

    #[test]
    fn heartbeat_and_small_json_always_include_length_frame() {
        // Known-good heartbeat frame.
        let hb = encode_live_data(DataType::Heartbeat, &[]).unwrap();
        let expected_hb = [
            0x7e, 0x00, 0x05, 0xff, 0x00, 0x00, 0x00, 0x01, 0xe0, 0x00, 0x82, 0xde, 0x7e, 0x00,
            0x01, 0x04, 0x29, 0xd4, 0xf6, 0xab,
        ];
        assert_eq!(hb, expected_hb);

        let small = encode_live_json(&serde_json::json!({"hardwareData": {"cpu": 12}})).unwrap();
        let mut dec = SimpleHdlcDecoder::default();
        let frames = dec.feed(&small);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0][0], 0xFF);
        assert_eq!(frames[1][0], DataType::Json as u8);
    }

    #[test]
    fn live_json_multi_chunk_roundtrip() {
        let big = "A".repeat(17000);
        let wire = encode_live_json(&serde_json::json!({"big": big})).unwrap();
        let mut dec = SimpleHdlcDecoder::default();
        let frames = dec.feed(&wire);
        assert_eq!(frames.len(), 4); // 1 ctrl + 3 chunks
        assert_eq!(frames[0][0], 0xFF);
        let adv = u32::from_be_bytes([frames[0][1], frames[0][2], frames[0][3], frames[0][4]]);
        let body: Vec<u8> = frames[1..].concat();
        assert_eq!(body.len(), adv as usize);
        assert_eq!(body[0], DataType::Json as u8);
    }
}
