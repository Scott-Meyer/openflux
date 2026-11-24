use crate::api::keyboard_edits::record_pulled_document;
use crate::api::state::{foreground_app, ApiState, DeviceSink};
use crate::bridges::{
    build_live_data_json, layout_to_language_code, match_app_rule, OsSensorBridge,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

/// The mosaic tile type of the weather module.
const TILE_TYPE_WEATHER: u64 = 3;

pub enum UsbCommand {
    SetProfile {
        profile_id: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SetLanguage {
        lang_code: u32,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SendLiveJson {
        payload: serde_json::Value,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SaveConfig {
        snapshot: serde_json::Value,
        reply: oneshot::Sender<Result<crate::api::keyboard_edits::KeptFields, String>>,
    },
    PullConfig {
        discard_unsaved: bool,
        reply: oneshot::Sender<Result<serde_json::Value, String>>,
    },
    BackupDevice {
        out_dir: String,
        reply: oneshot::Sender<Result<String, String>>,
    },
    FlashUf2 {
        uf2_path: String,
        reply: oneshot::Sender<Result<String, String>>,
    },
    SideloadIpk {
        ipk_path: String,
        reply: oneshot::Sender<Result<String, String>>,
    },
    ReadPomodoroState {
        reply: oneshot::Sender<Option<serde_json::Value>>,
    },
    ReadSocStats {
        reply: oneshot::Sender<Option<serde_json::Value>>,
    },
}

/// Slowest USB file throughput a transfer is allowed before its caller gives up.
pub const MIN_TRANSFER_BYTES_PER_SEC: u64 = 2 * 1024 * 1024;

/// Time allowed for a command that moves `bytes` to or from the keyboard.
pub fn transfer_budget(base: Duration, bytes: u64) -> Duration {
    base + Duration::from_millis(bytes.saturating_mul(1000) / MIN_TRANSFER_BYTES_PER_SEC)
}

fn local_file_len(path: &str) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Bytes of media and icons a save plan uploads.
fn save_plan_upload_bytes(snapshot: &serde_json::Value) -> u64 {
    ["mediaFiles", "icons"]
        .iter()
        .filter_map(|k| snapshot.get(*k).and_then(|v| v.as_array()))
        .flatten()
        .filter_map(|f| f.get("localPath").and_then(|v| v.as_str()))
        .map(local_file_len)
        .sum()
}

#[derive(Clone)]
pub struct UsbWorkerHandle {
    tx: mpsc::Sender<UsbCommand>,
    accepting: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    join: Arc<std::sync::Mutex<Option<thread::JoinHandle<()>>>>,
}

/// How long the daemon stays busy after a bridge package upload while it waits to confirm the new
/// bridge. Restarting during that window could leave the install unconfirmed.
pub const BRIDGE_INSTALL_LEASE: Duration = Duration::from_secs(180);
/// How long a started bridge install is followed. After that the keyboard has restored the
/// previous bridge by itself.
pub const BRIDGE_INSTALL_FOLLOW: Duration = Duration::from_secs(600);
const BRIDGE_INSTALL_FILE: &str = "bridge_install.json";

#[derive(Default)]
pub struct WorkerLoopState {
    /// A bridge install started through the worker that still waits for its new bridge.
    pub pending_bridge: Option<(crate::bridge_install::PendingInstall, Instant)>,
    /// Held from a bridge package upload until the new bridge is confirmed, the keyboard
    /// disconnects or [`BRIDGE_INSTALL_LEASE`] passes.
    pub bridge_install_lease: Option<(crate::lifecycle::BusyLease, Instant)>,
    pub next_bridge_poll_at: Option<Instant>,
    /// A job had the keyboard to itself during the previous tick.
    pub keyboard_was_reserved: bool,
    pub last_live_sent: Option<serde_json::Value>,
    pub last_live_sent_at: Option<Instant>,
    pub last_idle_sample_at: Option<Instant>,
    pub next_versions_retry_at: Option<Instant>,
    pub pull_backoff_secs: u64,
    pub cached_pack_name: Option<String>,
    pub cached_pack_overlays: Vec<serde_json::Value>,
    /// Where the keyboard's clock stands; reset when the bridge disconnects.
    pub clock_sync: Option<ClockSync>,
    /// Reads the host's clock zone; [`HostClock::current`] when unset.
    pub host_clock: Option<Box<dyn Fn() -> HostClock + Send>>,
    /// Host broker subscription used for real-time Host -> Keyboard `WireFrame` streaming.
    pub v2_bus_sub: Option<openflux_bus::Subscription>,
    pub last_bus_poll_at: Option<Instant>,
    pub last_gamepad_cfg_sig: Option<String>,
    pub last_layer_override_sig: Option<String>,
}

/// The host's time zone and UTC offset, which the keyboard's clock follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostClock {
    /// IANA zone name, such as `Europe/Berlin`.
    pub zone: String,
    /// The current offset from UTC, where the host can tell.
    pub utc_offset_secs: Option<i64>,
}

impl HostClock {
    /// The host's zone, or `UTC` when it can't be determined (logged once).
    pub fn current() -> Self {
        static WARNED: AtomicBool = AtomicBool::new(false);
        let zone = match iana_time_zone::get_timezone() {
            Ok(z) if !z.is_empty() => z,
            other => {
                if !WARNED.swap(true, Ordering::Relaxed) {
                    eprintln!(
                        "openfluxd: the host's time zone couldn't be determined ({other:?}); the keyboard's clock uses UTC"
                    );
                }
                "UTC".into()
            }
        };
        Self {
            zone,
            utc_offset_secs: local_utc_offset_secs(),
        }
    }
}

#[cfg(unix)]
fn local_utc_offset_secs() -> Option<i64> {
    extern "C" {
        fn tzset();
    }
    // SAFETY: `localtime_r` writes only to the given `tm`; `tzset` rereads the zone settings.
    unsafe {
        tzset();
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return None;
        }
        // `tm_gmtoff` is a C `long`, 32 bits on some targets.
        #[allow(clippy::unnecessary_cast)]
        Some(tm.tm_gmtoff as i64)
    }
}

#[cfg(not(unix))]
fn local_utc_offset_secs() -> Option<i64> {
    None
}

/// How often the keyboard's clock is set again while nothing changes, so it doesn't drift.
pub const CLOCK_RESYNC: Duration = Duration::from_secs(3600);
/// How soon a clock update that failed is tried again.
const CLOCK_RETRY: Duration = Duration::from_secs(60);
/// How often the host's zone and offset are read for a change.
const CLOCK_CHECK: Duration = Duration::from_secs(5);

/// The clock last sent to the keyboard, and when it is due again.
#[derive(Debug, Clone)]
pub struct ClockSync {
    pub sent: HostClock,
    pub next_send: Instant,
    pub next_check: Instant,
    pub failing: bool,
}

/// Sets the keyboard's clock when the bridge has just attached, every [`CLOCK_RESYNC`], and when
/// the host's zone or UTC offset changes.
fn sync_keyboard_clock(device: &dyn DeviceSink, wstate: &mut WorkerLoopState) {
    let now = Instant::now();
    if wstate
        .clock_sync
        .as_ref()
        .is_some_and(|c| now < c.next_check && now < c.next_send)
    {
        return;
    }
    let clock = match &wstate.host_clock {
        Some(read) => read(),
        None => HostClock::current(),
    };
    if let Some(c) = wstate.clock_sync.as_mut() {
        if c.sent == clock && now < c.next_send {
            c.next_check = now + CLOCK_CHECK;
            return;
        }
    }
    let was_failing = wstate.clock_sync.as_ref().is_some_and(|c| c.failing);
    let utc = crate::usb::protocol::current_utc_system_time();
    let res = device.set_system_time(&utc, &clock.zone);
    if let Err(e) = &res {
        if !was_failing {
            eprintln!(
                "openfluxd: setting the keyboard's clock ({}) failed: {e}",
                clock.zone
            );
        }
    }
    wstate.clock_sync = Some(ClockSync {
        sent: clock,
        next_send: now
            + if res.is_ok() {
                CLOCK_RESYNC
            } else {
                CLOCK_RETRY
            },
        next_check: now + CLOCK_CHECK,
        failing: res.is_err(),
    });
}

/// Records a started bridge install in the data directory, or removes the record.
fn persist_pending_bridge(
    state: &ApiState,
    pending: Option<&crate::bridge_install::PendingInstall>,
) {
    let Some(dir) = state.config_dir() else {
        return;
    };
    let path = dir.join(BRIDGE_INSTALL_FILE);
    match pending {
        Some(p) => {
            if let Ok(raw) = serde_json::to_vec_pretty(p) {
                let _ = crate::updates::source::write_durably(&path, &raw);
            }
        }
        None => {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn load_pending_bridge(state: &ApiState) -> Option<crate::bridge_install::PendingInstall> {
    let raw = std::fs::read(state.config_dir()?.join(BRIDGE_INSTALL_FILE)).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Spawns the event-driven macOS focus watcher (`openflux-sensors --watch-focus`), which listens
/// natively to `NSWorkspace.didActivateApplicationNotification` (<1ms latency, 0% CPU idle) and
/// pushes `host/app/active` straight onto the unified v2 bus with zero polling delay!
#[cfg(target_os = "macos")]
fn spawn_macos_focus_watcher(state: Arc<ApiState>, running: Arc<AtomicBool>) {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let helper = exe_dir
        .map(|d| d.join("openflux-sensors"))
        .filter(|p| p.is_file());
    let Some(helper_path) = helper else {
        return;
    };
    let _ = thread::Builder::new()
        .name("openflux-focus-watch".into())
        .spawn(move || {
            use std::io::{BufRead, BufReader};
            use std::process::{Command, Stdio};
            while running.load(Ordering::Relaxed) {
                let Ok(mut child) = Command::new(&helper_path)
                    .arg("--watch-focus")
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                else {
                    thread::sleep(Duration::from_secs(2));
                    continue;
                };
                if let Some(stdout) = child.stdout.take() {
                    let reader = BufReader::new(stdout);
                    for line in reader.lines().map_while(Result::ok) {
                        if !running.load(Ordering::Relaxed) {
                            break;
                        }
                        let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) else {
                            continue;
                        };
                        let app_id = val
                            .get("bundleId")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let process_name = val
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if app_id.is_empty() && process_name.is_empty() {
                            continue;
                        }
                        let low = format!("{app_id} {process_name}").to_ascii_lowercase();
                        if low.contains("com.apple.dock")
                            || low.contains("windowserver")
                            || low.contains("spotlight")
                            || low.contains("notificationcenter")
                            || low.contains("controlcenter")
                        {
                            continue;
                        }
                        let (profile_index, profile_id, profile_name) = {
                            let g = state.inner.lock().unwrap();
                            let idx = g.desired_profile_index;
                            let pid = g
                                .profiles
                                .get(idx)
                                .map(|p| p.id.clone())
                                .unwrap_or_else(|| "default".into());
                            let pname = g
                                .profiles
                                .get(idx)
                                .map(|p| p.name.clone())
                                .unwrap_or_else(|| "Default".into());
                            (idx, pid, pname)
                        };
                        let payload = serde_json::json!({
                            "app_id": app_id,
                            "process_name": process_name,
                            "window_title": "",
                            "profile_index": profile_index,
                            "profile_id": profile_id,
                            "profile_name": profile_name,
                        });
                        let changed = state
                            .v2
                            .broker
                            .get_retained(openflux_bus::topics::HOST_APP_ACTIVE)
                            .map(|m| m.payload != payload)
                            .unwrap_or(true);
                        if changed {
                            let _ = state.v2.broker.publish(
                                openflux_bus::topics::HOST_APP_ACTIVE,
                                payload,
                                openflux_bus::PublishOptions::retained().with_origin("focus-watch"),
                            );
                        }
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
                if running.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(500));
                }
            }
        });
}

/// Pure v2 host telemetry broadcaster (`openflux-host-telemetry`).
/// Completely decoupled from USB — simply broadcasts `host/system/metrics`, `host/media/now_playing`,
/// and `host/weather/current` onto the broker every ~1.5s regardless of USB state, so a USB
/// reconnect/handshake can NEVER block computer stats or weather!
#[cfg(target_os = "macos")]
fn is_host_display_asleep() -> bool {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayIsAsleep(display: u32) -> u32;
    }
    unsafe { CGDisplayIsAsleep(CGMainDisplayID()) != 0 }
}

#[cfg(not(target_os = "macos"))]
fn is_host_display_asleep() -> bool {
    false
}

fn spawn_host_telemetry_publisher(state: Arc<ApiState>, running: Arc<AtomicBool>) {
    let _ = thread::Builder::new()
        .name("openflux-host-telemetry".into())
        .spawn(move || {
            let mut bridge = crate::bridges::create_platform_bridge();
            let mut last_time_pub: Option<Instant> = None;
            let mut last_display_asleep: Option<bool> = None;
            while running.load(Ordering::Relaxed) {
                let display_asleep = is_host_display_asleep();
                if last_display_asleep != Some(display_asleep) {
                    last_display_asleep = Some(display_asleep);
                    let _ = state.v2.broker.publish(
                        "host/display/state",
                        serde_json::json!({
                            "asleep": display_asleep,
                            "platform": std::env::consts::OS,
                        }),
                        openflux_bus::PublishOptions::retained().with_origin("host-telemetry"),
                    );
                    let _ = state.v2.broker.publish_typed(
                        openflux_bus::topics::HOST_CONNECTION,
                        &openflux_bus::HostConnectionState {
                            connected: true,
                            host_asleep: display_asleep,
                            last_seen_ms: openflux_bus::now_unix_ms(),
                            host_platform: Some(std::env::consts::OS.to_string()),
                        },
                        openflux_bus::PublishOptions::retained().with_origin("host-telemetry"),
                    );
                }
                if last_time_pub.map(|t| t.elapsed() >= Duration::from_secs(30)).unwrap_or(true) {
                    last_time_pub = Some(Instant::now());
                    let hc = HostClock::current();
                    let unix_sec = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    let _ = state.v2.broker.publish(
                        "host/time",
                        serde_json::json!({
                            "unix_sec": unix_sec,
                            "tz_offset_sec": hc.utc_offset_secs.unwrap_or(0),
                            "zone": hc.zone,
                        }),
                        openflux_bus::PublishOptions::retained().with_origin("host-telemetry"),
                    );
                }
                let hw = bridge.sample_hardware();
                let media = bridge.media_snapshot();

                let _ = state.v2.broker.publish_typed(
                    openflux_bus::topics::HOST_SYSTEM_METRICS,
                    &openflux_bus::SystemMetrics {
                        cpu_percent: hw.cpu_pct as f32,
                        memory_used_gb: hw.mem_used_gb as f32,
                        memory_total_gb: hw.mem_total_gb as f32,
                        gpu_percent: hw.gpu_pct as f32,
                        gpu_unified: hw.gpu_unified_mem,
                        vram_used_gb: hw.vram_used_gb.map(|v| v as f32),
                        vram_total_gb: hw.vram_total_gb.map(|v| v as f32),
                        net_down: hw.net_down.clone(),
                        net_up: hw.net_up.clone(),
                        net_down_kbps: 0.0,
                        net_up_kbps: 0.0,
                    },
                    openflux_bus::PublishOptions::retained(),
                );
                let _ = state.v2.broker.publish_typed(
                    openflux_bus::topics::HOST_MEDIA_NOW_PLAYING,
                    &openflux_bus::MediaNowPlaying {
                        title: media.title.clone(),
                        artist: media.artists.clone(),
                        album: media.album.clone(),
                        playing: media.playing,
                        position_sec: media.played_s as f32,
                        length_sec: media.length_s as f32,
                        art_base64: String::new(),
                        track_id: String::new(),
                    },
                    openflux_bus::PublishOptions::retained(),
                );

                // Poll weather whenever ANY enabled module subscribes to `host/weather/#`
                let saved = state.v2.registry.load_state();
                let disc = state.v2.registry.discover();
                let mut wants_weather = false;
                let mut city = "Denver".to_string();
                let mut celsius = false;
                for (id, dm) in &disc {
                    if let Some(inst) = saved.modules.get(id) {
                        if !inst.enabled {
                            continue;
                        }
                        if dm.manifest.channels.subscribes.iter().any(|p| {
                            p == openflux_bus::topics::HOST_WEATHER_CURRENT
                                || p == "host/weather/#"
                                || p == "host/weather/+"
                        }) {
                            wants_weather = true;
                            if let Some(c) = inst
                                .config
                                .get("city")
                                .and_then(|x| x.as_str())
                                .filter(|s| !s.trim().is_empty())
                            {
                                city = c.trim().to_string();
                            }
                            if let Some(cel) = inst.config.get("celsius").and_then(|x| x.as_bool())
                            {
                                celsius = cel;
                            }
                            break;
                        }
                    }
                }
                if wants_weather {
                    let w = state.weather_bridge.poll_nonblocking(&city, celsius);
                    let hourly = w
                        .hourly
                        .iter()
                        .take(3)
                        .map(|h| openflux_bus::WeatherHourly {
                            hour: h.hour_label.clone(),
                            temp: h.temp as f32,
                            condition: h.condition.clone(),
                            is_day: h.is_day,
                        })
                        .collect();
                    let _ = state.v2.broker.publish_typed(
                        openflux_bus::topics::HOST_WEATHER_CURRENT,
                        &openflux_bus::WeatherState {
                            city: w.city.clone(),
                            temp: w.temp as f32,
                            high: w.max_temp as f32,
                            low: w.min_temp as f32,
                            condition: w.condition.clone(),
                            is_day: w.is_day,
                            celsius: w.celsius,
                            ready: w.ready,
                            aqi: w.aqi.map(|a| a as u16),
                            sunrise: w.sunrise.clone(),
                            sunset: w.sunset.clone(),
                            utc_offset_sec: w.utc_offset_seconds,
                            hourly,
                        },
                        openflux_bus::PublishOptions::retained(),
                    );
                }

                thread::sleep(Duration::from_millis(1500));
            }
        });
}

impl UsbWorkerHandle {
    pub fn spawn(
        state: Arc<ApiState>,
        device: Arc<dyn DeviceSink>,
        mut bridge: Box<dyn OsSensorBridge>,
        platform_code: u32,
        running: Arc<AtomicBool>,
    ) -> Self {
        #[cfg(target_os = "macos")]
        spawn_macos_focus_watcher(state.clone(), running.clone());
        spawn_host_telemetry_publisher(state.clone(), running.clone());
        let (tx, mut rx) = mpsc::channel::<UsbCommand>(32);
        let accepting = Arc::new(AtomicBool::new(true));
        let state_bg = state.clone();
        let run_bg = running.clone();
        let jh = thread::spawn(move || {
            let mut next_poll = Instant::now();
            let mut wstate = WorkerLoopState {
                pending_bridge: load_pending_bridge(&state_bg).map(|p| (p, Instant::now())),
                ..Default::default()
            };
            while run_bg.load(Ordering::Relaxed) {
                while let Ok(cmd) = rx.try_recv() {
                    let bridge_target = match &cmd {
                        UsbCommand::SideloadIpk { ipk_path, .. } if is_bridge_package(ipk_path) => {
                            Some(crate::bridge_install::package_target(ipk_path))
                        }
                        _ => None,
                    };
                    let install_lease = if bridge_target.is_some() {
                        state_bg
                            .lifecycle
                            .hold("installing the keyboard bridge")
                            .ok()
                    } else {
                        None
                    };
                    // The attempt on the keyboard before the upload, so the install's own attempt
                    // can be told apart from it.
                    let pending = bridge_target
                        .clone()
                        .flatten()
                        .map(|t| crate::bridge_install::PendingInstall::begin(&*device, t));
                    let uploaded = handle_usb_cmd(&state_bg, &*device, cmd);
                    if uploaded {
                        if let Some(p) = pending {
                            persist_pending_bridge(&state_bg, Some(&p));
                            wstate.pending_bridge = Some((p, Instant::now()));
                            wstate.next_bridge_poll_at = None;
                            wstate.pull_backoff_secs = 0;
                            wstate.bridge_install_lease =
                                install_lease.map(|l| (l, Instant::now()));
                        }
                    }
                }
                if Instant::now() >= next_poll {
                    reconcile_tick_with_state(
                        &state_bg,
                        &mut *bridge,
                        &*device,
                        platform_code,
                        &mut wstate,
                    );
                    next_poll = Instant::now() + Duration::from_millis(1000);
                }
                thread::sleep(Duration::from_millis(20));
            }
            // Commands already queued still run before the worker exits.
            while let Ok(cmd) = rx.try_recv() {
                handle_usb_cmd(&state_bg, &*device, cmd);
            }
        });
        Self {
            tx,
            accepting,
            running,
            join: Arc::new(std::sync::Mutex::new(Some(jh))),
        }
    }

    /// Stops accepting new USB write commands, signals worker shutdown, and joins the worker
    /// thread so any in-flight USB save/flash completes cleanly before process exit.
    pub fn shutdown_and_join(&self) {
        self.accepting.store(false, Ordering::SeqCst);
        self.running.store(false, Ordering::SeqCst);
        if let Ok(mut g) = self.join.lock() {
            if let Some(jh) = g.take() {
                let _ = jh.join();
            }
        }
    }

    pub async fn set_profile(&self, profile_id: String) -> Result<(), String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .try_send(UsbCommand::SetProfile {
                profile_id,
                reply: reply_tx,
            })
            .map_err(|_| "The keyboard is busy; try again".to_string())?;
        tokio::time::timeout(Duration::from_secs(5), reply_rx)
            .await
            .map_err(|_| "The keyboard didn't answer in time".to_string())?
            .map_err(|_| {
                "The keyboard connection stopped before answering; try again".to_string()
            })?
    }

    pub async fn set_language(&self, lang_code: u32) -> Result<(), String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .try_send(UsbCommand::SetLanguage {
                lang_code,
                reply: reply_tx,
            })
            .map_err(|_| "The keyboard is busy; try again".to_string())?;
        tokio::time::timeout(Duration::from_secs(5), reply_rx)
            .await
            .map_err(|_| "The keyboard didn't answer in time".to_string())?
            .map_err(|_| {
                "The keyboard connection stopped before answering; try again".to_string()
            })?
    }

    pub async fn send_live_json(&self, payload: serde_json::Value) -> Result<(), String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .try_send(UsbCommand::SendLiveJson {
                payload,
                reply: reply_tx,
            })
            .map_err(|_| "The keyboard is busy; try again".to_string())?;
        tokio::time::timeout(Duration::from_secs(5), reply_rx)
            .await
            .map_err(|_| "The keyboard didn't answer in time".to_string())?
            .map_err(|_| {
                "The keyboard connection stopped before answering; try again".to_string()
            })?
    }

    /// Saves `snapshot` to the keyboard, keeping edits made on the keyboard itself (see
    /// [`crate::api::keyboard_edits`]). Returns the fields kept from the keyboard.
    pub async fn save_config(
        &self,
        snapshot: serde_json::Value,
    ) -> Result<crate::api::keyboard_edits::KeptFields, String> {
        if !self.accepting.load(Ordering::SeqCst) {
            return Err("OpenFlux is shutting down".to_string());
        }
        // Uploads are written twice in the worst case (the save, then a rollback).
        let budget = transfer_budget(
            Duration::from_secs(20),
            save_plan_upload_bytes(&snapshot).saturating_mul(2),
        );
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .try_send(UsbCommand::SaveConfig {
                snapshot,
                reply: reply_tx,
            })
            .map_err(|_| "The keyboard is busy; try again".to_string())?;
        tokio::time::timeout(budget, reply_rx)
            .await
            .map_err(|_| "Saving to the keyboard didn't finish in time".to_string())?
            .map_err(|_| {
                "The keyboard connection stopped before answering; try again".to_string()
            })?
    }

    pub async fn pull_config(&self, discard_unsaved: bool) -> Result<serde_json::Value, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .try_send(UsbCommand::PullConfig {
                discard_unsaved,
                reply: reply_tx,
            })
            .map_err(|_| "The keyboard is busy; try again".to_string())?;
        tokio::time::timeout(Duration::from_secs(10), reply_rx)
            .await
            .map_err(|_| "Reading the keyboard didn't finish in time".to_string())?
            .map_err(|_| {
                "The keyboard connection stopped before answering; try again".to_string()
            })?
    }

    pub async fn backup_device(&self, out_dir: String) -> Result<String, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .try_send(UsbCommand::BackupDevice {
                out_dir,
                reply: reply_tx,
            })
            .map_err(|_| "The keyboard is busy; try again".to_string())?;
        tokio::time::timeout(Duration::from_secs(30), reply_rx)
            .await
            .map_err(|_| "The keyboard backup didn't finish in time".to_string())?
            .map_err(|_| {
                "The keyboard connection stopped before answering; try again".to_string()
            })?
    }

    pub async fn flash_uf2(&self, uf2_path: String) -> Result<String, String> {
        if !self.accepting.load(Ordering::SeqCst) {
            return Err("OpenFlux is shutting down".to_string());
        }
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .try_send(UsbCommand::FlashUf2 {
                uf2_path,
                reply: reply_tx,
            })
            .map_err(|_| "The keyboard is busy; try again".to_string())?;
        tokio::time::timeout(Duration::from_secs(60), reply_rx)
            .await
            .map_err(|_| "Flashing the firmware didn't finish in time".to_string())?
            .map_err(|_| {
                "The keyboard connection stopped before answering; try again".to_string()
            })?
    }

    pub async fn sideload_ipk(&self, ipk_path: String) -> Result<String, String> {
        let budget = transfer_budget(Duration::from_secs(30), local_file_len(&ipk_path));
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .try_send(UsbCommand::SideloadIpk {
                ipk_path,
                reply: reply_tx,
            })
            .map_err(|_| "The keyboard is busy; try again".to_string())?;
        tokio::time::timeout(budget, reply_rx)
            .await
            .map_err(|_| "Installing the package didn't finish in time".to_string())?
            .map_err(|_| {
                "The keyboard connection stopped before answering; try again".to_string()
            })?
    }

    pub async fn read_device_pomodoro_state(&self) -> Option<serde_json::Value> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .try_send(UsbCommand::ReadPomodoroState { reply: reply_tx })
            .ok()?;
        tokio::time::timeout(Duration::from_secs(2), reply_rx)
            .await
            .ok()?
            .ok()?
    }

    pub async fn read_device_soc_stats(&self) -> Option<serde_json::Value> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .try_send(UsbCommand::ReadSocStats { reply: reply_tx })
            .ok()?;
        tokio::time::timeout(Duration::from_secs(2), reply_rx)
            .await
            .ok()?
            .ok()?
    }
}

