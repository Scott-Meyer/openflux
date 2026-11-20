use crate::bridges::linux_gpu::{choose_gpu, LinuxGpuSampler};
use crate::bridges::net::{format_rate, parse_proc_net_dev, sum_interfaces, RateMeter};
use crate::bridges::poller::Poller;
use crate::bridges::{
    layout_to_language_code, output_with_timeout, ActiveWindowInfo, HardwareMetrics, MediaSnapshot,
    OsSensorBridge, PROBE_TIMEOUT,
};
use std::process::Command;
use std::time::{Duration, Instant};

/// How often the media players are asked what's playing.
#[cfg(target_os = "linux")]
const MEDIA_POLL: Duration = Duration::from_secs(1);

/// How often GPU load is read; GPU counters move more slowly than the worker ticks.
const GPU_POLL: Duration = Duration::from_secs(1);

/// How often the network counters are read: the window their rates are measured over.
const NET_POLL: Duration = Duration::from_secs(1);

pub struct LinuxBridge {
    prev_idle: u64,
    prev_total: u64,
    net: RateMeter,
    net_rates: (u64, u64),
    net_read_at: Option<Instant>,
    gpu: LinuxGpuSampler,
    gpu_last: Option<(Instant, Option<crate::bridges::linux_gpu::GpuSummary>)>,
    media: Option<Poller<MediaSnapshot>>,
}

impl Default for LinuxBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxBridge {
    pub fn new() -> Self {
        Self {
            prev_idle: 0,
            prev_total: 0,
            net: RateMeter::default(),
            net_rates: (0, 0),
            net_read_at: None,
            gpu: LinuxGpuSampler::default(),
            gpu_last: None,
            media: None,
        }
    }

    pub fn parse_proc_stat_cpu(proc_stat: &str, prev_idle: &mut u64, prev_total: &mut u64) -> f32 {
        let Some(first) = proc_stat.lines().find(|l| l.starts_with("cpu ")) else {
            return 0.0;
        };
        let nums: Vec<u64> = first
            .split_whitespace()
            .skip(1)
            .filter_map(|s| s.parse::<u64>().ok())
            .collect();
        if nums.len() < 4 {
            return 0.0;
        }
        let idle = nums[3] + nums.get(4).copied().unwrap_or(0);
        // The fields after `steal` (`guest`, `guest_nice`) are already counted in `user` and `nice`.
        let total: u64 = nums.iter().take(8).sum();
        let dt = total.saturating_sub(*prev_total);
        let di = idle.saturating_sub(*prev_idle);
        *prev_total = total;
        *prev_idle = idle;
        if dt == 0 {
            return 0.0;
        }
        (((dt - di) as f64 / dt as f64) * 100.0).clamp(0.0, 100.0) as f32
    }

    pub fn parse_proc_meminfo_gb(meminfo: &str) -> (f32, f32) {
        let mut total_kb = 0u64;
        let mut avail_kb = 0u64;
        for line in meminfo.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                total_kb = rest
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
                avail_kb = rest
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
            }
        }
        let used_kb = total_kb.saturating_sub(avail_kb);
        let kb_to_gb = 1024.0 * 1024.0;
        (
            (used_kb as f64 / kb_to_gb) as f32,
            (total_kb as f64 / kb_to_gb) as f32,
        )
    }

    /// The total bytes received and sent by the network interfaces with a device behind them.
    fn network_totals() -> Option<(u64, u64)> {
        let text = std::fs::read_to_string("/proc/net/dev").ok()?;
        let ifaces = parse_proc_net_dev(&text);
        Some(sum_interfaces(&ifaces, |name| {
            std::path::Path::new("/sys/class/net")
                .join(name)
                .join("device")
                .exists()
        }))
    }
}

