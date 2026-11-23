//! OpenFlux v2 Full-Duplex Priority-Multiplexed USB & Display Socket Router (`OFV2`).
//!
//! Replaces v1's synchronous `transact()` / SimpleHDLC / `live_json` / `CMD_POLL_BUS_FRAMES`
//! pipeline with:
//! - Binary `MuxFrame` stream on USB bulk endpoints (`EP_OUT` / `EP_IN`) after `OFV2` handshake.
//! - Binary `MuxFrame` Unix socket (`/tmp/openflux_v2.sock`) directly to `openflux-display`
//!   so `CH_BUS` (P0) and `CH_MEDIA` (P1 live H.264/JPEG) cross with `< 0.2 ms` latency and zero disk I/O.
//! - Chunked `CH_BULK_REQ` / `CH_BULK_RSP` (P2) file, clock, soc-stats, and sideload RPCs.

use base64::Engine;
use openflux_bus::{
    BulkRpcRequest, BulkRpcResponse, MuxChannel, MuxDecoder, MuxFrame, V2_UPGRADE_ACK,
    V2_UPGRADE_MAGIC,
};
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

pub const V2_MUX_SOCK_PATH: &str = "/tmp/openflux_v2.sock";

struct DisplayMuxClient {
    stream: UnixStream,
    decoder: MuxDecoder,
}

struct ActivePutFile {
    target_path: PathBuf,
    tmp_path: PathBuf,
    file: File,
}

pub struct V2BridgeMux {
    pub active: bool,
    usb_decoder: MuxDecoder,
    listener: Option<UnixListener>,
    clients: Vec<DisplayMuxClient>,
    /// Priority 0 outgoing USB frames (`CH_BUS`). Always drained before P2 bulk responses!
    tx_bus: VecDeque<Vec<u8>>,
    /// Priority 2 outgoing USB frames (`CH_BULK_RSP`).
    tx_bulk: VecDeque<Vec<u8>>,
    active_puts: HashMap<u16, ActivePutFile>,
}

impl Default for V2BridgeMux {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for V2BridgeMux {
    fn drop(&mut self) {
        self.close_socket();
    }
}

impl V2BridgeMux {
    pub fn new() -> Self {
        Self {
            active: false,
            usb_decoder: MuxDecoder::new(),
            listener: None,
            clients: Vec::new(),
            tx_bus: VecDeque::with_capacity(128),
            tx_bulk: VecDeque::with_capacity(64),
            active_puts: HashMap::new(),
        }
    }

    pub fn init_socket(&mut self) -> Result<(), String> {
        self.close_socket();
        let _ = std::fs::remove_file(V2_MUX_SOCK_PATH);
        let listener =
            UnixListener::bind(V2_MUX_SOCK_PATH).map_err(|e| format!("bind {V2_MUX_SOCK_PATH}: {e}"))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        self.listener = Some(listener);
        Ok(())
    }

    pub fn close_socket(&mut self) {
        self.clients.clear();
        self.listener = None;
        let _ = std::fs::remove_file(V2_MUX_SOCK_PATH);
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
                    let _ = stream.set_nonblocking(true);
                    self.clients.push(DisplayMuxClient {
                        stream,
                        decoder: MuxDecoder::new(),
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => break,
            }
        }
    }

    /// Resets v2 session cleanly on USB disconnect; drops display v2 socket clients so
    /// `openflux-display` immediately detects disconnect and runs `BrokerMirror::on_disconnect()` LWT!
    pub fn on_usb_disconnect(&mut self) {
        if self.active {
            self.active = false;
            self.usb_decoder.clear();
            self.tx_bus.clear();
            self.tx_bulk.clear();
            self.active_puts.clear();
            self.clients.clear();
        }
    }

    /// Checks whether an incoming USB `EP_OUT` chunk initiates the `OFV2` upgrade handshake.
    /// Returns `true` if handled as `OFV2` (either initial upgrade or active v2 stream).
    pub fn try_feed_usb(&mut self, chunk: &[u8]) -> bool {
        if !self.active {
            if chunk.len() >= V2_UPGRADE_MAGIC.len() && chunk[..4] == V2_UPGRADE_MAGIC {
                self.active = true;
                self.usb_decoder.clear();
                self.tx_bus.clear();
                self.tx_bulk.clear();
                self.tx_bus.push_back(V2_UPGRADE_ACK.to_vec());
                if chunk.len() > 4 {
                    self.feed_v2_bytes(&chunk[4..]);
                }
                return true;
            }
            return false;
        }
        // Re-handshake support if host daemon restarted without USB re-enumeration!
        if chunk.len() >= V2_UPGRADE_MAGIC.len() && chunk[..4] == V2_UPGRADE_MAGIC {
            self.usb_decoder.clear();
            self.tx_bus.clear();
            self.tx_bulk.clear();
            self.active_puts.clear();
            // Bounce display clients briefly so display re-sends fresh `Subscribe` & gets clean retained sync!
            self.clients.clear();
            self.tx_bus.push_back(V2_UPGRADE_ACK.to_vec());
            if chunk.len() > 4 {
                self.feed_v2_bytes(&chunk[4..]);
            }
            return true;
        }
        self.feed_v2_bytes(chunk);
        true
    }

