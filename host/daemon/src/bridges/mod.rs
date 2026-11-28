pub mod artwork;
pub mod linux;
pub mod linux_gpu;
pub mod macos;
pub mod mpris;
pub mod net;
pub mod nvml;
pub mod poller;
pub mod smtc;
pub mod weather;
pub mod windows;
pub mod windows_gpu;

pub use artwork::MediaArtwork;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How long a sensor probe (a helper or a system command) may run. Probes run on the USB worker's
/// thread, so one that hangs must not stall keyboard work.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Runs `cmd` like [`Command::output`], but kills and reaps it when it runs longer than
/// `timeout`. `None` when it couldn't be started, timed out, or its output couldn't be read.
pub fn output_with_timeout(cmd: &mut Command, timeout: Duration) -> Option<Output> {
    use std::io::Read;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Read on another thread so a child that fills the pipe still exits.
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    // A process the child left running can hold the pipe open; it doesn't hold up the caller.
    let stdout = rx
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .ok()?;
    Some(Output {
        status,
        stdout,
        stderr: Vec::new(),
    })
}

static INPUT_LANG_MAP: OnceLock<HashMap<String, u32>> = OnceLock::new();

fn input_language_map() -> &'static HashMap<String, u32> {
    INPUT_LANG_MAP.get_or_init(|| {
        let raw = include_str!("../../data/input_language_codes.json");
        serde_json::from_str::<HashMap<String, u32>>(raw).unwrap_or_default()
    })
}

/// Maps an input source identifier (e.g. macOS TIS `com.apple.keylayout.US`) or a layout name to
/// the firmware's language code from `input_language_codes.json`. Unknown sources return `None`
/// rather than a guessed code.
pub fn input_source_to_language_code(source_id: &str) -> Option<u32> {
    let map = input_language_map();
    if let Some(&code) = map.get(source_id) {
        return Some(code);
    }
    let key_cand = format!("com.apple.keylayout.{source_id}");
    map.get(&key_cand).copied()
}

