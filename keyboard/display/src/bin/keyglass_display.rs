//! Keyglass v2 Keyboard Display Compositor (pure v2 — zero legacy v1 rendering/config).
//!
//! Runs directly on the RK3566 keyboard Wayland/EGL Mali-G52 GPU:
//! - Receives real-time key events from `keyglass-bridge` over `/tmp/keyglass_bridge.sock`
//! - Syncs full-duplex v2 Bus (`CH_BUS`) + live H.264 video (`CH_MEDIA`) over `/tmp/keyglass_v2.sock`
//! - Runs all enabled Lua v2 modules (`KeyboardV2Display`), 480x270 water PDE, particles, custom
//!   GLSL shaders, hardware `rkmpp` NV12 video loops, and bounded `ctx:stream_rect` video popups
//! - Applies pure-v2 `display/parallax` & `display/power` (backlight brightness & sleep timeout)

#[cfg(target_os = "linux")]
use keyglass_display::device_paths::{
    BACKLIGHT_BL_POWER_PATH, BACKLIGHT_BRIGHTNESS_PATH, BACKLIGHT_MAX_BRIGHTNESS_PATH,
    BRIDGE_SOCK_DEFAULT, DISPLAY_ACTIVE_MARKER_DEFAULT, DISPLAY_LOG_DIR_DEFAULT,
    DISPLAY_MEDIA_DIR_DEFAULT, SAVED_BRIGHTNESS_PATH,
};
use keyglass_display::layout::{DISPLAY_HEIGHT, DISPLAY_WIDTH};
#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "linux")]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DISPLAY_VERSION: &str = match option_env!("KEYGLASS_DISPLAY_VERSION_STR") {
    Some(v) => v,
    None => concat!("keyglass-display ", env!("CARGO_PKG_VERSION")),
};

#[cfg(target_os = "linux")]
static RUNNING: AtomicBool = AtomicBool::new(true);

#[cfg(target_os = "linux")]
extern "C" fn handle_sig(_: i32) {
    RUNNING.store(false, Ordering::SeqCst);
}

#[cfg(target_os = "linux")]
fn install_signal_handlers() {
    extern "C" {
        fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
    }
    unsafe {
        signal(2, handle_sig);  // SIGINT
        signal(15, handle_sig); // SIGTERM
    }
}

#[cfg(target_os = "linux")]
fn is_video_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".mp4")
        || lower.ends_with(".mkv")
        || lower.ends_with(".webm")
        || lower.ends_with(".mov")
        || lower.ends_with(".avi")
}

