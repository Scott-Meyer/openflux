//! Keyglass v2 Priority-Multiplexed Full-Duplex Binary Framing (`OFV2`).
//!
//! Shared across `keyglassd` (host), `keyglass-bridge` (keyboard USB daemon), and
//! `keyglass-display` (keyboard compositor) so all three speak one zero-HDLC binary stream:
//!
//! ```text
//! Offset  Size  Field
//! 0       2     Magic: [0x4F, 0x56] ("OV")
//! 2       1     Channel: 0 = BUS (P0), 1 = MEDIA (P1), 2 = BULK_REQ (P2), 3 = BULK_RSP (P2)
//! 3       1     Flags: bit0 = STREAM_CLOSE/EOF, bit1 = KEYFRAME
//! 4       2     Stream/Request ID (u16 LE)
//! 6       4     Payload length (u32 LE, <= 2 MiB)
//! 10      N     Payload bytes
//! ```

use serde::{Deserialize, Serialize};

/// 4-byte upgrade handshake sent once by `keyglassd` to switch `keyglass-bridge` into
/// pure full-duplex v2 multiplexed stream mode.
pub const V2_UPGRADE_MAGIC: [u8; 4] = *b"OFV2";
pub const V2_UPGRADE_ACK: [u8; 4] = *b"OV2!";

pub const MUX_MAGIC: [u8; 2] = [0x4F, 0x56]; // "OV"
pub const MUX_HEADER_LEN: usize = 10;
pub const MUX_MAX_PAYLOAD: usize = 2 * 1024 * 1024;

pub const FLAG_STREAM_CLOSE: u8 = 0x01;
pub const FLAG_KEYFRAME: u8 = 0x02;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MuxChannel {
    /// Priority 0: Full-duplex pub/sub broker mirroring (always preempts media & bulk!).
    Bus = 0,
    /// Priority 1: Live real-time H.264 / JPEG video & frame streams (drop-old backpressure!).
    Media = 1,
    /// Priority 2: Chunked file/module sync, stat, delete, sideload, soc-stats RPC requests.
    BulkReq = 2,
    /// Priority 2: RPC responses matching `stream_id = req_id`.
    BulkRsp = 3,
}

impl MuxChannel {
    pub fn from_u8(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Bus),
            1 => Some(Self::Media),
            2 => Some(Self::BulkReq),
            3 => Some(Self::BulkRsp),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MuxFrame {
    pub channel: MuxChannel,
    pub flags: u8,
    pub stream_id: u16,
    pub payload: Vec<u8>,
}

impl MuxFrame {
    pub fn new(channel: MuxChannel, flags: u8, stream_id: u16, payload: Vec<u8>) -> Self {
        Self {
            channel,
            flags,
            stream_id,
            payload,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(MUX_HEADER_LEN + self.payload.len());
        out.extend_from_slice(&MUX_MAGIC);
        out.push(self.channel as u8);
        out.push(self.flags);
        out.extend_from_slice(&self.stream_id.to_le_bytes());
        out.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.payload);
        out
    }
}

/// Incremental stream decoder handling arbitrary USB bulk packet splits and coalescing.
#[derive(Debug, Default)]
pub struct MuxDecoder {
    buf: Vec<u8>,
}

impl MuxDecoder {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(64 * 1024),
        }
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<MuxFrame> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            if self.buf.len() < MUX_HEADER_LEN {
                break;
            }
            // Resync to MUX_MAGIC if noise ever precedes a frame.
            if self.buf[0] != MUX_MAGIC[0] || self.buf[1] != MUX_MAGIC[1] {
                if let Some(pos) = self.buf.windows(2).position(|w| w == MUX_MAGIC) {
                    self.buf.drain(..pos);
                } else {
                    let keep = self.buf.last().copied() == Some(MUX_MAGIC[0]);
                    self.buf.clear();
                    if keep {
                        self.buf.push(MUX_MAGIC[0]);
                    }
                    break;
                }
                if self.buf.len() < MUX_HEADER_LEN {
                    break;
                }
            }
            let Some(channel) = MuxChannel::from_u8(self.buf[2]) else {
                self.buf.drain(..2);
                continue;
            };
            let flags = self.buf[3];
            let stream_id = u16::from_le_bytes([self.buf[4], self.buf[5]]);
            let len = u32::from_le_bytes([self.buf[6], self.buf[7], self.buf[8], self.buf[9]]) as usize;
            if len > MUX_MAX_PAYLOAD {
                self.buf.drain(..2);
                continue;
            }
            if self.buf.len() < MUX_HEADER_LEN + len {
                break;
            }
            let payload = self.buf[MUX_HEADER_LEN..MUX_HEADER_LEN + len].to_vec();
            self.buf.drain(..MUX_HEADER_LEN + len);
            out.push(MuxFrame {
                channel,
                flags,
                stream_id,
                payload,
            });
        }
        out
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCodec {
    /// Raw H.264 Annex-B byte stream (fed straight to RK3566 `rkmpp` hardware decoder!).
    H264AnnexB = 0,
    /// Single JPEG frame (decoded in-memory straight to GPU texture, zero disk writes).
    Jpeg = 1,
}

impl MediaCodec {
    pub fn from_u8(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::H264AnnexB),
            1 => Some(Self::Jpeg),
            _ => None,
        }
    }
}

/// Encodes a `CH_MEDIA` payload: `[codec: u8, name_len: u8, name_utf8..., data...]`.
pub fn encode_media_payload(codec: MediaCodec, stream_name: &str, data: &[u8]) -> Vec<u8> {
    let nb = stream_name.as_bytes();
    let nlen = nb.len().min(255);
    let mut out = Vec::with_capacity(2 + nlen + data.len());
    out.push(codec as u8);
    out.push(nlen as u8);
    out.extend_from_slice(&nb[..nlen]);
    out.extend_from_slice(data);
    out
}

/// Decodes a `CH_MEDIA` payload into `(codec, stream_name, data)`.
pub fn decode_media_payload(payload: &[u8]) -> Option<(MediaCodec, &str, &[u8])> {
    if payload.len() < 2 {
        return None;
    }
    let codec = MediaCodec::from_u8(payload[0])?;
    let nlen = payload[1] as usize;
    if payload.len() < 2 + nlen {
        return None;
    }
    let name = std::str::from_utf8(&payload[2..2 + nlen]).ok()?;
    Some((codec, name, &payload[2 + nlen..]))
}

/// Priority-2 Bulk RPC requests carried on `CH_BULK_REQ` (chunked so `CH_BUS` & `CH_MEDIA` preempt!).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BulkRpcRequest {
    GetVersions,
    GetSocStats,
    StatFile { path: String },
    DeleteFile { path: String },
    ListFiles { dir: String },
    GetFile { path: String },
    PutFileStart { path: String, total_len: usize },
    PutFileChunk { offset: usize, data_b64: String },
    PutFileFinish { path: String },
    SetClock { utc_sec: i64, timezone: String },
    SideloadIpk { remote_path: String },
}

/// Priority-2 Bulk RPC responses carried on `CH_BULK_RSP`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BulkRpcResponse {
    Ok {
        #[serde(default)]
        data: serde_json::Value,
    },
    Err {
        message: String,
    },
}
