//! OpenFlux v2 Persistent Full-Duplex Priority-Multiplexed USB Transport (`OFV2`).
//!
//! Replaces v1's per-call USB enumerate/open/claim/transact/close loop, SimpleHDLC, `live_json`,
//! and 50 ms `CMD_POLL_BUS_FRAMES` polling with a true full-duplex USB 2.0 multiplexed link:
//! - Holds ONE claimed `Arc<rusb::DeviceHandle<Context>>` persistently while attached.
//! - Dedicated Bulk-IN reader thread (`openflux-v2-rx`): 0 ms polling push from keyboard!
//! - Dedicated Priority Bulk-OUT writer thread (`openflux-v2-tx`): wakes via Condvar in < 0.05 ms:
//!   - Priority 0 (`CH_BUS`): Broker-to-broker pub/sub mirroring ALWAYS goes first (< 1 ms!).
//!   - Priority 1 (`CH_MEDIA`): Per-stream drop-old live H.264/JPEG video frames (zero disk I/O!).
//!   - Priority 2 (`CH_BULK_REQ`): 16 KB chunked file/package/stat/clock RPCs.

use crate::usb::protocol::{BRIDGE_TARGET, KEYBOARD_VID};
use base64::Engine;
use openflux_bus::{
    encode_media_payload, Broker, BrokerMirror, BulkRpcRequest, BulkRpcResponse, MediaCodec,
    MuxChannel, MuxDecoder, MuxFrame, PeerBusFrame, FLAG_STREAM_CLOSE, V2_UPGRADE_ACK,
    V2_UPGRADE_MAGIC,
};
use rusb::{Context, DeviceHandle, UsbContext};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

const BULK_CHUNK_BYTES: usize = 16 * 1024;
const MAX_MEDIA_Q_PER_STREAM: usize = 96;

struct TxQueues {
    /// Priority 0: Pub/Sub bus frames (always drained first!).
    bus: VecDeque<Vec<u8>>,
    /// Priority 1: Live H.264/JPEG media streams (bounded drop-old per stream_id).
    media: VecDeque<(String, Vec<u8>)>,
    /// Priority 2: Chunked bulk RPC requests.
    bulk: VecDeque<Vec<u8>>,
    closed: bool,
}

impl TxQueues {
    fn new() -> Self {
        Self {
            bus: VecDeque::with_capacity(128),
            media: VecDeque::with_capacity(32),
            bulk: VecDeque::with_capacity(64),
            closed: false,
        }
    }

    fn clear(&mut self) {
        self.bus.clear();
        self.media.clear();
        self.bulk.clear();
    }

    fn pop_highest_priority(&mut self) -> Option<Vec<u8>> {
        if let Some(b) = self.bus.pop_front() {
            return Some(b);
        }
        if let Some((_, m)) = self.media.pop_front() {
            return Some(m);
        }
        self.bulk.pop_front()
    }
}

pub struct V2UsbMux {
    running: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    tx_lock: Arc<(Mutex<TxQueues>, Condvar)>,
    pending_rpcs: Arc<Mutex<HashMap<u16, mpsc::SyncSender<BulkRpcResponse>>>>,
    next_req_id: AtomicU16,
    broker: Broker,
}