impl OsSensorBridge for LinuxBridge {
    fn sample_hardware(&mut self) -> HardwareMetrics {
        let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
        let cpu = Self::parse_proc_stat_cpu(&stat, &mut self.prev_idle, &mut self.prev_total);
        let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
        let (mem_used_gb, mem_total_gb) = Self::parse_proc_meminfo_gb(&meminfo);
        let now = Instant::now();
        if self
            .net_read_at
            .is_none_or(|t| now.saturating_duration_since(t) >= NET_POLL)
        {
            self.net_read_at = Some(now);
            if let Some((rx, tx)) = Self::network_totals() {
                if let Some(rates) = self.net.update(now, rx, tx) {
                    self.net_rates = rates;
                }
            }
        }
        let gpu_due = self
            .gpu_last
            .as_ref()
            .is_none_or(|(at, _)| now.saturating_duration_since(*at) >= GPU_POLL);
        if gpu_due {
            let readings = self.gpu.sample(now);
            self.gpu_last = Some((now, choose_gpu(&readings)));
        }
        let gpu = self.gpu_last.as_ref().and_then(|(_, g)| g.clone());
        HardwareMetrics {
            cpu_pct: cpu,
            mem_used_gb,
            mem_total_gb,
            gpu_pct: gpu.as_ref().map(|g| g.util_pct).unwrap_or(0.0),
            vram_used_gb: gpu.as_ref().and_then(|g| g.vram_used_gb),
            vram_total_gb: gpu.as_ref().and_then(|g| g.vram_total_gb),
            gpu_unified_mem: gpu.as_ref().map(|g| g.unified).unwrap_or(false),
            net_down: format_rate(self.net_rates.0),
            net_up: format_rate(self.net_rates.1),
        }
    }

    fn active_window(&mut self) -> Option<ActiveWindowInfo> {
        let out = output_with_timeout(
            Command::new("xdotool").args(["getactivewindow", "getwindowname"]),
            PROBE_TIMEOUT,
        )?;
        if !out.status.success() {
            return None;
        }
        let title = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Some(ActiveWindowInfo {
            app_id: title.clone(),
            process_name: String::new(),
            window_title: title,
        })
    }

    fn active_language_code(&mut self) -> Option<u32> {
        // `xkb-switch -p` reports the active group, not the configured layout list. Where it isn't
        // available (many Wayland compositors), apps can report the layout through
        // POST /v1/config/updateInputLanguage.
        if let Some(out) = output_with_timeout(Command::new("xkb-switch").arg("-p"), PROBE_TIMEOUT)
        {
            if out.status.success() {
                let active = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !active.is_empty() {
                    return layout_to_language_code(&active);
                }
            }
        }
        None
    }

    fn media_snapshot(&mut self) -> MediaSnapshot {
        #[cfg(target_os = "linux")]
        let poller = self.media.get_or_insert_with(|| {
            Poller::spawn(
                "openflux-mpris",
                MEDIA_POLL,
                crate::bridges::mpris::MprisState::default,
                |state| state.sample(),
            )
        });
        // Other Unix systems have no MPRIS session bus to ask.
        #[cfg(not(target_os = "linux"))]
        let Some(poller) = self.media.as_ref() else {
            return MediaSnapshot::default();
        };
        poller
            .latest()
            .map(crate::bridges::media_now)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_linux_proc_stat_and_meminfo() {
        let mut pi = 0;
        let mut pt = 0;
        let _ = LinuxBridge::parse_proc_stat_cpu("cpu  100 0 100 800 0 0 0 0", &mut pi, &mut pt);
        let pct = LinuxBridge::parse_proc_stat_cpu("cpu  200 0 200 1400 0 0 0 0", &mut pi, &mut pt);
        assert!((pct - 25.0).abs() < 0.5);
        // Guest time is part of user and nice time, so it isn't counted twice.
        let mut pi = 0;
        let mut pt = 0;
        let _ =
            LinuxBridge::parse_proc_stat_cpu("cpu  100 0 100 800 0 0 0 0 50 0", &mut pi, &mut pt);
        let pct =
            LinuxBridge::parse_proc_stat_cpu("cpu  200 0 200 1400 0 0 0 0 150 0", &mut pi, &mut pt);
        assert!((pct - 25.0).abs() < 0.5, "{pct}");

        let (used, tot) = LinuxBridge::parse_proc_meminfo_gb(
            "MemTotal:       16777216 kB\nMemAvailable:   12582912 kB\n",
        );
        assert!((tot - 16.0).abs() < 0.01);
        assert!((used - 4.0).abs() < 0.01);
    }
}