    fn feed_v2_bytes(&mut self, bytes: &[u8]) {
        let frames = self.usb_decoder.feed(bytes);
        for frame in frames {
            match frame.channel {
                MuxChannel::Bus | MuxChannel::Media => {
                    self.broadcast_to_displays(&frame.encode());
                }
                MuxChannel::BulkReq => {
                    let rsp = self.handle_bulk_req(frame.stream_id, &frame.payload);
                    if let Ok(payload) = serde_json::to_vec(&rsp) {
                        let out = MuxFrame::new(MuxChannel::BulkRsp, 0, frame.stream_id, payload);
                        self.tx_bulk.push_back(out.encode());
                    }
                }
                MuxChannel::BulkRsp => {}
            }
        }
    }

    fn broadcast_to_displays(&mut self, encoded: &[u8]) {
        let mut idx = 0;
        while idx < self.clients.len() {
            match write_all_nonblocking(&mut self.clients[idx].stream, encoded) {
                Ok(()) => idx += 1,
                Err(_) => {
                    self.clients.remove(idx);
                }
            }
        }
    }

    /// Reads outgoing `CH_BUS` frames pushed by `openflux-display` and queues them at Priority 0!
    pub fn read_display_client_by_fd(&mut self, fd: RawFd) {
        let Some(idx) = self.clients.iter().position(|c| c.stream.as_raw_fd() == fd) else {
            return;
        };
        let mut tmp = [0u8; 16384];
        let mut disconnected = false;
        let mut frames = Vec::new();
        loop {
            match self.clients[idx].stream.read(&mut tmp) {
                Ok(0) => {
                    disconnected = true;
                    break;
                }
                Ok(n) => {
                    frames.extend(self.clients[idx].decoder.feed(&tmp[..n]));
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if disconnected {
            self.clients.remove(idx);
        }
        if !self.active {
            return;
        }
        for frame in frames {
            if frame.channel == MuxChannel::Bus {
                if self.tx_bus.len() < 256 {
                    self.tx_bus.push_back(frame.encode());
                }
            }
        }
    }

    /// Pops the next highest-priority USB Bulk-IN buffer to enqueue into `FfsTxQueue`:
    /// Priority 0 (`CH_BUS`) ALWAYS preempts Priority 2 (`CH_BULK_RSP`)!
    pub fn pop_next_usb_tx(&mut self) -> Option<Vec<u8>> {
        if let Some(b) = self.tx_bus.pop_front() {
            return Some(b);
        }
        self.tx_bulk.pop_front()
    }

    fn handle_bulk_req(&mut self, req_id: u16, payload: &[u8]) -> BulkRpcResponse {
        let req: BulkRpcRequest = match serde_json::from_slice(payload) {
            Ok(r) => r,
            Err(e) => {
                return BulkRpcResponse::Err {
                    message: format!("invalid bulk RPC JSON: {e}"),
                }
            }
        };
        match req {
            BulkRpcRequest::GetVersions => {
                let display_ver = std::fs::read_to_string("/usr/lib/openflux/display/version.txt")
                    .or_else(|_| std::fs::read_to_string("/tmp/openflux_display_version.txt"))
                    .unwrap_or_else(|_| "2.0.0".into());
                let os_ver = std::fs::read_to_string("/etc/openflux-os-version")
                    .or_else(|_| std::fs::read_to_string("/etc/os-release"))
                    .unwrap_or_default();
                BulkRpcResponse::Ok {
                    data: serde_json::json!({
                        "bridge": crate::protocol::BRIDGE_VERSION,
                        "display": display_ver.trim(),
                        "os": os_ver.lines().next().unwrap_or("").trim(),
                        "protocol": "ofv2",
                    }),
                }
            }
            BulkRpcRequest::GetSocStats => {
                for p in ["/tmp/openflux_soc_stats.json", "/run/openflux_soc_stats.json"] {
                    if let Ok(text) = std::fs::read_to_string(p) {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                            return BulkRpcResponse::Ok { data: val };
                        }
                    }
                }
                BulkRpcResponse::Ok {
                    data: serde_json::json!({ "offline": true }),
                }
            }
            BulkRpcRequest::StatFile { path } => {
                let p = Path::new(&path);
                let exists = p.is_file();
                let bytes = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                BulkRpcResponse::Ok {
                    data: serde_json::json!({ "exists": exists, "bytes": bytes }),
                }
            }
            BulkRpcRequest::DeleteFile { path } => match std::fs::remove_file(&path) {
                Ok(()) => BulkRpcResponse::Ok {
                    data: serde_json::json!({ "deleted": true }),
                },
                Err(e) => BulkRpcResponse::Err {
                    message: e.to_string(),
                },
            },
            BulkRpcRequest::ListFiles { dir } => match std::fs::read_dir(&dir) {
                Ok(rd) => {
                    let mut names: Vec<String> = rd
                        .flatten()
                        .filter_map(|e| e.file_name().into_string().ok())
                        .collect();
                    names.sort();
                    BulkRpcResponse::Ok {
                        data: serde_json::json!({ "files": names }),
                    }
                }
                Err(e) => BulkRpcResponse::Err {
                    message: e.to_string(),
                },
            },
            BulkRpcRequest::GetFile { path } => match std::fs::read(&path) {
                Ok(bytes) => {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    BulkRpcResponse::Ok {
                        data: serde_json::json!({ "bytes": bytes.len(), "data_b64": b64 }),
                    }
                }
                Err(e) => BulkRpcResponse::Err {
                    message: e.to_string(),
                },
            },
            BulkRpcRequest::PutFileStart { path, .. } => {
                let target = PathBuf::from(&path);
                if let Some(parent) = target.parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        return BulkRpcResponse::Err {
                            message: format!("mkdir {}: {e}", parent.display()),
                        };
                    }
                }
                let tmp = PathBuf::from(format!("{path}.ofv2.tmp.{req_id}"));
                match File::create(&tmp) {
                    Ok(file) => {
                        self.active_puts.insert(
                            req_id,
                            ActivePutFile {
                                target_path: target,
                                tmp_path: tmp,
                                file,
                            },
                        );
                        BulkRpcResponse::Ok {
                            data: serde_json::json!({ "ready": true }),
                        }
                    }
                    Err(e) => BulkRpcResponse::Err {
                        message: format!("create {}: {e}", tmp.display()),
                    },
                }
            }
            BulkRpcRequest::PutFileChunk { data_b64, .. } => {
                let Some(active) = self.active_puts.get_mut(&req_id) else {
                    return BulkRpcResponse::Err {
                        message: "no active PutFileStart for req_id".into(),
                    };
                };
                let bytes = match base64::engine::general_purpose::STANDARD.decode(&data_b64) {
                    Ok(b) => b,
                    Err(e) => {
                        return BulkRpcResponse::Err {
                            message: format!("invalid base64 chunk: {e}"),
                        }
                    }
                };
                if let Err(e) = active.file.write_all(&bytes) {
                    return BulkRpcResponse::Err {
                        message: format!("write chunk: {e}"),
                    };
                }
                BulkRpcResponse::Ok {
                    data: serde_json::json!({ "written": bytes.len() }),
                }
            }
            BulkRpcRequest::PutFileFinish { .. } => {
                let Some(mut active) = self.active_puts.remove(&req_id) else {
                    return BulkRpcResponse::Err {
                        message: "no active PutFileStart for req_id".into(),
                    };
                };
                let _ = active.file.sync_all();
                drop(active.file);
                match std::fs::rename(&active.tmp_path, &active.target_path) {
                    Ok(()) => BulkRpcResponse::Ok {
                        data: serde_json::json!({ "path": active.target_path.to_string_lossy() }),
                    },
                    Err(e) => {
                        let _ = std::fs::remove_file(&active.tmp_path);
                        BulkRpcResponse::Err {
                            message: format!("rename failed: {e}"),
                        }
                    }
                }
            }
            BulkRpcRequest::SetClock { utc_sec, timezone } => {
                #[cfg(target_os = "linux")]
                {
                    let ts = libc::timespec {
                        tv_sec: utc_sec as _,
                        tv_nsec: 0,
                    };
                    unsafe {
                        let _ = libc::clock_settime(libc::CLOCK_REALTIME, &ts);
                    }
                    if !timezone.is_empty() && !timezone.contains("..") {
                        let tz_src = format!("/usr/share/zoneinfo/{timezone}");
                        if Path::new(&tz_src).is_file() {
                            let _ = std::fs::remove_file("/etc/localtime");
                            let _ = std::os::unix::fs::symlink(&tz_src, "/etc/localtime");
                        }
                    }
                }
                let _ = (utc_sec, timezone);
                BulkRpcResponse::Ok {
                    data: serde_json::json!({ "set": true }),
                }
            }
            BulkRpcRequest::SideloadIpk { remote_path } => {
                // Spawn detached opkg install so response ACKs cleanly before service restarts!
                let cmd = format!(
                    "sleep 1; opkg install --force-reinstall --force-downgrade '{remote_path}' >/tmp/openflux_sideload.log 2>&1"
                );
                let _ = std::process::Command::new("/bin/sh")
                    .args(["-c", &format!("({cmd}) </dev/null >/dev/null 2>&1 &")])
                    .spawn();
                BulkRpcResponse::Ok {
                    data: serde_json::json!({ "scheduled": true, "path": remote_path }),
                }
            }
        }
    }
}

fn write_all_nonblocking(stream: &mut UnixStream, mut data: &[u8]) -> io::Result<()> {
    let mut spins = 0u32;
    while !data.is_empty() {
        match stream.write(data) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "display mux socket closed",
                ))
            }
            Ok(n) => {
                data = &data[n..];
                spins = 0;
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                spins += 1;
                if spins > 40 {
                    return Err(e);
                }
                std::thread::sleep(std::time::Duration::from_micros(250));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
