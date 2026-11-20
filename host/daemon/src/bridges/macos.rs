use crate::bridges::artwork::ArtworkCache;
use crate::bridges::{
    input_source_to_language_code, output_with_timeout, ActiveWindowInfo, HardwareMetrics,
    MediaSnapshot, OsSensorBridge, PROBE_TIMEOUT,
};
use base64::Engine as _;
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Default, Clone)]
struct CachedSensorTick {
    at: Option<Instant>,
    raw: Option<String>,
}

pub struct MacOsBridge {
    last_hw: HardwareMetrics,
    cached_tick: CachedSensorTick,
    prev_cpu_ticks: Option<[u64; 4]>,
    art: ArtworkCache,
    /// The sensors helper to run; found next to the daemon when not set.
    helper: Option<String>,
}

impl Default for MacOsBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl MacOsBridge {
    pub fn new() -> Self {
        Self {
            last_hw: HardwareMetrics {
                cpu_pct: 0.0,
                mem_used_gb: 0.0,
                mem_total_gb: 0.0,
                gpu_pct: 0.0,
                vram_used_gb: None,
                vram_total_gb: None,
                gpu_unified_mem: false,
                net_down: "0 KB/s".into(),
                net_up: "0 KB/s".into(),
            },
            cached_tick: CachedSensorTick::default(),
            prev_cpu_ticks: None,
            art: ArtworkCache::default(),
            helper: None,
        }
    }

    /// A bridge that runs the sensors helper at `helper`.
    pub fn with_helper(helper: &str) -> Self {
        Self {
            helper: Some(helper.to_string()),
            ..Self::new()
        }
    }

    fn helper_path(helper: &Option<String>) -> String {
        helper.clone().unwrap_or_else(Self::resolve_helper_path)
    }

    pub fn parse_vm_stat_gb(vm_stat_out: &str, total_bytes: u64) -> (f32, f32) {
        let total_gb = (total_bytes as f64) / (1024.0 * 1024.0 * 1024.0);
        let mut page_size: u64 = 16384;
        let mut active: u64 = 0;
        let mut wired: u64 = 0;
        let mut compressed: u64 = 0;
        for line in vm_stat_out.lines() {
            if let Some(pos) = line.find("page size of ") {
                if let Some(val) = line[pos + 13..].split_whitespace().next() {
                    if let Ok(ps) = val.parse::<u64>() {
                        page_size = ps;
                    }
                }
            } else if let Some((k, v)) = line.split_once(':') {
                let num = v.trim().trim_end_matches('.').parse::<u64>().unwrap_or(0);
                if k.contains("Pages active") {
                    active = num;
                } else if k.contains("Pages wired down") {
                    wired = num;
                } else if k.contains("Pages occupied by compressor") {
                    compressed = num;
                }
            }
        }
        let used_bytes = (active + wired + compressed) * page_size;
        let used_gb = (used_bytes as f64) / (1024.0 * 1024.0 * 1024.0);
        (used_gb as f32, total_gb as f32)
    }

    pub fn parse_frontmost_tsv(out: &str) -> Option<ActiveWindowInfo> {
        let parts: Vec<&str> = out.trim().split('\t').collect();
        if parts.is_empty() || parts[0].is_empty() {
            return None;
        }
        Some(ActiveWindowInfo {
            app_id: parts[0].to_string(),
            process_name: parts.get(1).copied().unwrap_or("").to_string(),
            window_title: parts.get(2).copied().unwrap_or("").to_string(),
        })
    }