/// A package file is a bridge package when its name says so (`openflux-bridge_<version>_<arch>.ipk`).
pub fn is_bridge_package(path: &str) -> bool {
    std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.to_ascii_lowercase().starts_with("openflux-bridge"))
        .unwrap_or(false)
}

/// Runs one command. Keyboard operations that must not be interrupted hold a busy lease for their
/// duration and fail if the daemon is about to exit. Returns whether the command succeeded.
pub fn handle_usb_cmd(state: &Arc<ApiState>, device: &dyn DeviceSink, cmd: UsbCommand) -> bool {
    // A job that has the keyboard to itself isn't interleaved with queued commands.
    if let Err(r) = state.check_keyboard_unreserved() {
        refuse_usb_cmd(cmd, r.message);
        return false;
    }
    let busy = |activity: &'static str| state.lifecycle.hold(activity).map_err(|e| e.to_string());
    match cmd {
        UsbCommand::SetProfile { profile_id, reply } => {
            let resolved = {
                let g = state.inner.lock().unwrap();
                let local_idx = g.profiles.iter().position(|p| p.id == profile_id);
                local_idx
                    .and_then(|li| ApiState::hardware_profile_slot(&g, li).map(|slot| (li, slot)))
            };
            let Some((local_idx, hw_slot)) = resolved else {
                let _ = reply.send(Err("Save this profile to the keyboard first".into()));
                return false;
            };
            let res = device.set_active_profile(hw_slot as u32);
            let mut g = state.inner.lock().unwrap();
            if res.is_ok() {
                // The profile list may have changed while the command was queued.
                let still =
                    g.profiles.get(local_idx).map(|p| p.id.as_str()) == Some(profile_id.as_str());
                g.record_applied_profile(still.then_some(local_idx), hw_slot as u32);
            } else {
                g.record_firmware_unanswered();
            }
            drop(g);
            let ok = res.is_ok();
            let _ = reply.send(res);
            ok
        }
        UsbCommand::SetLanguage { lang_code, reply } => {
            let res = device.set_language(lang_code);
            let mut g = state.inner.lock().unwrap();
            if res.is_ok() {
                g.applied_language_code = Some(lang_code);
            } else {
                g.record_firmware_unanswered();
            }
            drop(g);
            let ok = res.is_ok();
            let _ = reply.send(res);
            ok
        }
        UsbCommand::SendLiveJson { payload, reply } => {
            let res = device.send_live_json(&payload);
            let ok = res.is_ok();
            let _ = reply.send(res);
            ok
        }
        UsbCommand::SaveConfig { snapshot, reply } => {
            if let Err(r) = state.check_keyboard_write(&crate::api::routes::save_areas(&snapshot)) {
                let _ = reply.send(Err(r.message));
                return false;
            }
            let _lease = match busy("saving to the keyboard") {
                Ok(l) => l,
                Err(e) => {
                    let _ = reply.send(Err(e));
                    return false;
                }
            };
            let res =
                crate::api::keyboard_edits::save_keeping_keyboard_edits(state, device, &snapshot);
            match &res {
                Ok(_) => {
                    {
                        let mut g = state.inner.lock().unwrap();
                        ApiState::record_committed_profiles_from_snapshot(&mut g, &snapshot);
                        ApiState::record_saved_keymap_from_snapshot(&mut g, &snapshot);
                    }
                    let _ = state.persist_profiles();
                }
                Err(e) => {
                    if snapshot.get("profiles").is_some()
                        && (e.contains("PARTIAL_SAVE_UNKNOWN")
                            || e.contains("PARTIAL_SAVE_MISMATCH"))
                    {
                        {
                            let mut g = state.inner.lock().unwrap();
                            ApiState::invalidate_committed_profiles_on_uncertain_save(
                                &mut g, &snapshot, e,
                            );
                        }
                        let _ = state.persist_profiles();
                    }
                }
            }
            let ok = res.is_ok();
            let _ = reply.send(res);
            ok
        }
        UsbCommand::PullConfig {
            discard_unsaved,
            reply,
        } => {
            let res = apply_device_pull(state, device, discard_unsaved);
            let ok = res.is_ok();
            let _ = reply.send(res);
            ok
        }
        UsbCommand::BackupDevice { out_dir, reply } => {
            let res =
                busy("backing up the keyboard").and_then(|_lease| device.backup_device(&out_dir));
            let ok = res.is_ok();
            let _ = reply.send(res);
            ok
        }
        UsbCommand::FlashUf2 { uf2_path, reply } => {
            let res = state
                .check_keyboard_write(&[crate::updates::Area::FirmwareUsb])
                .map_err(|r| r.message)
                .and_then(|_| busy("flashing the keyboard firmware"))
                .and_then(|_lease| device.flash_uf2(&uf2_path));
            let ok = res.is_ok();
            let _ = reply.send(res);
            ok
        }
        UsbCommand::SideloadIpk { ipk_path, reply } => {
            let res = state
                .check_keyboard_write(&[crate::updates::Area::BridgeUsb])
                .map_err(|r| r.message)
                .and_then(|_| busy("installing a keyboard package"))
                .and_then(|_lease| device.sideload_ipk(&ipk_path));
            let ok = res.is_ok();
            let _ = reply.send(res);
            ok
        }
        UsbCommand::ReadPomodoroState { reply } => {
            let _ = reply.send(device.read_device_pomodoro_state());
            true
        }
        UsbCommand::ReadSocStats { reply } => {
            let _ = reply.send(device.read_device_soc_stats());
            true
        }
    }
}

