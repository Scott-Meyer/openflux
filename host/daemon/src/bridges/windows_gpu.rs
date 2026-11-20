//! GPU load and video memory on Windows, from the same performance counters Task Manager reads
//! (`GPU Engine` and `GPU Adapter Memory`) and the adapters DXGI lists. Every vendor's driver
//! feeds them, and reading them doesn't wake a sleeping GPU. NVML stands in for an NVIDIA GPU when
//! the counters aren't there.

use std::collections::HashMap;

const GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// Below this much dedicated memory an adapter is treated as integrated.
const DISCRETE_VRAM_BYTES: u64 = 1024 * 1024 * 1024;

/// A display adapter DXGI lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterInfo {
    pub luid_high: u32,
    pub luid_low: u32,
    pub dedicated_bytes: u64,
    pub software: bool,
}

impl AdapterInfo {
    /// The adapter's name in performance counter instances: `luid_0x00000000_0x0000D1F5`.
    pub fn counter_luid(&self) -> String {
        format!("luid_0x{:08X}_0x{:08X}", self.luid_high, self.luid_low)
    }
}

/// The adapter to show: the hardware adapter with the most dedicated memory.
pub fn choose_adapter(adapters: &[AdapterInfo]) -> Option<AdapterInfo> {
    adapters
        .iter()
        .filter(|a| !a.software)
        .max_by_key(|a| a.dedicated_bytes)
        .copied()
}

/// The adapter part of a counter instance name (`luid_0x…_0x…`), upper-cased hex.
fn instance_luid(instance: &str) -> Option<String> {
    let at = instance.find("luid_0x")?;
    let rest = &instance[at..];
    // `luid_0x`, 8 hex digits, `_0x`, 8 hex digits.
    let (high, low) = (rest.get(7..15)?, rest.get(18..26)?);
    let hex_ok = |s: &str| s.bytes().all(|b| b.is_ascii_hexdigit());
    if !hex_ok(high) || rest.get(15..18)? != "_0x" || !hex_ok(low) {
        return None;
    }
    Some(format!(
        "luid_0x{}_0x{}",
        high.to_ascii_uppercase(),
        low.to_ascii_uppercase()
    ))
}

/// Per-adapter load from `\GPU Engine(*)\Utilization Percentage` instances, named like
/// `pid_1234_luid_0x00000000_0x0000D1F5_phys_0_eng_3_engtype_VideoDecode`. Each engine's load is
/// the sum over processes, and an adapter's load is its busiest engine, as Task Manager shows.
pub fn adapter_loads(engine_samples: &[(String, f64)]) -> HashMap<String, f32> {
    let mut engines: HashMap<(String, String), f64> = HashMap::new();
    for (instance, value) in engine_samples {
        let Some(luid) = instance_luid(instance) else {
            continue;
        };
        let engine = instance
            .find("_phys_")
            .map(|i| {
                let tail = &instance[i..];
                tail.find("_engtype_").map(|j| &tail[..j]).unwrap_or(tail)
            })
            .unwrap_or("")
            .to_string();
        if value.is_finite() && *value > 0.0 {
            *engines.entry((luid, engine)).or_insert(0.0) += value;
        }
    }
    let mut loads: HashMap<String, f32> = HashMap::new();
    for ((luid, _), sum) in engines {
        let l = loads.entry(luid).or_insert(0.0);
        *l = l.max(sum.min(100.0) as f32);
    }
    loads
}

/// Dedicated memory in use per adapter, from `\GPU Adapter Memory(*)\Dedicated Usage` instances
/// (`luid_0x00000000_0x0000D1F5_phys_0`), summed over an adapter's physical parts.
pub fn adapter_memory(usage_samples: &[(String, f64)]) -> HashMap<String, u64> {
    let mut out: HashMap<String, u64> = HashMap::new();
    for (instance, value) in usage_samples {
        if let Some(luid) = instance_luid(instance) {
            if value.is_finite() && *value >= 0.0 {
                *out.entry(luid).or_insert(0) += *value as u64;
            }
        }
    }
    out
}

/// The GPU figures the keyboard shows.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuFigures {
    pub util_pct: f32,
    pub vram_used_gb: Option<f32>,
    pub vram_total_gb: Option<f32>,
    pub unified: bool,
}

/// Puts the counters of the chosen adapter together.
pub fn figures_for(
    adapter: &AdapterInfo,
    loads: &HashMap<String, f32>,
    memory: &HashMap<String, u64>,
) -> GpuFigures {
    let key = adapter.counter_luid();
    let unified = adapter.dedicated_bytes < DISCRETE_VRAM_BYTES;
    let used = memory.get(&key).copied();
    GpuFigures {
        util_pct: loads.get(&key).copied().unwrap_or(0.0).clamp(0.0, 100.0),
        vram_used_gb: used
            .filter(|_| !unified)
            .map(|u| (u.min(adapter.dedicated_bytes) as f64 / GIB) as f32),
        vram_total_gb: (!unified).then_some((adapter.dedicated_bytes as f64 / GIB) as f32),
        unified,
    }
}

