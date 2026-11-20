//! GPU load and video memory on Linux: AMD and Intel GPUs from sysfs, NVIDIA GPUs through NVML.
//!
//! A GPU that the kernel has put to sleep (runtime power management, common for the discrete GPU
//! of a laptop) isn't queried: reading its counters would wake it.

use std::path::{Path, PathBuf};
use std::time::Instant;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Below this much dedicated memory a GPU is treated as integrated: its memory is the system's.
const DISCRETE_VRAM_BYTES: u64 = 1024 * 1024 * 1024;

/// What one GPU reports.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuReading {
    pub util_pct: f32,
    /// Dedicated memory used and total, in bytes, for a GPU with its own memory.
    pub vram: Option<(u64, u64)>,
    /// The GPU shares the system's memory.
    pub unified: bool,
    /// The GPU is powered; a sleeping GPU reports no load.
    pub awake: bool,
}

/// The GPU the keyboard shows, in the units of [`super::HardwareMetrics`].
#[derive(Debug, Clone, PartialEq)]
pub struct GpuSummary {
    pub util_pct: f32,
    pub vram_used_gb: Option<f32>,
    pub vram_total_gb: Option<f32>,
    pub unified: bool,
}

/// Picks the GPU to show: an awake one before a sleeping one, and among those the one with the
/// most dedicated memory (the discrete GPU of a machine that has two).
pub fn choose_gpu(readings: &[GpuReading]) -> Option<GpuSummary> {
    let best = readings.iter().max_by_key(|r| {
        (
            r.awake,
            r.vram.map(|(_, total)| total).unwrap_or(0),
            (r.util_pct * 10.0) as u32,
        )
    })?;
    let vram = best.vram.filter(|_| !best.unified);
    Some(GpuSummary {
        util_pct: best.util_pct.clamp(0.0, 100.0),
        vram_used_gb: vram.map(|(used, _)| (used as f64 / GIB) as f32),
        vram_total_gb: vram.map(|(_, total)| (total as f64 / GIB) as f32),
        unified: best.unified,
    })
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
}

fn read_u64(p: &Path) -> Option<u64> {
    read_trim(p)?.parse().ok()
}

/// Whether the kernel has suspended the device at `device_dir`.
fn is_suspended(device_dir: &Path) -> bool {
    read_trim(&device_dir.join("power/runtime_status")).as_deref() == Some("suspended")
}

/// A DRM card of the sysfs tree: `/sys/class/drm/cardN`.
#[derive(Debug, Clone)]
struct Card {
    dir: PathBuf,
    driver: String,
}

fn list_cards(drm_root: &Path) -> Vec<Card> {
    let Ok(entries) = std::fs::read_dir(drm_root) else {
        return Vec::new();
    };
    let mut cards: Vec<Card> = entries
        .filter_map(|e| {
            let e = e.ok()?;
            let name = e.file_name().to_string_lossy().to_string();
            let n = name.strip_prefix("card")?;
            if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let dir = e.path();
            let driver = std::fs::read_link(dir.join("device/driver"))
                .ok()?
                .file_name()?
                .to_string_lossy()
                .to_string();
            Some(Card { dir, driver })
        })
        .collect();
    cards.sort_by(|a, b| a.dir.cmp(&b.dir));
    cards
}

/// Samples the machine's GPUs. Keeps what a load figure needs between samples (Intel's idle
/// counters) and NVML once it's loaded.
pub struct LinuxGpuSampler {
    drm_root: PathBuf,
    nvidia_pci_root: PathBuf,
    /// Per Intel card: the idle residency in ms and when it was read.
    idle_prev: Vec<(PathBuf, u64, Instant)>,
    #[cfg(target_os = "linux")]
    nvml: Option<super::nvml::Nvml>,
    #[cfg(target_os = "linux")]
    nvml_tried_at: Option<Instant>,
}

impl Default for LinuxGpuSampler {
    fn default() -> Self {
        Self::with_roots(
            "/sys/class/drm".into(),
            "/sys/bus/pci/drivers/nvidia".into(),
        )
    }
}

impl LinuxGpuSampler {
    pub fn with_roots(drm_root: PathBuf, nvidia_pci_root: PathBuf) -> Self {
        Self {
            drm_root,
            nvidia_pci_root,
            idle_prev: Vec::new(),
            #[cfg(target_os = "linux")]
            nvml: None,
            #[cfg(target_os = "linux")]
            nvml_tried_at: None,
        }
    }