/// Refuses a command: writes get `message` as their error, reads get `None`.
fn refuse_usb_cmd(cmd: UsbCommand, message: String) {
    match cmd {
        UsbCommand::SetProfile { reply, .. }
        | UsbCommand::SetLanguage { reply, .. }
        | UsbCommand::SendLiveJson { reply, .. } => {
            let _ = reply.send(Err(message));
        }
        UsbCommand::SaveConfig { reply, .. } => {
            let _ = reply.send(Err(message));
        }
        UsbCommand::BackupDevice { reply, .. }
        | UsbCommand::FlashUf2 { reply, .. }
        | UsbCommand::SideloadIpk { reply, .. } => {
            let _ = reply.send(Err(message));
        }
        UsbCommand::PullConfig { reply, .. } => {
            let _ = reply.send(Err(message));
        }
        UsbCommand::ReadPomodoroState { reply } | UsbCommand::ReadSocStats { reply } => {
            let _ = reply.send(None);
        }
    }
}

pub fn reconcile_pomodoro_from_device(state: &ApiState, dev_json: Option<&serde_json::Value>) {
    let Some(v) = dev_json else { return };
    let dev_rev = v.get("devRev").and_then(|x| x.as_u64()).unwrap_or(0);
    let dev_boot = v
        .get("devBootId")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let host_cmd_seq = v
        .get("hostCmdSeq")
        .or_else(|| v.get("hostRev"))
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let Some(dev_running) = v.get("running").and_then(|x| x.as_bool()) else {
        return;
    };
    let Some(dev_rem) = v
        .get("remainingS")
        .and_then(|x| x.as_u64())
        .map(|n| n as u32)
    else {
        return;
    };
    let mut g = state.inner.lock().unwrap();
    let boot_changed = dev_boot.is_some() && dev_boot != g.pomo_dev_boot_id;
    if boot_changed {
        g.pomo_dev_boot_id = dev_boot;
        g.pomo_last_dev_rev = 0;
        g.pomo_ack_baseline_seq = g.pomo_expected_cmd_seq;
    }
    let host_caught_up = g.pomo_expected_cmd_seq == 0
        || g.pomo_expected_cmd_seq == g.pomo_ack_baseline_seq
        || host_cmd_seq >= g.pomo_expected_cmd_seq;
    if (boot_changed || dev_rev > g.pomo_last_dev_rev) && host_caught_up {
        g.pomo_last_dev_rev = dev_rev;
        g.pomo_running = dev_running;
        g.pomo_anchor_rem_s = dev_rem;
        g.pomo_anchor_at = if dev_running {
            Some(Instant::now())
        } else {
            None
        };
    }
}

/// The keymap in a pulled configuration: `keymap.json`, the firmware keymap blob, and whether
/// they agree.
struct PulledKeymap {
    doc: Option<serde_json::Value>,
    /// The firmware keymap, when the pull carried one that decodes.
    blob: Option<Vec<u8>>,
    outcome: crate::api::state::KeymapPull,
}

impl PulledKeymap {
    fn read(pulled: &serde_json::Value) -> Option<Self> {
        use crate::api::state::KeymapPull;
        let doc = pulled.get("keymap").filter(|v| v.is_object()).cloned();
        let hex = pulled.get("firmwareKeymapBlobHex");
        if doc.is_none() && hex.is_none() {
            return None;
        }
        let blob = hex.map(|v| {
            v.as_str()
                .ok_or_else(|| "it isn't a hex string".to_string())
                .and_then(ApiState::decode_hex_bytes)
                .and_then(|b| ApiState::validate_firmware_blob(&b).map(|_| b))
        });
        let outcome = match (&doc, &blob) {
            (_, None) => {
                KeymapPull::Error("The keyboard's firmware keymap couldn't be read".into())
            }
            (_, Some(Err(e))) => {
                KeymapPull::Error(format!("The keyboard's firmware keymap is unreadable: {e}"))
            }
            (None, Some(Ok(_))) => {
                KeymapPull::Mismatch("The keyboard has a firmware keymap but no keymap.json".into())
            }
            (Some(k), Some(Ok(b))) => match crate::codecs::keymap::verify_keymap_matches_blob(k, b)
            {
                Ok(()) => KeymapPull::Ok,
                Err(e) => KeymapPull::Mismatch(mismatch_reason(k, &e.to_string())),
            },
        };
        Some(Self {
            doc,
            blob: blob.and_then(Result::ok),
            outcome,
        })
    }
}

/// Why `keymap.json` and the firmware keymap disagree, in plain words.
fn mismatch_reason(doc: &serde_json::Value, codec_error: &str) -> String {
    let factory = crate::codecs::keycode_names::factory_names_in(doc);
    if factory.is_empty() {
        format!("keymap.json doesn't match the keyboard's firmware keymap: {codec_error}")
    } else {
        format!(
            "keymap.json uses key names of the keyboard's factory software that OpenFlux doesn't use ({})",
            factory.join(", ")
        )
    }
}

/// Applies the keymap of a pull. Only a `keymap.json` that agrees with the firmware keymap is
/// trusted; otherwise the keymap stays locked and the reason is recorded for `keymapSync`.
fn apply_pulled_keymap(g: &mut crate::api::state::InnerState, pk: PulledKeymap) {
    use crate::api::state::KeymapPull;
    g.device_keymap_blob = pk.blob.clone();
    if let Some(k) = pk.doc {
        g.keymap = k;
        g.keymap_dirty = false;
    }
    match (&pk.outcome, pk.blob) {
        (KeymapPull::Ok, Some(bytes)) => {
            let has_m = g
                .keymap
                .get("macros")
                .map(|v| {
                    v.is_array()
                        || (v.is_object() && v.as_object().map(|o| !o.is_empty()).unwrap_or(false))
                })
                .unwrap_or(false);
            if !has_m {
                if let Ok(decoded_m) = crate::codecs::keymap::decode_macros_from_blob(&bytes) {
                    let non_empty = decoded_m.as_array().map(|a| !a.is_empty()).unwrap_or(false)
                        || decoded_m
                            .as_object()
                            .map(|o| {
                                o.values()
                                    .any(|v| v.as_array().map(|a| !a.is_empty()).unwrap_or(false))
                            })
                            .unwrap_or(false);
                    if non_empty {
                        if let Some(obj) = g.keymap.as_object_mut() {
                            obj.insert("macros".into(), decoded_m);
                        }
                    }
                }
            }
            if let Ok(modes) = crate::codecs::keymap::decode_actuation_modes_from_blob(&bytes) {
                g.actuation_modes = crate::codecs::keymap::name_actuation_modes(modes, &g.keymap);
            }
            g.firmware_keymap_blob = Some(bytes);
            g.keymap_hydrated = true;
        }
        _ => {
            // The keymap JSON and the firmware blob don't match, so neither is trusted.
            g.firmware_keymap_blob = None;
            g.keymap_hydrated = false;
        }
    }
    g.record_keymap_pull(pk.outcome);
}