#[cfg(target_os = "windows")]
pub use imp::WindowsGpuSampler;

#[cfg(target_os = "windows")]
mod imp {
    use super::*;
    use std::time::{Duration, Instant};
    use windows::core::w;
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE,
    };
    use windows::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
        PdhOpenQueryW, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W,
        PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
    };

    const ERROR_SUCCESS: u32 = 0;

    struct Counters {
        query: PDH_HQUERY,
        engine: PDH_HCOUNTER,
        memory: PDH_HCOUNTER,
    }

    impl Drop for Counters {
        fn drop(&mut self) {
            // SAFETY: the query was opened by PdhOpenQueryW and is closed once.
            unsafe {
                PdhCloseQuery(self.query);
            }
        }
    }

    impl Counters {
        fn open() -> Option<Self> {
            let mut query = PDH_HQUERY::default();
            let mut engine = PDH_HCOUNTER::default();
            let mut memory = PDH_HCOUNTER::default();
            // SAFETY: out-pointers to locals; constant counter paths. A query that fails to get
            // its load counter is closed before returning.
            unsafe {
                if PdhOpenQueryW(None, 0, &mut query) != ERROR_SUCCESS {
                    return None;
                }
                if PdhAddEnglishCounterW(
                    query,
                    w!("\\GPU Engine(*)\\Utilization Percentage"),
                    0,
                    &mut engine,
                ) != ERROR_SUCCESS
                {
                    PdhCloseQuery(query);
                    return None;
                }
                // Memory figures are optional; the load works without them.
                if PdhAddEnglishCounterW(
                    query,
                    w!("\\GPU Adapter Memory(*)\\Dedicated Usage"),
                    0,
                    &mut memory,
                ) != ERROR_SUCCESS
                {
                    memory = PDH_HCOUNTER::default();
                }
                // The first collection is the baseline a rate counter needs.
                let _ = PdhCollectQueryData(query);
                Some(Self {
                    query,
                    engine,
                    memory,
                })
            }
        }

        /// Every instance of a wildcard counter with its value.
        fn read(counter: PDH_HCOUNTER) -> Vec<(String, f64)> {
            if counter.is_invalid() {
                return Vec::new();
            }
            let mut size = 0u32;
            let mut count = 0u32;
            // SAFETY: the first call only reports the buffer size; the second fills a buffer of
            // that size, aligned for the items it starts with.
            unsafe {
                let status = PdhGetFormattedCounterArrayW(
                    counter,
                    PDH_FMT_DOUBLE,
                    &mut size,
                    &mut count,
                    None,
                );
                if status != PDH_MORE_DATA || size == 0 {
                    return Vec::new();
                }
                let item = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
                let mut buf: Vec<PDH_FMT_COUNTERVALUE_ITEM_W> =
                    vec![Default::default(); (size as usize).div_ceil(item)];
                if PdhGetFormattedCounterArrayW(
                    counter,
                    PDH_FMT_DOUBLE,
                    &mut size,
                    &mut count,
                    Some(buf.as_mut_ptr()),
                ) != ERROR_SUCCESS
                {
                    return Vec::new();
                }
                buf.iter()
                    .take(count as usize)
                    .filter(|i| {
                        i.FmtValue.CStatus == PDH_CSTATUS_VALID_DATA
                            || i.FmtValue.CStatus == PDH_CSTATUS_NEW_DATA
                    })
                    .filter_map(|i| {
                        let name = i.szName.to_string().ok()?;
                        Some((name, i.FmtValue.Anonymous.doubleValue))
                    })
                    .collect()
            }
        }
    }

    fn list_adapters() -> Vec<AdapterInfo> {
        // SAFETY: plain DXGI enumeration; every returned interface is released on drop.
        unsafe {
            let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for i in 0..16 {
                let Ok(adapter) = factory.EnumAdapters1(i) else {
                    break;
                };
                if let Ok(desc) = adapter.GetDesc1() {
                    out.push(AdapterInfo {
                        luid_high: desc.AdapterLuid.HighPart as u32,
                        luid_low: desc.AdapterLuid.LowPart,
                        dedicated_bytes: desc.DedicatedVideoMemory as u64,
                        software: desc.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0,
                    });
                }
            }
            out
        }
    }

    /// How often the adapter list is read again, for a GPU that's added or whose driver is
    /// reinstalled (both give it a new LUID).
    const ADAPTERS_TTL: Duration = Duration::from_secs(60);

    /// Samples the shown GPU. Keeps the counter query open between samples.
    pub struct WindowsGpuSampler {
        counters: Option<Counters>,
        adapter: Option<AdapterInfo>,
        adapters_at: Instant,
        nvml: Option<crate::bridges::nvml::Nvml>,
        tried_nvml: bool,
    }

    // The PDH query handle is only used from the thread that owns the sampler.
    unsafe impl Send for WindowsGpuSampler {}

    impl Default for WindowsGpuSampler {
        fn default() -> Self {
            Self {
                counters: Counters::open(),
                adapter: choose_adapter(&list_adapters()),
                adapters_at: Instant::now(),
                nvml: None,
                tried_nvml: false,
            }
        }
    }

    impl WindowsGpuSampler {
        pub fn sample(&mut self) -> Option<GpuFigures> {
            if self.adapters_at.elapsed() >= ADAPTERS_TTL {
                self.adapters_at = Instant::now();
                self.adapter = choose_adapter(&list_adapters());
            }
            if let (Some(counters), Some(adapter)) = (&self.counters, &self.adapter) {
                // SAFETY: the query is open.
                if unsafe { PdhCollectQueryData(counters.query) } == ERROR_SUCCESS {
                    let loads = adapter_loads(&Counters::read(counters.engine));
                    let memory = adapter_memory(&Counters::read(counters.memory));
                    return Some(figures_for(adapter, &loads, &memory));
                }
            }
            if !self.tried_nvml {
                self.tried_nvml = true;
                self.nvml = crate::bridges::nvml::Nvml::load();
            }
            let reading = self
                .nvml
                .as_ref()?
                .sample()
                .into_iter()
                .max_by_key(|r| r.vram_total_bytes)?;
            Some(GpuFigures {
                util_pct: reading.util_pct,
                vram_used_gb: Some((reading.vram_used_bytes as f64 / GIB) as f32),
                vram_total_gb: Some((reading.vram_total_bytes as f64 / GIB) as f32),
                unified: false,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DGPU: AdapterInfo = AdapterInfo {
        luid_high: 0,
        luid_low: 0xD1F5,
        dedicated_bytes: 10 * 1024 * 1024 * 1024,
        software: false,
    };

    #[test]
    fn engine_counters_give_each_adapter_its_busiest_engine() {
        let samples = vec![
            (
                "pid_100_luid_0x00000000_0x0000D1F5_phys_0_eng_0_engtype_3D".to_string(),
                60.0,
            ),
            (
                "pid_200_luid_0x00000000_0x0000d1f5_phys_0_eng_0_engtype_3D".to_string(),
                34.0,
            ),
            (
                "pid_100_luid_0x00000000_0x0000D1F5_phys_0_eng_3_engtype_VideoDecode".to_string(),
                20.0,
            ),
            (
                "pid_300_luid_0x00000000_0x0000AAAA_phys_0_eng_0_engtype_3D".to_string(),
                70.0,
            ),
            (
                "pid_300_luid_0x00000000_0x0000AAAA_phys_0_eng_0_engtype_3D".to_string(),
                70.0,
            ),
            ("garbage".to_string(), 50.0),
        ];
        let loads = adapter_loads(&samples);
        assert_eq!(loads["luid_0x00000000_0x0000D1F5"], 94.0);
        // A sum over processes never shows more than 100 %.
        assert_eq!(loads["luid_0x00000000_0x0000AAAA"], 100.0);
        assert_eq!(loads.len(), 2);
    }

    #[test]
    fn the_shown_adapter_is_the_discrete_one_with_its_memory() {
        let igpu = AdapterInfo {
            luid_high: 0,
            luid_low: 0xAAAA,
            dedicated_bytes: 128 * 1024 * 1024,
            software: false,
        };
        let basic = AdapterInfo {
            luid_high: 0,
            luid_low: 0xBBBB,
            dedicated_bytes: 64 * 1024 * 1024 * 1024,
            software: true,
        };
        assert_eq!(choose_adapter(&[igpu, DGPU, basic]), Some(DGPU));
        assert_eq!(DGPU.counter_luid(), "luid_0x00000000_0x0000D1F5");

        let memory = adapter_memory(&[
            (
                "luid_0x00000000_0x0000D1F5_phys_0".to_string(),
                7.5 * 1024.0 * 1024.0 * 1024.0,
            ),
            ("luid_0x00000000_0x0000AAAA_phys_0".to_string(), 1.0e8),
        ]);
        let mut loads = HashMap::new();
        loads.insert("luid_0x00000000_0x0000D1F5".to_string(), 94.0f32);
        let f = figures_for(&DGPU, &loads, &memory);
        assert_eq!(
            f,
            GpuFigures {
                util_pct: 94.0,
                vram_used_gb: Some(7.5),
                vram_total_gb: Some(10.0),
                unified: false,
            }
        );
        // An integrated adapter shares the system's memory.
        let f = figures_for(&igpu, &HashMap::new(), &memory);
        assert!(f.unified);
        assert_eq!(
            (f.util_pct, f.vram_used_gb, f.vram_total_gb),
            (0.0, None, None)
        );
    }
}