/// Maps a keyboard layout name or BCP-47 tag (`en-US`, `de`, `fr`, ...) to the firmware's language
/// code. Returns `None` when unknown.
pub fn layout_to_language_code(tag: &str) -> Option<u32> {
    if let Some(c) = input_source_to_language_code(tag) {
        return Some(c);
    }
    let low = tag.to_lowercase();
    let canonical = if low == "en-us" || low == "en_us" || low == "us" {
        "com.apple.keylayout.US"
    } else if low == "en-gb" || low == "en_gb" || low == "gb" || low == "uk" {
        "com.apple.keylayout.British"
    } else if low.starts_with("de") {
        "com.apple.keylayout.German"
    } else if low.starts_with("fr") {
        "com.apple.keylayout.French"
    } else if low.starts_with("es") {
        "com.apple.keylayout.Spanish"
    } else if low.starts_with("ja") {
        "com.apple.inputmethod.Kotoeri.KanaTyping.Japanese"
    } else {
        return None;
    };
    input_source_to_language_code(canonical)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HardwareMetrics {
    pub cpu_pct: f32,
    pub mem_used_gb: f32,
    pub mem_total_gb: f32,
    pub gpu_pct: f32,
    #[serde(default)]
    pub vram_used_gb: Option<f32>,
    #[serde(default)]
    pub vram_total_gb: Option<f32>,
    #[serde(default)]
    pub gpu_unified_mem: bool,
    pub net_down: String,
    pub net_up: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaSnapshot {
    pub title: String,
    pub artists: String,
    pub album: String,
    pub length_s: u32,
    pub played_s: u32,
    pub playing: bool,
    /// The track's album art, when the player has one.
    #[serde(default)]
    pub artwork: Option<MediaArtwork>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActiveWindowInfo {
    pub app_id: String,
    pub process_name: String,
    pub window_title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppProfileRule {
    pub matcher: String, // bundle id, exe name, or substring
    pub profile_index: u32,
    pub language_code: Option<u32>,
}

pub trait OsSensorBridge: Send {
    fn sample_hardware(&mut self) -> HardwareMetrics;
    fn active_window(&mut self) -> Option<ActiveWindowInfo>;
    fn active_language_code(&mut self) -> Option<u32>;
    fn media_snapshot(&mut self) -> MediaSnapshot;
    fn display_asleep(&mut self) -> bool {
        false
    }
}

/// A media sample taken a moment ago, brought up to now: a playing track's position has moved on
/// by the time since the sample.
pub fn media_now(sample: poller::Sampled<MediaSnapshot>) -> MediaSnapshot {
    let mut snap = sample.value;
    if snap.playing {
        let moved = sample.at.elapsed().as_secs().min(u32::MAX as u64) as u32;
        snap.played_s = snap.played_s.saturating_add(moved);
        if snap.length_s > 0 {
            snap.played_s = snap.played_s.min(snap.length_s);
        }
    }
    snap
}

pub fn match_app_rule<'a>(
    rules: &'a [AppProfileRule],
    win: &ActiveWindowInfo,
) -> Option<&'a AppProfileRule> {
    let app_low = win.app_id.to_lowercase();
    let proc_low = win.process_name.to_lowercase();
    let title_low = win.window_title.to_lowercase();
    rules.iter().find(|r| {
        let m = r.matcher.to_lowercase();
        !m.is_empty() && (app_low.contains(&m) || proc_low.contains(&m) || title_low.contains(&m))
    })
}

/// The live data sent to the keyboard. Media is always included: a snapshot with no title is sent
/// as a stopped, empty track, so the keyboard clears the previous one when the player closes.
pub fn build_live_data_json(
    hw: &HardwareMetrics,
    media: &MediaSnapshot,
    weather: Option<&weather::WeatherSnapshot>,
) -> serde_json::Value {
    let mut hw_obj = serde_json::json!({
        "cpu": hw.cpu_pct.round() as u32,
        "memory": (hw.mem_used_gb * 10.0).round() / 10.0,
        "totalMemory": hw.mem_total_gb.round() as u32,
        "gpuUsage": hw.gpu_pct.round() as u32,
        "gpuUnified": hw.gpu_unified_mem,
        "netDown": hw.net_down,
        "netUp": hw.net_up,
    });
    if let Some(vu) = hw.vram_used_gb {
        hw_obj["vramUsed"] = serde_json::json!((vu * 10.0).round() / 10.0);
    } else {
        hw_obj["vramUsed"] = serde_json::Value::Null;
    }
    if let Some(vt) = hw.vram_total_gb {
        hw_obj["totalVram"] = serde_json::json!((vt * 10.0).round() / 10.0);
    } else {
        hw_obj["totalVram"] = serde_json::Value::Null;
    }
    let mut obj = serde_json::json!({
        "hardwareData": hw_obj
    });
    obj["mediaFile"] = if media.title.is_empty() {
        serde_json::json!({
            "title": "",
            "artists": "",
            "album": "",
            "length": 0,
            "played": 0,
            "playing": false,
            "base64Image": "",
            "trackId": "",
        })
    } else {
        let art = media.artwork.as_ref();
        serde_json::json!({
            "title": media.title,
            "artists": media.artists,
            "album": media.album,
            "length": media.length_s,
            "played": media.played_s,
            "playing": media.playing,
            "base64Image": art.map(|a| a.base64_jpeg.as_str()).unwrap_or(""),
            "trackId": art.map(|a| a.id.as_str()).unwrap_or(""),
        })
    };
    if let Some(w) = weather {
        let hourly_json: Vec<serde_json::Value> = w
            .hourly
            .iter()
            .take(3)
            .map(|h| {
                serde_json::json!({
                    "hour": h.hour_label,
                    "temp": h.temp,
                    "condition": h.condition,
                    "isDay": h.is_day,
                })
            })
            .collect();
        obj["weather"] = serde_json::json!({
            "city": w.city,
            "temp": w.temp,
            "high": w.max_temp,
            "low": w.min_temp,
            "condition": w.condition,
            "isDay": w.is_day,
            "celsius": w.celsius,
            "ready": w.ready,
            "sunrise": w.sunrise,
            "sunset": w.sunset,
            "aqi": w.aqi,
            "utcOffsetSec": w.utc_offset_seconds,
            "hourly": hourly_json,
        });
    }
    obj
}

pub fn create_platform_bridge() -> Box<dyn OsSensorBridge> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacOsBridge::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxBridge::new())
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsBridge::new())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Box::new(linux::LinuxBridge::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_rule_matching_and_verified_wire_language_codes() {
        let rules = vec![AppProfileRule {
            matcher: "com.microsoft.VSCode".into(),
            profile_index: 2,
            language_code: Some(334),
        }];
        let w = ActiveWindowInfo {
            app_id: "com.microsoft.VSCode".into(),
            process_name: "Code".into(),
            window_title: "main.rs".into(),
        };
        let r = match_app_rule(&rules, &w).unwrap();
        assert_eq!(r.profile_index, 2);
        assert_eq!(r.language_code, Some(334));

        // Firmware language codes from data/input_language_codes.json.
        assert_eq!(
            input_source_to_language_code("com.apple.keylayout.US"),
            Some(471)
        );
        assert_eq!(
            input_source_to_language_code("com.apple.keylayout.German"),
            Some(334)
        );
        assert_eq!(
            input_source_to_language_code("com.apple.keylayout.French"),
            Some(329)
        );
        assert_eq!(
            input_source_to_language_code("unknown.layout.Invalid"),
            None
        );
        assert_eq!(layout_to_language_code("en-US"), Some(471));
        assert_eq!(layout_to_language_code("de-DE"), Some(334));
        assert_eq!(layout_to_language_code("zz-ZZ"), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_probe_that_hangs_is_killed_at_its_timeout() {
        let out = output_with_timeout(
            Command::new("sh").args(["-c", "echo ready"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ready");

        let started = Instant::now();
        let hung = output_with_timeout(
            Command::new("sh").args(["-c", "exec sleep 30"]),
            Duration::from_millis(200),
        );
        assert!(hung.is_none());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(output_with_timeout(
            &mut Command::new("/nonexistent/keyglass-probe"),
            Duration::from_secs(1)
        )
        .is_none());
    }

    #[test]
    fn a_playing_track_moves_on_from_its_sample_and_a_paused_one_doesnt() {
        let track = MediaSnapshot {
            title: "Track".into(),
            length_s: 100,
            played_s: 40,
            playing: true,
            ..Default::default()
        };
        let at = Instant::now() - Duration::from_secs(3);
        let moved = media_now(poller::Sampled {
            value: track.clone(),
            at,
        });
        assert_eq!(moved.played_s, 43);
        let near_end = MediaSnapshot {
            played_s: 99,
            ..track.clone()
        };
        assert_eq!(
            media_now(poller::Sampled {
                value: near_end,
                at
            })
            .played_s,
            100
        );
        let paused = MediaSnapshot {
            playing: false,
            ..track
        };
        assert_eq!(
            media_now(poller::Sampled { value: paused, at }).played_s,
            40
        );
    }

    #[test]
    fn a_closed_player_is_sent_as_a_stopped_empty_track() {
        let hw = HardwareMetrics {
            cpu_pct: 0.0,
            mem_used_gb: 0.0,
            mem_total_gb: 0.0,
            gpu_pct: 0.0,
            vram_used_gb: None,
            vram_total_gb: None,
            gpu_unified_mem: false,
            net_down: String::new(),
            net_up: String::new(),
        };
        let playing = MediaSnapshot {
            title: "Track".into(),
            artists: "Artist".into(),
            album: "Album".into(),
            length_s: 100,
            played_s: 40,
            playing: true,
            artwork: Some(MediaArtwork {
                id: "0123abcd".into(),
                base64_jpeg: "/9j/AA==".into(),
            }),
        };
        let live = build_live_data_json(&hw, &playing, None);
        assert_eq!(live["mediaFile"]["title"], "Track");
        assert_eq!(live["mediaFile"]["playing"], true);
        assert_eq!(live["mediaFile"]["base64Image"], "/9j/AA==");
        assert_eq!(live["mediaFile"]["trackId"], "0123abcd");
        let no_art = MediaSnapshot {
            artwork: None,
            ..playing.clone()
        };
        let live = build_live_data_json(&hw, &no_art, None);
        assert_eq!(live["mediaFile"]["base64Image"], "");
        assert_eq!(live["mediaFile"]["trackId"], "");
        // A player that closed leaves stale fields behind; none of them reach the keyboard.
        let closed = MediaSnapshot {
            title: String::new(),
            ..playing
        };
        let live = build_live_data_json(&hw, &closed, None);
        assert_eq!(
            live["mediaFile"],
            serde_json::json!({"title": "", "artists": "", "album": "", "length": 0, "played": 0, "playing": false, "base64Image": "", "trackId": ""})
        );
    }
}