#[cfg(target_os = "linux")]
fn resolve_media_file(media_dir: &str, raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    if Path::new(raw).is_file() {
        return Some(raw.to_string());
    }
    let direct = Path::new(media_dir).join(raw);
    if direct.is_file() {
        return Some(direct.to_string_lossy().into_owned());
    }
    if let Some(fname) = Path::new(raw).file_name().and_then(|n| n.to_str()) {
        let cand = Path::new(media_dir).join(fname);
        if cand.is_file() {
            return Some(cand.to_string_lossy().into_owned());
        }
    }
    None
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut width = DISPLAY_WIDTH;
    let mut height = DISPLAY_HEIGHT;
    #[cfg(target_os = "linux")]
    let mut media_dir = std::env::var("KEYGLASS_MEDIA_DIR")
        .unwrap_or_else(|_| DISPLAY_MEDIA_DIR_DEFAULT.to_string());

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--version" => {
                println!("{DISPLAY_VERSION}");
                return;
            }
            s if s.starts_with("--width=") => {
                if let Ok(v) = s[8..].parse() {
                    width = v;
                }
            }
            s if s.starts_with("--height=") => {
                if let Ok(v) = s[9..].parse() {
                    height = v;
                }
            }
            "--media-dir" if i + 1 < args.len() => {
                i += 1;
                #[cfg(target_os = "linux")]
                {
                    media_dir = args[i].clone();
                }
            }
            "--config-dir" if i + 1 < args.len() => {
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (width, height);
        eprintln!("{DISPLAY_VERSION}: pure-v2 Wayland/RKMPP display binary runs on Linux keyboard hardware.");
    }

    #[cfg(target_os = "linux")]
    {
        use keyglass_display::ipc::BridgeIpcClient;
        use keyglass_display::rkmpp::{Nv12Frame, RkmppVideoStream, StreamEnded};
        use keyglass_display::v2_mux_client::V2MuxClient;
        use keyglass_display::v2_runtime::KeyboardV2Display;
        use keyglass_display::wayland_egl::WaylandGpuPresenter;

        install_signal_handlers();

        let log_dir = std::env::var("KEYGLASS_LOG_DIR")
            .unwrap_or_else(|_| DISPLAY_LOG_DIR_DEFAULT.to_string());
        let v2_root = std::env::var("KEYGLASS_V2_ROOT")
            .unwrap_or_else(|_| "/home/root/keyglass/v2".to_string());
        let _ = std::fs::create_dir_all(&log_dir);
        let _ = std::fs::create_dir_all(&v2_root);

        let diag_path = Path::new(&log_dir).join("display_diag.log");
        let mut log_diag = |msg: &str| {
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&diag_path)
            {
                use std::io::Write;
                let _ = writeln!(f, "[{ts}] {msg}");
            }
        };

        log_diag(&format!("Starting pure-v2 {DISPLAY_VERSION} ({width}x{height})"));

        // Ensure backlight wakes cleanly if a prior sleep left it off.
        let mut saved_backlight_brightness: u32 =
            std::fs::read_to_string(SAVED_BRIGHTNESS_PATH)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
                .filter(|&v| v > 0)
                .or_else(|| {
                    std::fs::read_to_string(BACKLIGHT_BRIGHTNESS_PATH)
                        .ok()
                        .and_then(|s| s.trim().parse::<u32>().ok())
                        .filter(|&v| v > 0)
                })
                .unwrap_or(200);
        apply_panel_sleep(false, &mut saved_backlight_brightness, &mut log_diag);

        let mut gpu = match WaylandGpuPresenter::open(width, height, "keyglass-display") {
            Ok(g) => g,
            Err(e) => {
                log_diag(&format!("WaylandGpuPresenter failed: {e}"));
                eprintln!("WaylandGpuPresenter failed: {e}");
                std::process::exit(1);
            }
        };
        let _ = std::fs::write(DISPLAY_ACTIVE_MARKER_DEFAULT, format!("{}\n", std::process::id()));

        let v2_font_bytes: Option<Vec<u8>> = [
            "/usr/lib/keyglass/display/assets/fonts/Montserrat-Medium.ttf",
            "/usr/lib/keyglass/display/fonts/Montserrat-Medium.ttf",
            "/home/root/keyglass/v2/Montserrat-Medium.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        ]
        .iter()
        .find_map(|p| std::fs::read(p).ok());

        let v2_state_path = Path::new(&v2_root).join("modules_state.json");
        let mut v2 = KeyboardV2Display::new(
            std::path::PathBuf::from("/usr/lib/keyglass/display/modules"),
            std::path::PathBuf::from(&v2_root).join("modules"),
            std::path::PathBuf::from(&v2_root).join("local"),
            v2_state_path,
            keyglass_bus::FrameLayout::Ansi,
            v2_font_bytes.as_deref(),
        );

        let sock_path = std::env::var("KEYGLASS_BRIDGE_SOCK")
            .unwrap_or_else(|_| BRIDGE_SOCK_DEFAULT.to_string());
        let mut ipc = BridgeIpcClient::new(sock_path);
        let mut v2_mux = V2MuxClient::new();
        let mut logged_ipc_conn = false;

        let mut rkmpp_stream: Option<RkmppVideoStream> = None;
        let mut rkmpp_path = String::new();
        let mut rkmpp_failed_path = String::new();
        let mut rkmpp_pending: Option<Nv12Frame> = None;
        let mut rkmpp_epoch_inst: Option<std::time::Instant> = None;
        let mut rkmpp_first_pts_ms: u64 = 0;
        let mut rkmpp_last_pts_ms: u64 = 0;
        let mut rkmpp_underruns: u64 = 0;
        let mut rkmpp_uploads: u64 = 0;

        let mut v2_img_path = String::new();
        let mut v2_img_mtime: Option<SystemTime> = None;
        let mut v2_img_loaded = false;

        let mut cached_suspended = false;
        let start = std::time::Instant::now();
        let mut last_step_ms: u64 = 0;
        let mut last_frame_inst: Option<std::time::Instant> = None;
        let mut last_soc_ms: u64 = 0;

        // Offload 1 Hz eMMC writes (`display_heartbeat.log` + `soc_stats.json` atomic rename) to a
        // dedicated background thread so ext4 journal flushes (~120 ms!) NEVER hitch the 60 Hz render loop!
        let stat_render_ms = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let stat_rkmpp = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let bg_log_dir = log_dir.clone();
            let bg_render_ms = stat_render_ms.clone();
            let bg_rkmpp = stat_rkmpp.clone();
            let bg_start = start;
            let _ = std::thread::Builder::new()
                .name("keyglass-soc-stats".into())
                .spawn(move || {
                    let mut prev_cpu: Option<(u64, u64)> = None;
                    while RUNNING.load(Ordering::SeqCst) {
                        let now_sec = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs() as i64;
                        let mono_ms = bg_start.elapsed().as_millis() as u64;
                        let hb = Path::new(&bg_log_dir).join("display_heartbeat.log");
                        let _ = std::fs::write(&hb, format!("{now_sec}\n"));
                        write_soc_stats(
                            &bg_log_dir,
                            &mut prev_cpu,
                            bg_render_ms.load(Ordering::Relaxed),
                            bg_rkmpp.load(Ordering::Relaxed),
                            now_sec,
                            mono_ms,
                        );
                        std::thread::sleep(Duration::from_secs(1));
                    }
                });
        }

        // Rolling 120-frame latency & hitch profiler (published 1 Hz to `display/perf`)
        let mut perf = PerfWindow::new();

        while RUNNING.load(Ordering::SeqCst) {
            let iter_start = std::time::Instant::now();
            let mono_ms = start.elapsed().as_millis() as u64;
            let _unix_ms = keyglass_bus::now_unix_ms();

            // 1. Drain UART / bridge IPC lines (auto-connects to `/tmp/keyglass_bridge.sock`!)
            for line in ipc.poll_lines() {
                v2.ingest_bridge_ipc_line(&line);
            }
            if ipc.is_connected() && !logged_ipc_conn {
                logged_ipc_conn = true;
                log_diag("Bridge IPC connected (/tmp/keyglass_bridge.sock)");
            } else if !ipc.is_connected() {
                logged_ipc_conn = false;
            }

            // 2. Full-duplex OFV2 bus & live H.264 media mux (auto-connects to `/tmp/keyglass_v2.sock`!)
            let mux_res = v2_mux.poll();
            if mux_res.connected_now {
                log_diag("OFV2 mux connected (/tmp/keyglass_v2.sock)");
                if let Some(sub_frame) = v2.build_mirror_subscribe(true) {
                    v2_mux.send_bus_frame(&sub_frame);
                }
            }
            if mux_res.disconnected {
                log_diag("OFV2 mux disconnected; purging remote topics");
                v2.on_host_disconnect();
            }
            for frame in mux_res.bus_frames {
                for reply in v2.on_mirror_frame(frame) {
                    v2_mux.send_bus_frame(&reply);
                }
            }
            for (sname, active, codec) in mux_res.stream_changes {
                v2.publish_stream_state(&sname, active, codec);
            }
            if let Some(sub_frame) = v2.build_mirror_subscribe(false) {
                v2_mux.send_bus_frame(&sub_frame);
            }
            for out_frame in v2.drain_mirror_outgoing() {
                v2_mux.send_bus_frame(&out_frame);
            }

            let ipc_mux_us = iter_start.elapsed().as_micros() as u64;

            // 3. Hot-reload modules whenever `fluxctl keyboard sync` writes new files (throttled 500ms)
            if v2.poll_reload() {
                rkmpp_failed_path.clear();
                v2_img_path.clear();
            }

            // 4. Apply pure-v2 display power (brightness + monotonic idle sleep + match host display sleep)
            v2.poll_power_and_host_state();
            if let Some(pct) = v2.pending_brightness_pct.take() {
                apply_brightness_pct(pct, cached_suspended, &mut saved_backlight_brightness, &mut log_diag);
            }
            let susp = v2.is_display_asleep();
            if susp != cached_suspended {
                cached_suspended = susp;
                if !susp {
                    rkmpp_failed_path.clear();
                }
                apply_panel_sleep(susp, &mut saved_backlight_brightness, &mut log_diag);
            }

            // 5. Wayland event dispatch
            match gpu.dispatch() {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => {
                    log_diag(&format!("Wayland dispatch error: {e}"));
                    break;
                }
            }

            // 6. Background media management: live H.264 stream > local RKMPP loop > static image
            let live_stream_override = v2
                .active_stream_name
                .as_deref()
                .is_some_and(|sname| v2_mux.has_stream(sname));
            let desired_bg = if live_stream_override {
                String::new()
            } else {
                v2.active_media_path.clone().unwrap_or_default()
            };
            let bg_is_vid = is_video_path(&desired_bg);

            if !bg_is_vid || cached_suspended {
                if rkmpp_stream.is_some() {
                    rkmpp_stream = None;
                    rkmpp_path.clear();
                    rkmpp_pending = None;
                    rkmpp_epoch_inst = None;
                }
                if !cached_suspended && !desired_bg.is_empty() {
                    if let Some(rp) = resolve_media_file(&media_dir, &desired_bg) {
                        let mt = std::fs::metadata(&rp).ok().and_then(|m| m.modified().ok());
                        if mt.is_some() && (rp != v2_img_path || mt != v2_img_mtime) {
                            if let Ok(img) = image::open(&rp) {
                                let rgba = img.to_rgba8();
                                let (iw, ih) = rgba.dimensions();
                                let mut argb = Vec::with_capacity((iw * ih) as usize);
                                for px in rgba.chunks_exact(4) {
                                    argb.push(
                                        ((px[3] as u32) << 24)
                                            | ((px[0] as u32) << 16)
                                            | ((px[1] as u32) << 8)
                                            | (px[2] as u32),
                                    );
                                }
                                gpu.upload_bg_argb(iw as i32, ih as i32, &argb);
                                v2_img_path = rp;
                                v2_img_mtime = mt;
                                v2_img_loaded = true;
                            }
                        }
                    }
                } else {
                    v2_img_loaded = false;
                }
            } else {
                v2_img_loaded = false;
                if desired_bg != rkmpp_path && desired_bg != rkmpp_failed_path {
                    rkmpp_stream = None;
                    rkmpp_path.clear();
                    rkmpp_pending = None;
                    rkmpp_epoch_inst = None;
                    if let Some(rp) = resolve_media_file(&media_dir, &desired_bg) {
                        if let Some(st_mpp) = keyglass_display::rkmpp::open_rkmpp_stream(&rp) {
                            log_diag(&format!(
                                "RKMPP video opened: {} ({:.1} fps)",
                                desired_bg, st_mpp.fps
                            ));
                            rkmpp_stream = Some(st_mpp);
                            rkmpp_path = desired_bg.clone();
                        } else {
                            log_diag(&format!("RKMPP failed for: {desired_bg}"));
                            rkmpp_failed_path = desired_bg.clone();
                        }
                    } else {
                        rkmpp_failed_path = desired_bg.clone();
                    }
                }
            }

            // 7. Step v2 Lua modules + water PDE + particles + vsync-locked video + GPU composite!
            // While panel is asleep, skip GPU/Lua rendering and yield cleanly at 20 Hz so the
            // RK3566 stays cool and wakes within <50 ms on real keypress or host monitor wake.
            if cached_suspended {
                last_frame_inst = None;
                std::thread::sleep(Duration::from_millis(50));
                continue;
            }
            if !gpu.is_frame_pending() {
                let frame_start = std::time::Instant::now();
                let frame_interval_us = last_frame_inst
                    .map(|p| frame_start.duration_since(p).as_micros() as u64)
                    .unwrap_or(16_667);
                last_frame_inst = Some(frame_start);

                let dt_sec = if last_step_ms == 0 {
                    0.016
                } else {
                    (mono_ms.saturating_sub(last_step_ms) as f32 / 1000.0).clamp(0.004, 0.05)
                };
                last_step_ms = mono_ms;
                v2.step_frame(dt_sec, mono_ms);

                // Vsync-locked & epoch-anchored RKMPP background upload (eliminates 59.94 vs 60 Hz
                // beat judder AND avoids mid-frame GL texture stalls by uploading only when ready!)
                let t_vid = std::time::Instant::now();
                let mut stream_ended = false;
                if let Some(ref mut st_mpp) = rkmpp_stream {
                    if st_mpp.fps >= 55.0 {
                        // Native 60 Hz video on 60 Hz panel: exact 1:1 frame-per-vsync lock!
                        match st_mpp.try_read_frame() {
                            Ok(Some(f)) => {
                                gpu.upload_bg_nv12(f.width, f.height, &f.nv12);
                                rkmpp_uploads += 1;
                            }
                            Ok(None) => {
                                if rkmpp_uploads > 0 {
                                    rkmpp_underruns += 1;
                                }
                            }
                            Err(StreamEnded) => stream_ended = true,
                        }
                    } else {
                        // Lower-fps video (24/25/30 fps): drift-free monotonic presentation epoch
                        if rkmpp_pending.is_none() {
                            match st_mpp.try_read_frame() {
                                Ok(Some(f)) => rkmpp_pending = Some(f),
                                Ok(None) => {}
                                Err(StreamEnded) => stream_ended = true,
                            }
                        }
                        if !stream_ended {
                            if let Some(ref pend) = rkmpp_pending {
                                let now_inst = std::time::Instant::now();
                                let epoch = match rkmpp_epoch_inst {
                                    Some(ep) if pend.pts_ms >= rkmpp_last_pts_ms => ep,
                                    _ => {
                                        rkmpp_first_pts_ms = pend.pts_ms;
                                        rkmpp_last_pts_ms = pend.pts_ms;
                                        rkmpp_epoch_inst = Some(now_inst);
                                        now_inst
                                    }
                                };
                                let rel_ms = pend.pts_ms.saturating_sub(rkmpp_first_pts_ms);
                                let due = epoch + Duration::from_millis(rel_ms);
                                if now_inst + Duration::from_millis(2) >= due {
                                    if now_inst.saturating_duration_since(due) > Duration::from_millis(250) {
                                        rkmpp_first_pts_ms = pend.pts_ms;
                                        rkmpp_epoch_inst = Some(now_inst);
                                    }
                                    rkmpp_last_pts_ms = pend.pts_ms;
                                    if let Some(f) = rkmpp_pending.take() {
                                        gpu.upload_bg_nv12(f.width, f.height, &f.nv12);
                                        rkmpp_uploads += 1;
                                    }
                                }
                            }
                        }
                    }
                }
                if stream_ended {
                    rkmpp_stream = None;
                    rkmpp_path.clear();
                    rkmpp_pending = None;
                    rkmpp_epoch_inst = None;
                }

                // Live H.264 background stream (`ctx:stream_background`)
                let mut live_stream_active = false;
                if let Some(ref sname) = v2.active_stream_name {
                    if v2_mux.has_stream(sname) {
                        live_stream_active = true;
                        if let Some(f) = v2_mux.take_h264_frame(sname) {
                            gpu.upload_bg_nv12(f.width, f.height, &f.nv12);
                        }
                    }
                }

                // Live bounded H.264 popup streams (`ctx:stream_rect`)
                let mut stream_overlay_rects: Vec<(i32, i32, i32, i32)> = Vec::new();
                for so in &v2.stream_overlays {
                    if v2_mux.has_stream(&so.stream_name) {
                        if let Some(f) = v2_mux.take_h264_frame(&so.stream_name) {
                            gpu.upload_overlay_nv12(f.width, f.height, &f.nv12);
                        }
                        stream_overlay_rects.push((
                            so.x.round() as i32,
                            so.y.round() as i32,
                            so.w.round() as i32,
                            so.h.round() as i32,
                        ));
                    }
                }
                let video_upload_us = t_vid.elapsed().as_micros() as u64;

                let (ww, wh) = v2.wave_dims();
                let has_bg_media = live_stream_active || rkmpp_stream.is_some() || v2_img_loaded;
                let t_gpu = std::time::Instant::now();
                if let Err(e) = gpu.render_v2_and_swap(
                    cached_suspended,
                    has_bg_media,
                    v2.bg_dimming,
                    v2.time_sec,
                    v2.dt_sec,
                    v2.wave_enabled,
                    ww as i32,
                    wh as i32,
                    &v2.wave.rgba,
                    v2.wave_params.refraction_strength,
                    v2.wave_params.specular_strength,
                    [
                        v2.wave_params.tint[0],
                        v2.wave_params.tint[1],
                        v2.wave_params.tint[2],
                        v2.top_fade.clamp(0.0, 0.85),
                    ],
                    &v2.background_shaders,
                    &v2.distortion_shaders,
                    &v2.under_key_shaders,
                    &v2.glows,
                    &v2.particle_dots,
                    &v2.overlay_rgba,
                    v2.overlay_dirty,
                    v2.overlay_dirty_band,
                    &stream_overlay_rects,
                ) {
                    log_diag(&format!("render_v2_and_swap error: {e}"));
                }
                let gpu_swap_us = t_gpu.elapsed().as_micros() as u64;
                let total_render_us = frame_start.elapsed().as_micros() as u64;
                stat_render_ms.store((total_render_us + 500) / 1000, Ordering::Relaxed);
                stat_rkmpp.store(
                    rkmpp_stream.is_some() || v2.active_stream_name.is_some(),
                    Ordering::Relaxed,
                );
                let rkmpp_q = rkmpp_stream.as_ref().map(|s| s.queued_len()).unwrap_or(0);
                perf.record_frame(
                    mono_ms,
                    frame_interval_us,
                    total_render_us,
                    ipc_mux_us,
                    v2.last_lua_us,
                    v2.last_physics_us,
                    video_upload_us,
                    gpu_swap_us,
                    v2.overlay_dirty,
                    rkmpp_q,
                );
            }

            if mono_ms.saturating_sub(last_soc_ms) >= 1000 {
                last_soc_ms = mono_ms;
                let mod_perf = v2.take_module_perf_snapshot();
                let vfps = rkmpp_stream.as_ref().map(|s| s.fps).unwrap_or(0.0);
                let rkmpp_q = rkmpp_stream.as_ref().map(|s| s.queued_len()).unwrap_or(0);
                let perf_json = perf.build_report_json(
                    &mod_perf,
                    vfps,
                    rkmpp_q,
                    rkmpp_underruns,
                    rkmpp_uploads,
                );
                v2.publish_perf_snapshot(perf_json);
            }

            if cached_suspended {
                std::thread::sleep(Duration::from_millis(50));
            } else {
                std::thread::sleep(Duration::from_micros(500));
            }
        }

        let _ = std::fs::remove_file(DISPLAY_ACTIVE_MARKER_DEFAULT);
        log_diag("keyglass-display exited cleanly");
    }
}

