//! Hardware video decode via Rockchip `librockchip_mpp.so.1` (`/dev/mpp_service`)
//! fed by an Annex-B elementary stream from `openflux-ffmpeg` (packet copy, no software decode: `-c:v copy -bsf:v *_mp4toannexb`).
//! Outputs tight linear `NV12` (`Y` WxH + `UV` Wx(H/2)) frames at the clip's native frame rate.

use std::ffi::{c_char, c_void, CString};
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

pub const MPP_CTX_DEC: i32 = 0;
pub const MPP_VIDEO_CODING_AVC: i32 = 7;
pub const MPP_VIDEO_CODING_HEVC: i32 = 0x0100_0004;
pub const MPP_FMT_YUV420SP: i32 = 0;

// Constants from Rockchip MPP (rk_mpi_cmd.h).
pub const MPP_SET_INPUT_TIMEOUT: i32 = 0x0020_0006;
pub const MPP_SET_OUTPUT_TIMEOUT: i32 = 0x0020_0007;
pub const MPP_DEC_SET_INFO_CHANGE_READY: i32 = 0x0031_0003;
pub const MPP_DEC_SET_PARSER_SPLIT_MODE: i32 = 0x0031_0005;
pub const MPP_DEC_SET_OUTPUT_FORMAT: i32 = 0x0031_000A;

#[repr(C)]
struct MppApi {
    size: u32,
    version: u32,
    decode: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> i32>,
    decode_put_packet: Option<unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32>,
    decode_get_frame: Option<unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> i32>,
    encode: *const c_void,
    encode_put_frame: *const c_void,
    encode_get_packet: *const c_void,
    isp: *const c_void,
    isp_put_frame: *const c_void,
    isp_get_frame: *const c_void,
    poll: *const c_void,
    dequeue: *const c_void,
    enqueue: *const c_void,
    reset: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
    control: Option<unsafe extern "C" fn(*mut c_void, i32, *mut c_void) -> i32>,
    reserv: [u32; 16],
}

struct MppLib {
    _handle: *mut c_void,
    mpp_create: unsafe extern "C" fn(*mut *mut c_void, *mut *const MppApi) -> i32,
    mpp_init: unsafe extern "C" fn(*mut c_void, i32, i32) -> i32,
    mpp_destroy: unsafe extern "C" fn(*mut c_void) -> i32,
    mpp_packet_init: unsafe extern "C" fn(*mut *mut c_void, *mut c_void, usize) -> i32,
    mpp_packet_deinit: unsafe extern "C" fn(*mut *mut c_void) -> i32,
    mpp_packet_set_eos: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
    mpp_frame_deinit: unsafe extern "C" fn(*mut *mut c_void) -> i32,
    mpp_frame_get_width: unsafe extern "C" fn(*mut c_void) -> i32,
    mpp_frame_get_height: unsafe extern "C" fn(*mut c_void) -> i32,
    mpp_frame_get_hor_stride: unsafe extern "C" fn(*mut c_void) -> i32,
    mpp_frame_get_ver_stride: unsafe extern "C" fn(*mut c_void) -> i32,
    mpp_frame_get_fmt: unsafe extern "C" fn(*mut c_void) -> i32,
    mpp_frame_get_info_change: unsafe extern "C" fn(*mut c_void) -> u32,
    mpp_frame_get_eos: unsafe extern "C" fn(*mut c_void) -> u32,
    mpp_frame_get_buffer: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    mpp_buffer_get_ptr: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    mpp_buffer_get_size: unsafe extern "C" fn(*mut c_void, *const c_char) -> usize,
}

unsafe impl Send for MppLib {}
unsafe impl Sync for MppLib {}