    /// What every GPU reports now.
    pub fn sample(&mut self, now: Instant) -> Vec<GpuReading> {
        let mut out = Vec::new();
        for card in list_cards(&self.drm_root) {
            let device = card.dir.join("device");
            let awake = !is_suspended(&device);
            match card.driver.as_str() {
                "amdgpu" => {
                    if !awake {
                        out.push(GpuReading {
                            util_pct: 0.0,
                            vram: None,
                            unified: false,
                            awake,
                        });
                        continue;
                    }
                    let util = read_u64(&device.join("gpu_busy_percent")).unwrap_or(0) as f32;
                    let total = read_u64(&device.join("mem_info_vram_total")).unwrap_or(0);
                    let used = read_u64(&device.join("mem_info_vram_used")).unwrap_or(0);
                    let unified = total < DISCRETE_VRAM_BYTES;
                    out.push(GpuReading {
                        util_pct: util,
                        vram: (total > 0).then_some((used.min(total), total)),
                        unified,
                        awake,
                    });
                }
                "i915" | "xe" => {
                    let util = if awake {
                        self.intel_busy_pct(&card.dir, now)
                    } else {
                        0.0
                    };
                    out.push(GpuReading {
                        util_pct: util,
                        vram: None,
                        unified: true,
                        awake,
                    });
                }
                _ => {}
            }
        }
        out.extend(self.nvidia_readings(now));
        out
    }

    /// Intel GPUs report how long they've been idle (RC6 residency); the load is the share of
    /// the time since the last sample they weren't.
    fn intel_busy_pct(&mut self, card_dir: &Path, now: Instant) -> f32 {
        let candidates = [
            "gt/gt0/rc6_residency_ms",
            "power/rc6_residency_ms",
            "device/tile0/gt0/gtidle/idle_residency_ms",
        ];
        let Some(idle_ms) = candidates
            .iter()
            .find_map(|rel| read_u64(&card_dir.join(rel)))
        else {
            return 0.0;
        };
        let prev = self.idle_prev.iter_mut().find(|(d, _, _)| d == card_dir);
        match prev {
            Some((_, prev_ms, prev_at)) => {
                let wall_ms = now.saturating_duration_since(*prev_at).as_secs_f64() * 1000.0;
                let idle_delta = idle_ms.saturating_sub(*prev_ms) as f64;
                *prev_ms = idle_ms;
                *prev_at = now;
                if wall_ms <= 0.0 {
                    return 0.0;
                }
                (100.0 * (1.0 - idle_delta / wall_ms)).clamp(0.0, 100.0) as f32
            }
            None => {
                self.idle_prev.push((card_dir.to_path_buf(), idle_ms, now));
                0.0
            }
        }
    }