    /// Parses what `openflux-sensors --once` prints: the frontmost app, the active keyboard input
    /// source (`com.apple.keylayout.*`, not the preferred languages), media and hardware metrics.
    pub fn parse_macos_sensors_json(
        json_str: &str,
    ) -> (
        Option<ActiveWindowInfo>,
        Option<u32>,
        Option<MediaSnapshot>,
        Option<HardwareMetrics>,
    ) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json_str) else {
            return (None, None, None, None);
        };
        let win = v.get("frontmostApp").and_then(|f| {
            let bid = f.get("bundleId").and_then(|x| x.as_str()).unwrap_or("");
            let name = f.get("name").and_then(|x| x.as_str()).unwrap_or("");
            if bid.is_empty() && name.is_empty() {
                None
            } else {
                Some(ActiveWindowInfo {
                    app_id: bid.to_string(),
                    process_name: name.to_string(),
                    window_title: String::new(),
                })
            }
        });
        let lang = v
            .get("inputSource")
            .and_then(|i| i.get("id"))
            .and_then(|x| x.as_str())
            .and_then(input_source_to_language_code);
        let media = v.get("media").and_then(|m| {
            let status = m.get("status").and_then(|x| x.as_str()).unwrap_or("");
            if status != "ok" {
                return None;
            }
            Some(MediaSnapshot {
                title: m
                    .get("title")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                artists: m
                    .get("artist")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                album: m
                    .get("album")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string(),
                length_s: m
                    .get("duration")
                    .and_then(|x| x.as_f64())
                    .unwrap_or(0.0)
                    .max(0.0) as u32,
                played_s: m
                    .get("elapsed")
                    .and_then(|x| x.as_f64())
                    .unwrap_or(0.0)
                    .max(0.0) as u32,
                playing: m.get("playing").and_then(|x| x.as_bool()).unwrap_or(false),
                artwork: None,
            })
        });
        let hw = v.get("hardware").map(|h| {
            let fmt_bps = crate::bridges::net::format_rate;
            let rx_bps = h.get("netRxBps").and_then(|x| x.as_u64()).unwrap_or(0);
            let tx_bps = h.get("netTxBps").and_then(|x| x.as_u64()).unwrap_or(0);
            HardwareMetrics {
                cpu_pct: h.get("cpuPct").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
                mem_used_gb: (h.get("memUsedMB").and_then(|x| x.as_f64()).unwrap_or(0.0) / 1024.0)
                    as f32,
                mem_total_gb: (h.get("memTotalMB").and_then(|x| x.as_f64()).unwrap_or(0.0) / 1024.0)
                    as f32,
                gpu_pct: h.get("gpuPct").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
                vram_used_gb: h
                    .get("vramUsedMB")
                    .and_then(|x| x.as_f64())
                    .map(|m| (m / 1024.0) as f32),
                vram_total_gb: h
                    .get("vramTotalMB")
                    .and_then(|x| x.as_f64())
                    .map(|m| (m / 1024.0) as f32),
                gpu_unified_mem: h
                    .get("gpuUnified")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false),
                net_down: fmt_bps(rx_bps),
                net_up: fmt_bps(tx_bps),
            }
        });
        (win, lang, media, hw)
    }

    fn resolve_helper_path() -> String {
        if let Ok(env_p) = std::env::var("OPENFLUX_MACOS_SENSORS_BIN") {
            if !env_p.is_empty() {
                return env_p;
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                for name in ["openflux-sensors", "macos_sensors"] {
                    let candidate = dir.join(name);
                    if candidate.is_file() {
                        return candidate.to_string_lossy().to_string();
                    }
                }
            }
        }
        "macos_sensors".to_string()
    }

    /// The id the helper gives the current track's artwork; empty when it has none.
    pub fn parse_artwork_id(json_str: &str) -> String {
        serde_json::from_str::<serde_json::Value>(json_str)
            .ok()
            .and_then(|v| {
                v.get("media")?
                    .get("artworkId")?
                    .as_str()
                    .map(str::to_string)
            })
            .unwrap_or_default()
    }

    /// The artwork bytes in what `openflux-sensors --artwork` prints.
    pub fn parse_artwork(json_str: &str) -> Option<Vec<u8>> {
        let v = serde_json::from_str::<serde_json::Value>(json_str).ok()?;
        let b64 = v.get("media")?.get("artwork")?.as_str()?;
        base64::engine::general_purpose::STANDARD.decode(b64).ok()
    }

    /// Asks the helper for the current track's artwork.
    fn query_artwork(helper: &str) -> Option<Vec<u8>> {
        let out = output_with_timeout(Command::new(helper).arg("--artwork"), PROBE_TIMEOUT)
            .filter(|o| o.status.success())?;
        Self::parse_artwork(&String::from_utf8_lossy(&out.stdout))
    }

    fn query_macos_sensors_helper(&mut self) -> Option<String> {
        if let Some(at) = self.cached_tick.at {
            if at.elapsed() < Duration::from_millis(500) {
                return self.cached_tick.raw.clone();
            }
        }
        let helper = Self::helper_path(&self.helper);
        let raw = output_with_timeout(Command::new(helper).arg("--once"), PROBE_TIMEOUT)
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string());
        self.cached_tick = CachedSensorTick {
            at: Some(Instant::now()),
            raw: raw.clone(),
        };
        raw
    }
}