#[cfg(target_os = "linux")]
fn apply_brightness_pct(
    pct: u32,
    cached_suspended: bool,
    saved_brightness: &mut u32,
    log_diag: &mut dyn FnMut(&str),
) {
    let max_b = std::fs::read_to_string(BACKLIGHT_MAX_BRIGHTNESS_PATH)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(255);
    let target = (((pct.clamp(5, 100) as f64 / 100.0) * max_b as f64).round() as u32).clamp(1, max_b);
    *saved_brightness = target;
    if !cached_suspended {
        let bl = Path::new(BACKLIGHT_BRIGHTNESS_PATH);
        if bl.exists() {
            let _ = std::fs::write(bl, format!("{target}\n"));
            log_diag(&format!("Backlight set to {pct}% ({target}/{max_b})"));
        }
    }
}

#[cfg(target_os = "linux")]
fn apply_panel_sleep(sleep: bool, saved_brightness: &mut u32, log_diag: &mut dyn FnMut(&str)) {
    let bl_power = Path::new(BACKLIGHT_BL_POWER_PATH);
    let bl_bright = Path::new(BACKLIGHT_BRIGHTNESS_PATH);
    if !bl_power.exists() && !bl_bright.exists() {
        return;
    }
    if sleep {
        if let Ok(raw) = std::fs::read_to_string(bl_bright) {
            if let Ok(cur) = raw.trim().parse::<u32>() {
                if cur > 0 {
                    *saved_brightness = cur;
                    let _ = std::fs::write(SAVED_BRIGHTNESS_PATH, format!("{cur}\n"));
                }
            }
        }
        let _ = std::fs::write(bl_bright, "0\n");
        let _ = std::fs::write(bl_power, "1\n");
        log_diag("Panel sleep: backlight off");
    } else {
        let max_b = std::fs::read_to_string(BACKLIGHT_MAX_BRIGHTNESS_PATH)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(255);
        let restore = (*saved_brightness).clamp(1, max_b);
        let _ = std::fs::write(bl_power, "0\n");
        let _ = std::fs::write(bl_bright, format!("{restore}\n"));
        let _ = std::fs::remove_file(SAVED_BRIGHTNESS_PATH);
        log_diag(&format!("Panel wake: backlight restored ({restore}/{max_b})"));
    }
}