impl MppLib {
    fn load() -> Option<Self> {
        if !Path::new("/dev/mpp_service").exists() {
            return None;
        }
        unsafe {
            let mut h = std::ptr::null_mut();
            for name in [
                "librockchip_mpp.so.1\0",
                "/usr/lib/librockchip_mpp.so.1\0",
                "/usr/lib/openflux/display/lib/librockchip_mpp.so.1\0",
            ] {
                h = libc::dlopen(name.as_ptr() as *const c_char, libc::RTLD_NOW);
                if !h.is_null() {
                    break;
                }
            }
            if h.is_null() {
                return None;
            }
            macro_rules! sym {
                ($s:literal) => {{
                    let p = libc::dlsym(h, concat!($s, "\0").as_ptr() as *const c_char);
                    if p.is_null() {
                        libc::dlclose(h);
                        return None;
                    }
                    crate::dl::fn_ptr(p)
                }};
            }
            let ptr_fn = {
                let p = libc::dlsym(h, c"mpp_buffer_get_ptr_with_caller".as_ptr());
                if p.is_null() {
                    libc::dlclose(h);
                    return None;
                }
                crate::dl::fn_ptr(p)
            };
            let size_fn = {
                let p = libc::dlsym(h, c"mpp_buffer_get_size_with_caller".as_ptr());
                if p.is_null() {
                    libc::dlclose(h);
                    return None;
                }
                crate::dl::fn_ptr(p)
            };
            let eos_p = libc::dlsym(h, c"mpp_packet_set_eos".as_ptr());
            let mpp_packet_set_eos = if eos_p.is_null() {
                None
            } else {
                Some(crate::dl::fn_ptr(eos_p))
            };
            Some(Self {
                _handle: h,
                mpp_create: sym!("mpp_create"),
                mpp_init: sym!("mpp_init"),
                mpp_destroy: sym!("mpp_destroy"),
                mpp_packet_init: sym!("mpp_packet_init"),
                mpp_packet_deinit: sym!("mpp_packet_deinit"),
                mpp_packet_set_eos,
                mpp_frame_deinit: sym!("mpp_frame_deinit"),
                mpp_frame_get_width: sym!("mpp_frame_get_width"),
                mpp_frame_get_height: sym!("mpp_frame_get_height"),
                mpp_frame_get_hor_stride: sym!("mpp_frame_get_hor_stride"),
                mpp_frame_get_ver_stride: sym!("mpp_frame_get_ver_stride"),
                mpp_frame_get_fmt: sym!("mpp_frame_get_fmt"),
                mpp_frame_get_info_change: sym!("mpp_frame_get_info_change"),
                mpp_frame_get_eos: sym!("mpp_frame_get_eos"),
                mpp_frame_get_buffer: sym!("mpp_frame_get_buffer"),
                mpp_buffer_get_ptr: ptr_fn,
                mpp_buffer_get_size: size_fn,
            })
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RkmppCodec {
    H264,
    Hevc,
}

/// The decoder has stopped producing frames: the clip ended, or decoding failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamEnded;

pub struct Nv12Frame {
    pub pts_ms: u64,
    pub width: i32,
    pub height: i32,
    pub nv12: Vec<u8>,
}

pub fn validate_nv12_frame_bounds(
    fmt: i32,
    w: i32,
    h: i32,
    hs: i32,
    vs: i32,
    buf_sz: usize,
) -> Option<(usize, usize, usize, usize, usize, usize)> {
    if fmt != MPP_FMT_YUV420SP
        || w <= 0
        || h <= 0
        || (w & 1) != 0
        || (h & 1) != 0
        || hs < w
        || vs < h
    {
        return None;
    }
    let wu = w as usize;
    let hu = h as usize;
    let hsu = hs as usize;
    let vsu = vs as usize;
    let uv_off = hsu.checked_mul(vsu)?;
    let uv_rows = hu / 2;
    let uv_need = uv_rows.checked_sub(1)?.checked_mul(hsu)?.checked_add(wu)?;
    let req = uv_off.checked_add(uv_need)?;
    if req > buf_sz {
        return None;
    }
    let out_sz = wu.checked_mul(hu)?.checked_mul(3)? / 2;
    Some((wu, hu, hsu, uv_off, uv_rows, out_sz))
}

struct WorkerCleanupGuard {
    disconnected: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    done: Arc<(Mutex<bool>, Condvar)>,
}

impl Drop for WorkerCleanupGuard {
    fn drop(&mut self) {
        self.disconnected.store(true, Ordering::SeqCst);
        if let Ok(mut g) = self.child.lock() {
            if let Some(mut c) = g.take() {
                let pgid = -(c.id() as libc::pid_t);
                unsafe {
                    libc::kill(pgid, libc::SIGKILL);
                }
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        let (mtx, cv) = &*self.done;
        if let Ok(mut g) = mtx.lock() {
            *g = true;
            cv.notify_all();
        }
    }
}

pub struct RkmppVideoStream {
    rx: Option<Receiver<Nv12Frame>>,
    queued: Arc<AtomicUsize>,
    disconnected: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    done: Arc<(Mutex<bool>, Condvar)>,
    join: Option<JoinHandle<()>>,
    pub fps: f64,
}

impl RkmppVideoStream {
    pub fn poll_ready(&self) -> bool {
        self.queued.load(Ordering::SeqCst) > 0
    }

    pub fn is_disconnected(&self) -> bool {
        self.disconnected.load(Ordering::SeqCst)
    }

    pub fn queued_len(&self) -> usize {
        self.queued.load(Ordering::SeqCst)
    }

    /// The next decoded frame, `Ok(None)` while none is ready yet, or [`StreamEnded`] once the
    /// decoder has stopped producing frames.
    pub fn try_read_frame(&mut self) -> Result<Option<Nv12Frame>, StreamEnded> {
        let Some(ref rx) = self.rx else {
            return Err(StreamEnded);
        };
        match rx.try_recv() {
            Ok(f) => {
                self.queued.fetch_sub(1, Ordering::SeqCst);
                Ok(Some(f))
            }
            Err(TryRecvError::Empty) => {
                if self.disconnected.load(Ordering::SeqCst) {
                    Err(StreamEnded)
                } else {
                    Ok(None)
                }
            }
            Err(TryRecvError::Disconnected) => Err(StreamEnded),
        }
    }
}

impl Drop for RkmppVideoStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Drop receiver first to unblock any `tx.send()` on a full channel
        drop(self.rx.take());
        if let Ok(mut g) = self.child.lock() {
            if let Some(mut c) = g.take() {
                let pgid = -(c.id() as libc::pid_t);
                unsafe {
                    libc::kill(pgid, libc::SIGKILL);
                }
                let _ = c.kill();
                let _ = c.wait();
            }
        }
        let (mtx, cv) = &*self.done;
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(600);
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

pub fn frame_pts_ms(frame_idx: u64, fps: f64) -> u64 {
    if !fps.is_finite() || fps <= 0.0 {
        return 0;
    }
    ((frame_idx as f64) * 1000.0 / fps).round() as u64
}

fn find_ffmpeg_bin() -> Option<&'static str> {
    Path::new(crate::device_paths::DISPLAY_FFMPEG_PATH)
        .is_file()
        .then_some(crate::device_paths::DISPLAY_FFMPEG_PATH)
}

fn probe_codec_and_fps(ff_bin: &str, path: &str) -> Option<(RkmppCodec, f64)> {
    let out = Command::new(ff_bin)
        .args(["-hide_banner", "-i", path])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    let txt = String::from_utf8_lossy(&out.stderr);
    let vline = txt.lines().find(|l| l.contains("Video:"))?;
    let codec = if vline.contains("hevc") || vline.contains("h265") {
        RkmppCodec::Hevc
    } else if vline.contains("h264") {
        RkmppCodec::H264
    } else {
        return None;
    };
    let mut fps = 30.0_f64;
    for part in vline.split(',') {
        let p = part.trim();
        if let Some(num_s) = p.strip_suffix(" fps") {
            if let Ok(v) = num_s.trim().parse::<f64>() {
                if (5.0..=120.0).contains(&v) {
                    fps = v;
                    break;
                }
            }
        }
    }
    Some((codec, fps))
}

pub fn open_rkmpp_stream(path: &str) -> Option<RkmppVideoStream> {
    if path.is_empty() || !Path::new(path).is_file() {
        return None;
    }
    let mpp = Arc::new(MppLib::load()?);
    let ff_bin = find_ffmpeg_bin()?;
    let (codec, fps) = probe_codec_and_fps(ff_bin, path)?;
    // Preserve source frame rate so packet-copied streams play at 1.0x speed.
    let (bsf, fmt, mpp_coding) = match codec {
        RkmppCodec::Hevc => ("hevc_mp4toannexb", "hevc", MPP_VIDEO_CODING_HEVC),
        RkmppCodec::H264 => ("h264_mp4toannexb", "h264", MPP_VIDEO_CODING_AVC),
    };

    let mut cmd = Command::new(ff_bin);
    cmd.args([
        "-loglevel",
        "error",
        "-stream_loop",
        "-1",
        "-i",
        path,
        "-an",
        "-c:v",
        "copy",
        "-bsf:v",
        bsf,
        "-f",
        fmt,
        "-",
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;

    let (tx, rx) = mpsc::sync_channel::<Nv12Frame>(6);
    let queued = Arc::new(AtomicUsize::new(0));
    let disconnected = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let child_slot = Arc::new(Mutex::new(Some(child)));
    let done = Arc::new((Mutex::new(false), Condvar::new()));

    let q_bg = queued.clone();
    let disc_bg = disconnected.clone();
    let stop_bg = stop.clone();
    let child_bg = child_slot.clone();
    let done_bg = done.clone();

    let jh = thread::spawn(move || {
        let _cleanup = WorkerCleanupGuard {
            disconnected: disc_bg.clone(),
            child: child_bg.clone(),
            done: done_bg.clone(),
        };
        unsafe {
            let mut ctx: *mut c_void = std::ptr::null_mut();
            let mut mpi: *const MppApi = std::ptr::null();
            if (mpp.mpp_create)(&mut ctx, &mut mpi) != 0 || ctx.is_null() || mpi.is_null() {
                return;
            }
            if let Some(ctrl) = (*mpi).control {
                let mut split: u32 = 1;
                let _ = ctrl(
                    ctx,
                    MPP_DEC_SET_PARSER_SPLIT_MODE,
                    &mut split as *mut u32 as *mut c_void,
                );
            }
            if (mpp.mpp_init)(ctx, MPP_CTX_DEC, mpp_coding) != 0 {
                (mpp.mpp_destroy)(ctx);
                return;
            }
            let mut fmt_ok = true;
            if let Some(ctrl) = (*mpi).control {
                let mut in_tmo: i64 = 0;
                let _ = ctrl(
                    ctx,
                    MPP_SET_INPUT_TIMEOUT,
                    &mut in_tmo as *mut i64 as *mut c_void,
                );
                let mut out_tmo: i64 = 0;
                let _ = ctrl(
                    ctx,
                    MPP_SET_OUTPUT_TIMEOUT,
                    &mut out_tmo as *mut i64 as *mut c_void,
                );
                let mut fmt_out: i32 = MPP_FMT_YUV420SP;
                if ctrl(
                    ctx,
                    MPP_DEC_SET_OUTPUT_FORMAT,
                    &mut fmt_out as *mut i32 as *mut c_void,
                ) != 0
                {
                    fmt_ok = false;
                }
            }
            if fmt_ok {
                let put_pkt = (*mpi).decode_put_packet;
                let get_frm = (*mpi).decode_get_frame;
                let ctrl_fn = (*mpi).control;
                let caller = CString::new("openflux_rkmpp").unwrap();

                let mut chunk = vec![0u8; 64 * 1024];
                let mut frame_idx: u64 = 0;

                'outer: while !stop_bg.load(Ordering::SeqCst) {
                    let n = match stdout.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    };
                    let mut pkt: *mut c_void = std::ptr::null_mut();
                    if (mpp.mpp_packet_init)(&mut pkt, chunk.as_ptr() as *mut c_void, n) != 0
                        || pkt.is_null()
                    {
                        break;
                    }
                    let mut sent = false;
                    while !sent && !stop_bg.load(Ordering::SeqCst) {
                        if let Some(put) = put_pkt {
                            if put(ctx, pkt) == 0 {
                                sent = true;
                            }
                        }
                        loop {
                            let mut frm: *mut c_void = std::ptr::null_mut();
                            let Some(getf) = get_frm else { break };
                            let ret = getf(ctx, &mut frm);
                            if ret != 0 || frm.is_null() {
                                break;
                            }
                            if (mpp.mpp_frame_get_info_change)(frm) != 0 {
                                if let Some(ctrl) = ctrl_fn {
                                    let _ = ctrl(
                                        ctx,
                                        MPP_DEC_SET_INFO_CHANGE_READY,
                                        std::ptr::null_mut(),
                                    );
                                }
                                (mpp.mpp_frame_deinit)(&mut frm);
                                continue;
                            }
                            let eos = (mpp.mpp_frame_get_eos)(frm) != 0;
                            let fmt = (mpp.mpp_frame_get_fmt)(frm);
                            let buf = (mpp.mpp_frame_get_buffer)(frm);
                            if !buf.is_null() {
                                let w = (mpp.mpp_frame_get_width)(frm);
                                let h = (mpp.mpp_frame_get_height)(frm);
                                let hs = (mpp.mpp_frame_get_hor_stride)(frm);
                                let vs = (mpp.mpp_frame_get_ver_stride)(frm);
                                let buf_sz = (mpp.mpp_buffer_get_size)(buf, caller.as_ptr());
                                let ptr =
                                    (mpp.mpp_buffer_get_ptr)(buf, caller.as_ptr()) as *const u8;
                                if let Some((wu, hu, hsu, uv_off, uv_rows, nv12_sz)) =
                                    validate_nv12_frame_bounds(fmt, w, h, hs, vs, buf_sz)
                                {
                                    if !ptr.is_null() {
                                        let mut nv12 = Vec::with_capacity(nv12_sz);
                                        for row in 0..hu {
                                            let src =
                                                std::slice::from_raw_parts(ptr.add(row * hsu), wu);
                                            nv12.extend_from_slice(src);
                                        }
                                        let uv_base = ptr.add(uv_off);
                                        for row in 0..uv_rows {
                                            let src = std::slice::from_raw_parts(
                                                uv_base.add(row * hsu),
                                                wu,
                                            );
                                            nv12.extend_from_slice(src);
                                        }
                                        let pts_ms = frame_pts_ms(frame_idx, fps);
                                        frame_idx += 1;
                                        q_bg.fetch_add(1, Ordering::SeqCst);
                                        if tx
                                            .send(Nv12Frame {
                                                pts_ms,
                                                width: w,
                                                height: h,
                                                nv12,
                                            })
                                            .is_err()
                                        {
                                            q_bg.fetch_sub(1, Ordering::SeqCst);
                                            (mpp.mpp_frame_deinit)(&mut frm);
                                            (mpp.mpp_packet_deinit)(&mut pkt);
                                            break 'outer;
                                        }
                                    }
                                } else {
                                    (mpp.mpp_frame_deinit)(&mut frm);
                                    (mpp.mpp_packet_deinit)(&mut pkt);
                                    break 'outer;
                                }
                            }
                            (mpp.mpp_frame_deinit)(&mut frm);
                            if eos {
                                break;
                            }
                        }
                        if !sent {
                            thread::sleep(std::time::Duration::from_millis(1));
                        }
                    }
                    (mpp.mpp_packet_deinit)(&mut pkt);
                }
                if let Some(set_eos) = mpp.mpp_packet_set_eos {
                    let mut eos_pkt: *mut c_void = std::ptr::null_mut();
                    if (mpp.mpp_packet_init)(&mut eos_pkt, std::ptr::null_mut(), 0) == 0
                        && !eos_pkt.is_null()
                    {
                        let _ = set_eos(eos_pkt);
                        if let Some(put) = put_pkt {
                            let _ = put(ctx, eos_pkt);
                        }
                        (mpp.mpp_packet_deinit)(&mut eos_pkt);
                    }
                }
            }
            (mpp.mpp_destroy)(ctx);
        }
    });

    Some(RkmppVideoStream {
        rx: Some(rx),
        queued,
        disconnected,
        stop,
        child: child_slot,
        done,
        join: Some(jh),
        fps,
    })
}

/// Direct live H.264 Annex-B hardware VPU decoder (`librockchip_mpp.so.1`) for real-time USB
/// `CH_MEDIA` streams (YouTube, Twitch, screen share, RTSP camera) at 1080p 60 FPS with ZERO
/// ffmpeg subprocess and ZERO disk writes!
pub struct LiveRkmppDecoder {
    in_tx: Option<mpsc::SyncSender<Vec<u8>>>,
    latest_frame: Arc<Mutex<Option<Nv12Frame>>>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl LiveRkmppDecoder {
    pub fn open_h264() -> Option<Self> {
        let mpp = MppLib::load()?;
        let (in_tx, in_rx) = mpsc::sync_channel::<Vec<u8>>(32);
        let latest_frame: Arc<Mutex<Option<Nv12Frame>>> = Arc::new(Mutex::new(None));
        let latest_bg = latest_frame.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_bg = stop.clone();

        let join = thread::Builder::new()
            .name("rkmpp-live-h264".into())
            .spawn(move || unsafe {
                let mut ctx: *mut c_void = std::ptr::null_mut();
                let mut mpi: *const MppApi = std::ptr::null();
                if (mpp.mpp_create)(&mut ctx, &mut mpi) != 0 || ctx.is_null() || mpi.is_null() {
                    return;
                }
                if let Some(ctrl) = (*mpi).control {
                    let mut split: u32 = 1;
                    let _ = ctrl(
                        ctx,
                        MPP_DEC_SET_PARSER_SPLIT_MODE,
                        &mut split as *mut u32 as *mut c_void,
                    );
                }
                if (mpp.mpp_init)(ctx, MPP_CTX_DEC, MPP_VIDEO_CODING_AVC) != 0 {
                    let _ = (mpp.mpp_destroy)(ctx);
                    return;
                }
                if let Some(ctrl) = (*mpi).control {
                    let mut in_tmo: i64 = 10;
                    let _ = ctrl(
                        ctx,
                        MPP_SET_INPUT_TIMEOUT,
                        &mut in_tmo as *mut i64 as *mut c_void,
                    );
                    let mut out_tmo: i64 = 0;
                    let _ = ctrl(
                        ctx,
                        MPP_SET_OUTPUT_TIMEOUT,
                        &mut out_tmo as *mut i64 as *mut c_void,
                    );
                    let mut fmt_out: i32 = MPP_FMT_YUV420SP;
                    let _ = ctrl(
                        ctx,
                        MPP_DEC_SET_OUTPUT_FORMAT,
                        &mut fmt_out as *mut i32 as *mut c_void,
                    );
                }
                let put_pkt = (*mpi).decode_put_packet;
                let get_frm = (*mpi).decode_get_frame;
                let ctrl_fn = (*mpi).control;
                let caller = CString::new("openflux_rkmpp_live").unwrap();
                let mut frame_idx = 0u64;

                while !stop_bg.load(Ordering::SeqCst) {
                    let Ok(chunk) = in_rx.recv_timeout(std::time::Duration::from_millis(50)) else {
                        continue;
                    };
                    if chunk.is_empty() {
                        continue;
                    }
                    let mut pkt: *mut c_void = std::ptr::null_mut();
                    if (mpp.mpp_packet_init)(&mut pkt, chunk.as_ptr() as *mut c_void, chunk.len()) != 0
                        || pkt.is_null()
                    {
                        continue;
                    }
                    let mut sent = false;
                    let mut attempts = 0u32;
                    while !sent && attempts < 20 && !stop_bg.load(Ordering::SeqCst) {
                        attempts += 1;
                        if let Some(put) = put_pkt {
                            if put(ctx, pkt) == 0 {
                                sent = true;
                            }
                        }
                        loop {
                            let mut frm: *mut c_void = std::ptr::null_mut();
                            let Some(getf) = get_frm else { break };
                            let ret = getf(ctx, &mut frm);
                            if ret != 0 || frm.is_null() {
                                break;
                            }
                            if (mpp.mpp_frame_get_info_change)(frm) != 0 {
                                if let Some(ctrl) = ctrl_fn {
                                    let _ = ctrl(
                                        ctx,
                                        MPP_DEC_SET_INFO_CHANGE_READY,
                                        std::ptr::null_mut(),
                                    );
                                }
                                (mpp.mpp_frame_deinit)(&mut frm);
                                continue;
                            }
                            let fmt = (mpp.mpp_frame_get_fmt)(frm);
                            let buf = (mpp.mpp_frame_get_buffer)(frm);
                            if !buf.is_null() {
                                let w = (mpp.mpp_frame_get_width)(frm);
                                let h = (mpp.mpp_frame_get_height)(frm);
                                let hs = (mpp.mpp_frame_get_hor_stride)(frm);
                                let vs = (mpp.mpp_frame_get_ver_stride)(frm);
                                let buf_sz = (mpp.mpp_buffer_get_size)(buf, caller.as_ptr());
                                let ptr =
                                    (mpp.mpp_buffer_get_ptr)(buf, caller.as_ptr()) as *const u8;
                                if let Some((wu, hu, hsu, uv_off, uv_rows, nv12_sz)) =
                                    validate_nv12_frame_bounds(fmt, w, h, hs, vs, buf_sz)
                                {
                                    if !ptr.is_null() {
                                        let mut nv12 = Vec::with_capacity(nv12_sz);
                                        for row in 0..hu {
                                            nv12.extend_from_slice(std::slice::from_raw_parts(
                                                ptr.add(row * hsu),
                                                wu,
                                            ));
                                        }
                                        let uv_base = ptr.add(uv_off);
                                        for row in 0..uv_rows {
                                            nv12.extend_from_slice(std::slice::from_raw_parts(
                                                uv_base.add(row * hsu),
                                                wu,
                                            ));
                                        }
                                        frame_idx += 1;
                                        *latest_bg.lock().unwrap() = Some(Nv12Frame {
                                            pts_ms: frame_idx * 16,
                                            width: w,
                                            height: h,
                                            nv12,
                                        });
                                    }
                                }
                            }
                            (mpp.mpp_frame_deinit)(&mut frm);
                        }
                        if !sent {
                            thread::sleep(std::time::Duration::from_millis(1));
                        }
                    }
                    let _ = (mpp.mpp_packet_deinit)(&mut pkt);
                }
                let _ = (mpp.mpp_destroy)(ctx);
            })
            .ok()?;

        Some(Self {
            in_tx: Some(in_tx),
            latest_frame,
            stop,
            join: Some(join),
        })
    }

    pub fn push_annexb(&self, data: Vec<u8>) {
        if let Some(ref tx) = self.in_tx {
            let _ = tx.try_send(data);
        }
    }

    pub fn take_latest(&self) -> Option<Nv12Frame> {
        self.latest_frame.lock().unwrap().take()
    }
}

impl Drop for LiveRkmppDecoder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.in_tx.take();
        if let Some(jh) = self.join.take() {
            let _ = jh.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_nv12_bounds_and_rejects_10bit_fbc_and_short_buffers() {
        assert!(validate_nv12_frame_bounds(
            MPP_FMT_YUV420SP,
            1920,
            1080,
            1920,
            1088,
            1920 * 1088 * 3 / 2
        )
        .is_some());
        // 10-bit or AFBC flag bits must be rejected
        assert!(validate_nv12_frame_bounds(1, 1920, 1080, 1920, 1080, 1920 * 1080 * 3).is_none());
        assert!(
            validate_nv12_frame_bounds(0x0010_0000, 1920, 1080, 1920, 1080, 1920 * 1080 * 3)
                .is_none()
        );
        // Short buffer rejected
        assert!(
            validate_nv12_frame_bounds(MPP_FMT_YUV420SP, 1920, 1080, 1920, 1080, 1000).is_none()
        );
        // Odd dimensions rejected
        assert!(validate_nv12_frame_bounds(
            MPP_FMT_YUV420SP,
            1919,
            1080,
            1920,
            1080,
            1920 * 1080 * 2
        )
        .is_none());
    }

    #[test]
    fn full_channel_drop_unblocks_and_finishes_promptly() {
        let (tx, rx) = mpsc::sync_channel::<Nv12Frame>(1);
        let queued = Arc::new(AtomicUsize::new(0));
        let disconnected = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(None));
        let done = Arc::new((Mutex::new(false), Condvar::new()));

        let disc_bg = disconnected.clone();
        let child_bg = child.clone();
        let done_bg = done.clone();
        let q_bg = queued.clone();
        let jh = thread::spawn(move || {
            let _g = WorkerCleanupGuard {
                disconnected: disc_bg,
                child: child_bg,
                done: done_bg,
            };
            for i in 0..5u64 {
                q_bg.fetch_add(1, Ordering::SeqCst);
                if tx
                    .send(Nv12Frame {
                        pts_ms: i,
                        width: 2,
                        height: 2,
                        nv12: vec![0; 6],
                    })
                    .is_err()
                {
                    q_bg.fetch_sub(1, Ordering::SeqCst);
                    break;
                }
            }
        });
        std::thread::sleep(std::time::Duration::from_millis(30));
        let t0 = std::time::Instant::now();
        let stream = RkmppVideoStream {
            rx: Some(rx),
            queued,
            disconnected,
            stop,
            child,
            done,
            join: Some(jh),
            fps: 60.0,
        };
        drop(stream);
        assert!(
            t0.elapsed() < std::time::Duration::from_millis(250),
            "drop stalled on a full channel"
        );
    }

    #[test]
    fn source_pts_rate_stays_invariant_when_display_fps_cap_differs() {
        assert_eq!(frame_pts_ms(0, 24.0), 0);
        assert_eq!(frame_pts_ms(24, 24.0), 1000);
        assert_eq!(frame_pts_ms(30, 30.0), 1000);
    }
}
