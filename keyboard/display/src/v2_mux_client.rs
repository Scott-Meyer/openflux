//! Binary `OFV2` Unix socket client (`openflux-display` <-> `openflux-bridge` at `/tmp/openflux_v2.sock`).
//!
//! Receives:
//! - `CH_BUS`: `PeerBusFrame` messages for subscription-driven `BrokerMirror` mirroring (< 0.2 ms!).
//! - `CH_MEDIA`: raw H.264 Annex-B (fed straight into hardware `LiveRkmppDecoder`) or in-memory
//!   JPEG frames (zero disk writes, drop-old backpressure, instant transparent close on EOF!).

#[cfg(unix)]
use openflux_bus::{
    decode_media_payload, MediaCodec, MuxChannel, MuxDecoder, PeerBusFrame, FLAG_STREAM_CLOSE,
};
#[cfg(unix)]
use std::collections::HashMap;
#[cfg(unix)]
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
#[cfg(unix)]
use std::os::unix::net::UnixStream;

pub const V2_MUX_SOCK_PATH: &str = "/tmp/openflux_v2.sock";

#[cfg(target_os = "linux")]
pub enum LiveMediaSlot {
    H264(crate::rkmpp::LiveRkmppDecoder),
    Jpeg(Option<(i32, i32, Vec<u32>)>),
}

#[cfg(not(target_os = "linux"))]
pub enum LiveMediaSlot {
    Jpeg(Option<(i32, i32, Vec<u32>)>),
}