#[cfg(target_os = "linux")]
fn write_soc_stats(
    log_dir: &str,
    prev_cpu: &mut Option<(u64, u64)>,
    render_ms: u64,
    rkmpp: bool,
    ts_sec: i64,
    mono_ms: u64,
) {
    let mut cpu_pct = 0.0_f64;
    if let Ok(stat) = std::fs::read_to_string("/proc/stat") {
        if let Some(line) = stat.lines().next() {
            let nums: Vec<u64> = line
                .split_whitespace()
                .skip(1)
                .filter_map(|s| s.parse::<u64>().ok())
                .collect();
            if nums.len() >= 4 {
                let total: u64 = nums.iter().sum();
                let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
                if let Some((pt, pi)) = *prev_cpu {
                    let dt = total.saturating_sub(pt);
                    let di = idle.saturating_sub(pi);
                    if dt > 0 {
                        cpu_pct = ((dt.saturating_sub(di)) as f64 / dt as f64) * 100.0;
                    }
                }
                *prev_cpu = Some((total, idle));
            }
        }
    }
    let (mut mem_tot_kb, mut mem_avail_kb) = (0_u64, 0_u64);
    if let Ok(mi) = std::fs::read_to_string("/proc/meminfo") {
        for line in mi.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                mem_tot_kb = rest.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                mem_avail_kb = rest.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0);
            }
        }
    }
    let mem_used_mb = mem_tot_kb.saturating_sub(mem_avail_kb) / 1024;
    let mem_total_mb = mem_tot_kb / 1024;

    let mut gpu_pct: Option<f64> = None;
    for dir in ["/sys/devices/platform", "/sys/class/devfreq"] {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for ent in rd.flatten() {
                let name = ent.file_name().to_string_lossy().to_string();
                if name.contains("gpu") {
                    for leaf in ["utilisation", "load"] {
                        if let Ok(s) = std::fs::read_to_string(ent.path().join(leaf)) {
                            if let Ok(v) = s.trim().split('@').next().unwrap_or("").trim().parse::<f64>() {
                                gpu_pct = Some(v.clamp(0.0, 100.0));
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    let gpu_json = gpu_pct.map(|v| format!("{v:.1}")).unwrap_or_else(|| "null".into());
    let p = Path::new(log_dir).join("soc_stats.json");
    let p_tmp = Path::new(log_dir).join("soc_stats.json.tmp");
    let body = format!(
        "{{\"cpuPct\":{cpu_pct:.1},\"gpuPct\":{gpu_json},\"memUsedMb\":{mem_used_mb},\"memTotalMb\":{mem_total_mb},\"renderMs\":{render_ms},\"rkmpp\":{rkmpp},\"tsSec\":{ts_sec},\"sampleMs\":{mono_ms}}}\n"
    );
    if std::fs::write(&p_tmp, body).is_ok() {
        let _ = std::fs::rename(&p_tmp, &p);
    }
}

#[cfg(target_os = "linux")]
struct HitchSample {
    mono_ms: u64,
    interval_ms: f64,
    render_us: u64,
    top_stage: &'static str,
    ipc_mux_us: u64,
    lua_us: u64,
    physics_us: u64,
    video_upload_us: u64,
    gpu_swap_us: u64,
    overlay_uploaded: bool,
    rkmpp_queue: usize,
}

#[cfg(target_os = "linux")]
struct PerfWindow {
    intervals_us: std::collections::VecDeque<u64>,
    renders_us: std::collections::VecDeque<u64>,
    ipc_mux_us: std::collections::VecDeque<u64>,
    lua_us: std::collections::VecDeque<u64>,
    physics_us: std::collections::VecDeque<u64>,
    video_upload_us: std::collections::VecDeque<u64>,
    gpu_swap_us: std::collections::VecDeque<u64>,
    recent_hitches: std::collections::VecDeque<HitchSample>,
    jank_frames_total: u64,
    jank_frames_window: u64,
    overlay_uploads_window: u64,
    frames_window: u64,
}

#[cfg(target_os = "linux")]
impl PerfWindow {
    fn new() -> Self {
        Self {
            intervals_us: std::collections::VecDeque::with_capacity(120),
            renders_us: std::collections::VecDeque::with_capacity(120),
            ipc_mux_us: std::collections::VecDeque::with_capacity(120),
            lua_us: std::collections::VecDeque::with_capacity(120),
            physics_us: std::collections::VecDeque::with_capacity(120),
            video_upload_us: std::collections::VecDeque::with_capacity(120),
            gpu_swap_us: std::collections::VecDeque::with_capacity(120),
            recent_hitches: std::collections::VecDeque::with_capacity(16),
            jank_frames_total: 0,
            jank_frames_window: 0,
            overlay_uploads_window: 0,
            frames_window: 0,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_frame(
        &mut self,
        mono_ms: u64,
        interval_us: u64,
        render_us: u64,
        ipc_mux_us: u64,
        lua_us: u64,
        physics_us: u64,
        video_upload_us: u64,
        gpu_swap_us: u64,
        overlay_uploaded: bool,
        rkmpp_queue: usize,
    ) {
        fn push_cap(q: &mut std::collections::VecDeque<u64>, v: u64) {
            if q.len() >= 120 {
                q.pop_front();
            }
            q.push_back(v);
        }
        push_cap(&mut self.intervals_us, interval_us);
        push_cap(&mut self.renders_us, render_us);
        push_cap(&mut self.ipc_mux_us, ipc_mux_us);
        push_cap(&mut self.lua_us, lua_us);
        push_cap(&mut self.physics_us, physics_us);
        push_cap(&mut self.video_upload_us, video_upload_us);
        push_cap(&mut self.gpu_swap_us, gpu_swap_us);
        self.frames_window += 1;
        if overlay_uploaded {
            self.overlay_uploads_window += 1;
        }
        // Any frame interval > 22 ms (missed a 60 Hz 16.67 ms vsync) counts as a hitch/jank frame.
        if interval_us > 22_000 {
            self.jank_frames_total += 1;
            self.jank_frames_window += 1;
            let stages = [
                ("ipc_mux", ipc_mux_us),
                ("lua", lua_us),
                ("physics_wave", physics_us),
                ("video_upload", video_upload_us),
                ("gpu_swap", gpu_swap_us),
            ];
            let top_stage = stages
                .iter()
                .max_by_key(|(_, v)| *v)
                .map(|(n, _)| *n)
                .unwrap_or("vsync_wait");
            if self.recent_hitches.len() >= 12 {
                self.recent_hitches.pop_front();
            }
            self.recent_hitches.push_back(HitchSample {
                mono_ms,
                interval_ms: (interval_us as f64) / 1000.0,
                render_us,
                top_stage,
                ipc_mux_us,
                lua_us,
                physics_us,
                video_upload_us,
                gpu_swap_us,
                overlay_uploaded,
                rkmpp_queue,
            });
        }
    }

    fn percentile(q: &std::collections::VecDeque<u64>, pct: f64) -> u64 {
        if q.is_empty() {
            return 0;
        }
        let mut v: Vec<u64> = q.iter().copied().collect();
        v.sort_unstable();
        let idx = (((v.len() - 1) as f64) * pct).round() as usize;
        v[idx.min(v.len() - 1)]
    }

    fn avg(q: &std::collections::VecDeque<u64>) -> u64 {
        if q.is_empty() {
            return 0;
        }
        q.iter().sum::<u64>() / q.len() as u64
    }

    fn max(q: &std::collections::VecDeque<u64>) -> u64 {
        q.iter().copied().max().unwrap_or(0)
    }

    fn build_report_json(
        &mut self,
        mod_perf: &[keyglass_modules::ModulePerfStats],
        video_fps: f64,
        rkmpp_queue: usize,
        rkmpp_underruns: u64,
        rkmpp_uploads: u64,
    ) -> serde_json::Value {
        let p50_int = Self::percentile(&self.intervals_us, 0.50);
        let p95_int = Self::percentile(&self.intervals_us, 0.95);
        let p99_int = Self::percentile(&self.intervals_us, 0.99);
        let max_int = Self::max(&self.intervals_us);
        let fps = self.frames_window as f64;
        let jank_1s = self.jank_frames_window;
        let ov_1s = self.overlay_uploads_window;
        self.frames_window = 0;
        self.jank_frames_window = 0;
        self.overlay_uploads_window = 0;

        let hitches: Vec<serde_json::Value> = self
            .recent_hitches
            .iter()
            .rev()
            .take(8)
            .map(|h| {
                serde_json::json!({
                    "mono_ms": h.mono_ms,
                    "interval_ms": (h.interval_ms * 10.0).round() / 10.0,
                    "render_us": h.render_us,
                    "top_stage": h.top_stage,
                    "ipc_mux_us": h.ipc_mux_us,
                    "lua_us": h.lua_us,
                    "physics_us": h.physics_us,
                    "video_upload_us": h.video_upload_us,
                    "gpu_swap_us": h.gpu_swap_us,
                    "overlay_uploaded": h.overlay_uploaded,
                    "rkmpp_queue": h.rkmpp_queue,
                })
            })
            .collect();

        serde_json::json!({
            "fps": fps,
            "interval_ms": {
                "p50": (p50_int as f64 / 100.0).round() / 10.0,
                "p95": (p95_int as f64 / 100.0).round() / 10.0,
                "p99": (p99_int as f64 / 100.0).round() / 10.0,
                "max": (max_int as f64 / 100.0).round() / 10.0,
            },
            "render_us": {
                "p50": Self::percentile(&self.renders_us, 0.50),
                "p95": Self::percentile(&self.renders_us, 0.95),
                "p99": Self::percentile(&self.renders_us, 0.99),
                "max": Self::max(&self.renders_us),
            },
            "stages_us": {
                "ipc_mux_avg": Self::avg(&self.ipc_mux_us),
                "ipc_mux_max": Self::max(&self.ipc_mux_us),
                "lua_avg": Self::avg(&self.lua_us),
                "lua_max": Self::max(&self.lua_us),
                "physics_avg": Self::avg(&self.physics_us),
                "physics_max": Self::max(&self.physics_us),
                "video_upload_avg": Self::avg(&self.video_upload_us),
                "video_upload_max": Self::max(&self.video_upload_us),
                "gpu_swap_avg": Self::avg(&self.gpu_swap_us),
                "gpu_swap_max": Self::max(&self.gpu_swap_us),
            },
            "jank_frames_1s": jank_1s,
            "jank_frames_total": self.jank_frames_total,
            "overlay_uploads_1s": ov_1s,
            "video": {
                "fps": (video_fps * 100.0).round() / 100.0,
                "rkmpp_queue": rkmpp_queue,
                "rkmpp_underruns": rkmpp_underruns,
                "rkmpp_uploads": rkmpp_uploads,
            },
            "modules": mod_perf.iter().take(12).collect::<Vec<_>>(),
            "recent_hitches": hitches,
        })
    }
}