impl OsSensorBridge for MacOsBridge {
    fn sample_hardware(&mut self) -> HardwareMetrics {
        if let Some(raw) = self.query_macos_sensors_helper() {
            let (_, _, _, hw) = Self::parse_macos_sensors_json(&raw);
            if let Some(mut h) = hw {
                let mut resolved_cpu: Option<f32> = None;
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                    if let Some(arr) = v
                        .get("hardware")
                        .and_then(|x| x.get("cpuTicks"))
                        .and_then(|x| x.as_array())
                    {
                        if arr.len() >= 4 {
                            if let (Some(u), Some(s), Some(i), Some(n)) = (
                                arr[0].as_u64(),
                                arr[1].as_u64(),
                                arr[2].as_u64(),
                                arr[3].as_u64(),
                            ) {
                                let cur = [u, s, i, n];
                                if let Some(prev) = self.prev_cpu_ticks {
                                    // All four counters must move forward before a delta is computed.
                                    if cur[0] >= prev[0]
                                        && cur[1] >= prev[1]
                                        && cur[2] >= prev[2]
                                        && cur[3] >= prev[3]
                                    {
                                        let du = cur[0] - prev[0];
                                        let ds = cur[1] - prev[1];
                                        let di = cur[2] - prev[2];
                                        let dn = cur[3] - prev[3];
                                        let tot = du + ds + di + dn;
                                        if tot > 0 {
                                            resolved_cpu = Some(
                                                (((du + ds + dn) as f64 * 100.0) / (tot as f64))
                                                    .clamp(0.0, 100.0)
                                                    as f32,
                                            );
                                        }
                                        self.prev_cpu_ticks = Some(cur);
                                    } else {
                                        // A counter went backwards (a reset, even of one counter): re-anchor without a delta.
                                        self.prev_cpu_ticks = Some(cur);
                                    }
                                } else {
                                    self.prev_cpu_ticks = Some(cur);
                                    if let Some(c) = v
                                        .get("hardware")
                                        .and_then(|x| x.get("cpuPct"))
                                        .and_then(|x| x.as_f64())
                                    {
                                        resolved_cpu = Some(c as f32);
                                    }
                                }
                            }
                        }
                    } else if let Some(c) = v
                        .get("hardware")
                        .and_then(|x| x.get("cpuPct"))
                        .and_then(|x| x.as_f64())
                    {
                        resolved_cpu = Some(c as f32);
                    }
                }
                h.cpu_pct = resolved_cpu.unwrap_or(self.last_hw.cpu_pct);
                self.last_hw = h;
                return self.last_hw.clone();
            }
        }
        let total_bytes = output_with_timeout(
            Command::new("sysctl").args(["-n", "hw.memsize"]),
            PROBE_TIMEOUT,
        )
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<u64>()
                .ok()
        })
        .unwrap_or(16 * 1024 * 1024 * 1024);

        if let Some(vm) = output_with_timeout(&mut Command::new("vm_stat"), PROBE_TIMEOUT) {
            let txt = String::from_utf8_lossy(&vm.stdout);
            let (used, tot) = Self::parse_vm_stat_gb(&txt, total_bytes);
            self.last_hw.mem_used_gb = used;
            self.last_hw.mem_total_gb = tot;
        }

        if let Some(top) =
            output_with_timeout(Command::new("ps").args(["-A", "-o", "%cpu"]), PROBE_TIMEOUT)
        {
            let txt = String::from_utf8_lossy(&top.stdout);
            let sum: f32 = txt
                .lines()
                .skip(1)
                .filter_map(|l| l.trim().parse::<f32>().ok())
                .sum();
            let cpus = std::thread::available_parallelism()
                .map(|n| n.get() as f32)
                .unwrap_or(8.0);
            self.last_hw.cpu_pct = (sum / cpus).clamp(0.0, 100.0);
        }
        self.last_hw.clone()
    }

    fn active_window(&mut self) -> Option<ActiveWindowInfo> {
        if let Some(raw) = self.query_macos_sensors_helper() {
            let (win, _, _, _) = Self::parse_macos_sensors_json(&raw);
            if win.is_some() {
                return win;
            }
        }
        let script = r#"tell application "System Events"
set fp to first application process whose frontmost is true
set bid to bundle identifier of fp
set pname to name of fp
return bid & tab & pname
end tell"#;
        let out = output_with_timeout(
            Command::new("osascript").args(["-e", script]),
            PROBE_TIMEOUT,
        )?;
        if !out.status.success() {
            return None;
        }
        Self::parse_frontmost_tsv(&String::from_utf8_lossy(&out.stdout))
    }

    fn active_language_code(&mut self) -> Option<u32> {
        // The input source the sensors helper reports. Without the helper the language is
        // unknown; it isn't guessed from the configured input sources.
        let raw = self.query_macos_sensors_helper()?;
        let (_, lang, _, _) = Self::parse_macos_sensors_json(&raw);
        lang
    }

    fn display_asleep(&mut self) -> bool {
        let Some(raw) = self.query_macos_sensors_helper() else {
            return false;
        };
        serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .and_then(|v| v.get("displayAsleep").and_then(|x| x.as_bool()))
            .unwrap_or(false)
    }

    fn media_snapshot(&mut self) -> MediaSnapshot {
        if let Some(raw) = self.query_macos_sensors_helper() {
            let (_, _, media, _) = Self::parse_macos_sensors_json(&raw);
            if let Some(mut m) = media {
                // The artwork is fetched once per id, by a second run of the helper.
                let art_id = Self::parse_artwork_id(&raw);
                let helper = &self.helper;
                m.artwork = self
                    .art
                    .resolve(&art_id, || Self::query_artwork(&Self::helper_path(helper)));
                return m;
            }
        }
        MediaSnapshot::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn the_helpers_artwork_is_fetched_once_per_track_and_sent_with_it() {
        use base64::Engine as _;
        let dir = std::env::temp_dir().join(format!("openflux-macos-art-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = base64::engine::general_purpose::STANDARD
            .encode(crate::bridges::artwork::test_png(600, 600));
        let fetches = dir.join("fetches");
        let helper = dir.join("openflux-sensors");
        std::fs::write(
            &helper,
            format!(
                "#!/bin/sh\n\
                 if [ \"$1\" = --artwork ]; then\n\
                 echo x >> '{fetches}'\n\
                 echo '{{\"media\":{{\"artworkId\":\"a1\",\"artwork\":\"{png}\"}}}}'\n\
                 else\n\
                 echo '{{\"media\":{{\"status\":\"ok\",\"title\":\"Song\",\"playing\":true,\"duration\":200,\"elapsed\":4,\"artworkId\":\"a1\"}}}}'\n\
                 fi\n",
                fetches = fetches.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut bridge = MacOsBridge::with_helper(helper.to_str().unwrap());
        let first = bridge.media_snapshot();
        assert_eq!(first.title, "Song");
        let art = first.artwork.expect("artwork");
        bridge.cached_tick = CachedSensorTick::default();
        assert_eq!(bridge.media_snapshot().artwork, Some(art));
        let fetched = std::fs::read_to_string(&fetches).unwrap();
        assert_eq!(fetched.lines().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_macos_vm_stat_frontmost_and_hitoolbox_source() {
        let sample = "\
Mach Virtual Memory Statistics: (page size of 16384 bytes)
Pages free:                               10000.
Pages active:                            262144.
Pages inactive:                          100000.
Pages wired down:                        131072.
Pages occupied by compressor:            131072.";
        let (used, tot) = MacOsBridge::parse_vm_stat_gb(sample, 32 * 1024 * 1024 * 1024);
        assert!((tot - 32.0).abs() < 0.01);
        assert!((used - 8.0).abs() < 0.1);

        let w = MacOsBridge::parse_frontmost_tsv("com.microsoft.VSCode\tCode\tmain.rs\n").unwrap();
        assert_eq!(w.app_id, "com.microsoft.VSCode");

        // The schema openflux_sensors.swift prints.
        let sensor_json = r#"{
            "frontmostApp":{"bundleId":"com.microsoft.VSCode","name":"Code","pid":1234},
            "hardware":{"batteryPct":88,"cpuPct":14.2,"gpuPct":31.5,"memTotalMB":32768,"memUsedMB":16384,"netRxBps":2097152,"netTxBps":65536},
            "inputSource":{"id":"com.apple.inputmethod.Kotoeri.KanaTyping.Japanese","language":"ja"},
            "media":{"album":"Alb","artist":"Art","duration":200.0,"elapsed":45.0,"playing":true,"status":"ok","title":"Song"}
        }"#;
        let (sw, sl, sm, shw) = MacOsBridge::parse_macos_sensors_json(sensor_json);
        assert_eq!(sw.unwrap().app_id, "com.microsoft.VSCode");
        assert_eq!(sl, Some(226));
        assert_eq!(sm.unwrap().title, "Song");
        let hw = shw.unwrap();
        assert!((hw.cpu_pct - 14.2).abs() < 0.01);
        assert!((hw.gpu_pct - 31.5).abs() < 0.01);
        assert_eq!(hw.net_down, "2.0 MB/s");
        assert_eq!(hw.net_up, "64 KB/s");

        // The track's artwork is named in every sample and fetched by id.
        let with_art =
            r#"{"media":{"status":"ok","title":"Song","playing":true,"artworkId":"9f3a-1200"}}"#;
        assert_eq!(MacOsBridge::parse_artwork_id(with_art), "9f3a-1200");
        assert_eq!(MacOsBridge::parse_artwork_id(sensor_json), "");
        assert_eq!(
            MacOsBridge::parse_artwork(r#"{"media":{"artworkId":"x","artwork":"AAEC"}}"#),
            Some(vec![0, 1, 2])
        );
        assert_eq!(
            MacOsBridge::parse_artwork(r#"{"media":{"artworkId":""}}"#),
            None
        );

        let unknown_tis = r#"{"inputSource":{"id":"com.custom.MyUnknownIME"}}"#;
        let (_, ul, _, _) = MacOsBridge::parse_macos_sensors_json(unknown_tis);
        assert_eq!(ul, None);

        // CPU usage comes from how far the cpuTicks counters moved since the previous poll.
        let mut br = MacOsBridge::new();
        let make_tick = |u: u64, s: u64, i: u64, n: u64| {
            format!(
                r#"{{"hardware":{{"cpuTicks":[{u},{s},{i},{n}],"gpuPct":0,"memTotalMB":16384,"memUsedMB":4096,"netRxBps":0,"netTxBps":0}}}}"#
            )
        };
        let apply_tick = |b: &mut MacOsBridge, raw: String| {
            b.cached_tick = CachedSensorTick {
                at: Some(Instant::now()),
                raw: Some(raw),
            };
            b.sample_hardware().cpu_pct
        };
        assert_eq!(apply_tick(&mut br, make_tick(100, 50, 850, 0)), 0.0);
        // A quarter of the ticks were busy: 25 %.
        assert!((apply_tick(&mut br, make_tick(125, 50, 925, 0)) - 25.0).abs() < 0.1);
        // Ticks that didn't move keep the last reading (25 %).
        assert!((apply_tick(&mut br, make_tick(125, 50, 925, 0)) - 25.0).abs() < 0.1);
        // A sample without cpuTicks keeps the last reading.
        let missing_cpu = r#"{"hardware":{"gpuPct":0,"memTotalMB":16384,"memUsedMB":4096,"netRxBps":0,"netTxBps":0}}"#.to_string();
        assert!((apply_tick(&mut br, missing_cpu) - 25.0).abs() < 0.1);
        // A malformed counter ("bad" or null) keeps the last reading and the previous baseline.
        let malformed = r#"{"hardware":{"cpuTicks":[130,null,"bad",0],"gpuPct":0,"memTotalMB":16384,"memUsedMB":4096,"netRxBps":0,"netTxBps":0}}"#.to_string();
        assert!((apply_tick(&mut br, malformed) - 25.0).abs() < 0.1);
        assert_eq!(br.prev_cpu_ticks, Some([125, 50, 925, 0]));
        // A counter that went backwards (user 125 to 10) starts a new baseline instead of a bogus
        // reading.
        assert!((apply_tick(&mut br, make_tick(10, 50, 1000, 0)) - 25.0).abs() < 0.1);
        // From the new baseline only idle ticks advance: 0 %.
        assert_eq!(apply_tick(&mut br, make_tick(10, 50, 1100, 0)), 0.0);
    }
}
