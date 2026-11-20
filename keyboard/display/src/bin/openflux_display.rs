#[cfg(target_os = "linux")]
use openflux_display::device_paths::DISPLAY_ACTIVE_MARKER_DEFAULT;
use openflux_display::device_paths::{
    BACKLIGHT_BL_POWER_PATH, BACKLIGHT_BRIGHTNESS_PATH, BACKLIGHT_MAX_BRIGHTNESS_PATH,
    BRIDGE_SOCK_DEFAULT, DISPLAY_CONFIG_DIR_DEFAULT, DISPLAY_LOG_DIR_DEFAULT,
    DISPLAY_MEDIA_DIR_DEFAULT, SAVED_BRIGHTNESS_PATH,
};
use openflux_display::layout::{DISPLAY_HEIGHT, DISPLAY_WIDTH};
use openflux_display::{DisplayConfig, FfmpegVideoAdapter, LiveState, Renderer};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "linux")]
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DISPLAY_VERSION: &str = match option_env!("OPENFLUX_DISPLAY_VERSION_STR") {
    Some(v) => v,
    None => concat!("openflux-display ", env!("CARGO_PKG_VERSION")),
};

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn needs_per_second_ui_tick(cfg: &DisplayConfig) -> bool {
    cfg.mosaic.tiles.iter().any(|t| {
        matches!(
            t.tile_type,
            openflux_display::config::TileType::AnalogClock
                | openflux_display::config::TileType::VirtualPet
        )
    })
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct UiRenderReq {
    cfg: DisplayConfig,
    state: LiveState,
    media_dir: String,
    now_sec: i64,
    mono_ms: u64,
    suspended: bool,
    invalidate_assets: bool,
}

/// Coalescing off-thread CPU UI overlay worker so video presentation never waits on UI rasterization.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct UiOverlayWorker {
    req_slot: Arc<(Mutex<Option<UiRenderReq>>, Condvar)>,
    ready_slot: Arc<Mutex<Option<Vec<u32>>>>,
    spare_slot: Arc<Mutex<Option<Vec<u32>>>>,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl UiOverlayWorker {
    fn spawn(width: i32, height: i32) -> Self {
        let req_slot: Arc<(Mutex<Option<UiRenderReq>>, Condvar)> =
            Arc::new((Mutex::new(None), Condvar::new()));
        let ready_slot: Arc<Mutex<Option<Vec<u32>>>> = Arc::new(Mutex::new(None));
        let spare_slot: Arc<Mutex<Option<Vec<u32>>>> = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));

        let req_bg = req_slot.clone();
        let ready_bg = ready_slot.clone();
        let spare_bg = spare_slot.clone();
        let stop_bg = stop.clone();

        let join = std::thread::Builder::new()
            .name("ui-overlay".into())
            .spawn(move || {
                let mut renderer = Renderer::new(width, height);
                let px_len = (width.max(1) as usize) * (height.max(1) as usize);
                let mut scratch = vec![0u32; px_len];
                loop {
                    let req = {
                        let (mtx, cv) = &*req_bg;
                        let mut g = mtx.lock().unwrap();
                        loop {
                            if stop_bg.load(Ordering::SeqCst) {
                                return;
                            }
                            if let Some(r) = g.take() {
                                break r;
                            }
                            g = cv.wait(g).unwrap();
                        }
                    };
                    if req.invalidate_assets {
                        renderer.assets.invalidate();
                    }
                    let mdir = (!req.media_dir.is_empty()).then_some(req.media_dir.as_str());
                    renderer.render_ui_overlay_at(
                        &req.cfg,
                        &req.state,
                        mdir,
                        req.now_sec,
                        req.mono_ms,
                        req.suspended,
                    );
                    if let Ok(mut sp) = spare_bg.lock() {
                        if let Some(b) = sp.take() {
                            if b.len() == renderer.pixels.len() {
                                scratch = b;
                            }
                        }
                    }
                    if scratch.len() != renderer.pixels.len() {
                        scratch.resize(renderer.pixels.len(), 0);
                    }
                    scratch.copy_from_slice(&renderer.pixels);
                    if let Ok(mut rg) = ready_bg.lock() {
                        if let Some(prev) = rg.replace(scratch) {
                            scratch = prev;
                        } else {
                            scratch = Vec::new();
                        }
                    }
                }
            })
            .ok();

        Self {
            req_slot,
            ready_slot,
            spare_slot,
            stop,
            join,
        }
    }

    fn submit(&self, req: UiRenderReq) {
        let (mtx, cv) = &*self.req_slot;
        if let Ok(mut g) = mtx.lock() {
            if let Some(prev) = g.as_mut() {
                let keep_inv = prev.invalidate_assets || req.invalidate_assets;
                *prev = req;
                prev.invalidate_assets = keep_inv;
            } else {
                *g = Some(req);
            }
            cv.notify_one();
        }
    }

    fn take_ready(&self) -> Option<Vec<u32>> {
        self.ready_slot.try_lock().ok().and_then(|mut g| g.take())
    }

    fn recycle(&self, buf: Vec<u32>) {
        if let Ok(mut g) = self.spare_slot.try_lock() {
            *g = Some(buf);
        }
    }
}

impl Drop for UiOverlayWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let (mtx, cv) = &*self.req_slot;
        if let Ok(_g) = mtx.lock() {
            cv.notify_one();
        }
        if let Some(jh) = self.join.take() {
            let _ = jh.join();
        }
    }
}

#[cfg(target_os = "linux")]
enum IoTask {
    Heartbeat {
        log_dir: String,
        secs: u64,
    },
    PomoState {
        log_dir: String,
        body: String,
    },
    SocStats {
        log_dir: String,
        render_ms: u64,
        rkmpp: bool,
        ts_sec: i64,
        mono_ms: u64,
    },
    DiagLine {
        diag_path: std::path::PathBuf,
        msg: String,
    },
}

#[cfg(target_os = "linux")]
struct IoWorker {
    tx: Option<mpsc::SyncSender<IoTask>>,
    join: Option<std::thread::JoinHandle<()>>,
}