#[cfg(unix)]
pub struct V2MuxPollResult {
    pub bus_frames: Vec<PeerBusFrame>,
    pub stream_changes: Vec<(String, bool, &'static str)>,
    pub connected_now: bool,
    pub disconnected: bool,
}

#[cfg(unix)]
pub struct V2MuxClient {
    stream: Option<UnixStream>,
    decoder: MuxDecoder,
    pub streams: HashMap<String, LiveMediaSlot>,
    last_attempt: Option<std::time::Instant>,
}

#[cfg(unix)]
impl Default for V2MuxClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(unix)]
impl V2MuxClient {
    pub fn new() -> Self {
        Self {
            stream: None,
            decoder: MuxDecoder::new(),
            streams: HashMap::new(),
            last_attempt: None,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    pub fn raw_fd(&self) -> Option<i32> {
        self.stream.as_ref().map(|s| s.as_raw_fd())
    }

    /// Attempts a non-blocking connect to `/tmp/openflux_v2.sock`. Returns `true` ONLY on a
    /// fresh connection transition so the caller immediately sends its initial `Subscribe` frame.
    fn ensure_connected(&mut self) -> bool {
        if self.stream.is_some() {
            return false;
        }
        if let Some(prev) = self.last_attempt {
            if prev.elapsed() < std::time::Duration::from_millis(250) {
                return false;
            }
        }
        self.last_attempt = Some(std::time::Instant::now());
        if let Ok(s) = UnixStream::connect(V2_MUX_SOCK_PATH) {
            let _ = s.set_nonblocking(true);
            self.decoder.clear();
            self.stream = Some(s);
            true
        } else {
            false
        }
    }

    pub fn send_bus_frame(&mut self, frame: &PeerBusFrame) {
        let Some(ref mut s) = self.stream else {
            return;
        };
        let encoded = frame.to_mux_frame().encode();
        if s.write_all(&encoded).is_err() {
            self.stream = None;
            self.decoder.clear();
            self.streams.clear();
        }
    }

    /// Automatically connects/reconnects to `/tmp/openflux_v2.sock` and polls all available
    /// binary `MuxFrame`s with an explicit, unambiguous [`V2MuxPollResult`].
    pub fn poll(&mut self) -> V2MuxPollResult {
        let connected_now = self.ensure_connected();
        let mut bus_out = Vec::new();
        let mut stream_events = Vec::new();
        let mut raw_frames = Vec::new();
        let mut disconnected = false;
        let mut tmp = [0u8; 65536];

        if let Some(ref mut s) = self.stream {
            loop {
                match s.read(&mut tmp) {
                    Ok(0) => {
                        disconnected = true;
                        break;
                    }
                    Ok(n) => {
                        raw_frames.extend(self.decoder.feed(&tmp[..n]));
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        } else {
            return V2MuxPollResult {
                bus_frames: Vec::new(),
                stream_changes: Vec::new(),
                connected_now,
                disconnected: false,
            };
        }

        for frame in raw_frames {
            match frame.channel {
                MuxChannel::Bus => {
                    if let Some(pf) = PeerBusFrame::from_mux_frame(&frame) {
                        bus_out.push(pf);
                    }
                }
                MuxChannel::Media => {
                    if let Some((codec, name, data)) = decode_media_payload(&frame.payload) {
                        self.ingest_media(codec, name, data, frame.flags, &mut stream_events);
                    }
                }
                _ => {}
            }
        }

        if disconnected {
            self.stream = None;
            self.decoder.clear();
            for name in self.streams.keys().cloned().collect::<Vec<_>>() {
                stream_events.push((name, false, "closed"));
            }
            self.streams.clear();
        }

        V2MuxPollResult {
            bus_frames: bus_out,
            stream_changes: stream_events,
            connected_now,
            disconnected,
        }
    }

    fn ingest_media(
        &mut self,
        codec: MediaCodec,
        name: &str,
        data: &[u8],
        flags: u8,
        events: &mut Vec<(String, bool, &'static str)>,
    ) {
        if (flags & FLAG_STREAM_CLOSE) != 0 {
            if self.streams.remove(name).is_some() {
                events.push((name.to_string(), false, "closed"));
            }
            return;
        }
        match codec {
            MediaCodec::H264AnnexB => {
                #[cfg(target_os = "linux")]
                {
                    let is_new = !matches!(self.streams.get(name), Some(LiveMediaSlot::H264(_)));
                    if is_new {
                        if let Some(dec) = crate::rkmpp::LiveRkmppDecoder::open_h264() {
                            self.streams
                                .insert(name.to_string(), LiveMediaSlot::H264(dec));
                            events.push((name.to_string(), true, "h264"));
                        }
                    }
                    if let Some(LiveMediaSlot::H264(dec)) = self.streams.get(name) {
                        if !data.is_empty() {
                            dec.push_annexb(data.to_vec());
                        }
                    }
                }
                #[cfg(not(target_os = "linux"))]
                {
                    let _ = (name, data, events);
                }
            }
            MediaCodec::Jpeg => {
                let is_new = !self.streams.contains_key(name);
                if let Ok(img) = image::load_from_memory(data) {
                    let rgba = img.to_rgba8();
                    let (w, h) = rgba.dimensions();
                    let mut argb = Vec::with_capacity((w * h) as usize);
                    for px in rgba.chunks_exact(4) {
                        argb.push(
                            ((px[3] as u32) << 24)
                                | ((px[0] as u32) << 16)
                                | ((px[1] as u32) << 8)
                                | (px[2] as u32),
                        );
                    }
                    self.streams
                        .insert(name.to_string(), LiveMediaSlot::Jpeg(Some((w as i32, h as i32, argb))));
                    if is_new {
                        events.push((name.to_string(), true, "jpeg"));
                    }
                }
            }
        }
    }

    pub fn has_stream(&self, name: &str) -> bool {
        self.streams.contains_key(name)
    }

    #[cfg(target_os = "linux")]
    pub fn take_h264_frame(&self, name: &str) -> Option<crate::rkmpp::Nv12Frame> {
        if let Some(LiveMediaSlot::H264(dec)) = self.streams.get(name) {
            return dec.take_latest();
        }
        None
    }

    pub fn take_jpeg_frame(&mut self, name: &str) -> Option<(i32, i32, Vec<u32>)> {
        if let Some(LiveMediaSlot::Jpeg(slot)) = self.streams.get_mut(name) {
            return slot.take();
        }
        None
    }
}