    /// Whether the NVIDIA driver has a GPU bound, and whether any of them is awake.
    fn nvidia_state(&self) -> (bool, bool) {
        let Ok(entries) = std::fs::read_dir(&self.nvidia_pci_root) else {
            return (false, false);
        };
        let mut any = false;
        let mut awake = false;
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            // Bound devices are the entries named by PCI address, such as `0000:01:00.0`.
            if name.matches(':').count() != 2 {
                continue;
            }
            any = true;
            awake |= !is_suspended(&e.path());
        }
        (any, awake)
    }

    #[cfg(target_os = "linux")]
    fn nvidia_readings(&mut self, now: Instant) -> Vec<GpuReading> {
        let (present, awake) = self.nvidia_state();
        if !present {
            return Vec::new();
        }
        if !awake {
            return vec![GpuReading {
                util_pct: 0.0,
                vram: None,
                unified: false,
                awake: false,
            }];
        }
        let retry_due = self
            .nvml_tried_at
            .is_none_or(|t| now.saturating_duration_since(t).as_secs() >= 60);
        if self.nvml.is_none() && retry_due {
            self.nvml_tried_at = Some(now);
            self.nvml = super::nvml::Nvml::load();
        }
        self.nvml
            .as_ref()
            .map(|n| {
                n.sample()
                    .into_iter()
                    .map(|r| GpuReading {
                        util_pct: r.util_pct,
                        vram: Some((r.vram_used_bytes, r.vram_total_bytes)),
                        unified: false,
                        awake: true,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[cfg(not(target_os = "linux"))]
    fn nvidia_readings(&mut self, _now: Instant) -> Vec<GpuReading> {
        let (present, awake) = self.nvidia_state();
        if present {
            vec![GpuReading {
                util_pct: 0.0,
                vram: None,
                unified: false,
                awake,
            }]
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "openflux-gpu-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn file(&self, rel: &str, contents: &str) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, contents).unwrap();
        }

        #[cfg(unix)]
        fn card(&self, n: u32, driver: &str) {
            let drivers = self.0.join("drivers").join(driver);
            std::fs::create_dir_all(&drivers).unwrap();
            let device = self.0.join(format!("drm/card{n}/device"));
            std::fs::create_dir_all(&device).unwrap();
            std::os::unix::fs::symlink(&drivers, device.join("driver")).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_shown_gpu_is_an_awake_one_with_the_most_dedicated_memory() {
        let igpu = GpuReading {
            util_pct: 30.0,
            vram: None,
            unified: true,
            awake: true,
        };
        let dgpu = GpuReading {
            util_pct: 94.0,
            vram: Some((8 * 1024 * 1024 * 1024, 10 * 1024 * 1024 * 1024)),
            unified: false,
            awake: true,
        };
        let shown = choose_gpu(&[igpu.clone(), dgpu.clone()]).unwrap();
        assert_eq!(shown.util_pct, 94.0);
        assert_eq!(shown.vram_used_gb, Some(8.0));
        assert_eq!(shown.vram_total_gb, Some(10.0));
        assert!(!shown.unified);

        // A sleeping discrete GPU gives way to the integrated one that's drawing the screen.
        let asleep = GpuReading {
            util_pct: 0.0,
            vram: None,
            unified: false,
            awake: false,
        };
        let shown = choose_gpu(&[asleep, igpu]).unwrap();
        assert_eq!(shown.util_pct, 30.0);
        assert!(shown.unified);
        assert_eq!(shown.vram_total_gb, None);
        assert!(choose_gpu(&[]).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn sysfs_gives_amd_load_and_memory_and_intel_load_from_idle_time() {
        let t = Tree::new("sysfs");
        t.card(0, "i915");
        t.file("drm/card0/gt/gt0/rc6_residency_ms", "1000\n");
        t.card(1, "amdgpu");
        t.file("drm/card1/device/gpu_busy_percent", "57\n");
        t.file("drm/card1/device/mem_info_vram_total", "8589934592\n");
        t.file("drm/card1/device/mem_info_vram_used", "2147483648\n");
        t.file("drm/card1-DP-1/status", "connected\n");
        let mut s = LinuxGpuSampler::with_roots(t.0.join("drm"), t.0.join("no-nvidia"));
        let t0 = Instant::now();
        let first = s.sample(t0);
        assert_eq!(first.len(), 2);
        assert_eq!(first[0].util_pct, 0.0);
        assert!(first[0].unified);
        assert_eq!(
            first[1],
            GpuReading {
                util_pct: 57.0,
                vram: Some((2147483648, 8589934592)),
                unified: false,
                awake: true,
            }
        );

        // Idle for 250 of the last 1000 ms: 75 % busy.
        t.file("drm/card0/gt/gt0/rc6_residency_ms", "1250\n");
        let second = s.sample(t0 + std::time::Duration::from_secs(1));
        assert!((second[0].util_pct - 75.0).abs() < 0.5, "{:?}", second[0]);

        let shown = choose_gpu(&second).unwrap();
        assert_eq!(shown.util_pct, 57.0);
        assert_eq!(shown.vram_total_gb, Some(8.0));

        // A suspended AMD GPU isn't read.
        t.file("drm/card1/device/power/runtime_status", "suspended\n");
        let third = s.sample(t0 + std::time::Duration::from_secs(2));
        assert_eq!(
            third[1],
            GpuReading {
                util_pct: 0.0,
                vram: None,
                unified: false,
                awake: false,
            }
        );
    }

    #[test]
    fn a_sleeping_nvidia_gpu_is_reported_without_loading_nvml() {
        let t = Tree::new("nvidia");
        t.file("nvidia/0000:01:00.0/power/runtime_status", "suspended\n");
        t.file("nvidia/module/refcnt", "1\n");
        let mut s = LinuxGpuSampler::with_roots(t.0.join("no-drm"), t.0.join("nvidia"));
        assert_eq!(s.nvidia_state(), (true, false));
        let readings = s.sample(Instant::now());
        assert_eq!(readings.len(), 1);
        assert!(!readings[0].awake);
        #[cfg(target_os = "linux")]
        assert!(s.nvml_tried_at.is_none());

        let none = LinuxGpuSampler::with_roots(t.0.join("no-drm"), t.0.join("absent"));
        assert_eq!(none.nvidia_state(), (false, false));
    }
}