#[cfg(target_os = "linux")]
impl IoWorker {
    fn spawn() -> Self {
        let (tx, rx) = mpsc::sync_channel::<IoTask>(32);
        let join = std::thread::Builder::new()
            .name("display-io".into())
            .spawn(move || {
                let mut prev_cpu_ticks: Option<(u64, u64)> = None;
                let mut diag_bytes: usize = 0;
                while let Ok(task) = rx.recv() {
                    match task {
                        IoTask::Heartbeat { log_dir, secs } => {
                            if !log_dir.is_empty() {
                                let p = Path::new(&log_dir).join("display_heartbeat.log");
                                let _ = std::fs::write(p, format!("{secs}\n"));
                            }
                        }
                        IoTask::PomoState { log_dir, body } => {
                            if !log_dir.is_empty() {
                                let p_path = Path::new(&log_dir).join("pomo_state.json");
                                let p_tmp = Path::new(&log_dir).join("pomo_state.json.tmp");
                                if std::fs::write(&p_tmp, body).is_ok() {
                                    let _ = std::fs::rename(&p_tmp, &p_path);
                                }
                            }
                        }
                        IoTask::SocStats {
                            log_dir,
                            render_ms,
                            rkmpp,
                            ts_sec,
                            mono_ms,
                        } => {
                            if !log_dir.is_empty() {
                                write_soc_stats(
                                    &log_dir,
                                    &mut prev_cpu_ticks,
                                    render_ms,
                                    rkmpp,
                                    ts_sec,
                                    mono_ms,
                                );
                            }
                        }
                        IoTask::DiagLine { diag_path, msg } => {
                            if diag_bytes > 8192 {
                                let _ = std::fs::write(&diag_path, "");
                                diag_bytes = 0;
                            }
                            use std::io::Write;
                            if let Ok(mut f) = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&diag_path)
                            {
                                if writeln!(f, "{msg}").is_ok() {
                                    diag_bytes += msg.len() + 1;
                                }
                            }
                        }
                    }
                }
            })
            .ok();
        Self { tx: Some(tx), join }
    }

    fn send(&self, task: IoTask) -> bool {
        self.tx.as_ref().is_some_and(|tx| tx.try_send(task).is_ok())
    }
}

#[cfg(target_os = "linux")]
impl Drop for IoWorker {
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(jh) = self.join.take() {
            let _ = jh.join();
        }
    }
}

fn write_ppm_file(path: &Path, w: i32, h: i32, pixels: &[u32]) -> Result<(), String> {
    let hdr = format!("P6\n{w} {h}\n255\n");
    let mut out = Vec::with_capacity(hdr.len() + pixels.len() * 3);
    out.extend_from_slice(hdr.as_bytes());
    for &argb in pixels {
        out.push(((argb >> 16) & 0xFF) as u8);
        out.push(((argb >> 8) & 0xFF) as u8);
        out.push((argb & 0xFF) as u8);
    }
    std::fs::write(path, out).map_err(|e| e.to_string())
}

fn apply_live_json_file(path: &str, state: &mut LiveState, now_ms: u64) {
    if let Ok(f) = std::fs::File::open(path) {
        let reader = BufReader::new(f);
        for line in reader.lines().map_while(Result::ok) {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                let _ = state.apply_live_json(trimmed, now_ms);
            }
        }
    }
}

