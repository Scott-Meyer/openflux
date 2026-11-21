//! Live preview of the keyboard display, rendered on the host by the same renderer the keyboard
//! runs (`openflux-display`), at 1920x1080 on macOS, Windows and Linux.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::io::AsRawFd;

pub fn release_process_heap_pages() {
    #[cfg(target_os = "macos")]
    unsafe {
        extern "C" {
            fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
        }
        let _ = malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    unsafe {
        extern "C" {
            fn malloc_trim(pad: usize) -> std::ffi::c_int;
        }
        let _ = malloc_trim(0);
    }
}

pub fn kill_and_reap_child(slot: &Arc<Mutex<Option<Child>>>) {
    if let Ok(mut guard) = slot.lock() {
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(unix)]
fn set_nonblocking_fd(fd: std::os::unix::io::RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            let _ = libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
}

pub type PreviewKeyRect = openflux_display::FluxKeyRect;

fn downsample_rgba_half(rgba_1080: &[u8]) -> Vec<u8> {
    if rgba_1080.len() != 1920 * 1080 * 4 {
        return rgba_1080.to_vec();
    }
    let mut out = vec![0u8; 960 * 540 * 4];
    for y in 0..540usize {
        let r0 = (y * 2) * 1920 * 4;
        let r1 = (y * 2 + 1) * 1920 * 4;
        let dst_row = y * 960 * 4;
        for x in 0..960usize {
            let s0 = r0 + x * 8;
            let s1 = r1 + x * 8;
            let d = dst_row + x * 4;
            for c in 0..4usize {
                let sum = rgba_1080[s0 + c] as u16
                    + rgba_1080[s0 + 4 + c] as u16
                    + rgba_1080[s1 + c] as u16
                    + rgba_1080[s1 + 4 + c] as u16;
                out[d + c] = (sum >> 2) as u8;
            }
        }
    }
    out
}

fn config_payload_hash(
    a: Option<&str>,
    s: Option<&str>,
    k: Option<&str>,
    m: Option<&str>,
    md: Option<&str>,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    a.hash(&mut h);
    s.hash(&mut h);
    k.hash(&mut h);
    m.hash(&mut h);
    md.hash(&mut h);
    h.finish()
}

/// Runs the `--preview-worker` subprocess loop reading length-framed commands from stdin and
/// writing responses to stdout, so the daemon itself never keeps the renderer's memory after close.
pub fn run_preview_worker_stdio() -> Result<(), String> {
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    let mut disp = DisplayPreview::new(1920, 1080)?;
    let mut last_cfg_hash: Option<u64> = None;
    loop {
        let mut hdr = [0u8; 5];
        match stdin.read_exact(&mut hdr) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(format!("worker read hdr: {e}")),
        }
        let cmd = hdr[0];
        let len = u32::from_le_bytes([hdr[1], hdr[2], hdr[3], hdr[4]]) as usize;
        if len > 16 * 1024 * 1024 {
            return Err("worker payload too large".into());
        }
        let mut payload = vec![0u8; len];
        if len > 0 {
            stdin.read_exact(&mut payload).map_err(|e| e.to_string())?;
        }
        let result: Result<Vec<u8>, String> = (|| match cmd {
            1 => {
                let v: serde_json::Value =
                    serde_json::from_slice(&payload).map_err(|e| e.to_string())?;
                let a = v.get("appearance").and_then(|x| x.as_str());
                let s = v.get("settings").and_then(|x| x.as_str());
                let k = v.get("keymap").and_then(|x| x.as_str());
                let m = v.get("mosaic").and_then(|x| x.as_str());
                let md = v.get("mediaDir").and_then(|x| x.as_str());
                let ch = config_payload_hash(a, s, k, m, md);
                if last_cfg_hash != Some(ch) {
                    disp.set_config_json(a, s, k, m, md)?;
                    last_cfg_hash = Some(ch);
                }
                Ok(Vec::new())
            }
            2 => {
                let v: serde_json::Value =
                    serde_json::from_slice(&payload).map_err(|e| e.to_string())?;
                let live = v.get("liveJson").and_then(|x| x.as_str()).unwrap_or("{}");
                let now_ms = v.get("nowMs").and_then(|x| x.as_u64()).unwrap_or(0);
                disp.apply_live_json(live, now_ms)?;
                Ok(Vec::new())
            }
            3 => {
                let rects: Vec<serde_json::Value> = disp
                    .get_key_rects()
                    .into_iter()
                    .map(|r| serde_json::json!({ "keyIndex": r.key_index, "x": r.x, "y": r.y, "w": r.w, "h": r.h }))
                    .collect();
                serde_json::to_vec(&rects).map_err(|e| e.to_string())
            }
            4 => {
                let v: serde_json::Value =
                    serde_json::from_slice(&payload).map_err(|e| e.to_string())?;
                let now_sec = v.get("nowSec").and_then(|x| x.as_i64()).unwrap_or(0);
                let now_ms = v.get("nowMs").and_then(|x| x.as_u64()).unwrap_or(0);
                let suspended = v
                    .get("suspended")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false);
                let half_res = v.get("halfRes").and_then(|x| x.as_bool()).unwrap_or(false);
                let full = disp.render_rgba(now_sec, now_ms, suspended)?;
                Ok(if half_res {
                    downsample_rgba_half(&full)
                } else {
                    full
                })
            }
            5 => {
                let v: serde_json::Value =
                    serde_json::from_slice(&payload).map_err(|e| e.to_string())?;
                let now_sec = v.get("nowSec").and_then(|x| x.as_i64()).unwrap_or(0);
                let now_ms = v.get("nowMs").and_then(|x| x.as_u64()).unwrap_or(0);
                let suspended = v
                    .get("suspended")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false);
                disp.render_ppm_bytes(now_sec, now_ms, suspended)
            }
            _ => Err(format!("unknown preview worker cmd {cmd}")),
        })();
        match result {
            Ok(bytes) => {
                let mut rh = [0u8; 5];
                rh[0] = 0;
                rh[1..5].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
                stdout.write_all(&rh).map_err(|e| e.to_string())?;
                if !bytes.is_empty() {
                    stdout.write_all(&bytes).map_err(|e| e.to_string())?;
                }
                stdout.flush().map_err(|e| e.to_string())?;
            }
            Err(err) => {
                let eb = err.as_bytes();
                let mut rh = [0u8; 5];
                rh[0] = 1;
                rh[1..5].copy_from_slice(&(eb.len() as u32).to_le_bytes());
                stdout.write_all(&rh).map_err(|e| e.to_string())?;
                stdout.write_all(eb).map_err(|e| e.to_string())?;
                stdout.flush().map_err(|e| e.to_string())?;
            }
        }
    }
}

