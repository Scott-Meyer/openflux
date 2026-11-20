use crate::assets::AssetCache;
use crate::config::AppearanceConfig;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, TryRecvError},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};

pub trait VideoAdapter: Send + Sync {
    fn open(&self, path: &str, width: i32, height: i32) -> Option<Box<dyn VideoStream>>;
}

pub type RawVideoFrame = (u64, i32, i32, Vec<u8>);

pub trait VideoStream: Send {
    fn poll_ready(&self) -> bool {
        true
    }
    fn is_disconnected(&self) -> bool {
        false
    }
    /// Returns Ok(Some((pts_ms, w, h, rgb24))) when a frame is ready, Ok(None) if pending, Err on EOF/error.
    #[allow(clippy::result_unit_err)]
    fn try_read_frame(&mut self) -> Result<Option<RawVideoFrame>, ()>;
}

struct FfmpegVideoStream {
    rx: Option<Receiver<RawVideoFrame>>,
    queued: Arc<std::sync::atomic::AtomicUsize>,
    disconnected: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<std::process::Child>>>,
    done: Arc<(Mutex<bool>, std::sync::Condvar)>,
    join: Option<JoinHandle<()>>,
}

impl Drop for FfmpegVideoStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        drop(self.rx.take());
        // Kill the process group so wrapper scripts and their children exit too, and bound the join so a stalled decoder cannot hang Drop.
        if let Ok(mut g) = self.child.lock() {
            if let Some(mut child) = g.take() {
                #[cfg(unix)]
                unsafe {
                    let pgid = child.id() as libc::pid_t;
                    if pgid > 0 {
                        let _ = libc::kill(-pgid, libc::SIGKILL);
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        let (mtx, cv) = &*self.done;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        if let Ok(mut g) = mtx.lock() {
            while !*g {
                let Some(rem) = deadline.checked_duration_since(std::time::Instant::now()) else {
                    break;
                };
                let (ng, _) = cv.wait_timeout(g, rem).unwrap();
                g = ng;
            }
            if *g {
                if let Some(jh) = self.join.take() {
                    let _ = jh.join();
                }
            }
        }
    }
}

impl VideoStream for FfmpegVideoStream {
    fn poll_ready(&self) -> bool {
        self.queued.load(Ordering::SeqCst) > 0
    }

    fn is_disconnected(&self) -> bool {
        self.disconnected.load(Ordering::SeqCst)
    }

    fn try_read_frame(&mut self) -> Result<Option<(u64, i32, i32, Vec<u8>)>, ()> {
        let Some(ref rx) = self.rx else {
            return Err(());
        };
        match rx.try_recv() {
            Ok(f) => {
                self.queued.fetch_sub(1, Ordering::SeqCst);
                Ok(Some(f))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(()),
        }
    }
}

#[derive(Clone)]
pub struct FfmpegVideoAdapter {
    /// Wait briefly for the first decoded frame so a one-shot PPM/RGBA render does not race decoder startup.
    pub sync_first_frame: bool,
    /// Optional downscale (for example, 960x540 on ARM64 SoCs for faster decode and a 2x upscale pass).
    pub target_scale: Option<(i32, i32)>,
    /// Decoder executable to use instead of FFmpeg; overrides `OPENFLUX_VIDEO_DECODER`.
    pub decoder_bin: Option<String>,
    /// Explicit FFmpeg CLI executable path. On host platforms this must be provided by the caller;
    /// when `None` on Linux keyboard targets, defaults to `DISPLAY_FFMPEG_PATH` with no system/PATH fallback.
    pub ffmpeg_bin: Option<std::path::PathBuf>,
}

impl Default for FfmpegVideoAdapter {
    fn default() -> Self {
        Self {
            sync_first_frame: false,
            target_scale: if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
                Some((960, 540))
            } else {
                None
            },
            decoder_bin: None,
            ffmpeg_bin: None,
        }
    }
}

impl FfmpegVideoAdapter {
    pub fn resolve_ffmpeg_bin(&self) -> Option<std::path::PathBuf> {
        if let Some(ref bin) = self.ffmpeg_bin {
            return (!bin.as_os_str().is_empty()).then(|| bin.clone());
        }
        if cfg!(target_os = "linux") {
            return Some(std::path::PathBuf::from(
                crate::device_paths::DISPLAY_FFMPEG_PATH,
            ));
        }
        None
    }
}

impl VideoAdapter for FfmpegVideoAdapter {
    fn open(&self, path: &str, width: i32, height: i32) -> Option<Box<dyn VideoStream>> {
        if path.is_empty() || width <= 0 || height <= 0 || !std::path::Path::new(path).is_file() {
            return None;
        }
        let (dw, dh) = self.target_scale.unwrap_or((width, height));
        let default_fps = if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
            18
        } else {
            24
        };
        let fps = std::env::var("OPENFLUX_VIDEO_FPS")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|&f| (5..=60).contains(&f))
            .unwrap_or(default_fps);
        let dec = self.decoder_bin.clone().or_else(|| {
            std::env::var("OPENFLUX_VIDEO_DECODER")
                .ok()
                .filter(|s| !s.is_empty())
        });

        let mut cmd = if let Some(bin) = dec {
            let mut c = Command::new(bin);
            c.args([path, &dw.to_string(), &dh.to_string()]);
            c
        } else {
            let ff_bin = self.resolve_ffmpeg_bin()?;
            let mut c = Command::new(ff_bin);
            c.args([
                "-loglevel",
                "error",
                "-stream_loop",
                "-1",
                "-i",
                path,
                "-vf",
                &format!("scale={dw}:{dh}:flags=fast_bilinear,fps={fps}"),
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "-",
            ]);
            c
        };

        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().ok()?;
        let mut stdout = child.stdout.take()?;

        let (tx, rx) = mpsc::sync_channel(4);
        let has_first = Arc::new(AtomicBool::new(false));
        let queued = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let disconnected = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let child_slot = Arc::new(Mutex::new(Some(child)));
        let done = Arc::new((Mutex::new(false), std::sync::Condvar::new()));

        let first_bg = has_first.clone();
        let queued_bg = queued.clone();
        let disc_bg = disconnected.clone();
        let stop_bg = stop.clone();
        let child_bg = child_slot.clone();
        let done_bg = done.clone();

        let frame_bytes = (dw as usize) * (dh as usize) * 3;
        let jh = thread::spawn(move || {
            let mut frame_idx: u64 = 0;
            let mut buf = vec![0u8; frame_bytes];
            while !stop_bg.load(Ordering::SeqCst) {
                let mut got = 0usize;
                let mut err = false;
                while got < frame_bytes {
                    match stdout.read(&mut buf[got..]) {
                        Ok(0) => {
                            err = true;
                            break;
                        }
                        Ok(n) => got += n,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => {
                            err = true;
                            break;
                        }
                    }
                }
                if err || stop_bg.load(Ordering::SeqCst) {
                    first_bg.store(true, Ordering::SeqCst);
                    break;
                }
                let pts_ms = (frame_idx * 1000) / fps as u64;
                frame_idx += 1;
                queued_bg.fetch_add(1, Ordering::SeqCst);
                if tx.send((pts_ms, dw, dh, buf.clone())).is_err() {
                    queued_bg.fetch_sub(1, Ordering::SeqCst);
                    break;
                }
                first_bg.store(true, Ordering::SeqCst);
            }
            drop(tx);
            disc_bg.store(true, Ordering::SeqCst);
            if let Ok(mut g) = child_bg.lock() {
                if let Some(mut c) = g.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
            }
            let (mtx, cv) = &*done_bg;
            if let Ok(mut g) = mtx.lock() {
                *g = true;
                cv.notify_all();
            }
        });

        if self.sync_first_frame {
            for _ in 0..150 {
                if has_first.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        Some(Box::new(FfmpegVideoStream {
            rx: Some(rx),
            queued,
            disconnected,
            stop,
            child: child_slot,
            done,
            join: Some(jh),
        }))
    }
}

pub struct BgCache {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u32>,
    loaded_path: String,
    loaded_blur: f64,
    loaded_dimming: f64,
    loaded_accent: u32,
    loaded_hue: f64,
    loaded_speed: f64,
    has_loaded_once: bool,
    custom_rgba: Option<(i32, i32, Vec<u32>)>,
    loaded_custom_dirty: bool,
    pub is_video: bool,
    pub has_submitted_video: bool,
    pub submitted_bg_path: String,
    submitted_frame: Option<(u64, i32, i32, Vec<u8>)>,
    video_stream: Option<Box<dyn VideoStream>>,
    video_adapter: Option<Arc<dyn VideoAdapter>>,
    pub video_epoch_set: bool,
    pub video_epoch_ms: u64,
    pub video_frames_rendered: u64,
}

impl BgCache {
    pub fn new(width: i32, height: i32) -> Self {
        let mut pixels = vec![0u32; (width as usize) * (height as usize)];
        render_fallback_gradient(&mut pixels, width, height, 0xFF66CCFF);
        Self {
            width,
            height,
            pixels,
            loaded_path: String::new(),
            loaded_blur: -1.0,
            loaded_dimming: -1.0,
            loaded_accent: 0,
            loaded_hue: 0.0,
            loaded_speed: 1.0,
            has_loaded_once: false,
            custom_rgba: None,
            loaded_custom_dirty: false,
            is_video: false,
            has_submitted_video: false,
            submitted_bg_path: String::new(),
            submitted_frame: None,
            video_stream: None,
            video_adapter: None,
            video_epoch_set: false,
            video_epoch_ms: 0,
            video_frames_rendered: 0,
        }
    }

    pub fn set_video_adapter(&mut self, adapter: Option<Arc<dyn VideoAdapter>>) {
        self.close_video();
        self.video_adapter = adapter;
    }

    pub fn close_video(&mut self) {
        self.video_stream = None;
        self.is_video = false;
        self.has_submitted_video = false;
        self.submitted_bg_path.clear();
        self.submitted_frame = None;
        self.video_epoch_set = false;
        self.video_epoch_ms = 0;
    }

    pub fn invalidate(&mut self) {
        self.close_video();
        self.loaded_path.clear();
        self.has_loaded_once = false;
        self.loaded_custom_dirty = true;
    }

    pub fn set_custom_rgba(&mut self, rgba: &[u8], width: i32, height: i32) -> Result<(), String> {
        if width <= 0 || height <= 0 || rgba.len() < (width as usize) * (height as usize) * 4 {
            return Err("Invalid custom RGBA dimensions".into());
        }
        self.close_video();
        let n = (width as usize) * (height as usize);
        let mut argb = Vec::with_capacity(n);
        for px in rgba[..n * 4].chunks_exact(4) {
            argb.push(
                ((px[3] as u32) << 24)
                    | ((px[0] as u32) << 16)
                    | ((px[1] as u32) << 8)
                    | (px[2] as u32),
            );
        }
        self.custom_rgba = Some((width, height, argb));
        self.loaded_custom_dirty = true;
        Ok(())
    }

    pub fn clear_custom(&mut self) {
        self.custom_rgba = None;
        self.loaded_custom_dirty = true;
    }

    pub fn submit_video_frame_rgb24(
        &mut self,
        bg_path: &str,
        rgb24: &[u8],
        width: i32,
        height: i32,
        pts_ms: u64,
    ) -> Result<(), String> {
        if width <= 0 || height <= 0 || rgb24.len() < (width as usize) * (height as usize) * 3 {
            return Err("Invalid video frame dimensions".into());
        }
        self.video_stream = None;
        if !self.has_submitted_video || self.submitted_bg_path != bg_path {
            self.video_epoch_set = false;
            self.video_epoch_ms = 0;
        }
        self.submitted_frame = Some((
            pts_ms,
            width,
            height,
            rgb24[..(width as usize * height as usize * 3)].to_vec(),
        ));
        self.is_video = true;
        self.has_submitted_video = true;
        self.submitted_bg_path = bg_path.to_string();
        Ok(())
    }

    pub fn has_ready_video_frame_at(&self, now_ms: u64) -> bool {
        if let Some((pts_ms, _, _, _)) = self.submitted_frame {
            if !self.video_epoch_set {
                return true;
            }
            let elapsed_ms = now_ms.saturating_sub(self.video_epoch_ms);
            return pts_ms <= elapsed_ms;
        }
        self.video_stream
            .as_ref()
            .map(|s| s.poll_ready() || s.is_disconnected())
            .unwrap_or(false)
    }

    pub fn has_ready_video_frame(&self) -> bool {
        self.has_ready_video_frame_at(u64::MAX / 2)
    }

    pub fn next_video_due_ms(&mut self) -> Option<u64> {
        if self.submitted_frame.is_none() {
            if let Some(ref mut s) = self.video_stream {
                match s.try_read_frame() {
                    Ok(Some(f)) => self.submitted_frame = Some(f),
                    Ok(None) => return None,
                    Err(()) => {
                        self.close_video();
                        return None;
                    }
                }
            }
        }
        if let Some((pts_ms, _, _, _)) = self.submitted_frame {
            return Some(if self.video_epoch_set {
                self.video_epoch_ms.saturating_add(pts_ms)
            } else {
                0
            });
        }
        None
    }

    pub fn step(
        &mut self,
        assets: &mut AssetCache,
        app: &AppearanceConfig,
        media_dir: Option<&str>,
        now_ms: u64,
    ) {
        if self.custom_rgba.is_none() && is_shader_path(&app.bg_path) {
            self.close_video();
            render_procedural_shader(
                &mut self.pixels,
                self.width,
                self.height,
                &app.bg_path,
                (now_ms as f32 * 0.001) * (app.shader_speed as f32),
                app.shader_hue_deg as f32,
                app.bg_dimming,
            );
            self.loaded_path = app.bg_path.clone();
            self.loaded_dimming = app.bg_dimming;
            self.loaded_hue = app.shader_hue_deg;
            self.loaded_speed = app.shader_speed;
            self.has_loaded_once = true;
            return;
        }
        let full_path = resolve_bg_media_path(media_dir, &app.bg_path);

        if self.has_loaded_once
            && !self.loaded_custom_dirty
            && self.loaded_path == full_path
            && (self.loaded_blur - app.bg_blur).abs() < 1e-6
            && (self.loaded_dimming - app.bg_dimming).abs() < 1e-6
            && self.loaded_accent == app.ring_glow_argb
            && (self.loaded_hue - app.shader_hue_deg).abs() < 1e-6
            && (self.loaded_speed - app.shader_speed).abs() < 1e-6
        {
            if self.is_video {
                self.step_video(app, now_ms);
            }
            return;
        }

        if self.has_submitted_video
            && self.custom_rgba.is_none()
            && self.submitted_bg_path == app.bg_path
            && (full_path.is_empty() || is_video_path(&full_path))
        {
            self.loaded_path = full_path;
            self.loaded_blur = app.bg_blur;
            self.loaded_dimming = app.bg_dimming;
            self.loaded_accent = app.ring_glow_argb;
            self.loaded_custom_dirty = false;
            self.has_loaded_once = true;
            self.step_video(app, now_ms);
            return;
        }

        self.close_video();

        let mut loaded = false;
        if let Some((cw, ch, ref src)) = self.custom_rgba {
            scale_argb_to_argb(
                src,
                cw,
                ch,
                &mut self.pixels,
                self.width,
                self.height,
                app.bg_dimming,
            );
            loaded = true;
        } else if !full_path.is_empty() {
            if is_video_path(&full_path) {
                if let Some(ref adapter) = self.video_adapter {
                    if let Some(stream) = adapter.open(&full_path, self.width, self.height) {
                        self.video_stream = Some(stream);
                        self.is_video = true;
                        loaded = true;
                    }
                }
            } else if has_suffix(&full_path, ".ppm") {
                loaded = load_ppm_p6(
                    &full_path,
                    &mut self.pixels,
                    self.width,
                    self.height,
                    app.bg_dimming,
                );
            } else if let Some(im) = assets.load_image(media_dir, &full_path) {
                scale_rgba_to_argb(
                    &im.rgba,
                    im.width as i32,
                    im.height as i32,
                    &mut self.pixels,
                    self.width,
                    self.height,
                    app.bg_dimming,
                );
                loaded = true;
            }
        }

        if !loaded {
            render_fallback_gradient(
                &mut self.pixels,
                self.width,
                self.height,
                app.ring_glow_argb,
            );
            if app.bg_dimming > 0.0 {
                apply_dimming(&mut self.pixels, app.bg_dimming);
            }
        } else if app.bg_blur > 0.5 && !self.is_video {
            apply_box_blur(
                &mut self.pixels,
                self.width,
                self.height,
                app.bg_blur as i32,
            );
        }

        self.loaded_path = full_path;
        self.loaded_blur = app.bg_blur;
        self.loaded_dimming = app.bg_dimming;
        self.loaded_accent = app.ring_glow_argb;
        self.loaded_hue = app.shader_hue_deg;
        self.loaded_speed = app.shader_speed;
        self.loaded_custom_dirty = false;
        self.has_loaded_once = true;

        if self.is_video {
            self.step_video(app, now_ms);
        }
    }

    fn step_video(&mut self, app: &AppearanceConfig, now_ms: u64) {
        let mut chosen: Option<(u64, i32, i32, Vec<u8>)> = None;
        for _ in 0..8 {
            if self.submitted_frame.is_none() {
                if let Some(ref mut vs) = self.video_stream {
                    match vs.try_read_frame() {
                        Ok(Some(f)) => self.submitted_frame = Some(f),
                        Ok(None) => {}
                        Err(()) => {
                            self.close_video();
                            // Invalidate background cache on decoder EOF so the next step() reopens the stream.
                            self.has_loaded_once = false;
                            break;
                        }
                    }
                }
            }
            let Some((pts_ms, _, _, _)) = self.submitted_frame else {
                if self.video_stream.is_none() && !self.video_epoch_set {
                    self.video_epoch_ms = now_ms;
                    self.video_epoch_set = true;
                }
                break;
            };
            if !self.video_epoch_set {
                self.video_epoch_ms = now_ms;
                self.video_epoch_set = true;
            }
            let mut elapsed_ms = now_ms.saturating_sub(self.video_epoch_ms);
            if self.video_stream.is_some() && elapsed_ms > pts_ms + 1000 {
                self.video_epoch_ms = now_ms.saturating_sub(pts_ms);
                elapsed_ms = pts_ms;
            }
            if pts_ms > elapsed_ms {
                break;
            }
            chosen = self.submitted_frame.take();
            self.video_frames_rendered += 1;
            if self.video_stream.is_none() {
                break;
            }
        }
        if let Some((_, sw, sh, rgb)) = chosen {
            scale_rgb24_to_argb(
                &rgb,
                sw,
                sh,
                &mut self.pixels,
                self.width,
                self.height,
                app.bg_dimming,
            );
            if app.bg_blur > 0.5 {
                apply_box_blur(
                    &mut self.pixels,
                    self.width,
                    self.height,
                    (app.bg_blur as i32).min(4),
                );
            }
        }
    }
}

fn has_suffix(s: &str, sfx: &str) -> bool {
    let sb = s.as_bytes();
    let xb = sfx.as_bytes();
    sb.len() >= xb.len() && sb[sb.len() - xb.len()..].eq_ignore_ascii_case(xb)
}

pub fn resolve_bg_media_path(media_dir: Option<&str>, bg_path: &str) -> String {
    if bg_path.is_empty() {
        String::new()
    } else if Path::new(bg_path).is_absolute() {
        bg_path.to_string()
    } else if let Some(p) = AssetCache::resolve_asset_path(media_dir, "media_themes", bg_path)
        .or_else(|| AssetCache::resolve_asset_path(media_dir, "default_backgrounds", bg_path))
        .or_else(|| AssetCache::resolve_asset_path(media_dir, "presets", bg_path))
    {
        p.to_string_lossy().to_string()
    } else {
        format!(
            "{}/{}",
            media_dir.unwrap_or(crate::device_paths::DISPLAY_MEDIA_DIR_DEFAULT),
            bg_path
        )
    }
}

pub fn is_video_path(s: &str) -> bool {
    has_suffix(s, ".mp4") || has_suffix(s, ".mkv") || has_suffix(s, ".webm")
}

fn load_ppm_p6(path: &str, dst: &mut [u32], dw: i32, dh: i32, dim_pct: f64) -> bool {
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    if !bytes.starts_with(b"P6") {
        return false;
    }
    let mut idx = 2usize;
    let mut vals = [0i32; 3];
    let mut vi = 0usize;
    while vi < 3 && idx < bytes.len() {
        while idx < bytes.len() && bytes[idx].is_ascii_whitespace() {
            idx += 1;
        }
        if idx < bytes.len() && bytes[idx] == b'#' {
            while idx < bytes.len() && bytes[idx] != b'\n' {
                idx += 1;
            }
            continue;
        }
        let start = idx;
        while idx < bytes.len() && bytes[idx].is_ascii_digit() {
            idx += 1;
        }
        if start == idx {
            return false;
        }
        vals[vi] = std::str::from_utf8(&bytes[start..idx])
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        vi += 1;
    }
    if vi < 3 || idx >= bytes.len() || vals[0] <= 0 || vals[1] <= 0 || vals[2] <= 0 {
        return false;
    }
    idx += 1;
    let sw = vals[0];
    let sh = vals[1];
    let need = (sw as usize) * (sh as usize) * 3;
    if bytes.len() < idx + need {
        return false;
    }
    scale_rgb24_to_argb(&bytes[idx..idx + need], sw, sh, dst, dw, dh, dim_pct);
    true
}

fn dimming_keep(dim_pct: f64) -> u32 {
    let d = dim_pct.clamp(0.0, 100.0);
    (((100.0 - d) / 100.0) * 256.0) as u32
}

fn scale_rgb24_to_argb(
    src: &[u8],
    sw: i32,
    sh: i32,
    dst: &mut [u32],
    dw: i32,
    dh: i32,
    dim_pct: f64,
) {
    let keep = dimming_keep(dim_pct);
    let mut lut = [0u32; 256];
    for (i, v) in lut.iter_mut().enumerate() {
        *v = if keep >= 256 {
            i as u32
        } else {
            (i as u32 * keep) >> 8
        };
    }
    if sw == dw && sh == dh {
        let n = (dw as usize) * (dh as usize);
        if keep >= 256 {
            for (d, px) in dst[..n].iter_mut().zip(src.chunks_exact(3)) {
                *d = 0xFF00_0000 | ((px[0] as u32) << 16) | ((px[1] as u32) << 8) | (px[2] as u32);
            }
        } else {
            for (d, px) in dst[..n].iter_mut().zip(src.chunks_exact(3)) {
                *d = 0xFF00_0000
                    | (lut[px[0] as usize] << 16)
                    | (lut[px[1] as usize] << 8)
                    | lut[px[2] as usize];
            }
        }
        return;
    }
    // Fast 2x upscale path (960x540 to 1920x1080) on ARM64 SoCs using row duplication without per-pixel division.
    if sw * 2 == dw && sh * 2 == dh {
        let sw_u = sw as usize;
        let dw_u = dw as usize;
        for sy in 0..(sh as usize) {
            let src_row = &src[sy * sw_u * 3..(sy + 1) * sw_u * 3];
            let dy0 = (sy * 2) * dw_u;
            let dst_row0 = &mut dst[dy0..dy0 + dw_u];
            for (sx, px) in src_row.chunks_exact(3).enumerate() {
                let c = 0xFF00_0000
                    | (lut[px[0] as usize] << 16)
                    | (lut[px[1] as usize] << 8)
                    | lut[px[2] as usize];
                let dx = sx * 2;
                dst_row0[dx] = c;
                dst_row0[dx + 1] = c;
            }
            dst.copy_within(dy0..dy0 + dw_u, dy0 + dw_u);
        }
        return;
    }
    for y in 0..dh {
        let sy = ((y as i64 * sh as i64) / dh as i64) as usize;
        let row_src = sy * (sw as usize) * 3;
        let row_dst = (y * dw) as usize;
        for x in 0..dw {
            let sx = ((x as i64 * sw as i64) / dw as i64) as usize;
            let o = row_src + sx * 3;
            dst[row_dst + x as usize] = 0xFF00_0000
                | (lut[src[o] as usize] << 16)
                | (lut[src[o + 1] as usize] << 8)
                | lut[src[o + 2] as usize];
        }
    }
}

fn scale_rgba_to_argb(
    src: &[u8],
    sw: i32,
    sh: i32,
    dst: &mut [u32],
    dw: i32,
    dh: i32,
    dim_pct: f64,
) {
    let keep = dimming_keep(dim_pct);
    for y in 0..dh {
        let sy = ((y as i64 * sh as i64) / dh as i64) as usize;
        let row_src = sy * (sw as usize) * 4;
        let row_dst = (y * dw) as usize;
        for x in 0..dw {
            let sx = ((x as i64 * sw as i64) / dw as i64) as usize;
            let o = row_src + sx * 4;
            let r = (src[o] as u32 * keep) >> 8;
            let g = (src[o + 1] as u32 * keep) >> 8;
            let b = (src[o + 2] as u32 * keep) >> 8;
            dst[row_dst + x as usize] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
        }
    }
}

fn scale_argb_to_argb(
    src: &[u32],
    sw: i32,
    sh: i32,
    dst: &mut [u32],
    dw: i32,
    dh: i32,
    dim_pct: f64,
) {
    let keep = dimming_keep(dim_pct);
    for y in 0..dh {
        let sy = ((y as i64 * sh as i64) / dh as i64) as usize;
        let row_src = sy * (sw as usize);
        let row_dst = (y * dw) as usize;
        for x in 0..dw {
            let sx = ((x as i64 * sw as i64) / dw as i64) as usize;
            let p = src[row_src + sx];
            let r = (((p >> 16) & 0xFF) * keep) >> 8;
            let g = (((p >> 8) & 0xFF) * keep) >> 8;
            let b = ((p & 0xFF) * keep) >> 8;
            dst[row_dst + x as usize] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
        }
    }
}

fn apply_dimming(pix: &mut [u32], dim_pct: f64) {
    let keep = dimming_keep(dim_pct);
    if keep >= 256 {
        return;
    }
    for p in pix.iter_mut() {
        let r = (((*p >> 16) & 0xFF) * keep) >> 8;
        let g = (((*p >> 8) & 0xFF) * keep) >> 8;
        let b = ((*p & 0xFF) * keep) >> 8;
        *p = 0xFF00_0000 | (r << 16) | (g << 8) | b;
    }
}

fn apply_box_blur(pix: &mut [u32], w: i32, h: i32, radius: i32) {
    let rad = radius.clamp(1, 12);
    let span = (2 * rad + 1) as u32;
    let mut tmp = vec![0u32; (w as usize) * (h as usize)];
    for y in 0..h {
        let row = (y * w) as usize;
        for x in 0..w {
            let (mut sr, mut sg, mut sb) = (0u32, 0u32, 0u32);
            for dx in -rad..=rad {
                let sx = (x + dx).clamp(0, w - 1) as usize;
                let p = pix[row + sx];
                sr += (p >> 16) & 0xFF;
                sg += (p >> 8) & 0xFF;
                sb += p & 0xFF;
            }
            tmp[row + x as usize] =
                0xFF00_0000 | ((sr / span) << 16) | ((sg / span) << 8) | (sb / span);
        }
    }
    for y in 0..h {
        let row = (y * w) as usize;
        for x in 0..w {
            let (mut sr, mut sg, mut sb) = (0u32, 0u32, 0u32);
            for dy in -rad..=rad {
                let sy = (y + dy).clamp(0, h - 1) as usize;
                let p = tmp[sy * w as usize + x as usize];
                sr += (p >> 16) & 0xFF;
                sg += (p >> 8) & 0xFF;
                sb += p & 0xFF;
            }
            pix[row + x as usize] =
                0xFF00_0000 | ((sr / span) << 16) | ((sg / span) << 8) | (sb / span);
        }
    }
}

pub fn is_shader_path(p: &str) -> bool {
    let low = p.to_ascii_lowercase();
    low.ends_with(".frag")
        || matches!(
            low.as_str(),
            "aurora_drift" | "cobalt_waves" | "horizon_split" | "tidal_caustics" | "perimeter_glow"
        )
}

#[inline]
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

#[inline]
fn rotate_rgb_hue(r: f32, g: f32, b: f32, cos_h: f32, sin_h: f32) -> (f32, f32, f32) {
    let nr = (r * ((0.213 + cos_h * 0.787) - sin_h * 0.213)
        + g * ((0.715 - cos_h * 0.715) - sin_h * 0.715)
        + b * ((0.072 - cos_h * 0.072) + sin_h * 0.928))
        .max(0.0);
    let ng = (r * ((0.213 - cos_h * 0.213) + sin_h * 0.143)
        + g * ((0.715 + cos_h * 0.285) + sin_h * 0.140)
        + b * ((0.072 - cos_h * 0.072) - sin_h * 0.283))
        .max(0.0);
    let nb = (r * ((0.213 - cos_h * 0.213) - sin_h * 0.787)
        + g * ((0.715 - cos_h * 0.715) + sin_h * 0.715)
        + b * ((0.072 + cos_h * 0.928) + sin_h * 0.072))
        .max(0.0);
    (nr, ng, nb)
}

pub fn shader_kind_from_id(shader_id: &str) -> i32 {
    let low = shader_id.to_ascii_lowercase();
    if low.contains("cobalt_waves") {
        1
    } else if low.contains("horizon_split") {
        2
    } else if low.contains("tidal_caustics") {
        3
    } else if low.contains("perimeter_glow") {
        4
    } else {
        0
    }
}

pub fn render_procedural_shader(
    pix: &mut [u32],
    w: i32,
    h: i32,
    shader_id: &str,
    u_time: f32,
    hue_deg: f32,
    dim_pct: f64,
) {
    let kind = shader_kind_from_id(shader_id);
    let hue_rad = hue_deg.to_radians();
    let (cos_h, sin_h) = (hue_rad.cos(), hue_rad.sin());
    let keep = (1.0 - (dim_pct.clamp(0.0, 100.0) as f32) / 100.0).clamp(0.0, 1.0);
    let wf = w.max(1) as f32;
    let hf = h.max(1) as f32;
    let inv_h = 1.0 / hf;

    // Software 4x4 block approximation matching GPU shader composition and post-dimming clamp order.
    for y0 in (0..h).step_by(4) {
        let py = (y0 as f32 - 0.5 * hf) * inv_h;
        for x0 in (0..w).step_by(4) {
            let px = (x0 as f32 - 0.5 * wf) * inv_h;
            let [mut r, mut g, mut b] = match kind {
                0 => {
                    // Aurora Drift: twilight indigo backdrop with four harmonic ribbons
                    let t = u_time * 0.72;
                    let r0 = 0.16 * (px * 2.1 + t).sin() + 0.05 * (px * 4.7 - t * 0.6).cos();
                    let r1 = -0.14
                        + 0.18 * (px * 1.6 - t * 0.75 + 1.4).sin()
                        + 0.04 * (px * 3.9 + t * 0.9).sin();
                    let r2 = 0.10
                        + 0.15 * (px * 2.5 + t * 0.55 + 2.8).cos()
                        + 0.03 * (px * 5.1 - t * 0.8).sin();
                    let r3 = -0.03 + 0.12 * (px * 3.0 - t * 0.4 + 4.3).sin();
                    let b0 = 1.0 - smoothstep(0.02, 0.44, (py - r0).abs());
                    let b1 = 1.0 - smoothstep(0.01, 0.38, (py - r1).abs());
                    let b2 = 1.0 - smoothstep(0.01, 0.34, (py - r2).abs());
                    let b3 = 1.0 - smoothstep(0.00, 0.28, (py - r3).abs());
                    let mut c = mix3(
                        [0.014, 0.020, 0.072],
                        [0.092, 0.028, 0.155],
                        (py + 0.5).clamp(0.0, 1.0),
                    );
                    c = mix3(c, [0.040, 0.520, 0.680], b0 * 0.65);
                    c = mix3(c, [0.380, 0.110, 0.720], b1 * 0.58);
                    c = mix3(c, [0.880, 0.240, 0.480], b2 * 0.46);
                    mix3(c, [0.160, 0.780, 0.620], b3 * 0.36)
                }
                1 => {
                    // Cobalt Waves: monochrome deep navy and cerulean swells
                    let t = u_time * 0.68;
                    let w0 = 0.15 * (px * 2.0 + t).sin() + 0.04 * (px * 4.4 - t * 0.5).sin();
                    let w1 = -0.10 + 0.17 * (px * 1.5 - t * 0.65 + 1.9).sin();
                    let w2 = 0.13 + 0.13 * (px * 2.4 + t * 0.5 + 3.5).cos();
                    let w3 = -0.04 + 0.10 * (px * 3.1 - t * 0.4 + 5.0).sin();
                    let b0 = 1.0 - smoothstep(0.02, 0.46, (py - w0).abs());
                    let b1 = 1.0 - smoothstep(0.01, 0.40, (py - w1).abs());
                    let b2 = 1.0 - smoothstep(0.01, 0.34, (py - w2).abs());
                    let b3 = 1.0 - smoothstep(0.00, 0.30, (py - w3).abs());
                    let mut c = mix3(
                        [0.008, 0.016, 0.048],
                        [0.020, 0.050, 0.115],
                        (py + 0.5).clamp(0.0, 1.0),
                    );
                    c = mix3(c, [0.220, 0.680, 0.920], b0 * 0.66);
                    c = mix3(c, [0.050, 0.440, 0.760], b1 * 0.60);
                    c = mix3(c, [0.030, 0.240, 0.520], b2 * 0.50);
                    mix3(c, [0.320, 0.820, 0.950], b3 * 0.34)
                }
                2 => {
                    // Horizon Split: cool cerulean lower tide meeting warm amber upper crest
                    let t = u_time * 0.52;
                    let w0 =
                        -0.14 + 0.12 * (px * 1.4 + t).sin() + 0.03 * (px * 3.2 - t * 0.6).cos();
                    let w1 = 0.15 + 0.10 * (px * 1.7 - t * 0.7 + 2.2).sin();
                    let w2 = 0.01 + 0.08 * (px * 2.2 + t * 0.5 + 4.1).sin();
                    let b0 = 1.0 - smoothstep(0.03, 0.55, (py - w0).abs());
                    let b1 = 1.0 - smoothstep(0.02, 0.48, (py - w1).abs());
                    let b2 = 1.0 - smoothstep(0.01, 0.36, (py - w2).abs());
                    let mut c = mix3(
                        [0.010, 0.026, 0.062],
                        [0.060, 0.026, 0.058],
                        (py + 0.5).clamp(0.0, 1.0),
                    );
                    c = mix3(c, [0.020, 0.490, 0.740], b0 * 0.68);
                    c = mix3(c, [0.950, 0.380, 0.150], b1 * 0.54);
                    mix3(c, [0.960, 0.660, 0.140], b2 * 0.32)
                }
                3 => {
                    // Tidal Caustics: domain-warped aquatic interference web
                    let t = u_time * 0.28;
                    let qx = px + 0.10 * (py * 3.6 + t).sin();
                    let qy = py + 0.10 * (px * 4.1 - t * 0.75).cos();
                    let f1 = (qx * 5.8 + qy * 4.2 + t * 1.1).sin();
                    let f2 = (qx * 4.0 - qy * 6.5 - t * 0.85 + 1.2).sin();
                    let f3 = ((qx + qy) * 5.0 + t * 0.55 + 2.7).sin();
                    let sum = f1 + f2 + f3;
                    let c_r = (1.0 - ((sum + 0.06).abs() * 0.35).clamp(0.0, 1.0)).powi(3);
                    let c_g = (1.0 - (sum.abs() * 0.35).clamp(0.0, 1.0)).powi(3);
                    let c_b = (1.0 - ((sum - 0.06).abs() * 0.35).clamp(0.0, 1.0)).powi(3);
                    let depth =
                        (0.64 + 0.24 * (qx * 2.8 - qy * 3.4 + t * 0.4).sin()).clamp(0.0, 1.0);
                    let ty = (py + 0.5).clamp(0.0, 1.0);
                    [
                        0.012 + 0.010 * ty + c_r * depth * 0.48,
                        0.068 + 0.028 * ty + c_g * depth * 0.72,
                        0.082 + 0.036 * ty + c_b * depth * 0.82,
                    ]
                }
                _ => {
                    // Perimeter Glow: four orbiting perimeter light lobes with edge weighting
                    let t = u_time * 0.45;
                    let half_x = 0.5 * wf * inv_h;
                    let half_y = 0.5;
                    let ex = half_x - px.abs();
                    let ey = half_y - py.abs();
                    let edge_near = ex.min(ey);
                    let edge_boost = if edge_near < 0.14 {
                        let u = (0.14 - edge_near) / 0.14;
                        u * u * 1.15
                    } else {
                        0.0
                    };
                    let spin = t * 0.24 + 0.22 * (t * 0.12).sin();
                    let lobes: [(f32, [f32; 3]); 4] = [
                        (0.0, [1.00, 0.72, 0.34]),
                        (std::f32::consts::FRAC_PI_2, [0.96, 0.48, 0.26]),
                        (std::f32::consts::PI, [0.82, 0.34, 0.46]),
                        (3.0 * std::f32::consts::FRAC_PI_2, [1.00, 0.86, 0.64]),
                    ];
                    let mut c = [0.020_f32, 0.016_f32, 0.024_f32];
                    for (ph, tint) in lobes {
                        let a = spin + ph;
                        let lx = (half_x + 0.22) * a.cos();
                        let ly = (half_y + 0.22) * a.sin();
                        let d2 = (px - lx) * (px - lx) + (py - ly) * (py - ly);
                        let w_lobe = (-d2 * 1.85).exp() * (0.68 + edge_boost);
                        c[0] += w_lobe * tint[0];
                        c[1] += w_lobe * tint[1];
                        c[2] += w_lobe * tint[2];
                    }
                    c
                }
            };
            if hue_deg.abs() > 0.5 {
                (r, g, b) = rotate_rgb_hue(r, g, b, cos_h, sin_h);
            }
            let ru = ((r * keep).clamp(0.0, 1.0) * 255.0) as u32;
            let gu = ((g * keep).clamp(0.0, 1.0) * 255.0) as u32;
            let bu = ((b * keep).clamp(0.0, 1.0) * 255.0) as u32;
            let px_val = 0xFF00_0000 | (ru << 16) | (gu << 8) | bu;
            for dy in 0..4 {
                let yy = y0 + dy;
                if yy >= h {
                    break;
                }
                let row = (yy * w) as usize;
                for dx in 0..4 {
                    let xx = x0 + dx;
                    if xx >= w {
                        break;
                    }
                    pix[row + xx as usize] = px_val;
                }
            }
        }
    }
}

pub fn render_fallback_gradient(pix: &mut [u32], w: i32, h: i32, accent: u32) {
    let ar = ((accent >> 16) & 0xFF) as f32;
    let ag = ((accent >> 8) & 0xFF) as f32;
    let ab = (accent & 0xFF) as f32;
    for y in 0..h {
        let ty = y as f32 / (h.max(1) as f32);
        let row = (y * w) as usize;
        for x in 0..w {
            let tx = x as f32 / (w.max(1) as f32);
            let wave1 = ((tx * 5.2 + ty * 2.1).sin() * 0.5 + 0.5).powf(2.0);
            let wave2 = ((tx * 3.4 - ty * 3.8 + 1.4).sin() * 0.5 + 0.5).powf(2.0);
            let r =
                (10.0 + ty * 14.0 + wave1 * (ar * 0.16 + 38.0) + wave2 * 24.0).min(255.0) as u32;
            let g =
                (11.0 + ty * 12.0 + wave1 * (ag * 0.10 + 10.0) + wave2 * 26.0).min(255.0) as u32;
            let b =
                (22.0 + ty * 20.0 + wave1 * (ab * 0.18 + 34.0) + wave2 * 64.0).min(255.0) as u32;
            pix[row + x as usize] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_bg_media_path_checks_media_themes_and_presets() {
        let tmp = std::env::temp_dir().join(format!("flux_bg_res_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("media_themes")).unwrap();
        std::fs::create_dir_all(tmp.join("presets")).unwrap();
        let mt = tmp.join("media_themes/clip.mp4");
        let pr = tmp.join("presets/preset.png");
        std::fs::write(&mt, b"x").unwrap();
        std::fs::write(&pr, b"y").unwrap();
        let mdir = tmp.to_str().unwrap();
        assert_eq!(
            resolve_bg_media_path(Some(mdir), "clip.mp4"),
            mt.to_string_lossy()
        );
        assert_eq!(
            resolve_bg_media_path(Some(mdir), "preset.png"),
            pr.to_string_lossy()
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn procedural_shader_clamps_after_dimming_matching_gpu_order() {
        // At 1920x1080 perimeter_glow right-edge center (x=1918, y=540) exceeds 1.0 before dimming;
        // dimming must multiply before clamping (matching FS_SHADER_BG clamp(color * u_dim, 0, 1)).
        let mut pix = vec![0u32; 1920 * 1080];
        render_procedural_shader(&mut pix, 1920, 1080, "perimeter_glow.frag", 0.0, 0.0, 50.0);
        let idx = 540 * 1920 + 1918;
        let p = pix[idx];
        let r = (p >> 16) & 0xFF;
        assert!(
            r > 140,
            "expected red > 140 when dimming applies before clamp, got {r}"
        );

        // Nonzero hue rotation also produces distinct channels without premature clamping
        let mut pix_h = vec![0u32; 1920 * 1080];
        render_procedural_shader(
            &mut pix_h,
            1920,
            1080,
            "perimeter_glow.frag",
            0.0,
            120.0,
            50.0,
        );
        assert_ne!(pix[idx], pix_h[idx]);
    }
}