/// Records a pull that failed outright, unless the keymap is already trusted.
fn record_failed_pull(state: &ApiState, error: &str) {
    let mut g = state.inner.lock().unwrap();
    if !g.keymap_trusted() {
        g.record_keymap_pull(crate::api::state::KeymapPull::Error(format!(
            "Reading the keyboard's configuration failed: {error}"
        )));
    }
}

pub fn apply_device_pull(
    state: &Arc<ApiState>,
    device: &dyn DeviceSink,
    discard_unsaved: bool,
) -> Result<serde_json::Value, String> {
    let (app_rev, set_rev, mos_rev, km_rev, prof_rev, any_dirty) = {
        let g = state.inner.lock().unwrap();
        (
            g.appearance_rev,
            g.settings_rev,
            g.mosaic_rev,
            g.keymap_rev,
            g.profiles_rev,
            g.appearance_dirty
                || g.settings_dirty
                || g.mosaic_dirty
                || g.keymap_dirty
                || g.profiles_dirty,
        )
    };
    if !discard_unsaved && any_dirty {
        return Err(
            "Conflict: there are unsaved changes; save or discard them before reading the keyboard again"
                .into(),
        );
    }
    let pulled = device.pull_config_dir().inspect_err(|e| {
        record_failed_pull(state, e);
    })?;
    let valid_profs = pulled
        .get("profiles")
        .and_then(|v| {
            serde_json::from_value::<Vec<crate::api::state::ShortcutProfileState>>(v.clone()).ok()
        })
        .filter(|p| ApiState::validate_profile_list(p).is_ok());
    let verified_ids: Option<Vec<String>> = valid_profs.as_ref().and_then(|profs| {
        pulled
            .get("firmwareProfileSettingsHex")
            .and_then(|v| v.as_str())
            .and_then(|hex| ApiState::decode_hex_bytes(hex).ok())
            .filter(|bytes| ApiState::verify_profiles_match_settings(profs, bytes).is_ok())
            .map(|_| profs.iter().map(|p| p.id.clone()).collect())
    });
    let pulled_keymap = PulledKeymap::read(&pulled);
    let mut pulled_profiles = false;
    {
        let mut g = state.inner.lock().unwrap();
        if g.appearance_rev != app_rev
            || g.settings_rev != set_rev
            || g.mosaic_rev != mos_rev
            || g.keymap_rev != km_rev
            || g.profiles_rev != prof_rev
        {
            return Err(
                "Conflict: the settings changed while the keyboard was being read; try again"
                    .into(),
            );
        }
        if let Some(a) = pulled.get("appearance").filter(|v| v.is_object()) {
            g.appearance = a.clone();
            g.appearance_hydrated = true;
            g.appearance_dirty = false;
            record_pulled_document(&mut g, "appearance", a);
        }
        if let Some(s) = pulled.get("settings").filter(|v| v.is_object()) {
            g.settings = s.clone();
            g.settings_hydrated = true;
            g.settings_dirty = false;
            record_pulled_document(&mut g, "settings", s);
        }
        if let Some(m) = pulled.get("mosaic").filter(|v| v.is_object()) {
            g.mosaic = m.clone();
            g.mosaic_hydrated = true;
            g.mosaic_dirty = false;
            record_pulled_document(&mut g, "mosaic", m);
        }
        if g.committed_profile_ids != verified_ids {
            g.committed_profile_ids = verified_ids;
            g.applied_profile_index = None;
            pulled_profiles = true;
        }
        if let Some(profs) = valid_profs {
            ApiState::replace_profiles_and_remap(&mut g, profs, false, false);
            g.adopt_pulled_profile_themes();
            g.profiles_dirty = false;
            pulled_profiles = true;
        }
        if let Some(pk) = pulled_keymap {
            apply_pulled_keymap(&mut g, pk);
        }
    }
    if pulled_profiles {
        let _ = state.persist_profiles();
    }
    Ok(pulled)
}

/// Reads the keyboard's configuration in the background. Sections with unsaved edits, or edited
/// while the read was in flight, keep the edits.
pub fn apply_device_pull_bg(state: &Arc<ApiState>, device: &dyn DeviceSink) {
    let (app_rev, set_rev, mos_rev, km_rev, prof_rev) = {
        let g = state.inner.lock().unwrap();
        (
            g.appearance_rev,
            g.settings_rev,
            g.mosaic_rev,
            g.keymap_rev,
            g.profiles_rev,
        )
    };
    let pulled = match device.pull_config_dir() {
        Ok(p) => p,
        Err(e) => {
            record_failed_pull(state, &e);
            return;
        }
    };
    let valid_profs = pulled
        .get("profiles")
        .and_then(|v| {
            serde_json::from_value::<Vec<crate::api::state::ShortcutProfileState>>(v.clone()).ok()
        })
        .filter(|p| ApiState::validate_profile_list(p).is_ok());
    let verified_ids: Option<Vec<String>> = valid_profs.as_ref().and_then(|profs| {
        pulled
            .get("firmwareProfileSettingsHex")
            .and_then(|v| v.as_str())
            .and_then(|hex| ApiState::decode_hex_bytes(hex).ok())
            .filter(|bytes| ApiState::verify_profiles_match_settings(profs, bytes).is_ok())
            .map(|_| profs.iter().map(|p| p.id.clone()).collect())
    });
    let pulled_keymap = PulledKeymap::read(&pulled);
    let mut pulled_profiles = false;
    {
        let mut g = state.inner.lock().unwrap();
        if !g.appearance_dirty && g.appearance_rev == app_rev {
            if let Some(a) = pulled.get("appearance").filter(|v| v.is_object()) {
                g.appearance = a.clone();
                g.appearance_hydrated = true;
                record_pulled_document(&mut g, "appearance", a);
            }
        }
        if !g.settings_dirty && g.settings_rev == set_rev {
            if let Some(s) = pulled.get("settings").filter(|v| v.is_object()) {
                g.settings = s.clone();
                g.settings_hydrated = true;
                record_pulled_document(&mut g, "settings", s);
            }
        }
        if !g.mosaic_dirty && g.mosaic_rev == mos_rev {
            if let Some(m) = pulled.get("mosaic").filter(|v| v.is_object()) {
                g.mosaic = m.clone();
                g.mosaic_hydrated = true;
                record_pulled_document(&mut g, "mosaic", m);
            }
        }
        if g.committed_profile_ids != verified_ids {
            g.committed_profile_ids = verified_ids;
            g.applied_profile_index = None;
            pulled_profiles = true;
        }
        if !g.profiles_dirty && g.profiles_rev == prof_rev {
            if let Some(profs) = valid_profs {
                ApiState::replace_profiles_and_remap(&mut g, profs, false, false);
                g.adopt_pulled_profile_themes();
                pulled_profiles = true;
            }
        }
        if let Some(pk) = pulled_keymap {
            if !g.keymap_dirty && g.keymap_rev == km_rev {
                apply_pulled_keymap(&mut g, pk);
            } else {
                g.device_keymap_blob = pk.blob;
            }
        }
    }
    if pulled_profiles {
        let _ = state.persist_profiles();
    }
}

/// Follows a bridge install started through the worker and confirms the new bridge once it runs
/// (see [`crate::bridge_install`]). Nothing is confirmed for a bridge that merely answers.
fn follow_bridge_install(
    state: &Arc<ApiState>,
    device: &dyn DeviceSink,
    has_bridge: bool,
    wstate: &mut WorkerLoopState,
) {
    if let Some((_, since)) = wstate.bridge_install_lease.as_ref() {
        if wstate.pending_bridge.is_none() || since.elapsed() >= BRIDGE_INSTALL_LEASE {
            wstate.bridge_install_lease = None;
        }
    }
    let Some((pending, since)) = wstate.pending_bridge.as_mut() else {
        return;
    };
    if since.elapsed() >= BRIDGE_INSTALL_FOLLOW {
        wstate.pending_bridge = None;
        wstate.bridge_install_lease = None;
        persist_pending_bridge(state, None);
        return;
    }
    if !has_bridge
        || wstate
            .next_bridge_poll_at
            .is_some_and(|t| Instant::now() < t)
    {
        return;
    }
    let had_attempt = pending.attempt.is_some();
    match pending.poll(device) {
        Ok(crate::bridge_install::Progress::Confirmed { .. }) => {
            wstate.pending_bridge = None;
            wstate.bridge_install_lease = None;
            wstate.next_bridge_poll_at = None;
            persist_pending_bridge(state, None);
            state.inner.lock().unwrap().versions_loaded = false;
        }
        _ => {
            if !had_attempt && pending.attempt.is_some() {
                let p = pending.clone();
                persist_pending_bridge(state, Some(&p));
            }
            wstate.next_bridge_poll_at = Some(Instant::now() + Duration::from_secs(2));
        }
    }
}

/// Longest wait between reads of a keyboard whose profile map or keymap is still unknown.
pub const PULL_RETRY_MAX: Duration = Duration::from_secs(60);

/// Whether something the worker reads from the keyboard is still unknown: which profiles it
/// holds, or a keymap that can be trusted.
fn pull_incomplete(g: &crate::api::state::InnerState) -> bool {
    g.committed_profile_ids.is_none() || !g.keymap_trusted()
}

/// Sets when the keyboard is read again: 2 s after the first failed read, doubling up to
/// [`PULL_RETRY_MAX`].
fn schedule_pull_retry(g: &mut crate::api::state::InnerState, wstate: &mut WorkerLoopState) {
    let next_secs = if wstate.pull_backoff_secs == 0 {
        2
    } else {
        (wstate.pull_backoff_secs * 2).min(PULL_RETRY_MAX.as_secs())
    };
    wstate.pull_backoff_secs = next_secs;
    g.next_pull_retry_at = Some(Instant::now() + Duration::from_secs(next_secs));
}

/// The keyboard is read again while which profiles it holds or its keymap is unknown, so a read
/// that failed (or found `keymap.json` and the firmware keymap disagreeing) recovers once the
/// keyboard answers. Returns whether it read the keyboard.
fn retry_incomplete_pull(state: &Arc<ApiState>, device: &dyn DeviceSink) -> bool {
    let due = {
        let g = state.inner.lock().unwrap();
        pull_incomplete(&g)
            && g.next_pull_retry_at
                .map(|t| Instant::now() >= t)
                .unwrap_or(true)
    };
    if due {
        apply_device_pull_bg(state, device);
    }
    due
}

/// After a read: once everything is known the backoff starts over; otherwise the next read is
/// scheduled.
fn settle_pull_backoff(state: &ApiState, wstate: &mut WorkerLoopState) {
    let mut g = state.inner.lock().unwrap();
    if pull_incomplete(&g) {
        schedule_pull_retry(&mut g, wstate);
    } else {
        wstate.pull_backoff_secs = 0;
        g.next_pull_retry_at = None;
    }
}

/// Reads the firmware's runtime state. An active profile slot other than the one the daemon last
/// applied was chosen on the keyboard: it becomes the desired profile (shown in Studio and kept
/// across restarts), and app rules don't switch the profile again until the foreground app
/// changes. Returns whether such a choice was taken over.
fn observe_firmware_runtime(
    state: &Arc<ApiState>,
    device: &dyn DeviceSink,
    foreground: Option<&str>,
) -> bool {
    let runtime = device.read_firmware_runtime().ok();
    let adopted = {
        let mut g = state.inner.lock().unwrap();
        g.firmware_runtime = runtime;
        let Some(rt) = runtime else {
            return false;
        };
        if g.applied_profile_slot
            .is_none_or(|slot| slot == rt.active_profile_slot)
        {
            return false;
        }
        let local = g
            .committed_profile_ids
            .as_ref()
            .and_then(|ids| ids.get(rt.active_profile_slot as usize))
            .and_then(|id| g.profiles.iter().position(|p| &p.id == id));
        let Some(local) = local else {
            // A slot the daemon can't place: nothing is taken over, and nothing is sent over it.
            return false;
        };
        ApiState::apply_profile_selection(&mut g, local);
        g.adopt_profile_theme(local);
        g.record_applied_profile(Some(local), rt.active_profile_slot);
        g.app_rules_hold = Some(crate::api::state::AppRulesHold {
            foreground: foreground.map(str::to_string),
        });
        local
    };
    if let Err(e) = state.persist_profiles() {
        eprintln!("openfluxd: the profile chosen on the keyboard couldn't be stored: {e}");
    }
    state.emit(
        "profileChange",
        serde_json::json!({ "profileIndex": adopted, "source": "keyboard" }),
    );
    true
}

pub fn reconcile_tick(
    state: &Arc<ApiState>,
    bridge: &mut dyn OsSensorBridge,
    device: &dyn DeviceSink,
    platform_code: u32,
) {
    let mut tmp = WorkerLoopState::default();
    reconcile_tick_with_state(state, bridge, device, platform_code, &mut tmp);
}