pub struct PreviewWorkerProcess {
    child: Arc<Mutex<Option<Child>>>,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl PreviewWorkerProcess {
    pub fn spawn(
        exe: &Path,
        child_slot: Arc<Mutex<Option<Child>>>,
        epoch_check: Option<(&Arc<AtomicU64>, u64)>,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(exe);
        cmd.arg("--preview-worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawn --preview-worker: {e}"))?;
        let stdin = child.stdin.take().ok_or("missing worker stdin")?;
        let stdout = child.stdout.take().ok_or("missing worker stdout")?;
        #[cfg(unix)]
        {
            set_nonblocking_fd(stdin.as_raw_fd());
            set_nonblocking_fd(stdout.as_raw_fd());
        }
        *child_slot.lock().unwrap() = Some(child);
        if let Some((cur_epoch, expected)) = epoch_check {
            if cur_epoch.load(Ordering::SeqCst) != expected {
                kill_and_reap_child(&child_slot);
                return Err("CLOSED_DURING_LAUNCH".into());
            }
        }
        Ok(Self {
            child: child_slot,
            stdin,
            stdout,
        })
    }

    fn rpc_timeout() -> Duration {
        let ms = std::env::var("OPENFLUX_PREVIEW_RPC_TIMEOUT_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(5000);
        Duration::from_millis(ms)
    }

    fn read_exact_bounded(&mut self, buf: &mut [u8], deadline: Instant) -> Result<(), String> {
        let mut off = 0usize;
        while off < buf.len() {
            let rem = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| "preview worker RPC read timeout".to_string())?;
            // Without poll, the deadline is checked between reads only.
            #[cfg(not(unix))]
            let _ = rem;
            #[cfg(unix)]
            {
                let mut pfd = libc::pollfd {
                    fd: self.stdout.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ms = rem.as_millis().clamp(1, i32::MAX as u128) as std::ffi::c_int;
                let rc = unsafe { libc::poll(&mut pfd, 1, ms) };
                if rc == 0 {
                    return Err("preview worker RPC read timeout".into());
                }
                if rc < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(format!("preview worker poll error: {e}"));
                }
                if (pfd.revents & (libc::POLLERR | libc::POLLNVAL)) != 0 {
                    return Err("preview worker pipe error".into());
                }
            }
            match self.stdout.read(&mut buf[off..]) {
                Ok(0) => return Err("preview worker EOF".into()),
                Ok(n) => off += n,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::Interrupted =>
                {
                    continue
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }

    fn write_all_bounded(&mut self, buf: &[u8], deadline: Instant) -> Result<(), String> {
        let mut off = 0usize;
        while off < buf.len() {
            let rem = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| "preview worker RPC write timeout".to_string())?;
            // Without poll, the deadline is checked between writes only.
            #[cfg(not(unix))]
            let _ = rem;
            #[cfg(unix)]
            {
                let mut pfd = libc::pollfd {
                    fd: self.stdin.as_raw_fd(),
                    events: libc::POLLOUT,
                    revents: 0,
                };
                let ms = rem.as_millis().clamp(1, i32::MAX as u128) as std::ffi::c_int;
                let rc = unsafe { libc::poll(&mut pfd, 1, ms) };
                if rc == 0 {
                    return Err("preview worker RPC write timeout".into());
                }
                if rc < 0 {
                    let e = std::io::Error::last_os_error();
                    if e.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(format!("preview worker write poll error: {e}"));
                }
                if (pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL)) != 0 {
                    return Err("preview worker broken pipe".into());
                }
            }
            match self.stdin.write(&buf[off..]) {
                Ok(0) => return Err("preview worker short write".into()),
                Ok(n) => off += n,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::Interrupted =>
                {
                    continue
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }

    pub fn rpc_stream_to<F>(
        &mut self,
        cmd: u8,
        payload: &[u8],
        mut on_chunk: F,
    ) -> Result<(), String>
    where
        F: FnMut(&[u8]) -> Result<(), String>,
    {
        let timeout = Self::rpc_timeout();
        let deadline = Instant::now() + timeout;

        // A deadline watchdog that works on every platform, including Windows, where pipes block.
        let done_pair = Arc::new((Mutex::new(false), Condvar::new()));
        let done_wd = done_pair.clone();
        let child_wd = self.child.clone();
        let timed_out = Arc::new(AtomicBool::new(false));
        let timed_out_wd = timed_out.clone();
        let wd = std::thread::spawn(move || {
            let (m, cv) = &*done_wd;
            let mut g = m.lock().unwrap();
            while !*g {
                let Some(rem) = deadline.checked_duration_since(Instant::now()) else {
                    timed_out_wd.store(true, Ordering::SeqCst);
                    kill_and_reap_child(&child_wd);
                    break;
                };
                let (ng, res) = cv.wait_timeout(g, rem).unwrap();
                g = ng;
                if res.timed_out() && !*g {
                    timed_out_wd.store(true, Ordering::SeqCst);
                    kill_and_reap_child(&child_wd);
                    break;
                }
            }
        });

        let io_res: Result<(), String> = (|| {
            let mut hdr = [0u8; 5];
            hdr[0] = cmd;
            hdr[1..5].copy_from_slice(&(payload.len() as u32).to_le_bytes());
            self.write_all_bounded(&hdr, deadline)?;
            if !payload.is_empty() {
                self.write_all_bounded(payload, deadline)?;
            }
            let _ = self.stdin.flush();

            let mut rh = [0u8; 5];
            self.read_exact_bounded(&mut rh, deadline)?;
            let status = rh[0];
            let rlen = u32::from_le_bytes([rh[1], rh[2], rh[3], rh[4]]) as usize;
            if rlen > 16 * 1024 * 1024 {
                return Err("preview worker response exceeded 16 MiB".into());
            }
            if status != 0 {
                let mut eb = vec![0u8; rlen];
                if rlen > 0 {
                    self.read_exact_bounded(&mut eb, deadline)?;
                }
                return Err(String::from_utf8_lossy(&eb).to_string());
            }
            let mut rem = rlen;
            let mut buf = [0u8; 65536];
            while rem > 0 {
                let want = rem.min(buf.len());
                self.read_exact_bounded(&mut buf[..want], deadline)?;
                on_chunk(&buf[..want])?;
                rem -= want;
            }
            Ok(())
        })();

        {
            let (m, cv) = &*done_pair;
            *m.lock().unwrap() = true;
            cv.notify_all();
        }
        let _ = wd.join();
        if io_res.is_err() {
            kill_and_reap_child(&self.child);
            if timed_out.load(Ordering::SeqCst) {
                return Err("preview worker RPC timed out".into());
            }
        }
        io_res
    }

    fn rpc(&mut self, cmd: u8, payload: &[u8]) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        self.rpc_stream_to(cmd, payload, |chunk| {
            out.extend_from_slice(chunk);
            Ok(())
        })?;
        Ok(out)
    }
}

impl Drop for PreviewWorkerProcess {
    fn drop(&mut self) {
        kill_and_reap_child(&self.child);
    }
}

/// Preview backend used by `ApiState`: an isolated `--preview-worker` subprocess inside `openfluxd`
/// (closing the preview kills it and returns its memory to the OS), or an in-process
/// `DisplayPreview` in test binaries.
pub enum PreviewBackend {
    Worker(PreviewWorkerProcess),
    Direct(Box<DisplayPreview>),
}

impl PreviewBackend {
    pub fn open_auto(
        child_slot: Arc<Mutex<Option<Child>>>,
        epoch_check: Option<(&Arc<AtomicU64>, u64)>,
    ) -> Result<Self, String> {
        if let Ok(p) = std::env::var("OPENFLUX_PREVIEW_WORKER_BIN") {
            if !p.is_empty() {
                return Ok(Self::Worker(PreviewWorkerProcess::spawn(
                    Path::new(&p),
                    child_slot,
                    epoch_check,
                )?));
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            let fname = exe.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if fname == "openfluxd" {
                return Ok(Self::Worker(PreviewWorkerProcess::spawn(
                    &exe,
                    child_slot,
                    epoch_check,
                )?));
            }
        }
        Ok(Self::Direct(Box::new(DisplayPreview::new(1920, 1080)?)))
    }

    pub fn render_rgba_stream_to_sized<F>(
        &mut self,
        now_sec: i64,
        now_ms: u64,
        suspended: bool,
        half_res: bool,
        mut on_chunk: F,
    ) -> Result<(), String>
    where
        F: FnMut(&[u8]) -> Result<(), String>,
    {
        match self {
            Self::Worker(w) => {
                let body = serde_json::to_vec(&serde_json::json!({
                    "nowSec": now_sec,
                    "nowMs": now_ms,
                    "suspended": suspended,
                    "halfRes": half_res,
                }))
                .map_err(|e| e.to_string())?;
                w.rpc_stream_to(4, &body, on_chunk)
            }
            Self::Direct(d) => {
                let full = d.render_rgba(now_sec, now_ms, suspended)?;
                let bytes = if half_res {
                    downsample_rgba_half(&full)
                } else {
                    full
                };
                for chunk in bytes.chunks(65536) {
                    on_chunk(chunk)?;
                }
                Ok(())
            }
        }
    }

    pub fn set_config_json(
        &mut self,
        appearance: Option<&str>,
        settings: Option<&str>,
        keymap: Option<&str>,
        mosaic: Option<&str>,
        media_dir: Option<&str>,
    ) -> Result<(), String> {
        match self {
            Self::Worker(w) => {
                let body = serde_json::to_vec(&serde_json::json!({
                    "appearance": appearance,
                    "settings": settings,
                    "keymap": keymap,
                    "mosaic": mosaic,
                    "mediaDir": media_dir,
                }))
                .map_err(|e| e.to_string())?;
                w.rpc(1, &body)?;
                Ok(())
            }
            Self::Direct(d) => d.set_config_json(appearance, settings, keymap, mosaic, media_dir),
        }
    }

    pub fn apply_live_json(&mut self, live_json: &str, now_ms: u64) -> Result<(), String> {
        match self {
            Self::Worker(w) => {
                let body = serde_json::to_vec(&serde_json::json!({
                    "liveJson": live_json,
                    "nowMs": now_ms,
                }))
                .map_err(|e| e.to_string())?;
                w.rpc(2, &body)?;
                Ok(())
            }
            Self::Direct(d) => d.apply_live_json(live_json, now_ms),
        }
    }

    pub fn get_key_rects_json(&mut self) -> Result<Vec<serde_json::Value>, String> {
        match self {
            Self::Worker(w) => {
                let raw = w.rpc(3, &[])?;
                serde_json::from_slice(&raw).map_err(|e| e.to_string())
            }
            Self::Direct(d) => Ok(d
                .get_key_rects()
                .into_iter()
                .map(|r| serde_json::json!({ "keyIndex": r.key_index, "x": r.x, "y": r.y, "w": r.w, "h": r.h }))
                .collect()),
        }
    }

    pub fn render_rgba(
        &mut self,
        now_sec: i64,
        now_ms: u64,
        suspended: bool,
    ) -> Result<Vec<u8>, String> {
        match self {
            Self::Worker(w) => {
                let body = serde_json::to_vec(&serde_json::json!({
                    "nowSec": now_sec,
                    "nowMs": now_ms,
                    "suspended": suspended,
                }))
                .map_err(|e| e.to_string())?;
                w.rpc(4, &body)
            }
            Self::Direct(d) => d.render_rgba(now_sec, now_ms, suspended),
        }
    }

    pub fn render_ppm_bytes(
        &mut self,
        now_sec: i64,
        now_ms: u64,
        suspended: bool,
    ) -> Result<Vec<u8>, String> {
        match self {
            Self::Worker(w) => {
                let body = serde_json::to_vec(&serde_json::json!({
                    "nowSec": now_sec,
                    "nowMs": now_ms,
                    "suspended": suspended,
                }))
                .map_err(|e| e.to_string())?;
                w.rpc(5, &body)
            }
            Self::Direct(d) => d.render_ppm_bytes(now_sec, now_ms, suspended),
        }
    }
}

pub struct DisplayPreview {
    inner: Option<openflux_display::DisplayPreview>,
}

impl Drop for DisplayPreview {
    fn drop(&mut self) {
        // Display, font and frame allocations are freed before the arena pages are trimmed.
        drop(self.inner.take());
        release_process_heap_pages();
    }
}

impl DisplayPreview {
    pub fn new(width: i32, height: i32) -> Result<Self, String> {
        Ok(Self {
            inner: Some(openflux_display::DisplayPreview::with_ffmpeg(
                width,
                height,
                crate::api::assets::find_ffmpeg().map(std::path::PathBuf::from),
            )),
        })
    }

    fn core(&self) -> &openflux_display::DisplayPreview {
        self.inner.as_ref().expect("DisplayPreview dropped")
    }

    fn core_mut(&mut self) -> &mut openflux_display::DisplayPreview {
        self.inner.as_mut().expect("DisplayPreview dropped")
    }

    pub fn load_config_dir(&mut self, config_dir: &str, media_dir: &str) -> Result<(), String> {
        self.core_mut().load_config_dir(config_dir, media_dir)
    }

    pub fn set_config_json(
        &mut self,
        appearance_json: Option<&str>,
        settings_json: Option<&str>,
        keymap_json: Option<&str>,
        mosaic_json: Option<&str>,
        media_dir: Option<&str>,
    ) -> Result<(), String> {
        self.core_mut().set_config_json(
            appearance_json,
            settings_json,
            keymap_json,
            mosaic_json,
            media_dir,
        )
    }

    pub fn apply_live_json(&mut self, json_line: &str, now_ms: u64) -> Result<(), String> {
        self.core_mut().apply_live_json(json_line, now_ms)
    }

    pub fn submit_video_frame_rgb24(
        &mut self,
        rgb24: &[u8],
        width: i32,
        height: i32,
        pts_ms: u64,
    ) -> Result<(), String> {
        self.core_mut()
            .submit_video_frame_rgb24(rgb24, width, height, pts_ms)
    }

    pub fn render_rgba(
        &mut self,
        now_sec: i64,
        now_ms: u64,
        suspended: bool,
    ) -> Result<Vec<u8>, String> {
        Ok(self.core_mut().render_rgba(now_sec, now_ms, suspended))
    }

    pub fn render_ppm_bytes(
        &mut self,
        now_sec: i64,
        now_ms: u64,
        suspended: bool,
    ) -> Result<Vec<u8>, String> {
        Ok(self.core_mut().render_ppm_bytes(now_sec, now_ms, suspended))
    }

    pub fn get_key_rects(&self) -> Vec<PreviewKeyRect> {
        self.core().get_key_rects()
    }
}