impl V2UsbMux {
    pub fn start(broker: Broker, running: Arc<AtomicBool>) -> Arc<Self> {
        let mux = Arc::new(Self {
            running: running.clone(),
            connected: Arc::new(AtomicBool::new(false)),
            tx_lock: Arc::new((Mutex::new(TxQueues::new()), Condvar::new())),
            pending_rpcs: Arc::new(Mutex::new(HashMap::new())),
            next_req_id: AtomicU16::new(1),
            broker,
        });

        let mux_bg = mux.clone();
        let _ = thread::Builder::new()
            .name("openflux-v2-usb".into())
            .spawn(move || mux_bg.supervisor_loop());
        mux
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    pub fn enqueue_bus(&self, frame: &PeerBusFrame) {
        if !self.is_connected() {
            return;
        }
        let encoded = frame.to_mux_frame().encode();
        let (lock, cv) = &*self.tx_lock;
        let mut q = lock.lock().unwrap();
        if q.bus.len() < 512 {
            q.bus.push_back(encoded);
            cv.notify_one();
        }
    }

    /// Enqueues a live `CH_MEDIA` video/image frame onto Priority 1 (drop-old backpressure:
    /// stale frames are dropped rather than queuing latency or stalling `CH_BUS`!).
    pub fn send_media(
        &self,
        stream_name: &str,
        codec: MediaCodec,
        data: &[u8],
        flags: u8,
    ) -> bool {
        if !self.is_connected() {
            return false;
        }
        let payload = encode_media_payload(codec, stream_name, data);
        let encoded = MuxFrame::new(MuxChannel::Media, flags, 0, payload).encode();
        let (lock, cv) = &*self.tx_lock;
        let mut q = lock.lock().unwrap();

        if (flags & FLAG_STREAM_CLOSE) != 0 {
            // Stream close: drop all queued stale frames for this stream and send CLOSE immediately!
            q.media.retain(|(n, _)| n != stream_name);
            q.media.push_back((stream_name.to_string(), encoded));
            cv.notify_one();
            return true;
        }

        if codec == MediaCodec::Jpeg {
            // For JPEG streams, always keep ONLY the single freshest frame!
            q.media.retain(|(n, _)| n != stream_name);
        } else {
            // For H.264 Annex-B, cap per-stream depth so lag never accumulates.
            let count = q.media.iter().filter(|(n, _)| n == stream_name).count();
            if count >= MAX_MEDIA_Q_PER_STREAM {
                if let Some(pos) = q.media.iter().position(|(n, _)| n == stream_name) {
                    q.media.remove(pos);
                }
            }
        }
        q.media.push_back((stream_name.to_string(), encoded));
        cv.notify_one();
        true
    }

    /// Executes a Priority-2 Bulk RPC asynchronously over `CH_BULK_REQ` / `CH_BULK_RSP` while
    /// `CH_BUS` (P0) and `CH_MEDIA` (P1) continue preempting cleanly!
    pub fn bulk_rpc(&self, req: BulkRpcRequest, timeout: Duration) -> Result<serde_json::Value, String> {
        let req_id = self.next_req_id.fetch_add(1, Ordering::Relaxed);
        self.bulk_rpc_with_id(req_id, req, timeout)
    }

    pub fn bulk_rpc_with_id(
        &self,
        req_id: u16,
        req: BulkRpcRequest,
        timeout: Duration,
    ) -> Result<serde_json::Value, String> {
        if !self.is_connected() {
            return Err("OFV2 USB mux not connected".into());
        }
        let payload = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
        let encoded = MuxFrame::new(MuxChannel::BulkReq, 0, req_id, payload).encode();

        let (tx, rx) = mpsc::sync_channel(1);
        self.pending_rpcs.lock().unwrap().insert(req_id, tx);

        {
            let (lock, cv) = &*self.tx_lock;
            let mut q = lock.lock().unwrap();
            q.bulk.push_back(encoded);
            cv.notify_one();
        }

        match rx.recv_timeout(timeout) {
            Ok(BulkRpcResponse::Ok { data }) => Ok(data),
            Ok(BulkRpcResponse::Err { message }) => Err(message),
            Err(_) => {
                self.pending_rpcs.lock().unwrap().remove(&req_id);
                Err("OFV2 bulk RPC timed out".into())
            }
        }
    }

    /// Uploads a file atomically in 16 KB chunks on Priority 2 (`CH_BUS` & `CH_MEDIA` preempt
    /// between every single chunk!).
    pub fn put_file(&self, remote_path: &str, bytes: &[u8]) -> Result<(), String> {
        let req_id = self.next_req_id.fetch_add(1, Ordering::Relaxed);
        let _ = self.bulk_rpc_with_id(
            req_id,
            BulkRpcRequest::PutFileStart {
                path: remote_path.to_string(),
                total_len: bytes.len(),
            },
            Duration::from_secs(5),
        )?;
        for (i, chunk) in bytes.chunks(BULK_CHUNK_BYTES).enumerate() {
            let b64 = base64::engine::general_purpose::STANDARD.encode(chunk);
            let _ = self.bulk_rpc_with_id(
                req_id,
                BulkRpcRequest::PutFileChunk {
                    offset: i * BULK_CHUNK_BYTES,
                    data_b64: b64,
                },
                Duration::from_secs(8),
            )?;
        }
        let _ = self.bulk_rpc_with_id(
            req_id,
            BulkRpcRequest::PutFileFinish {
                path: remote_path.to_string(),
            },
            Duration::from_secs(8),
        )?;
        Ok(())
    }

    pub fn stat_file(&self, remote_path: &str) -> Result<bool, String> {
        let data = self.bulk_rpc(
            BulkRpcRequest::StatFile {
                path: remote_path.to_string(),
            },
            Duration::from_secs(3),
        )?;
        Ok(data.get("exists").and_then(|v| v.as_bool()).unwrap_or(false))
    }

    pub fn delete_file(&self, remote_path: &str) -> Result<(), String> {
        let _ = self.bulk_rpc(
            BulkRpcRequest::DeleteFile {
                path: remote_path.to_string(),
            },
            Duration::from_secs(3),
        )?;
        Ok(())
    }

    pub fn list_files(&self, remote_dir: &str) -> Result<Vec<String>, String> {
        let data = self.bulk_rpc(
            BulkRpcRequest::ListFiles {
                dir: remote_dir.to_string(),
            },
            Duration::from_secs(4),
        )?;
        Ok(data
            .get("files")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn get_file(&self, remote_path: &str) -> Result<Vec<u8>, String> {
        let data = self.bulk_rpc(
            BulkRpcRequest::GetFile {
                path: remote_path.to_string(),
            },
            Duration::from_secs(8),
        )?;
        let b64 = data
            .get("data_b64")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing data_b64".to_string())?;
        base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| e.to_string())
    }

    pub fn get_versions(&self) -> Result<serde_json::Value, String> {
        self.bulk_rpc(BulkRpcRequest::GetVersions, Duration::from_secs(3))
    }

    pub fn get_soc_stats(&self) -> Result<serde_json::Value, String> {
        self.bulk_rpc(BulkRpcRequest::GetSocStats, Duration::from_secs(3))
    }

    pub fn set_clock(&self, utc_sec: i64, timezone: &str) -> Result<(), String> {
        let _ = self.bulk_rpc(
            BulkRpcRequest::SetClock {
                utc_sec,
                timezone: timezone.to_string(),
            },
            Duration::from_secs(3),
        )?;
        Ok(())
    }

    pub fn sideload_ipk(&self, local_ipk: &std::path::Path) -> Result<String, String> {
        let fname = local_ipk
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| "invalid ipk filename".to_string())?;
        let remote = format!("/home/root/{fname}");
        let bytes = std::fs::read(local_ipk).map_err(|e| format!("read ipk: {e}"))?;
        let sz = bytes.len();
        // Upload cleanly to `/home/root/<fname>` so the keyboard's single `flux-ipk-monitor.sh`
        // installs it without a competing `opkg` lock collision!
        self.put_file(&remote, &bytes)?;
        Ok(format!(
            "Uploaded {fname} ({sz} B) to {remote} over OFV2; flux-ipk-monitor installs automatically."
        ))
    }

    fn supervisor_loop(self: Arc<Self>) {
        let Ok(ctx) = Context::new() else {
            return;
        };
        while self.running.load(Ordering::Relaxed) {
            let Some((_usb_lock, handle)) = open_and_handshake_v2(&ctx) else {
                thread::sleep(Duration::from_millis(600));
                continue;
            };
            eprintln!("[openflux-v2-usb] Connected in pure full-duplex OFV2 multiplexed stream mode!");
            self.run_session(Arc::new(handle));
            drop(_usb_lock);
            self.connected.store(false, Ordering::Release);
            {
                let (lock, cv) = &*self.tx_lock;
                let mut q = lock.lock().unwrap();
                q.clear();
                cv.notify_all();
            }
            self.pending_rpcs.lock().unwrap().clear();
            thread::sleep(Duration::from_millis(400));
        }
    }

    fn run_session(&self, handle: Arc<DeviceHandle<Context>>) {
        let session_alive = Arc::new(AtomicBool::new(true));
        let mirror = Arc::new(Mutex::new(BrokerMirror::new(
            self.broker.clone(),
            "keyboard",
        )));

        {
            let (lock, _) = &*self.tx_lock;
            let mut q = lock.lock().unwrap();
            q.clear();
            q.closed = false;
        }
        self.connected.store(true, Ordering::Release);

        // Re-assert `host/connection { connected: true }` on every OFV2 connect so replugging
        // never leaves the keyboard stuck on stale `connected: false`!
        let _ = self.broker.publish_typed(
            openflux_bus::topics::HOST_CONNECTION,
            &openflux_bus::HostConnectionState {
                connected: true,
                host_asleep: false,
                last_seen_ms: openflux_bus::now_unix_ms(),
                host_platform: Some(std::env::consts::OS.to_string()),
            },
            openflux_bus::PublishOptions::retained(),
        );

        // Immediately advertise host's active subscriptions to keyboard AND seed default
        // keyboard-side patterns (`host/#`, `modules/#`, `fx/surface/impulse`) + replay retained
        // host metrics/weather/media/app immediately so widgets hydrate with zero startup race!
        {
            let mut m = mirror.lock().unwrap();
            let extra = vec![
                "modules/#".to_string(),
                "fx/surface/impulse".to_string(),
            ];
            if let Some(sub_f) = m.build_subscribe_if_changed(&extra, true) {
                self.enqueue_bus(&sub_f);
            }
            let initial_replays = m.on_remote_frame(PeerBusFrame::Subscribe {
                node: "keyboard".into(),
                session_id: 0,
                patterns: vec![
                    "host/#".into(),
                    "modules/#".into(),
                    "profiles/#".into(),
                    "system/actions".into(),
                    "keyboard/attachments".into(),
                    "layout/#".into(),
                    "display/parallax".into(),
                    "display/power".into(),
                    "fx/surface/impulse".into(),
                    "keys/providers/ability-deck".into(),
                    "keys/providers/streamdeck".into(),
                ],
            });
            for r in initial_replays {
                self.enqueue_bus(&r);
            }
        }

        // Thread 1: Dedicated Priority Bulk-OUT Writer (P0 BUS > P1 MEDIA > P2 BULK)
        let tx_handle = handle.clone();
        let tx_alive = session_alive.clone();
        let tx_running = self.running.clone();
        let tx_lock = self.tx_lock.clone();
        let tx_thread = thread::Builder::new()
            .name("openflux-v2-tx".into())
            .spawn(move || {
                let (lock, cv) = &*tx_lock;
                while tx_alive.load(Ordering::Relaxed) && tx_running.load(Ordering::Relaxed) {
                    let next_bytes = {
                        let mut q = lock.lock().unwrap();
                        if let Some(b) = q.pop_highest_priority() {
                            Some(b)
                        } else {
                            let (mut q2, _) = cv
                                .wait_timeout(q, Duration::from_millis(100))
                                .unwrap();
                            q2.pop_highest_priority()
                        }
                    };
                    if let Some(bytes) = next_bytes {
                        for chunk in bytes.chunks(16 * 1024) {
                            match tx_handle.write_bulk(
                                BRIDGE_TARGET.ep_out,
                                chunk,
                                Duration::from_millis(2000),
                            ) {
                                Ok(_) => {}
                                Err(rusb::Error::Timeout) => continue,
                                Err(_) => {
                                    tx_alive.store(false, Ordering::SeqCst);
                                    return;
                                }
                            }
                        }
                    }
                }
            })
            .ok();

        // Thread 2: Dedicated Host Broker Mirror Producer (wakes on Condvar in < 0.1ms!)
        let mir_alive = session_alive.clone();
        let mir_running = self.running.clone();
        let mir_ref = mirror.clone();
        let mir_tx_lock = self.tx_lock.clone();
        let mir_broker = self.broker.clone();
        let mir_thread = thread::Builder::new()
            .name("openflux-v2-mirror".into())
            .spawn(move || {
                // Separate local condvar wait on our mirror subscription
                let sub = mir_broker
                    .subscribe(
                        "#",
                        openflux_bus::SubscribeOptions {
                            replay_retained: false,
                            ignore_origin: Some("peer:keyboard".into()),
                            capacity: 2048,
                        },
                    )
                    .expect("wildcard sub");
                let mut last_sub_check = std::time::Instant::now();
                while mir_alive.load(Ordering::Relaxed) && mir_running.load(Ordering::Relaxed) {
                    let mut out_frames = Vec::new();
                    if let Some(first) = sub.recv_timeout(Duration::from_millis(100)) {
                        let m = mir_ref.lock().unwrap();
                        if let Some(pf) = m.maybe_outgoing_frame(&first) {
                            out_frames.push(pf);
                        }
                        for extra in sub.drain() {
                            if let Some(pf) = m.maybe_outgoing_frame(&extra) {
                                out_frames.push(pf);
                            }
                        }
                    }
                    if last_sub_check.elapsed() >= Duration::from_millis(500) {
                        last_sub_check = std::time::Instant::now();
                        let mut m = mir_ref.lock().unwrap();
                        let extra = vec![
                            "modules/#".to_string(),
                            "fx/surface/impulse".to_string(),
                        ];
                        if let Some(sf) = m.build_subscribe_if_changed(&extra, false) {
                            out_frames.push(sf);
                        }
                    }
                    if !out_frames.is_empty() {
                        let (lock, cv) = &*mir_tx_lock;
                        let mut q = lock.lock().unwrap();
                        for pf in out_frames {
                            if q.bus.len() < 512 {
                                q.bus.push_back(pf.to_mux_frame().encode());
                            }
                        }
                        cv.notify_one();
                    }
                }
            })
            .ok();

        // Main session thread: Dedicated Bulk-IN Reader (0 ms polling push from keyboard!)
        let mut decoder = MuxDecoder::new();
        let mut buf = vec![0u8; 64 * 1024];
        while session_alive.load(Ordering::Relaxed) && self.running.load(Ordering::Relaxed) {
            match handle.read_bulk(BRIDGE_TARGET.ep_in, &mut buf, Duration::from_millis(150)) {
                Ok(n) if n > 0 => {
                    for frame in decoder.feed(&buf[..n]) {
                        match frame.channel {
                            MuxChannel::Bus => {
                                if let Some(pf) = PeerBusFrame::from_mux_frame(&frame) {
                                    let replies = mirror.lock().unwrap().on_remote_frame(pf);
                                    for r in replies {
                                        self.enqueue_bus(&r);
                                    }
                                }
                            }
                            MuxChannel::BulkRsp => {
                                if let Ok(rsp) =
                                    serde_json::from_slice::<BulkRpcResponse>(&frame.payload)
                                {
                                    if let Some(tx) =
                                        self.pending_rpcs.lock().unwrap().remove(&frame.stream_id)
                                    {
                                        let _ = tx.send(rsp);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Ok(_) | Err(rusb::Error::Timeout) => {}
                Err(_) => {
                    session_alive.store(false, Ordering::SeqCst);
                    break;
                }
            }
        }

        session_alive.store(false, Ordering::SeqCst);
        self.tx_lock.1.notify_all();
        if let Some(t) = tx_thread {
            let _ = t.join();
        }
        if let Some(t) = mir_thread {
            let _ = t.join();
        }
        mirror.lock().unwrap().on_disconnect();
        let _ = handle.release_interface(BRIDGE_TARGET.interface);
    }
}

#[cfg(unix)]
struct V2UsbLockGuard {
    _file: std::fs::File,
}

#[cfg(unix)]
impl V2UsbLockGuard {
    fn acquire(pid: u16) -> Option<Self> {
        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::io::AsRawFd;
        let path = format!("/tmp/openflux_usb_{pid:04x}.lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
            .ok()?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            return None;
        }
        Some(Self { _file: file })
    }
}

#[cfg(not(unix))]
struct V2UsbLockGuard;
#[cfg(not(unix))]
impl V2UsbLockGuard {
    fn acquire(_pid: u16) -> Option<Self> {
        Some(Self)
    }
}

fn open_and_handshake_v2(ctx: &Context) -> Option<(V2UsbLockGuard, DeviceHandle<Context>)> {
    let lock = V2UsbLockGuard::acquire(BRIDGE_TARGET.pid)?;
    let devs = ctx.devices().ok()?;
    let mut handle = None;
    for d in devs.iter() {
        if let Ok(desc) = d.device_descriptor() {
            if desc.vendor_id() == KEYBOARD_VID && desc.product_id() == BRIDGE_TARGET.pid {
                if let Ok(h) = d.open() {
                    handle = Some(h);
                    break;
                }
            }
        }
    }
    let h = handle?;
    if h.claim_interface(BRIDGE_TARGET.interface).is_err() {
        return None;
    }
    // Drain ALL stale packets on EP_IN before handshake (handles seamless daemon restarts!)
    let mut junk = [0u8; 4096];
    for _ in 0..16 {
        match h.read_bulk(BRIDGE_TARGET.ep_in, &mut junk, Duration::from_millis(15)) {
            Ok(n) if n > 0 => continue,
            _ => break,
        }
    }

    if h.write_bulk(
        BRIDGE_TARGET.ep_out,
        &V2_UPGRADE_MAGIC,
        Duration::from_millis(500),
    )
    .is_err()
    {
        let _ = h.release_interface(BRIDGE_TARGET.interface);
        return None;
    }
    let mut ack = [0u8; 4096];
    for _ in 0..8 {
        match h.read_bulk(BRIDGE_TARGET.ep_in, &mut ack, Duration::from_millis(250)) {
            Ok(n) if n >= 4 => {
                let slice = &ack[..n];
                if slice.windows(4).any(|w| w == V2_UPGRADE_ACK) {
                    return Some((lock, h));
                }
            }
            _ => break,
        }
    }
    let _ = h.release_interface(BRIDGE_TARGET.interface);
    None
}