/// Single authoritative reconcile tick shared between background OS polling and API state.
pub fn reconcile_tick_with_state(
    state: &Arc<ApiState>,
    bridge: &mut dyn OsSensorBridge,
    device: &dyn DeviceSink,
    platform_code: u32,
    wstate: &mut WorkerLoopState,
) {
    state.prune_expired_overrides();

    // Evict idle DisplayPreview allocation if no preview frame was rendered for > 60s
    let evict_preview = state
        .preview_last_used
        .lock()
        .unwrap()
        .map(|t| t.elapsed() > Duration::from_secs(60))
        .unwrap_or(false);
    if evict_preview {
        state
            .preview_epoch
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *state.preview_last_used.lock().unwrap() = None;
        crate::preview::kill_and_reap_child(&state.preview_child);
        if let Ok(mut guard) = state.preview.try_lock() {
            *guard = None;
        }
        crate::preview::release_process_heap_pages();
    }

    // While a job has the keyboard to itself, the worker leaves it alone. Afterwards everything is
    // read again from the keyboard.
    if state.keyboard_reserved() {
        wstate.keyboard_was_reserved = true;
        return;
    }
    if std::mem::take(&mut wstate.keyboard_was_reserved) {
        let mut g = state.inner.lock().unwrap();
        g.versions_attempted = false;
        g.versions_loaded = false;
        g.appearance_hydrated = false;
        g.settings_hydrated = false;
        g.mosaic_hydrated = false;
        g.forget_keymap();
        g.committed_profile_ids = None;
        // The firmware may have restarted during the job, so the profile is applied again rather
        // than its active slot taken for a choice made on the keyboard.
        g.applied_profile_index = None;
        g.applied_profile_slot = None;
        g.firmware_runtime = None;
        drop(g);
        wstate.last_live_sent = None;
        wstate.pull_backoff_secs = 0;
        wstate.next_versions_retry_at = None;
    }

    let (has_bridge, has_firmware) = device.is_connected();
    if !has_bridge {
        wstate.last_live_sent = None;
        wstate.last_live_sent_at = None;
        wstate.clock_sync = None;
    }
    if !has_firmware {
        wstate.last_gamepad_cfg_sig = None;
        wstate.last_layer_override_sig = None;
    } else {
        // Push v2 `gamepad/config` -> RP2040 `FW_CMD_SET_GAMEPAD_CONFIG (0x74)` on change or connect!
        if let Some(msg) = state.v2.broker.get_retained("gamepad/config") {
            let sig = msg.payload.to_string();
            if wstate.last_gamepad_cfg_sig.as_deref() != Some(&sig) {
                let mode = match msg.payload.get("mode").and_then(|v| v.as_str()).unwrap_or("off") {
                    "hid" => 1u8,
                    "xinput" => 2u8,
                    _ => 0u8,
                };
                let dz = msg
                    .payload
                    .get("deadzone_um")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(150)
                    .clamp(20, 1500) as u16;
                let sat = msg
                    .payload
                    .get("saturation_um")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(2600)
                    .clamp(500, 2800) as u16;
                let curve = match msg.payload.get("curve").and_then(|v| v.as_str()).unwrap_or("s_curve") {
                    "linear" => 0u8,
                    "snappy" => 2u8,
                    "precision" => 3u8,
                    _ => 1u8,
                };
                let radial = msg
                    .payload
                    .get("radial_clamp")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                if device.set_gamepad_config(mode, dz, sat, curve, radial).is_ok() {
                    wstate.last_gamepad_cfg_sig = Some(sig);
                }
            }
        }
        // Push v2 `keyboard/layer_override` (App Linking!) -> RP2040 `FW_CMD_SET_LAYER_OVERRIDE (0x75)`
        if let Some(msg) = state.v2.broker.get_retained("keyboard/layer_override") {
            let sig = msg.payload.to_string();
            if wstate.last_layer_override_sig.as_deref() != Some(&sig) {
                let layer = msg
                    .payload
                    .get("layer")
                    .and_then(|v| v.as_u64())
                    .map(|v| (v.min(7)) as u8);
                if device.set_layer_override(layer).is_ok() {
                    wstate.last_layer_override_sig = Some(sig);
                }
            }
        }
    }
    // The bridge leaves the keyboard's clock to the host.
    if has_bridge
        && state
            .check_keyboard_write(&[crate::updates::Area::BridgeUsb])
            .is_ok()
    {
        sync_keyboard_clock(device, wstate);
    }
    // In pure OFV2 mode, `openflux-host-telemetry` + `focus-watch` already publish CPU/GPU/RAM,
    // media, weather, and app focus cleanly without blocking USB — skip duplicate subprocess forks!
    let sample_due = !device.is_v2_mux_active()
        && (has_bridge
            || has_firmware
            || wstate
                .last_idle_sample_at
                .map(|t| t.elapsed() >= Duration::from_secs(2))
                .unwrap_or(true));
    let (hw, media, win, os_lang, host_disp_asleep) = if sample_due {
        wstate.last_idle_sample_at = Some(Instant::now());
        (
            bridge.sample_hardware(),
            bridge.media_snapshot(),
            bridge.active_window(),
            bridge.active_language_code(),
            bridge.display_asleep(),
        )
    } else {
        let g = state.inner.lock().unwrap();
        (g.hardware.clone(), g.media.clone(), None, None, false)
    };
    let mut need_versions = false;

    let (
        mut rules,
        override_proc,
        override_lang,
        mut cur_desired_prof,
        mut cur_applied_prof,
        mut cur_prof_rev,
        cur_applied_lang,
        platform_sent,
    );
    let mut cur_desired_prof_id: Option<String>;
    let mut disconnected_now = false;
    {
        let mut g = state.inner.lock().unwrap();
        let prev_connected = g.cached_connected;
        let conn_changed = has_bridge != prev_connected.0 || has_firmware != prev_connected.1;
        let retry_due = wstate
            .next_versions_retry_at
            .is_none_or(|t| Instant::now() >= t);
        if (has_bridge || has_firmware) && (conn_changed || (!g.versions_loaded && retry_due)) {
            need_versions = true;
        }
        g.cached_connected = (has_bridge, has_firmware);
        if !has_bridge && !has_firmware {
            g.versions_attempted = false;
            g.versions_loaded = false;
            g.cached_versions = crate::api::state::empty_versions();
            g.appearance_hydrated = false;
            g.settings_hydrated = false;
            g.mosaic_hydrated = false;
            g.forget_keymap();
            g.committed_profile_ids = None;
            wstate.pull_backoff_secs = 0;
            wstate.next_versions_retry_at = None;
            if prev_connected.0 || prev_connected.1 {
                disconnected_now = true;
            }
        }
        g.hardware = hw.clone();
        g.media = media.clone();
        let _ = state.v2.broker.publish_typed(
            openflux_bus::topics::HOST_SYSTEM_METRICS,
            &openflux_bus::SystemMetrics {
                cpu_percent: hw.cpu_pct as f32,
                memory_used_gb: hw.mem_used_gb as f32,
                memory_total_gb: hw.mem_total_gb as f32,
                gpu_percent: hw.gpu_pct as f32,
                gpu_unified: hw.gpu_unified_mem,
                vram_used_gb: hw.vram_used_gb.map(|v| v as f32),
                vram_total_gb: hw.vram_total_gb.map(|v| v as f32),
                net_down_kbps: 0.0,
                net_up_kbps: 0.0,
                net_down: hw.net_down.clone(),
                net_up: hw.net_up.clone(),
            },
            openflux_bus::PublishOptions::retained(),
        );
        let _ = state.v2.broker.publish_typed(
            openflux_bus::topics::HOST_MEDIA_NOW_PLAYING,
            &openflux_bus::MediaNowPlaying {
                title: media.title.clone(),
                artist: media.artists.clone(),
                album: media.album.clone(),
                playing: media.playing,
                length_sec: media.length_s as f32,
                position_sec: media.played_s as f32,
                track_id: String::new(),
                art_base64: String::new(),
            },
            openflux_bus::PublishOptions::retained(),
        );
        if !has_firmware {
            g.record_firmware_detached();
            g.forget_keymap();
            g.committed_profile_ids = None;
        }
        rules = g.app_rules.clone();
        override_proc = g.detection_override_process.clone();
        override_lang = g.detection_override_layout.clone();
        cur_desired_prof = g.desired_profile_index;
        cur_desired_prof_id = g.profiles.get(cur_desired_prof).map(|p| p.id.clone());
        cur_applied_prof = g.applied_profile_index;
        cur_prof_rev = g.profiles_rev;
        cur_applied_lang = g.applied_language_code;
        platform_sent = g.platform_sent;
    }

    if disconnected_now {
        if let Some(u) = state.updates() {
            u.observe_keyboard((false, false), &crate::api::state::empty_versions());
        }
    }

    if need_versions {
        let v = device.versions();
        if let Some(u) = state.updates() {
            u.observe_keyboard((has_bridge, has_firmware), &v);
        }
        let bridge_complete = !has_bridge
            || (v.get("bridge").and_then(|x| x.as_str()).is_some()
                && v.get("display").and_then(|x| x.as_str()).is_some());
        let fw_complete = !has_firmware || v.get("firmware").and_then(|x| x.as_str()).is_some();
        let all_complete = bridge_complete && fw_complete;
        let mut g = state.inner.lock().unwrap();
        let changed = g.cached_versions != v;
        g.cached_versions = v;
        g.versions_attempted = true;
        g.versions_loaded = all_complete;
        wstate.next_versions_retry_at = if all_complete {
            None
        } else {
            Some(Instant::now() + Duration::from_secs(3))
        };
        if changed {
            g.appearance_hydrated = false;
            g.settings_hydrated = false;
            g.mosaic_hydrated = false;
            g.forget_keymap();
            g.committed_profile_ids = None;
            wstate.pull_backoff_secs = 0;
        }
        let should_pull = (!g.appearance_hydrated && !g.keymap_hydrated)
            || (has_bridge && has_firmware && g.committed_profile_ids.is_none());
        drop(g);
        let mut did_pull_bg = false;
        if should_pull {
            apply_device_pull_bg(state, device);
            did_pull_bg = true;
        }
        if !did_pull_bg && has_bridge && has_firmware {
            did_pull_bg = retry_incomplete_pull(state, device);
        }
        if did_pull_bg {
            settle_pull_backoff(state, wstate);
            let g = state.inner.lock().unwrap();
            rules = g.app_rules.clone();
            cur_desired_prof = g.desired_profile_index;
            cur_desired_prof_id = g.profiles.get(cur_desired_prof).map(|p| p.id.clone());
            cur_applied_prof = g.applied_profile_index;
            cur_prof_rev = g.profiles_rev;
        }
    } else if has_bridge && has_firmware && retry_incomplete_pull(state, device) {
        settle_pull_backoff(state, wstate);
        let g = state.inner.lock().unwrap();
        rules = g.app_rules.clone();
        cur_desired_prof = g.desired_profile_index;
        cur_desired_prof_id = g.profiles.get(cur_desired_prof).map(|p| p.id.clone());
        cur_applied_prof = g.applied_profile_index;
        cur_prof_rev = g.profiles_rev;
    }

    follow_bridge_install(state, device, has_bridge, wstate);

    let foreground = foreground_app(override_proc.as_deref(), win.as_ref());
    let firmware_writable = state
        .check_keyboard_write(&[crate::updates::Area::FirmwareUsb])
        .is_ok();
    if has_firmware
        && firmware_writable
        && observe_firmware_runtime(state, device, foreground.as_deref())
    {
        let g = state.inner.lock().unwrap();
        cur_desired_prof = g.desired_profile_index;
        cur_desired_prof_id = g.profiles.get(cur_desired_prof).map(|p| p.id.clone());
        cur_applied_prof = g.applied_profile_index;
        cur_prof_rev = g.profiles_rev;
    }
    let rules_switch_profiles = {
        let mut g = state.inner.lock().unwrap();
        g.observe_foreground_app(foreground.as_deref());
        g.app_rules_switch_profiles()
    };

    let matched_rule = if let Some(ref lp) = override_proc {
        rules.iter().find(|r| {
            let m = r.matcher.to_lowercase();
            !m.is_empty() && lp.to_lowercase().contains(&m)
        })
    } else {
        win.as_ref().and_then(|w| match_app_rule(&rules, w))
    };
    // While locked, or held after a profile chosen on the keyboard, a rule still sets the
    // language but not the profile.
    let matched = matched_rule.filter(|_| rules_switch_profiles);

    let desired_profile = matched
        .map(|r| r.profile_index as usize)
        .unwrap_or(cur_desired_prof);
    let desired_profile_id = if matched.is_some() {
        let g = state.inner.lock().unwrap();
        if g.profiles_rev == cur_prof_rev {
            g.profiles.get(desired_profile).map(|p| p.id.clone())
        } else {
            None
        }
    } else {
        cur_desired_prof_id
    };
    // Publish active foreground application and active profile onto the OpenFlux v2 pub/sub bus
    // so any module (e.g. game-specific StreamDeck ability bars, IDE shortcuts, companion behavior)
    // can react automatically when you switch apps or profiles!
    let active_profile_name = {
        let g = state.inner.lock().unwrap();
        g.profiles
            .get(desired_profile)
            .map(|p| p.name.clone())
            .unwrap_or_default()
    };
    let app_payload = serde_json::json!({
        "app_id": win.as_ref().map(|w| w.app_id.as_str()).unwrap_or(""),
        "process_name": override_proc
            .as_deref()
            .or_else(|| win.as_ref().map(|w| w.process_name.as_str()))
            .unwrap_or(""),
        "window_title": win.as_ref().map(|w| w.window_title.as_str()).unwrap_or(""),
        "profile_index": desired_profile,
        "profile_id": desired_profile_id.as_deref().unwrap_or(""),
        "profile_name": active_profile_name,
    });
    let focus_watch_active = state
        .v2
        .broker
        .get_retained(openflux_bus::topics::HOST_APP_ACTIVE)
        .and_then(|m| m.origin)
        .is_some_and(|o| o == "focus-watch");
    let app_changed = !focus_watch_active
        && state
            .v2
            .broker
            .get_retained(openflux_bus::topics::HOST_APP_ACTIVE)
            .map(|m| m.payload != app_payload)
            .unwrap_or(true);
    if app_changed {
        let _ = state.v2.broker.publish(
            openflux_bus::topics::HOST_APP_ACTIVE,
            app_payload,
            openflux_bus::PublishOptions::retained(),
        );
    }
    let prof_payload = serde_json::json!({
        "index": desired_profile,
        "id": desired_profile_id.as_deref().unwrap_or(""),
        "name": active_profile_name,
    });
    let prof_changed = state
        .v2
        .broker
        .get_retained("host/profile/active")
        .map(|m| m.payload != prof_payload)
        .unwrap_or(true);
    if prof_changed {
        let _ = state.v2.broker.publish(
            "host/profile/active",
            prof_payload,
            openflux_bus::PublishOptions::retained(),
        );
    }

    if let Some(ref pid) = desired_profile_id {
        let mut g = state.inner.lock().unwrap();
        let mut theme_rev_changed = false;
        if g.profiles_rev == cur_prof_rev {
            let expected_theme = g
                .profiles
                .get(desired_profile)
                .map(|p| p.theme_id.clone())
                .unwrap_or_default();
            let changed_binding = g
                .last_effective_profile_binding
                .as_ref()
                .map(|(lid, ltid)| lid != pid || ltid != &expected_theme)
                .unwrap_or(true);
            if changed_binding {
                crate::api::state::ApiState::apply_profile_theme(&mut g, desired_profile);
                theme_rev_changed = g.choose_profile_theme(desired_profile, false);
            }
        }
        drop(g);
        if theme_rev_changed {
            if let Err(e) = state.persist_profiles() {
                eprintln!("openfluxd: the theme revision couldn't be stored: {e}");
            }
        }
    }

    let override_lang_code = override_lang
        .as_deref()
        .filter(|s| !s.is_empty())
        .and_then(layout_to_language_code);
    let desired_lang = override_lang_code
        .or_else(|| matched_rule.and_then(|r| r.language_code))
        .or(os_lang);

    if has_firmware && firmware_writable {
        // The host's platform is sent once per attachment; a platform chosen on the keyboard
        // afterwards stays.
        let mut session_ok = platform_sent.is_some();
        if !session_ok {
            let sent = device.set_platform(platform_code).is_ok();
            let mut g = state.inner.lock().unwrap();
            if sent {
                g.firmware_attached = true;
                g.platform_sent = Some(platform_code);
                session_ok = true;
            } else {
                g.record_firmware_unanswered();
            }
        }
        if session_ok {
            if cur_applied_prof != Some(desired_profile) {
                let resolved_slot = desired_profile_id.as_deref().and_then(|pid| {
                    let g = state.inner.lock().unwrap();
                    if g.profiles_rev != cur_prof_rev {
                        return None;
                    }
                    let li = g.profiles.iter().position(|p| p.id == pid)?;
                    crate::api::state::ApiState::hardware_profile_slot(&g, li)
                        .map(|s| (pid.to_string(), s))
                });
                if let Some((pid, hw_slot)) = resolved_slot {
                    if device.set_active_profile(hw_slot as u32).is_ok() {
                        let mut g = state.inner.lock().unwrap();
                        let local = if g.profiles_rev == cur_prof_rev
                            && (matched.is_some() || g.desired_profile_index == cur_desired_prof)
                        {
                            g.profiles.iter().position(|p| p.id == pid)
                        } else {
                            None
                        };
                        g.record_applied_profile(local, hw_slot as u32);
                    } else {
                        state.inner.lock().unwrap().record_firmware_unanswered();
                        session_ok = false;
                    }
                }
            }
            if session_ok {
                if let Some(dl) = desired_lang {
                    if cur_applied_lang != Some(dl) {
                        if device.set_language(dl).is_ok() {
                            let mut g = state.inner.lock().unwrap();
                            g.applied_language_code = Some(dl);
                            g.firmware_attached = true;
                        } else {
                            state.inner.lock().unwrap().record_firmware_unanswered();
                        }
                    }
                }
            }
        }
    }

    let mut weather_city = String::from(crate::bridges::weather::DEFAULT_WEATHER_CITY);
    let mut weather_celsius = false;
    let mut has_weather_tile = false;
    {
        let g = state.inner.lock().unwrap();
        if let (Some(tt), Some(tc)) = (
            g.mosaic.get("tileTypes").and_then(|v| v.as_array()),
            g.mosaic.get("tileConfigs").and_then(|v| v.as_array()),
        ) {
            for (idx, tval) in tt.iter().enumerate() {
                if tval.as_u64() == Some(TILE_TYPE_WEATHER) {
                    has_weather_tile = true;
                    if let Some(cfg) = tc.get(idx) {
                        if let Some(c) = cfg
                            .get("city")
                            .and_then(|x| x.as_str())
                            .filter(|s| !s.is_empty())
                        {
                            weather_city = c.to_string();
                        }
                        // The mosaic format spells this key "celcius".
                        weather_celsius = cfg
                            .get("celcius")
                            .and_then(|x| x.as_bool())
                            .unwrap_or(false);
                    }
                    break;
                }
            }
        }
    }
    // Poll weather whenever ANY enabled module (built-in or third-party!) subscribes to
    // `host/weather/current` or `host/weather/#`!
    {
        let v2_saved = state.v2.registry.load_state();
        let v2_disc = state.v2.registry.discover();
        for (id, dm) in &v2_disc {
            let Some(inst) = v2_saved.modules.get(id) else {
                continue;
            };
            if !inst.enabled {
                continue;
            }
            let wants_weather = dm.manifest.channels.subscribes.iter().any(|p| {
                p == openflux_bus::topics::HOST_WEATHER_CURRENT
                    || p == "host/weather/#"
                    || p == "host/weather/+"
            });
            if wants_weather {
                has_weather_tile = true;
                if let Some(c) = inst
                    .config
                    .get("city")
                    .and_then(|x| x.as_str())
                    .filter(|s| !s.trim().is_empty())
                {
                    weather_city = c.trim().to_string();
                }
                if let Some(cel) = inst.config.get("celsius").and_then(|x| x.as_bool()) {
                    weather_celsius = cel;
                }
                break;
            }
        }
    }
    let weather_snap =
        if has_weather_tile && !state.override_ttl.is_zero() && state.token_store_path.is_some() {
            Some(
                state
                    .weather_bridge
                    .poll_nonblocking(&weather_city, weather_celsius),
            )
        } else {
            None
        };
    if let Some(ref w) = weather_snap {
        let hourly = w
            .hourly
            .iter()
            .take(3)
            .map(|h| openflux_bus::WeatherHourly {
                hour: h.hour_label.clone(),
                temp: h.temp as f32,
                condition: h.condition.clone(),
                is_day: h.is_day,
            })
            .collect();
        let _ = state.v2.broker.publish_typed(
            openflux_bus::topics::HOST_WEATHER_CURRENT,
            &openflux_bus::WeatherState {
                city: w.city.clone(),
                temp: w.temp as f32,
                high: w.max_temp as f32,
                low: w.min_temp as f32,
                condition: w.condition.clone(),
                is_day: w.is_day,
                celsius: w.celsius,
                ready: w.ready,
                aqi: w.aqi.map(|a| a as u16),
                sunrise: w.sunrise.clone(),
                sunset: w.sunset.clone(),
                utc_offset_sec: w.utc_offset_seconds,
                hourly,
            },
            openflux_bus::PublishOptions::retained(),
        );
    }
    let mut live_json = build_live_data_json(&hw, &media, weather_snap.as_ref());
    {
        let mut active_pack = {
            let g = state.inner.lock().unwrap();
            // Mirror the real active hardware keymap onto the host v2 bus so Studio preview
            // matches your exact remapped keys (e.g. Cmd on key 74, Fn on key 79).
            let layout_key = g
                .keymap
                .get("selectedFrameLayoutType")
                .and_then(|v| v.as_str())
                .unwrap_or("ansi");
            if let Some(km_layout) = g.keymap.get("keymaps").and_then(|m| m.get(layout_key)) {
                let changed = state
                    .v2
                    .broker
                    .get_retained("keyboard/keymap")
                    .map(|m| &m.payload != km_layout)
                    .unwrap_or(true);
                if changed {
                    let _ = state.v2.broker.publish(
                        "keyboard/keymap",
                        km_layout.clone(),
                        openflux_bus::PublishOptions::retained(),
                    );
                }
            }
            g.profiles
                .get(desired_profile)
                .and_then(|p| p.shortcut_pack.clone())
                .unwrap_or_default()
        };
        // Auto-select a matching v1/custom shortcut pack for the foreground app when the profile leaves it blank!
        // Ignore transient None probes or macOS switcher overlays (Dock/WindowServer/Spotlight)
        // so shortcuts NEVER flash back and forth!
        if active_pack.is_empty() {
            let fg_app = win.as_ref().map(|w| w.app_id.as_str()).unwrap_or("");
            let fg_proc = win.as_ref().map(|w| w.process_name.as_str()).unwrap_or("");
            let fg_l = format!("{fg_app} {fg_proc}").to_ascii_lowercase();
            let is_transient = (fg_app.is_empty() && fg_proc.is_empty())
                || fg_l.contains("com.apple.dock")
                || fg_l.contains("windowserver")
                || fg_l.contains("spotlight")
                || fg_l.contains("notificationcenter")
                || fg_l.contains("controlcenter");
            if is_transient {
                if let Some(ref prev_key) = wstate.cached_pack_name {
                    if let Some((prev_pack, _)) = prev_key.rsplit_once(':') {
                        active_pack = prev_pack.to_string();
                    }
                }
            }
        }
        if active_pack.is_empty() {
            let fg_app = win.as_ref().map(|w| w.app_id.as_str()).unwrap_or("");
            let fg_proc = win.as_ref().map(|w| w.process_name.as_str()).unwrap_or("");
            if let Some(matched) =
                crate::api::shortcuts::match_pack_for_app(state, fg_app, fg_proc, platform_code)
            {
                active_pack = matched;
            } else {
                let fg_l = format!("{fg_app} {fg_proc}").to_ascii_lowercase();
                active_pack = if fg_l.contains("chrome") {
                    "chrome.json".into()
                } else if fg_l.contains("safari") {
                    "safari.json".into()
                } else if fg_l.contains("firefox") {
                    "firefox.json".into()
                } else if fg_l.contains("vscode")
                    || fg_l.contains("visual studio code")
                    || fg_l.contains("cursor")
                {
                    "vscode.json".into()
                } else if fg_l.contains("finder") {
                    "finder.json".into()
                } else if fg_l.contains("slack") {
                    "slack.json".into()
                } else if fg_l.contains("discord") {
                    "discord.json".into()
                } else if fg_l.contains("figma") {
                    "figma.json".into()
                } else if fg_l.contains("zoom") {
                    "zoom.json".into()
                } else if fg_l.contains("blender") {
                    "blender.json".into()
                } else if fg_l.contains("photoshop") {
                    "photoshop.json".into()
                } else if fg_l.contains("terminal")
                    || fg_l.contains("iterm")
                    || fg_l.contains("flightdeck")
                {
                    "macos-terminal.json".into()
                } else if platform_code == 2 {
                    "macos.json".into()
                } else {
                    "windows.json".into()
                };
            }
        }
        let cache_key = format!("{active_pack}:{platform_code}");
        if wstate.cached_pack_name.as_deref() != Some(cache_key.as_str()) {
            wstate.cached_pack_name = Some(cache_key);
            let pack_rows = crate::api::shortcuts::overlay_rows(state, &active_pack);
            wstate.cached_pack_overlays = shortcut_overlays(platform_code, &pack_rows);
            let _ = state.v2.broker.publish(
                openflux_bus::topics::HOST_APP_SHORTCUTS,
                serde_json::json!({
                    "pack": active_pack,
                    "platform_code": platform_code,
                    "overlays": wstate.cached_pack_overlays,
                }),
                openflux_bus::PublishOptions::retained(),
            );
        }
        if let Some(obj) = live_json.as_object_mut() {
            obj.insert(
                "hostDisplayAsleep".into(),
                serde_json::json!(host_disp_asleep),
            );
            if !wstate.cached_pack_overlays.is_empty() {
                obj.insert(
                    "shortcutOverlays".into(),
                    serde_json::Value::Array(wstate.cached_pack_overlays.clone()),
                );
            }
        }

        let dev_pomo = if sample_due && has_bridge {
            device.read_device_pomodoro_state()
        } else {
            None
        };
        if let Some(ref dp) = dev_pomo {
            reconcile_pomodoro_from_device(state, Some(dp));
        }
        let mut g = state.inner.lock().unwrap();
        // The preview shows the pomodoro timer; the keyboard owns it, so it isn't sent back down.
        let preview_pomo = if g.pomo_rev > 0 || g.pomo_last_dev_rev > 0 {
            let rem = if g.pomo_running {
                let elapsed = g
                    .pomo_anchor_at
                    .map(|t| t.elapsed().as_secs() as u32)
                    .unwrap_or(0);
                let left = g.pomo_anchor_rem_s.saturating_sub(elapsed);
                if left == 0 {
                    g.pomo_running = false;
                    g.pomo_anchor_rem_s = 0;
                    g.pomo_anchor_at = None;
                }
                left
            } else {
                g.pomo_anchor_rem_s
            };
            Some(serde_json::json!({
                "running": g.pomo_running,
                "remainingS": rem,
                "cmdSeq": g.pomo_rev,
            }))
        } else {
            None
        };
        // The live theme follows the profile the keyboard acknowledged, and only while the
        // selection is still that profile.
        if has_firmware
            && g.profiles_rev == cur_prof_rev
            && g.applied_profile_index == Some(desired_profile)
            && (matched.is_some() || g.desired_profile_index == cur_desired_prof)
        {
            if let (Some(hw_slot), Some(active_theme)) = (
                crate::api::state::ApiState::hardware_profile_slot(&g, desired_profile),
                g.profiles.get(desired_profile).map(|p| p.theme_id.clone()),
            ) {
                if let Some(obj) = live_json.as_object_mut() {
                    obj.insert(
                        "activeThemeId".into(),
                        serde_json::Value::String(active_theme),
                    );
                    obj.insert(
                        "activeThemeProfileSlot".into(),
                        serde_json::json!(hw_slot as u32),
                    );
                    obj.insert("themeRev".into(), serde_json::json!(g.theme_rev));
                }
            }
        }
        let mut cached_preview = live_json.clone();
        if let Some(pm) = preview_pomo {
            if let Some(obj) = cached_preview.as_object_mut() {
                obj.insert("pomodoro".into(), pm);
            }
        }
        g.cached_live_data_json = Some(cached_preview.to_string());
    }

    // Attach retained v2 host-module state (`modules/#`, `host/profile/active`) for legacy keepalive;
    // `host/app/active` is owned exclusively by the real-time `openflux-bus-bridge` push thread so
    // slow 1 Hz sensor snapshots can NEVER overwrite a newer focus change with a stale app!
    let mut v2_map = serde_json::Map::new();
    for pat in ["modules/#", "host/profile/active"] {
        if let Ok(msgs) = state.v2.broker.query_retained(pat) {
            for m in msgs {
                v2_map.insert(m.topic, m.payload);
            }
        }
    }
    if !v2_map.is_empty() {
        if let Some(obj) = live_json.as_object_mut() {
            obj.insert("v2Retained".into(), serde_json::Value::Object(v2_map));
        }
    }

    // When OFV2 full-duplex mux is active, all telemetry/app/media/weather flow cleanly over
    // subscription-driven `CH_BUS` mirroring — ZERO legacy v1 `live_json` / HDLC traffic!
    if has_bridge && !device.is_v2_mux_active() {
        let changed = wstate.last_live_sent.as_ref() != Some(&live_json);
        let keepalive_due = wstate
            .last_live_sent_at
            .map(|t| t.elapsed() >= Duration::from_secs(5))
            .unwrap_or(true);
        if (changed || keepalive_due) && device.send_live_json(&live_json).is_ok() {
            wstate.last_live_sent = Some(live_json);
            wstate.last_live_sent_at = Some(Instant::now());
        }
    }
}