fn main() {
    let mut width = DISPLAY_WIDTH;
    let mut height = DISPLAY_HEIGHT;
    let mut config_dir =
        std::env::var("OPENFLUX_CONFIG_DIR").unwrap_or_else(|_| DISPLAY_CONFIG_DIR_DEFAULT.into());
    let mut media_dir =
        std::env::var("OPENFLUX_MEDIA_DIR").unwrap_or_else(|_| DISPLAY_MEDIA_DIR_DEFAULT.into());
    let log_dir =
        std::env::var("OPENFLUX_LOG_DIR").unwrap_or_else(|_| DISPLAY_LOG_DIR_DEFAULT.into());
    let sock_path =
        std::env::var("OPENFLUX_BRIDGE_SOCK").unwrap_or_else(|_| BRIDGE_SOCK_DEFAULT.into());
    let mut render_ppm: Option<String> = None;
    let mut live_json_path: Option<String> = None;

    let args: Vec<String> = std::env::args().collect();
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
            "--config-dir" if i + 1 < args.len() => {
                i += 1;
                config_dir = args[i].clone();
            }
            "--media-dir" if i + 1 < args.len() => {
                i += 1;
                media_dir = args[i].clone();
            }
            "--render-ppm" if i + 1 < args.len() => {
                i += 1;
                render_ppm = Some(args[i].clone());
            }
            "--live-json" if i + 1 < args.len() => {
                i += 1;
                live_json_path = Some(args[i].clone());
            }
            _ => {}
        }
        i += 1;
    }

    let cfg_path = Path::new(&config_dir);
    let mut cfg = DisplayConfig::load_dir(cfg_path);
    let mut state = LiveState::from_config(&cfg);
    if let Some(ref lj) = live_json_path {
        apply_live_json_file(lj, &mut state, 0);
        cfg.reconcile_live_theme(&state);
    }

    let mut renderer = Renderer::new(width, height);
    let ffmpeg_bin = std::env::var_os("OPENFLUX_FFMPEG")
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from);
    renderer
        .bg
        .set_video_adapter(Some(Arc::new(FfmpegVideoAdapter {
            ffmpeg_bin,
            ..FfmpegVideoAdapter::default()
        })));

    if let Some(ref ppm_out) = render_ppm {
        let target_frame_env = std::env::var("OPENFLUX_VIDEO_TARGET_FRAME")
            .ok()
            .and_then(|s| s.parse::<u64>().ok());
        let target_frame: u64 = target_frame_env.filter(|&v| v > 1).unwrap_or(1);
        let render_sec = std::env::var("OPENFLUX_FIXED_TIME_SEC")
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .filter(|&v| v > 0)
            .unwrap_or_else(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64
            });
        let mut step_ms = 0u64;
        renderer.render_frame_at(&cfg, &state, Some(&media_dir), render_sec, step_ms, false);
        let video_requested = renderer.bg.is_video;
        for _w in 0..500 {
            if !renderer.bg.is_video || renderer.bg.video_frames_rendered >= target_frame {
                break;
            }
            if let Some(due_ms) = renderer.bg.next_video_due_ms() {
                step_ms = step_ms.max(due_ms);
                renderer.render_frame_at(
                    &cfg,
                    &state,
                    Some(&media_dir),
                    render_sec,
                    step_ms,
                    false,
                );
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if video_requested
            && target_frame_env.is_some()
            && renderer.bg.video_frames_rendered < target_frame
        {
            eprintln!(
                "Timed out/EOF waiting for video frame {} (rendered {})",
                target_frame, renderer.bg.video_frames_rendered
            );
            drop(renderer);
            std::process::exit(1);
        }
        if let Err(e) = write_ppm_file(
            Path::new(ppm_out),
            renderer.width,
            renderer.height,
            &renderer.pixels,
        ) {
            eprintln!("Failed to write PPM: {e}");
            std::process::exit(1);
        }
        return;
    }

    #[cfg(target_os = "linux")]
    {
        use openflux_display::config::ConfigReloader;
        use openflux_display::ipc::BridgeIpcClient;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::Instant;

        static RUNNING: AtomicBool = AtomicBool::new(true);
        extern "C" fn on_sig(_sig: libc::c_int) {
            RUNNING.store(false, Ordering::SeqCst);
        }
        unsafe {
            libc::signal(libc::SIGINT, on_sig as usize);
            libc::signal(libc::SIGTERM, on_sig as usize);
        }

        struct MarkerGuard(String);
        impl Drop for MarkerGuard {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }

        let marker = std::env::var("OPENFLUX_DISPLAY_MARKER")
            .unwrap_or_else(|_| DISPLAY_ACTIVE_MARKER_DEFAULT.into());
        let _ = std::fs::write(&marker, "");
        let _marker_guard = MarkerGuard(marker);

        enum ActivePresenter {
            Gpu(openflux_display::wayland_egl::WaylandGpuPresenter),
            Shm(openflux_display::wayland::WaylandPresenter),
        }
        impl ActivePresenter {
            fn dispatch(&mut self) -> Result<bool, String> {
                match self {
                    Self::Gpu(g) => g.dispatch(),
                    Self::Shm(s) => s.dispatch(),
                }
            }
            fn is_gpu(&self) -> bool {
                matches!(self, Self::Gpu(_))
            }
            fn raw_fd(&self) -> i32 {
                match self {
                    Self::Gpu(g) => g.raw_fd(),
                    Self::Shm(s) => s.raw_fd(),
                }
            }
        }

        let force_shm = std::env::var("OPENFLUX_DISABLE_GPU").ok().as_deref() == Some("1")
            || std::env::var("OPENFLUX_LIVE_PPM_PATH")
                .map(|s| !s.is_empty())
                .unwrap_or(false);
        let mut presenter = if !force_shm {
            match openflux_display::wayland_egl::WaylandGpuPresenter::open(
                width,
                height,
                "openflux-display",
            ) {
                Ok(g) => ActivePresenter::Gpu(g),
                Err(ge) => {
                    eprintln!("Wayland EGL GPU unavailable ({ge}), falling back to Wayland SHM");
                    match openflux_display::wayland::WaylandPresenter::open(
                        width,
                        height,
                        "openflux-display",
                    ) {
                        Ok(s) => ActivePresenter::Shm(s),
                        Err(se) => {
                            eprintln!("Wayland unavailable: {se}");
                            drop(_marker_guard);
                            std::process::exit(1);
                        }
                    }
                }
            }
        } else {
            match openflux_display::wayland::WaylandPresenter::open(
                width,
                height,
                "openflux-display",
            ) {
                Ok(s) => ActivePresenter::Shm(s),
                Err(se) => {
                    eprintln!("Wayland unavailable: {se}");
                    drop(_marker_guard);
                    std::process::exit(1);
                }
            }
        };

        let mut ipc = BridgeIpcClient::new();
        let _ = ipc.connect(&sock_path);

        let start = Instant::now();
        let mut reloader = ConfigReloader::new(cfg_path);
        let mut last_hb_sec = 0u64;
        let mut render_dirty = true;
        let mut ui_dirty = true;
        let mut bg_dirty = true;
        let mut fx_dirty = true;
        let mut rkmpp_stream: Option<openflux_display::rkmpp::RkmppVideoStream> = None;
        let mut rkmpp_path = String::new();
        let mut rkmpp_failed_path = String::new();
        let mut rkmpp_epoch_ms: u64 = 0;
        let mut rkmpp_epoch_set = false;
        let mut rkmpp_pending: Option<openflux_display::rkmpp::Nv12Frame> = None;
        let mut rkmpp_due: Option<openflux_display::rkmpp::Nv12Frame> = None;
        let mut rkmpp_vframes: u64 = 0;
        let mut last_static_bg_sig: (String, i64, i64, u32) = (String::new(), -1, -1, 0);
        let mut last_layer = state.effective_layer(&cfg.keymap);
        let mut last_key_bitmap = state.key_bitmap;

        let mut frame_log_count: u32 = 0;
        let mut last_slow_log_ms: u64 = 0;
        let mut last_vframe_present_ms: Option<u64> = None;
        let mut last_gap_log_ms: u64 = 0;
        let mut rkmpp_starve_count: u64 = 0;
        let mut rkmpp_starve_at_last_gap: u64 = 0;
        let mut rkmpp_rebase_count: u64 = 0;
        let mut last_clock_min: i64 = -1;
        let diag_log = if !log_dir.is_empty() && Path::new(&log_dir).is_dir() {
            Some(Path::new(&log_dir).join("display_diag.log"))
        } else {
            None
        };
        if let Some(ref lp) = diag_log {
            let _ = std::fs::write(lp, "");
        }
        let io_worker = IoWorker::spawn();
        let ui_worker = if presenter.is_gpu() {
            Some(UiOverlayWorker::spawn(width, height))
        } else {
            None
        };
        let mut log_diag = |msg: &str| {
            eprintln!("{msg}");
            if let Some(ref lp) = diag_log {
                io_worker.send(IoTask::DiagLine {
                    diag_path: lp.clone(),
                    msg: msg.to_string(),
                });
            }
        };
        log_diag(&format!(
            "{DISPLAY_VERSION} starting (backend={}, cfg={}, media={})",
            if presenter.is_gpu() {
                "Mali-G52 EGL/GLES2+RKMPP"
            } else {
                "Wayland-SHM-CPU"
            },
            config_dir,
            media_dir
        ));

        let mut next_reload_ms: u64 = 0;
        let mut next_soc_stats_ms: u64 = 0;
        let mut last_render_ms: u64 = 0;
        let mut last_poll_ms: u64 = 0;
        let mut prev_had_ripple_anim = false;
        let mut ui_initial_uploaded = false;
        let mut ui_assets_dirty = false;
        let dev_boot_id = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );
        let mut last_pomo_state_written: (bool, u32, u64, u64) =
            (false, u32::MAX, u64::MAX, u64::MAX);
        let mut cached_suspended =
            state.is_display_asleep(&cfg, start.elapsed().as_millis() as u64);
        let mut saved_backlight_brightness = read_backlight_brightness();
        if cached_suspended {
            apply_panel_sleep(true, &mut saved_backlight_brightness, &mut log_diag);
        } else if is_panel_hw_asleep() {
            apply_panel_sleep(false, &mut saved_backlight_brightness, &mut log_diag);
        }
        let mut last_localtime_sig: (Option<std::path::PathBuf>, Option<std::time::SystemTime>) = (
            std::fs::read_link("/etc/localtime").ok(),
            std::fs::symlink_metadata("/etc/localtime")
                .and_then(|m| m.modified())
                .ok(),
        );

        while RUNNING.load(Ordering::SeqCst) {
            let now_sec = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let mono_ms = start.elapsed().as_millis() as u64;
            if now_sec as u64 != last_hb_sec {
                io_worker.send(IoTask::Heartbeat {
                    log_dir: log_dir.clone(),
                    secs: now_sec as u64,
                });
                last_hb_sec = now_sec as u64;
                let has_sec_tile = needs_per_second_ui_tick(&cfg);
                let cur_min = now_sec.div_euclid(60);
                if has_sec_tile || cur_min != last_clock_min {
                    last_clock_min = cur_min;
                    render_dirty = true;
                    ui_dirty = true;
                }
                let sig = (
                    std::fs::read_link("/etc/localtime").ok(),
                    std::fs::symlink_metadata("/etc/localtime")
                        .and_then(|m| m.modified())
                        .ok(),
                );
                if sig != last_localtime_sig {
                    last_localtime_sig = sig;
                    #[cfg(unix)]
                    {
                        let tz_val = std::fs::canonicalize("/etc/localtime")
                            .ok()
                            .and_then(|p| p.to_str().map(|s| s.to_string()))
                            .unwrap_or_else(|| "/etc/localtime".to_string());
                        std::env::set_var("TZ", &tz_val);
                        extern "C" {
                            fn tzset();
                        }
                        unsafe {
                            tzset();
                        }
                    }
                    render_dirty = true;
                    ui_dirty = true;
                }
            }
            let prev_pomo = state.pomo_remaining_s;
            state.tick_pomo(mono_ms);
            if state.pomo_remaining_s != prev_pomo {
                render_dirty = true;
                ui_dirty = true;
            }
            if !log_dir.is_empty() {
                let cur_pomo_sig = (
                    state.pomo_running,
                    state.pomo_remaining_s,
                    state.dev_pomo_rev,
                    state.last_pomo_rev,
                );
                if cur_pomo_sig != last_pomo_state_written {
                    let body = format!(
                        "{{\"running\":{},\"remainingS\":{},\"devRev\":{},\"hostCmdSeq\":{},\"devBootId\":\"{}\"}}\n",
                        state.pomo_running, state.pomo_remaining_s, state.dev_pomo_rev, state.last_pomo_rev, dev_boot_id
                    );
                    if io_worker.send(IoTask::PomoState {
                        log_dir: log_dir.clone(),
                        body,
                    }) {
                        last_pomo_state_written = cur_pomo_sig;
                    }
                }
            }
            let is_ripple =
                cfg.appearance.key_effect == openflux_display::config::KeyEffect::Ripple;
            let mut anim_active = false;
            let has_ripple_anim = is_ripple
                && state
                    .ripple_pulses
                    .iter()
                    .any(|&(_, ts)| mono_ms >= ts && mono_ms.saturating_sub(ts) < 2140);
            if has_ripple_anim || prev_had_ripple_anim {
                anim_active = has_ripple_anim;
                render_dirty = true;
                fx_dirty = true;
            }
            prev_had_ripple_anim = has_ripple_anim;
            for k in 0..openflux_display::config::MAX_KEYS {
                let anim_press = !is_ripple
                    && state.is_key_pressed(k)
                    && state.key_press_ms[k] > 0
                    && mono_ms.saturating_sub(state.key_press_ms[k]) < 460;
                let anim_rel = !is_ripple
                    && !state.is_key_pressed(k)
                    && state.key_release_ms[k] > 0
                    && mono_ms.saturating_sub(state.key_release_ms[k]) < 520;
                if anim_press || anim_rel {
                    anim_active = true;
                    render_dirty = true;
                    fx_dirty = true;
                    break;
                }
            }

            if !log_dir.is_empty() && mono_ms >= next_soc_stats_ms {
                next_soc_stats_ms = mono_ms + 2000;
                io_worker.send(IoTask::SocStats {
                    log_dir: log_dir.clone(),
                    render_ms: last_render_ms,
                    rkmpp: rkmpp_stream.is_some(),
                    ts_sec: now_sec,
                    mono_ms,
                });
            }

            if mono_ms >= next_reload_ms {
                next_reload_ms = mono_ms + 250;
                if reloader.poll_reload(&mut cfg) {
                    renderer.assets.invalidate();
                    ui_assets_dirty = true;
                    rkmpp_failed_path.clear();
                    render_dirty = true;
                    ui_dirty = true;
                    bg_dirty = true;
                    fx_dirty = true;
                }
                let susp = state.is_display_asleep(&cfg, mono_ms);
                if susp != cached_suspended {
                    cached_suspended = susp;
                    apply_panel_sleep(susp, &mut saved_backlight_brightness, &mut log_diag);
                    render_dirty = true;
                    ui_dirty = true;
                    bg_dirty = true;
                }
            }

            if !ipc.is_connected() {
                let _ = ipc.connect(&sock_path);
            }
            let prev_ui_sig = (
                state.active_profile,
                state.active_layer,
                state.active_layer_bitfield,
                state.active_actuation_mode,
                state.language_code,
                state.platform_code,
                state.caps_lock,
                state.num_lock,
                state.scroll_lock,
                state.key_bitmap,
            );
            let mut got_live = false;
            let mut got_live_data_payload = false;
            let mut triggered_action = false;
            let mut theme_assets_changed = false;
            let mut theme_reconciled = false;
            for line in ipc.poll_lines() {
                let res =
                    cfg.apply_ipc_line(&mut state, &line, mono_ms, Some(Path::new(&config_dir)));
                if res.applied {
                    got_live = true;
                    got_live_data_payload |= res.is_live_data_payload;
                    triggered_action |= res.triggered_action;
                    theme_assets_changed |= res.theme_cycled;
                    theme_reconciled |= res.theme_reconciled;
                }
                if !state.pending_brightness_steps.is_empty() {
                    let steps = std::mem::take(&mut state.pending_brightness_steps);
                    apply_brightness_steps(
                        &steps,
                        cached_suspended,
                        &mut saved_backlight_brightness,
                        &mut log_diag,
                    );
                }
            }
            if got_live {
                let susp = state.is_display_asleep(&cfg, mono_ms);
                if susp != cached_suspended {
                    cached_suspended = susp;
                    apply_panel_sleep(susp, &mut saved_backlight_brightness, &mut log_diag);
                    render_dirty = true;
                    ui_dirty = true;
                    bg_dirty = true;
                }
                let cur_layer = state.effective_layer(&cfg.keymap);
                let new_ui_sig = (
                    state.active_profile,
                    state.active_layer,
                    state.active_layer_bitfield,
                    state.active_actuation_mode,
                    state.language_code,
                    state.platform_code,
                    state.caps_lock,
                    state.num_lock,
                    state.scroll_lock,
                    state.key_bitmap,
                );
                if cur_layer != last_layer || state.key_bitmap != last_key_bitmap {
                    if cur_layer != last_layer {
                        last_layer = cur_layer;
                    }
                    last_key_bitmap = state.key_bitmap;
                    fx_dirty = true;
                    render_dirty = true;
                    ui_dirty = true;
                } else if new_ui_sig != prev_ui_sig || got_live_data_payload || triggered_action {
                    render_dirty = true;
                    ui_dirty = true;
                }
            }

            if cfg.reconcile_live_theme(&state) || theme_reconciled {
                render_dirty = true;
                ui_dirty = true;
                bg_dirty = true;
                fx_dirty = true;
            }
            if theme_assets_changed {
                renderer.assets.invalidate();
                ui_assets_dirty = true;
                rkmpp_failed_path.clear();
                render_dirty = true;
                ui_dirty = true;
                bg_dirty = true;
                fx_dirty = true;
            }

            if ui_dirty && ui_initial_uploaded {
                if let Some(ref worker) = ui_worker {
                    worker.submit(UiRenderReq {
                        cfg: cfg.clone(),
                        state: state.clone(),
                        media_dir: media_dir.clone(),
                        now_sec,
                        mono_ms,
                        suspended: cached_suspended,
                        invalidate_assets: ui_assets_dirty,
                    });
                    ui_assets_dirty = false;
                    ui_dirty = false;
                }
            }

            match presenter.dispatch() {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => {
                    eprintln!("Wayland dispatch error, exiting cleanly: {e}");
                    break;
                }
            }

            let mut video_wait_ms: Option<i32> = None;
            match presenter {
                ActivePresenter::Gpu(ref mut gpu) => {
                    let bg_is_shader =
                        openflux_display::media_bg::is_shader_path(&cfg.appearance.bg_path);
                    let bg_is_vid = !bg_is_shader
                        && openflux_display::media_bg::is_video_path(&cfg.appearance.bg_path);
                    if bg_is_shader && !cached_suspended {
                        anim_active = true;
                        bg_dirty = true;
                        render_dirty = true;
                    }
                    if !bg_is_vid || cached_suspended {
                        if rkmpp_stream.is_some() {
                            rkmpp_stream = None;
                            rkmpp_path.clear();
                            rkmpp_pending = None;
                            rkmpp_due = None;
                            rkmpp_epoch_set = false;
                            bg_dirty = true;
                        }
                        if cached_suspended {
                            renderer.bg.invalidate();
                            last_static_bg_sig = (String::new(), -1, -1, 0);
                        }
                    } else if cfg.appearance.bg_path != rkmpp_path
                        && cfg.appearance.bg_path != rkmpp_failed_path
                    {
                        rkmpp_stream = None;
                        rkmpp_path.clear();
                        rkmpp_pending = None;
                        rkmpp_due = None;
                        rkmpp_epoch_set = false;
                        let rp = openflux_display::media_bg::resolve_bg_media_path(
                            Some(&media_dir),
                            &cfg.appearance.bg_path,
                        );
                        if !rp.is_empty() && Path::new(&rp).is_file() {
                            if let Some(st_mpp) = openflux_display::rkmpp::open_rkmpp_stream(&rp) {
                                log_diag(&format!(
                                    "RKMPP hardware decode opened: {} ({:.1} fps)",
                                    cfg.appearance.bg_path, st_mpp.fps
                                ));
                                renderer.bg.close_video();
                                rkmpp_stream = Some(st_mpp);
                                rkmpp_path = cfg.appearance.bg_path.clone();
                                bg_dirty = true;
                            } else {
                                log_diag(&format!(
                                    "RKMPP fallback to openflux-ffmpeg for: {}",
                                    cfg.appearance.bg_path
                                ));
                                rkmpp_failed_path = cfg.appearance.bg_path.clone();
                                renderer.bg.invalidate();
                                bg_dirty = true;
                            }
                        } else {
                            rkmpp_failed_path = cfg.appearance.bg_path.clone();
                        }
                    }

                    if let Some(ref mut st_mpp) = rkmpp_stream {
                        if rkmpp_pending.is_none() {
                            match st_mpp.try_read_frame() {
                                Ok(Some(f)) => rkmpp_pending = Some(f),
                                Ok(None) => {}
                                Err(()) => {
                                    rkmpp_stream = None;
                                    rkmpp_failed_path = rkmpp_path.clone();
                                    rkmpp_path.clear();
                                    rkmpp_pending = None;
                                    rkmpp_due = None;
                                    renderer.bg.invalidate();
                                    last_static_bg_sig = (String::new(), -1, -1, 0);
                                    bg_dirty = true;
                                }
                            }
                        }
                        if let Some(ref mut st_mpp) = rkmpp_stream {
                            if let Some(ref pend) = rkmpp_pending {
                                if !rkmpp_epoch_set {
                                    rkmpp_epoch_ms = mono_ms.saturating_sub(pend.pts_ms);
                                    rkmpp_epoch_set = true;
                                }
                                const VSYNC_LEAD_MS: u64 = 10;
                                let elapsed = mono_ms.saturating_sub(rkmpp_epoch_ms);
                                if pend.pts_ms <= elapsed.saturating_add(VSYNC_LEAD_MS) {
                                    let mut best = rkmpp_pending.take().unwrap();
                                    loop {
                                        match st_mpp.try_read_frame() {
                                            Ok(Some(nxt)) => {
                                                if nxt.pts_ms <= elapsed {
                                                    best = nxt;
                                                } else {
                                                    rkmpp_pending = Some(nxt);
                                                    break;
                                                }
                                            }
                                            Ok(None) | Err(()) => break,
                                        }
                                    }
                                    if rkmpp_pending.is_none()
                                        && elapsed > best.pts_ms.saturating_add(120)
                                    {
                                        rkmpp_epoch_ms = mono_ms.saturating_sub(best.pts_ms);
                                        rkmpp_rebase_count = rkmpp_rebase_count.saturating_add(1);
                                    }
                                    rkmpp_due = Some(best);
                                } else {
                                    let target_ms = pend.pts_ms.saturating_sub(VSYNC_LEAD_MS);
                                    video_wait_ms = Some(
                                        (target_ms.saturating_sub(elapsed)).clamp(1, 8) as i32,
                                    );
                                }
                            } else if rkmpp_epoch_set {
                                rkmpp_starve_count = rkmpp_starve_count.saturating_add(1);
                            }
                        }
                    }

                    let mut pending_ui_frame = if !gpu.is_frame_pending() {
                        ui_worker.as_ref().and_then(|w| w.take_ready())
                    } else {
                        None
                    };
                    let has_ui_ready = pending_ui_frame.is_some();

                    let fallback_video_due = !cached_suspended
                        && rkmpp_stream.is_none()
                        && renderer.bg.has_ready_video_frame_at(mono_ms);
                    if !gpu.is_frame_pending()
                        && (rkmpp_due.is_some()
                            || fallback_video_due
                            || (bg_is_shader && !cached_suspended)
                            || bg_dirty
                            || !ui_initial_uploaded
                            || has_ui_ready
                            || fx_dirty)
                    {
                        let t_ren = Instant::now();
                        let mut presented_vframe = false;
                        let mut bg_up_ms = 0u128;
                        let mut ui_up_ms = 0u128;
                        if let Some(buf) = pending_ui_frame.take() {
                            let t_ui = Instant::now();
                            gpu.upload_ui_overlay_argb(&buf);
                            ui_up_ms = t_ui.elapsed().as_millis();
                            if let Some(ref w) = ui_worker {
                                w.recycle(buf);
                            }
                        }
                        if bg_is_shader && !cached_suspended {
                            let sk = openflux_display::media_bg::shader_kind_from_id(
                                &cfg.appearance.bg_path,
                            );
                            gpu.set_shader_bg(Some((
                                sk,
                                (mono_ms as f32 * 0.001) * (cfg.appearance.shader_speed as f32),
                                (cfg.appearance.shader_hue_deg as f32).to_radians(),
                            )));
                            bg_dirty = false;
                        } else {
                            gpu.set_shader_bg(None);
                            if let Some(f) = rkmpp_due.take() {
                                let t_bg = Instant::now();
                                gpu.upload_bg_nv12(f.width, f.height, &f.nv12);
                                bg_up_ms = t_bg.elapsed().as_millis();
                                rkmpp_vframes += 1;
                                presented_vframe = true;
                                bg_dirty = false;
                            } else if rkmpp_stream.is_none()
                                && !cached_suspended
                                && (fallback_video_due || bg_dirty)
                            {
                                let bg_sig = (
                                    cfg.appearance.bg_path.clone(),
                                    (cfg.appearance.bg_blur * 10.0).round() as i64,
                                    (cfg.appearance.bg_dimming * 10.0).round() as i64,
                                    cfg.appearance.ring_glow_argb,
                                );
                                if fallback_video_due || bg_sig != last_static_bg_sig {
                                    renderer.bg.step(
                                        &mut renderer.assets,
                                        &cfg.appearance,
                                        Some(&media_dir),
                                        mono_ms,
                                    );
                                    gpu.upload_bg_argb(
                                        renderer.width,
                                        renderer.height,
                                        &renderer.bg.pixels,
                                    );
                                    last_static_bg_sig = bg_sig;
                                }
                                bg_dirty = false;
                            } else if cached_suspended {
                                bg_dirty = false;
                            }
                        }

                        if !ui_initial_uploaded {
                            let t_ui = Instant::now();
                            renderer.render_ui_overlay_at(
                                &cfg,
                                &state,
                                Some(&media_dir),
                                now_sec,
                                mono_ms,
                                cached_suspended,
                            );
                            gpu.upload_ui_overlay_argb(&renderer.pixels);
                            ui_up_ms = t_ui.elapsed().as_millis();
                            ui_initial_uploaded = true;
                            ui_dirty = false;
                        }

                        let (dim_u, blur_u) = if rkmpp_stream.is_some() || bg_is_shader {
                            (
                                (1.0 - (cfg.appearance.bg_dimming / 100.0).clamp(0.0, 1.0)) as f32,
                                cfg.appearance.bg_blur as f32,
                            )
                        } else {
                            (1.0_f32, 0.0_f32)
                        };
                        let glass_raw = renderer.collect_gpu_glass_rects(&cfg);
                        let glass_rects: Vec<openflux_display::wayland_egl::GpuRoundRect> =
                            glass_raw
                                .into_iter()
                                .map(|(x, y, w, h, radius, argb)| {
                                    openflux_display::wayland_egl::GpuRoundRect {
                                        x,
                                        y,
                                        w,
                                        h,
                                        radius,
                                        argb,
                                    }
                                })
                                .collect();
                        let fx_raw = renderer.collect_gpu_key_effects(&cfg, &state, mono_ms);
                        let key_fx: Vec<openflux_display::wayland_egl::GpuKeyEffect> = fx_raw
                            .into_iter()
                            .map(
                                |(kcx, kcy, kw, kh, kind, age_ms, rel_age_ms, glow, ring_argb)| {
                                    openflux_display::wayland_egl::GpuKeyEffect {
                                        kcx,
                                        kcy,
                                        kw,
                                        kh,
                                        kind,
                                        age_ms,
                                        rel_age_ms,
                                        glow,
                                        ring_argb,
                                    }
                                },
                            )
                            .collect();
                        let ren_ms = t_ren.elapsed().as_millis();
                        let t_pres = Instant::now();
                        if let Err(e) = gpu.render_and_swap(
                            dim_u,
                            blur_u,
                            cached_suspended,
                            &glass_rects,
                            &key_fx,
                        ) {
                            eprintln!("Wayland EGL present error, exiting cleanly: {e}");
                            break;
                        }
                        let pres_ms = t_pres.elapsed().as_millis();
                        last_render_ms = (ren_ms + pres_ms) as u64;
                        fx_dirty = false;
                        render_dirty = false;
                        if presented_vframe {
                            let now_v_ms = start.elapsed().as_millis() as u64;
                            if let Some(prev_v_ms) = last_vframe_present_ms {
                                let gap = now_v_ms.saturating_sub(prev_v_ms);
                                if rkmpp_vframes > 3
                                    && gap > 45
                                    && now_v_ms.saturating_sub(last_gap_log_ms) >= 2_000
                                {
                                    last_gap_log_ms = now_v_ms;
                                    let d_starve =
                                        rkmpp_starve_count.saturating_sub(rkmpp_starve_at_last_gap);
                                    rkmpp_starve_at_last_gap = rkmpp_starve_count;
                                    let q_len = rkmpp_stream.as_ref().map_or(0, |s| s.queued_len());
                                    log_diag(&format!(
                                        "video gap: t={now_v_ms}ms gap={gap}ms bg={bg_up_ms}ms ui={ui_up_ms}ms ren={ren_ms}ms swap={pres_ms}ms poll={last_poll_ms}ms q={q_len} starve={rkmpp_starve_count} d_starve={d_starve} rebase={rkmpp_rebase_count}"
                                    ));
                                }
                            }
                            last_vframe_present_ms = Some(now_v_ms);
                        }
                        if frame_log_count < 8
                            || (ren_ms + pres_ms > 25
                                && mono_ms.saturating_sub(last_slow_log_ms) >= 15_000)
                        {
                            last_slow_log_ms = mono_ms;
                            log_diag(&format!(
                                "gpu frame #{frame_log_count}: cpu={}ms swap={}ms rkmpp={} vframes={}",
                                ren_ms, pres_ms, rkmpp_stream.is_some(), rkmpp_vframes
                            ));
                        }
                        frame_log_count = frame_log_count.saturating_add(1);
                    } else if let Some(buf) = pending_ui_frame {
                        gpu.upload_ui_overlay_argb(&buf);
                        if let Some(ref w) = ui_worker {
                            w.recycle(buf);
                        }
                    }
                }
                ActivePresenter::Shm(ref mut shm) => {
                    if renderer.bg.has_ready_video_frame_at(mono_ms)
                        || (openflux_display::media_bg::is_shader_path(&cfg.appearance.bg_path)
                            && !cached_suspended)
                        || render_dirty
                    {
                        let t_ren = Instant::now();
                        renderer.render_frame_at(
                            &cfg,
                            &state,
                            Some(&media_dir),
                            now_sec,
                            mono_ms,
                            cached_suspended,
                        );
                        let ren_ms = t_ren.elapsed().as_millis();
                        let t_pres = Instant::now();
                        let presented = match shm.present(&renderer.pixels) {
                            Ok(p) => p,
                            Err(e) => {
                                eprintln!("Wayland present error, exiting cleanly: {e}");
                                break;
                            }
                        };
                        let pres_ms = t_pres.elapsed().as_millis();
                        last_render_ms = (ren_ms + pres_ms) as u64;
                        if frame_log_count < 8
                            || (ren_ms + pres_ms > 45
                                && mono_ms.saturating_sub(last_slow_log_ms) >= 15_000)
                        {
                            last_slow_log_ms = mono_ms;
                            log_diag(&format!(
                                "shm frame #{frame_log_count}: render={}ms present={}ms video={} vframes={}",
                                ren_ms, pres_ms, renderer.bg.is_video, renderer.bg.video_frames_rendered
                            ));
                        }
                        frame_log_count = frame_log_count.saturating_add(1);
                        if let Ok(live_ppm) = std::env::var("OPENFLUX_LIVE_PPM_PATH") {
                            if !live_ppm.is_empty() {
                                let tmp = format!("{live_ppm}.tmp");
                                if write_ppm_file(
                                    Path::new(&tmp),
                                    renderer.width,
                                    renderer.height,
                                    &renderer.pixels,
                                )
                                .is_ok()
                                {
                                    let _ = std::fs::rename(&tmp, &live_ppm);
                                }
                            }
                        }
                        if presented {
                            render_dirty = false;
                            ui_dirty = false;
                            bg_dirty = false;
                            fx_dirty = false;
                        }
                    }
                }
            }

            let mut pfds: [libc::pollfd; 2] = [
                libc::pollfd {
                    fd: presenter.raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: -1,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let mut nfds: libc::nfds_t = 1;
            if let Some(ifd) = ipc.raw_fd() {
                pfds[nfds as usize].fd = ifd;
                nfds += 1;
            }
            let sleep_ms: i32 = if let Some(vw) = video_wait_ms {
                if anim_active {
                    vw.min(4)
                } else {
                    vw
                }
            } else if anim_active {
                4
            } else if rkmpp_stream.is_some() || renderer.bg.is_video {
                8
            } else {
                200
            };
            let t_poll = Instant::now();
            unsafe {
                libc::poll(pfds.as_mut_ptr(), nfds, sleep_ms);
            }
            last_poll_ms = t_poll.elapsed().as_millis() as u64;
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (sock_path, log_dir);
        eprintln!("Wayland presentation loop is Linux-only; use --render-ppm on macOS/Windows.");
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn is_panel_hw_asleep() -> bool {
    let bl_p = std::fs::read_to_string(BACKLIGHT_BL_POWER_PATH)
        .ok()
        .map(|s| s.trim() == "1")
        .unwrap_or(false);
    let bl_z = std::fs::read_to_string(BACKLIGHT_BRIGHTNESS_PATH)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .map(|v| v == 0)
        .unwrap_or(false);
    bl_p || bl_z
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn read_backlight_brightness() -> u32 {
    let max_b = std::fs::read_to_string(BACKLIGHT_MAX_BRIGHTNESS_PATH)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(255);
    std::fs::read_to_string(BACKLIGHT_BRIGHTNESS_PATH)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&v| v > 0)
        .or_else(|| {
            std::fs::read_to_string(SAVED_BRIGHTNESS_PATH)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
                .filter(|&v| v > 0)
        })
        .map(|v| v.min(max_b))
        .unwrap_or(max_b)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn apply_brightness_steps_at(
    bl_bright_path: &Path,
    bl_max_path: &Path,
    saved_path: &Path,
    steps: &[i8],
    cached_suspended: bool,
    saved_brightness: &mut u32,
    log_diag: &mut dyn FnMut(&str),
) {
    if steps.is_empty() || !bl_bright_path.exists() {
        return;
    }
    let max_b = std::fs::read_to_string(bl_max_path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(255);
    let min_b = (max_b / 20).max(1);
    let step_sz = (max_b / 10).max(1);
    let mut cur = if cached_suspended {
        *saved_brightness
    } else {
        std::fs::read_to_string(bl_bright_path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(*saved_brightness)
    };
    for &d in steps {
        if d > 0 {
            cur = cur
                .saturating_add(step_sz.saturating_mul(d as u32))
                .min(max_b);
        } else if d < 0 {
            cur = cur
                .saturating_sub(step_sz.saturating_mul(d.unsigned_abs() as u32))
                .max(min_b);
        }
    }
    *saved_brightness = cur;
    if cached_suspended {
        let _ = std::fs::write(saved_path, format!("{cur}\n"));
    } else if let Err(e) = std::fs::write(bl_bright_path, format!("{cur}\n")) {
        log_diag(&format!("Brightness sysfs write failed: {e:?}"));
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn apply_brightness_steps(
    steps: &[i8],
    cached_suspended: bool,
    saved_brightness: &mut u32,
    log_diag: &mut dyn FnMut(&str),
) {
    apply_brightness_steps_at(
        Path::new(BACKLIGHT_BRIGHTNESS_PATH),
        Path::new(BACKLIGHT_MAX_BRIGHTNESS_PATH),
        Path::new(SAVED_BRIGHTNESS_PATH),
        steps,
        cached_suspended,
        saved_brightness,
        log_diag,
    );
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
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
        let r1 = if bl_bright.exists() {
            std::fs::write(bl_bright, "0\n")
        } else {
            Ok(())
        };
        let r2 = if bl_power.exists() {
            std::fs::write(bl_power, "1\n")
        } else {
            Ok(())
        };
        match (r1, r2) {
            (Ok(()), Ok(())) => {
                log_diag("Panel sleep: backlight powered off (bl_power=1, brightness=0)")
            }
            (e1, e2) => log_diag(&format!(
                "Panel sleep sysfs write failed: brightness={e1:?} bl_power={e2:?}"
            )),
        }
    } else {
        let max_b = std::fs::read_to_string(BACKLIGHT_MAX_BRIGHTNESS_PATH)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(255);
        let restore = (*saved_brightness).clamp(1, max_b);
        let r1 = if bl_power.exists() {
            std::fs::write(bl_power, "0\n")
        } else {
            Ok(())
        };
        let r2 = if bl_bright.exists() {
            std::fs::write(bl_bright, format!("{restore}\n"))
        } else {
            Ok(())
        };
        let _ = std::fs::remove_file(SAVED_BRIGHTNESS_PATH);
        match (r1, r2) {
            (Ok(()), Ok(())) => log_diag(&format!(
                "Panel wake: backlight restored (bl_power=0, brightness={restore})"
            )),
            (e1, e2) => log_diag(&format!(
                "Panel wake sysfs write failed: bl_power={e1:?} brightness={e2:?}"
            )),
        }
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
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
                mem_tot_kb = rest
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                mem_avail_kb = rest
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            }
        }
    }
    let mem_used_mb = mem_tot_kb.saturating_sub(mem_avail_kb) / 1024;
    let mem_total_mb = mem_tot_kb / 1024;

    let mut gpu_pct: Option<f64> = None;
    let mut gpu_candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/sys/devices/platform") {
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            if name.contains("gpu") {
                gpu_candidates.push(ent.path().join("utilisation"));
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir("/sys/class/devfreq") {
        for ent in rd.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            if name.contains("gpu") {
                gpu_candidates.push(ent.path().join("load"));
            }
        }
    }
    for cand in gpu_candidates {
        if let Ok(s) = std::fs::read_to_string(&cand) {
            let first = s.trim().split('@').next().unwrap_or("").trim();
            if let Ok(v) = first.parse::<f64>() {
                gpu_pct = Some(v.clamp(0.0, 100.0));
                break;
            }
        }
    }

    let gpu_json = match gpu_pct {
        Some(v) => format!("{v:.1}"),
        None => "null".to_string(),
    };
    let p = Path::new(log_dir).join("soc_stats.json");
    let p_tmp = Path::new(log_dir).join("soc_stats.json.tmp");
    let body = format!(
        "{{\"cpuPct\":{cpu_pct:.1},\"gpuPct\":{gpu_json},\"memUsedMb\":{mem_used_mb},\"memTotalMb\":{mem_total_mb},\"renderMs\":{render_ms},\"rkmpp\":{rkmpp},\"tsSec\":{ts_sec},\"sampleMs\":{mono_ms}}}\n"
    );
    if std::fs::write(&p_tmp, body).is_ok() {
        let _ = std::fs::rename(&p_tmp, &p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_overlay_worker_coalesces_requests_and_returns_nonblocking_frame() {
        let worker = UiOverlayWorker::spawn(1920, 1080);
        assert!(worker.take_ready().is_none());

        let cfg = DisplayConfig::default();
        let state_a = LiveState::from_config(&cfg);
        let mut state_b = LiveState::from_config(&cfg);
        state_b.has_firmware_layer = true;
        state_b.active_layer = 1;
        state_b.active_layer_bitfield = 2;

        // First request keeps worker busy while second request overwrites the pending slot,
        // or the latest request wins directly; either way, a subsequent render must settle on state_b.
        worker.submit(UiRenderReq {
            cfg: cfg.clone(),
            state: state_a,
            media_dir: String::new(),
            now_sec: 1_700_000_000,
            mono_ms: 10,
            suspended: false,
            invalidate_assets: true,
        });
        worker.submit(UiRenderReq {
            cfg: cfg.clone(),
            state: state_b.clone(),
            media_dir: String::new(),
            now_sec: 1_700_000_001,
            mono_ms: 20,
            suspended: true,
            invalidate_assets: false,
        });

        let mut expected = Renderer::new(1920, 1080);
        expected.render_ui_overlay_at(&cfg, &state_b, None, 1_700_000_001, 20, true);

        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut latest: Option<Vec<u32>> = None;
        while std::time::Instant::now() < deadline {
            if let Some(b) = worker.take_ready() {
                let matches = b == expected.pixels;
                worker.recycle(b);
                if matches {
                    latest = Some(expected.pixels.clone());
                    break;
                }
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        assert!(
            latest.is_some(),
            "UiOverlayWorker never produced the latest submitted state"
        );
    }

    #[test]
    fn per_second_ui_tick_covers_analog_clock_and_virtual_pet() {
        let mut cfg = DisplayConfig::default();
        cfg.mosaic.tiles.iter_mut().for_each(|t| {
            t.tile_type = openflux_display::config::TileType::DigitalClock;
        });
        assert!(!needs_per_second_ui_tick(&cfg));
        cfg.mosaic.tiles[0].tile_type = openflux_display::config::TileType::AnalogClock;
        assert!(needs_per_second_ui_tick(&cfg));
        cfg.mosaic.tiles[0].tile_type = openflux_display::config::TileType::VirtualPet;
        assert!(needs_per_second_ui_tick(&cfg));
    }

    #[test]
    fn full_sync_channel_returns_false_so_pomo_state_retries() {
        let (tx, _rx) = std::sync::mpsc::sync_channel::<u32>(1);
        assert!(tx.try_send(1).is_ok());
        assert!(tx.try_send(2).is_err());
    }

    #[test]
    fn brightness_delta_steps_and_clamps_and_respects_suspend() {
        let tmp = std::env::temp_dir().join(format!("openflux_disp_bright_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let bl_b = tmp.join("brightness");
        let bl_m = tmp.join("max_brightness");
        let saved = tmp.join("saved");
        std::fs::write(&bl_m, "200\n").unwrap();
        std::fs::write(&bl_b, "100\n").unwrap();

        let mut saved_val = 100u32;
        let mut logs = Vec::new();
        let mut log = |m: &str| logs.push(m.to_string());

        apply_brightness_steps_at(&bl_b, &bl_m, &saved, &[1], false, &mut saved_val, &mut log);
        assert_eq!(saved_val, 120);
        assert_eq!(std::fs::read_to_string(&bl_b).unwrap().trim(), "120");

        apply_brightness_steps_at(&bl_b, &bl_m, &saved, &[20], false, &mut saved_val, &mut log);
        assert_eq!(saved_val, 200);
        assert_eq!(std::fs::read_to_string(&bl_b).unwrap().trim(), "200");

        // Up then down at the maximum ends one step below it (180).
        apply_brightness_steps_at(
            &bl_b,
            &bl_m,
            &saved,
            &[1, -1],
            false,
            &mut saved_val,
            &mut log,
        );
        assert_eq!(saved_val, 180);

        apply_brightness_steps_at(
            &bl_b,
            &bl_m,
            &saved,
            &[-50],
            false,
            &mut saved_val,
            &mut log,
        );
        assert_eq!(saved_val, 10); // min_b = 200 / 20 = 10
        assert_eq!(std::fs::read_to_string(&bl_b).unwrap().trim(), "10");

        // Down then up at the minimum ends one step above it (30).
        apply_brightness_steps_at(
            &bl_b,
            &bl_m,
            &saved,
            &[-1, 1],
            false,
            &mut saved_val,
            &mut log,
        );
        assert_eq!(saved_val, 30);

        apply_brightness_steps_at(&bl_b, &bl_m, &saved, &[1], true, &mut saved_val, &mut log);
        assert_eq!(saved_val, 50);
        assert_eq!(std::fs::read_to_string(&saved).unwrap().trim(), "50");
        assert_eq!(std::fs::read_to_string(&bl_b).unwrap().trim(), "30");
        assert!(logs.is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