/// The host platform code of macOS, whose shortcuts use Command rather than Ctrl.
const PLATFORM_MACOS: u32 = 2;
const MOD_CTRL: u64 = 1;
const MOD_SHIFT: u64 = 2;
const MOD_ALT: u64 = 4;
const MOD_GUI: u64 = 8;

/// The shortcuts shown on the keys while their modifiers are held: the host platform's common
/// shortcuts, with the active shortcut pack's single-key rows for that platform on top. Every pack
/// starts from the common shortcuts, so switching to no pack resets the overlays.
fn shortcut_overlays(
    platform_code: u32,
    pack_rows: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    let mac = platform_code == PLATFORM_MACOS;
    let primary = if mac { MOD_GUI } else { MOD_CTRL };
    let (redo_mods, redo_key) = if mac {
        (MOD_GUI | MOD_SHIFT, "KC_Z")
    } else {
        (MOD_CTRL, "KC_Y")
    };
    let (quit_mods, quit_key) = match platform_code {
        PLATFORM_MACOS => (MOD_GUI, "KC_Q"),
        1 => (MOD_ALT, "KC_F4"),
        _ => (MOD_CTRL, "KC_Q"),
    };
    let overlay = |mods: u64, key: &str, label: &str, icon: &str| serde_json::json!({"mods": mods, "keyCode": key, "label": label, "icon": icon});
    let mut merged = vec![
        overlay(primary, "KC_C", "Copy", ""),
        overlay(primary, "KC_V", "Paste", ""),
        overlay(primary, "KC_X", "Cut", ""),
        overlay(primary, "KC_Z", "Undo", ""),
        overlay(redo_mods, redo_key, "Redo", ""),
        overlay(primary, "KC_S", "Save", ""),
        overlay(primary, "KC_F", "Find", ""),
        overlay(primary, "KC_A", "All", ""),
        overlay(primary, "KC_W", "Close", ""),
        overlay(primary, "KC_N", "New", ""),
        overlay(primary, "KC_T", "Tab", ""),
        overlay(quit_mods, quit_key, "Quit", ""),
        overlay(primary, "KC_R", "Reload", ""),
    ];
    let prefix = if mac { "mac" } else { "win" };
    let (key_field, key2_field, mods_field) = (
        format!("{prefix}KeyCode"),
        format!("{prefix}KeyCode2"),
        format!("{prefix}Modifiers"),
    );
    for r in pack_rows {
        let Some(kc) = r.get(&key_field).and_then(|v| v.as_str()) else {
            continue;
        };
        if r.get(&key2_field).and_then(|v| v.as_str()).is_some() {
            continue;
        }
        let mods = r.get(&mods_field).and_then(|v| v.as_u64()).unwrap_or(0);
        if mods == 0 {
            continue;
        }
        let lbl = r.get("label").and_then(|v| v.as_str()).unwrap_or("");
        if lbl.is_empty() {
            continue;
        }
        let icon = r.get("icon").and_then(|v| v.as_str()).unwrap_or("");
        if let Some(pos) = merged.iter().position(|e| {
            e.get("mods").and_then(|v| v.as_u64()) == Some(mods)
                && e.get("keyCode").and_then(|v| v.as_str()) == Some(kc)
        }) {
            merged[pos] = overlay(mods, kc, lbl, icon);
        } else if merged.len() < 64 {
            merged.push(overlay(mods, kc, lbl, icon));
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridges::{HardwareMetrics, MediaSnapshot};
    use std::sync::atomic::AtomicUsize;

    /// A bridge-only keyboard that records the clocks it is given.
    #[derive(Default)]
    struct ClockSink {
        attached: AtomicBool,
        clocks: std::sync::Mutex<Vec<String>>,
    }

    impl DeviceSink for ClockSink {
        fn is_connected(&self) -> (bool, bool) {
            (self.attached.load(Ordering::SeqCst), false)
        }
        fn versions(&self) -> serde_json::Value {
            serde_json::json!({"bridge": "openflux-bridge 1.0.0"})
        }
        fn send_live_json(&self, _: &serde_json::Value) -> Result<(), String> {
            Ok(())
        }
        fn set_active_profile(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn set_language(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn set_platform(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn set_system_time(&self, utc: &str, zone: &str) -> Result<(), String> {
            assert_eq!(utc.len(), "2030-01-01 00:00:00".len(), "{utc}");
            self.clocks.lock().unwrap().push(zone.to_string());
            Ok(())
        }
        fn save_config_dir(&self, _: &serde_json::Value) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn the_keyboard_clock_is_set_on_attach_and_whenever_the_host_zone_or_offset_changes() {
        let state = ApiState::new(Arc::new(crate::api::state::NoopDeviceSink), None);
        let sink = ClockSink::default();
        let host = Arc::new(std::sync::Mutex::new(HostClock {
            zone: "Europe/Berlin".into(),
            utc_offset_secs: Some(3600),
        }));
        let read = host.clone();
        let mut wstate = WorkerLoopState {
            host_clock: Some(Box::new(move || read.lock().unwrap().clone())),
            ..Default::default()
        };
        let tick = |wstate: &mut WorkerLoopState| {
            reconcile_tick_with_state(&state, &mut DummyBridge, &sink, 2, wstate);
        };
        let clocks = || sink.clocks.lock().unwrap().clone();
        let due_for_check = |wstate: &mut WorkerLoopState| {
            wstate.clock_sync.as_mut().unwrap().next_check = Instant::now();
        };

        // Nothing is sent while no keyboard is attached.
        tick(&mut wstate);
        assert!(clocks().is_empty());

        // Attaching sends the clock once.
        sink.attached.store(true, Ordering::SeqCst);
        tick(&mut wstate);
        tick(&mut wstate);
        assert_eq!(clocks(), ["Europe/Berlin"]);

        // An offset change (summer time) sends it again, as does a new zone.
        host.lock().unwrap().utc_offset_secs = Some(7200);
        due_for_check(&mut wstate);
        tick(&mut wstate);
        assert_eq!(clocks().len(), 2);
        host.lock().unwrap().zone = "America/Denver".into();
        due_for_check(&mut wstate);
        tick(&mut wstate);
        assert_eq!(clocks().last().unwrap(), "America/Denver");

        // An unchanged clock is sent again only when the hourly resync is due.
        due_for_check(&mut wstate);
        tick(&mut wstate);
        assert_eq!(clocks().len(), 3);
        wstate.clock_sync.as_mut().unwrap().next_send = Instant::now();
        tick(&mut wstate);
        assert_eq!(clocks().len(), 4);

        // Reattaching sends it again.
        sink.attached.store(false, Ordering::SeqCst);
        tick(&mut wstate);
        sink.attached.store(true, Ordering::SeqCst);
        tick(&mut wstate);
        assert_eq!(clocks().len(), 5);
    }

    #[test]
    fn shortcut_overlays_follow_the_host_platform() {
        let rows = vec![serde_json::json!({
            "label": "Save As", "icon": "save.png",
            "macKeyCode": "KC_S", "macKeyCode2": null, "macModifiers": 0x0A,
            "winKeyCode": "KC_F12", "winKeyCode2": null, "winModifiers": 0,
        })];
        let find = |o: &[serde_json::Value], label: &str| {
            o.iter().find(|e| e["label"] == label).map(|e| {
                (
                    e["mods"].as_u64().unwrap(),
                    e["keyCode"].as_str().unwrap().to_string(),
                )
            })
        };
        let mac = shortcut_overlays(PLATFORM_MACOS, &rows);
        assert_eq!(find(&mac, "Copy"), Some((MOD_GUI, "KC_C".into())));
        assert_eq!(
            find(&mac, "Save As"),
            Some((MOD_GUI | MOD_SHIFT, "KC_S".into()))
        );
        let windows = shortcut_overlays(1, &rows);
        assert_eq!(find(&windows, "Copy"), Some((MOD_CTRL, "KC_C".into())));
        assert_eq!(find(&windows, "Redo"), Some((MOD_CTRL, "KC_Y".into())));
        assert_eq!(find(&windows, "Quit"), Some((MOD_ALT, "KC_F4".into())));
        // A row without a modifier on this platform isn't an overlay.
        assert_eq!(find(&windows, "Save As"), None);
        let linux = shortcut_overlays(3, &[]);
        assert_eq!(find(&linux, "Quit"), Some((MOD_CTRL, "KC_Q".into())));
    }

    struct DummyBridge;
    impl OsSensorBridge for DummyBridge {
        fn sample_hardware(&mut self) -> HardwareMetrics {
            HardwareMetrics {
                cpu_pct: 0.0,
                mem_used_gb: 0.0,
                mem_total_gb: 16.0,
                gpu_pct: 0.0,
                vram_used_gb: None,
                vram_total_gb: None,
                gpu_unified_mem: false,
                net_down: "0 KB/s".into(),
                net_up: "0 KB/s".into(),
            }
        }
        fn media_snapshot(&mut self) -> MediaSnapshot {
            MediaSnapshot::default()
        }
        fn active_window(&mut self) -> Option<crate::bridges::ActiveWindowInfo> {
            None
        }
        fn active_language_code(&mut self) -> Option<u32> {
            None
        }
    }

    /// A keyboard whose bridge install can be staged step by step.
    struct InstallSink {
        files: std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
        live_bridge: std::sync::Mutex<String>,
        confirms: AtomicUsize,
    }

    impl InstallSink {
        fn new(live: &str) -> Self {
            Self {
                files: Default::default(),
                live_bridge: std::sync::Mutex::new(live.into()),
                confirms: AtomicUsize::new(0),
            }
        }
        fn write(&self, path: &str, text: &str) {
            self.files
                .lock()
                .unwrap()
                .insert(path.into(), text.as_bytes().to_vec());
        }
        fn armed(&self) -> bool {
            self.files
                .lock()
                .unwrap()
                .contains_key(crate::device_paths::BRIDGE_ROLLBACK_ARMED)
        }
    }

    impl DeviceSink for InstallSink {
        fn is_connected(&self) -> (bool, bool) {
            (true, false)
        }
        fn versions(&self) -> serde_json::Value {
            serde_json::json!({"bridge": *self.live_bridge.lock().unwrap(), "display": "openflux-display 1.0.0"})
        }
        fn send_live_json(&self, _: &serde_json::Value) -> Result<(), String> {
            Ok(())
        }
        fn set_active_profile(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn set_language(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn set_platform(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn save_config_dir(&self, _: &serde_json::Value) -> Result<(), String> {
            Ok(())
        }
        fn stat_file(&self, path: &str) -> Result<bool, String> {
            Ok(self.files.lock().unwrap().contains_key(path))
        }
        fn get_file(&self, path: &str) -> Result<Vec<u8>, String> {
            self.files
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| "absent".into())
        }
        fn confirm_bridge_install(&self) -> Result<(), String> {
            self.confirms.fetch_add(1, Ordering::SeqCst);
            self.files
                .lock()
                .unwrap()
                .remove(crate::device_paths::BRIDGE_ROLLBACK_ARMED);
            Ok(())
        }
    }

    fn tick(state: &Arc<ApiState>, sink: &InstallSink, wstate: &mut WorkerLoopState) {
        wstate.next_bridge_poll_at = None;
        reconcile_tick_with_state(state, &mut DummyBridge, sink, 2, wstate);
    }

    const ATTEMPT: &str = crate::device_paths::BRIDGE_ROLLBACK_ATTEMPT;

    #[test]
    fn a_bridge_that_merely_answers_is_never_confirmed() {
        let state = ApiState::new(Arc::new(crate::api::state::NoopDeviceSink), None);
        let sink = InstallSink::new("openflux-bridge 1.0.0");
        sink.write(crate::device_paths::BRIDGE_ROLLBACK_ARMED, "");
        let mut wstate = WorkerLoopState::default();
        for _ in 0..3 {
            tick(&state, &sink, &mut wstate);
        }
        assert_eq!(sink.confirms.load(Ordering::SeqCst), 0);
        assert!(sink.armed());
    }

    #[test]
    fn only_the_new_trial_of_the_started_install_is_confirmed() {
        let state = ApiState::new(Arc::new(crate::api::state::NoopDeviceSink), None);
        let sink = InstallSink::new("openflux-bridge 1.0.0");
        sink.write(ATTEMPT, "attempt_id=aa01\nversion=openflux-bridge 1.0.0\n");
        let pending =
            crate::bridge_install::PendingInstall::begin(&sink, "openflux-bridge 1.0.1".into());
        let mut wstate = WorkerLoopState {
            pending_bridge: Some((pending, Instant::now())),
            ..Default::default()
        };

        // The package's preinst arms the rollback while the previous bridge still runs.
        sink.write(ATTEMPT, "attempt_id=bb02\nversion=openflux-bridge 1.0.1\n");
        sink.write(crate::device_paths::BRIDGE_ROLLBACK_ARMED, "");
        tick(&state, &sink, &mut wstate);
        assert_eq!(
            sink.confirms.load(Ordering::SeqCst),
            0,
            "the previous bridge answers; nothing is confirmed"
        );

        // A trial file of an earlier attempt proves nothing.
        sink.write(
            &crate::device_paths::bridge_trial_path("aa01"),
            "version=openflux-bridge 1.0.1\npid=1\n",
        );
        tick(&state, &sink, &mut wstate);
        assert_eq!(sink.confirms.load(Ordering::SeqCst), 0);

        // The new bridge's trial file, while another bridge still answers: not yet.
        sink.write(
            &crate::device_paths::bridge_trial_path("bb02"),
            "version=openflux-bridge 1.0.1\npid=42\n",
        );
        tick(&state, &sink, &mut wstate);
        assert_eq!(sink.confirms.load(Ordering::SeqCst), 0);

        // The new bridge answers with the target version: confirmed once.
        *sink.live_bridge.lock().unwrap() = "openflux-bridge 1.0.1".into();
        tick(&state, &sink, &mut wstate);
        assert_eq!(sink.confirms.load(Ordering::SeqCst), 1);
        assert!(!sink.armed());
        assert!(wstate.pending_bridge.is_none());
        tick(&state, &sink, &mut wstate);
        assert_eq!(sink.confirms.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_daemon_restart_mid_install_keeps_waiting_for_the_new_trial() {
        let dir =
            std::env::temp_dir().join(format!("openfluxd_bridge_install_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("config")).unwrap();
        let persistent = || {
            ApiState::with_persistent_stores(
                Arc::new(crate::api::state::NoopDeviceSink),
                None,
                Duration::from_secs(20),
                dir.join("icons"),
                dir.join("config/api_tokens.json"),
                "s".into(),
            )
        };
        let state = persistent();
        let sink = InstallSink::new("openflux-bridge 1.0.0");
        let pending =
            crate::bridge_install::PendingInstall::begin(&sink, "openflux-bridge 1.0.1".into());
        persist_pending_bridge(&state, Some(&pending));
        sink.write(ATTEMPT, "attempt_id=cc03\nversion=openflux-bridge 1.0.1\n");
        sink.write(crate::device_paths::BRIDGE_ROLLBACK_ARMED, "");

        // The restarted daemon reads the started install back and the previous bridge reconnects.
        let state = persistent();
        let mut wstate = WorkerLoopState {
            pending_bridge: load_pending_bridge(&state).map(|p| (p, Instant::now())),
            ..Default::default()
        };
        assert!(wstate.pending_bridge.is_some());
        tick(&state, &sink, &mut wstate);
        assert_eq!(sink.confirms.load(Ordering::SeqCst), 0);

        sink.write(
            &crate::device_paths::bridge_trial_path("cc03"),
            "version=openflux-bridge 1.0.1\npid=7\n",
        );
        *sink.live_bridge.lock().unwrap() = "openflux-bridge 1.0.1".into();
        tick(&state, &sink, &mut wstate);
        assert_eq!(sink.confirms.load(Ordering::SeqCst), 1);
        assert!(
            load_pending_bridge(&state).is_none(),
            "the finished install is no longer recorded"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An attached keyboard whose configuration reads answer `snapshot` (or fail while it is unset).
    struct PullSink {
        snapshot: std::sync::Mutex<Option<serde_json::Value>>,
        pulls: AtomicUsize,
    }

    impl DeviceSink for PullSink {
        fn is_connected(&self) -> (bool, bool) {
            (true, true)
        }
        fn versions(&self) -> serde_json::Value {
            serde_json::json!({"firmware": "openflux-firmware 1.0.0", "bridge": "openflux-bridge 1.0.0", "display": "openflux-display 1.0.0"})
        }
        fn send_live_json(&self, _: &serde_json::Value) -> Result<(), String> {
            Ok(())
        }
        fn set_active_profile(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn set_language(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn set_platform(&self, _: u32) -> Result<(), String> {
            Ok(())
        }
        fn save_config_dir(&self, _: &serde_json::Value) -> Result<(), String> {
            Ok(())
        }
        fn pull_config_dir(&self) -> Result<serde_json::Value, String> {
            self.pulls.fetch_add(1, Ordering::SeqCst);
            self.snapshot
                .lock()
                .unwrap()
                .clone()
                .ok_or_else(|| "the bridge didn't answer".into())
        }
    }

    /// A pulled configuration whose appearance and profiles are good, with `keymap` paired with
    /// the default firmware keymap.
    fn snapshot_with_keymap(keymap: serde_json::Value) -> serde_json::Value {
        let blob = crate::codecs::keymap::encode_keymap_json_to_firmware_blob(
            &crate::defaults::default_keymap(),
            None,
            None,
        )
        .unwrap();
        let ps = crate::codecs::keymap::ProfileSettingsFile {
            settings: vec![crate::codecs::keymap::ProfileSetting::from_layer(0, 0).unwrap()],
            file_version: 0,
            file_type: 0,
        }
        .to_transfer_bytes()
        .unwrap();
        serde_json::json!({
            "appearance": {"selectedId": "default"},
            "profiles": [{"id": "default", "name": "Default", "theme_id": "default", "contexts": ["Main"], "active_context": "Main", "actuation_mode_index": 0, "default_layer": 0}],
            "firmwareProfileSettingsHex": ApiState::encode_hex_bytes(&ps),
            "keymap": keymap,
            "firmwareKeymapBlobHex": ApiState::encode_hex_bytes(&blob),
        })
    }

    fn retry_now(state: &ApiState) {
        state.inner.lock().unwrap().next_pull_retry_at = Some(Instant::now());
    }

    #[test]
    fn a_keymap_that_fails_to_load_is_read_again_until_it_loads() {
        let state = ApiState::new(Arc::new(crate::api::state::NoopDeviceSink), None);
        let mut factory_named = crate::defaults::default_keymap();
        crate::codecs::keycode_names::to_factory(&mut factory_named);
        let sink = PullSink {
            snapshot: std::sync::Mutex::new(Some(snapshot_with_keymap(factory_named.clone()))),
            pulls: AtomicUsize::new(0),
        };
        let mut wstate = WorkerLoopState::default();
        let tick = |wstate: &mut WorkerLoopState| {
            reconcile_tick_with_state(&state, &mut DummyBridge, &sink, 2, wstate);
        };
        state.inner.lock().unwrap().cached_connected = (true, true);
        assert_eq!(
            state.inner.lock().unwrap().keymap_sync()["state"],
            "pulling",
            "nothing has been read yet"
        );

        // Appearance and profiles load; the keymap doesn't, and says why.
        tick(&mut wstate);
        {
            let g = state.inner.lock().unwrap();
            assert!(g.appearance_hydrated);
            assert_eq!(
                g.committed_profile_ids.as_deref(),
                Some(&["default".to_string()][..])
            );
            assert!(!g.keymap_trusted());
            let sync = g.keymap_sync();
            assert_eq!(sync["state"], "mismatch", "{sync}");
            let reason = sync["reason"].as_str().unwrap();
            assert!(reason.contains("KC_FF_PROFILE_CYCLE_NEXT"), "{reason}");
            assert_eq!(
                g.keymap, factory_named,
                "the keyboard's keymap.json is kept as read"
            );
            assert!(g.device_keymap_blob.is_some());
        }
        let pulls = sink.pulls.load(Ordering::SeqCst);

        // The keymap is read again only once the backoff has passed: 2 s, doubling to a minute.
        tick(&mut wstate);
        assert_eq!(sink.pulls.load(Ordering::SeqCst), pulls);
        let mut waits = Vec::new();
        for _ in 0..7 {
            retry_now(&state);
            tick(&mut wstate);
            waits.push(wstate.pull_backoff_secs);
        }
        assert_eq!(waits, [4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(sink.pulls.load(Ordering::SeqCst), pulls + 7);

        // A failed read is an error with its reason; nothing that was read is thrown away.
        *sink.snapshot.lock().unwrap() = None;
        retry_now(&state);
        tick(&mut wstate);
        {
            let g = state.inner.lock().unwrap();
            let sync = g.keymap_sync();
            assert_eq!(sync["state"], "error", "{sync}");
            assert!(sync["reason"]
                .as_str()
                .unwrap()
                .contains("the bridge didn't answer"));
            assert!(g.appearance_hydrated);
        }

        // Once the keyboard answers with a keymap that matches its firmware, the keymap loads.
        *sink.snapshot.lock().unwrap() =
            Some(snapshot_with_keymap(crate::defaults::default_keymap()));
        retry_now(&state);
        tick(&mut wstate);
        {
            let g = state.inner.lock().unwrap();
            assert!(g.keymap_trusted());
            assert_eq!(
                g.keymap_sync(),
                serde_json::json!({"state": "ok", "reason": null})
            );
            assert_eq!(g.keymap_pull_logged, None);
        }
        assert_eq!(wstate.pull_backoff_secs, 0, "the backoff starts over");
        let pulls = sink.pulls.load(Ordering::SeqCst);
        retry_now(&state);
        tick(&mut wstate);
        assert_eq!(
            sink.pulls.load(Ordering::SeqCst),
            pulls,
            "nothing is read again once everything is known"
        );
    }

    #[test]
    fn a_keyboard_that_isnt_attached_has_a_disconnected_keymap() {
        let state = ApiState::new(Arc::new(crate::api::state::NoopDeviceSink), None);
        let mut wstate = WorkerLoopState::default();
        reconcile_tick_with_state(
            &state,
            &mut DummyBridge,
            &crate::api::state::NoopDeviceSink,
            2,
            &mut wstate,
        );
        assert_eq!(
            state.inner.lock().unwrap().keymap_sync(),
            serde_json::json!({"state": "disconnected", "reason": null})
        );
    }
}
