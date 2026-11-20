use crate::bridges::net::{format_rate, RateMeter};
use crate::bridges::poller::Poller;
use crate::bridges::windows_gpu::GpuFigures;
use crate::bridges::{
    layout_to_language_code, ActiveWindowInfo, HardwareMetrics, MediaSnapshot, OsSensorBridge,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the media session is asked what's playing.
#[cfg(target_os = "windows")]
const MEDIA_POLL: Duration = Duration::from_secs(1);

/// How often GPU counters are collected. Collecting the per-process GPU engine counters costs
/// more than the other sensors together, so it runs on its own thread at a slower pace.
#[cfg(target_os = "windows")]
const GPU_POLL: Duration = Duration::from_secs(2);

/// How often processor and network counters are read: the window their rates are measured over.
const RATE_WINDOW: Duration = Duration::from_secs(1);

/// The system's idle, kernel and user time, in 100 ns ticks (`GetSystemTimes`). Kernel time
/// includes idle time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemTimes {
    pub idle: u64,
    pub kernel: u64,
    pub user: u64,
}

/// The share of time the processors were busy between two readings.
pub fn cpu_busy_pct(prev: &SystemTimes, cur: &SystemTimes) -> Option<f32> {
    let idle = cur.idle.checked_sub(prev.idle)?;
    let total = cur
        .kernel
        .checked_sub(prev.kernel)?
        .checked_add(cur.user.checked_sub(prev.user)?)?;
    if total == 0 {
        return None;
    }
    Some(((total.saturating_sub(idle) as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as f32)
}

/// One row of the network interface table (`MIB_IF_ROW2`), as far as counting goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceRow {
    pub in_octets: u64,
    pub out_octets: u64,
    /// `InterfaceAndOperStatusFlags`: bit 0 is a hardware interface, bit 1 a filter interface.
    pub flags: u8,
    pub oper_up: bool,
}

/// The bytes received and sent over the hardware interfaces that are up. Filter drivers, virtual
/// switches and VPN adapters carry the same traffic again, so they aren't counted.
pub fn sum_interface_rows(rows: &[InterfaceRow]) -> (u64, u64) {
    const HARDWARE: u8 = 1 << 0;
    const FILTER: u8 = 1 << 1;
    rows.iter()
        .filter(|r| r.oper_up && r.flags & HARDWARE != 0 && r.flags & FILTER == 0)
        .fold((0, 0), |(rx, tx), r| {
            (
                rx.saturating_add(r.in_octets),
                tx.saturating_add(r.out_octets),
            )
        })
}

#[derive(Clone, Default)]
struct CachedWinSample {
    completed_at: Option<Instant>,
    win: Option<ActiveWindowInfo>,
    lang: Option<u32>,
    hw: Option<HardwareMetrics>,
}

/// One sample of the foreground window, input language and hardware metrics.
pub type WindowsSampler = Box<
    dyn Fn() -> (
            Option<ActiveWindowInfo>,
            Option<u32>,
            Option<HardwareMetrics>,
        ) + Send
        + Sync,
>;

pub struct WindowsBridge {
    last_hw: HardwareMetrics,
    cache: Arc<Mutex<CachedWinSample>>,
    poll_ttl: Duration,
    sampler: WindowsSampler,
    cpu_prev: Option<SystemTimes>,
    net: RateMeter,
    net_rates: (u64, u64),
    rates_read_at: Option<Instant>,
    gpu: Option<Poller<Option<GpuFigures>>>,
    media: Option<Poller<MediaSnapshot>>,
}

impl Default for WindowsBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsBridge {
    pub fn new() -> Self {
        Self::with_sampler(
            Duration::from_millis(500),
            Box::new(Self::native_win32_sample),
        )
    }

    pub fn with_sampler(poll_ttl: Duration, sampler: WindowsSampler) -> Self {
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
            cache: Arc::new(Mutex::new(CachedWinSample::default())),
            poll_ttl,
            sampler,
            cpu_prev: None,
            net: RateMeter::default(),
            net_rates: (0, 0),
            rates_read_at: None,
            gpu: None,
            media: None,
        }
    }

    #[cfg(target_os = "windows")]
    fn native_win32_sample() -> (
        Option<ActiveWindowInfo>,
        Option<u32>,
        Option<HardwareMetrics>,
    ) {
        // Win32 calls in process, with no child process, so a sample takes well under a millisecond.
        #[repr(C)]
        struct MemoryStatusEx {
            dw_length: u32,
            dw_memory_load: u32,
            ull_total_phys: u64,
            ull_avail_phys: u64,
            ull_total_page_file: u64,
            ull_avail_page_file: u64,
            ull_total_virtual: u64,
            ull_avail_virtual: u64,
            ull_avail_extended_virtual: u64,
        }
        #[link(name = "user32")]
        extern "system" {
            fn GetForegroundWindow() -> isize;
            fn GetWindowThreadProcessId(hwnd: isize, lpdw_process_id: *mut u32) -> u32;
            fn GetKeyboardLayout(id_thread: u32) -> usize;
            fn GetWindowTextW(hwnd: isize, lp_string: *mut u16, n_max_count: i32) -> i32;
        }
        #[link(name = "kernel32")]
        extern "system" {
            fn GlobalMemoryStatusEx(lp_buffer: *mut MemoryStatusEx) -> i32;
            fn OpenProcess(
                dw_desired_access: u32,
                b_inherit_handle: i32,
                dw_process_id: u32,
            ) -> isize;
            fn QueryFullProcessImageNameW(
                h_process: isize,
                dw_flags: u32,
                lp_exe_name: *mut u16,
                lpdw_size: *mut u32,
            ) -> i32;
            fn CloseHandle(h_object: isize) -> i32;
        }
        unsafe {
            let mut mem = MemoryStatusEx {
                dw_length: std::mem::size_of::<MemoryStatusEx>() as u32,
                dw_memory_load: 0,
                ull_total_phys: 0,
                ull_avail_phys: 0,
                ull_total_page_file: 0,
                ull_avail_page_file: 0,
                ull_total_virtual: 0,
                ull_avail_virtual: 0,
                ull_avail_extended_virtual: 0,
            };
            let hw = if GlobalMemoryStatusEx(&mut mem) != 0 {
                let tot = (mem.ull_total_phys as f64) / (1024.0 * 1024.0 * 1024.0);
                let avail = (mem.ull_avail_phys as f64) / (1024.0 * 1024.0 * 1024.0);
                Some(HardwareMetrics {
                    cpu_pct: 0.0,
                    mem_used_gb: (tot - avail).max(0.0) as f32,
                    mem_total_gb: tot as f32,
                    gpu_pct: 0.0,
                    vram_used_gb: None,
                    vram_total_gb: None,
                    gpu_unified_mem: false,
                    net_down: "0 KB/s".into(),
                    net_up: "0 KB/s".into(),
                })
            } else {
                None
            };

            let hwnd = GetForegroundWindow();
            if hwnd == 0 {
                return (None, None, hw);
            }
            let mut pid: u32 = 0;
            let tid = GetWindowThreadProcessId(hwnd, &mut pid);
            let hkl = GetKeyboardLayout(tid);
            let lang = Self::langid_to_language_code((hkl & 0xFFFF) as u16);

            let mut title_buf = [0u16; 256];
            let tlen = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), title_buf.len() as i32);
            let title = if tlen > 0 {
                String::from_utf16_lossy(&title_buf[..tlen as usize])
            } else {
                String::new()
            };

            let mut exe_name = String::new();
            if pid != 0 {
                const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
                let hproc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                if hproc != 0 {
                    let mut pbuf = [0u16; 512];
                    let mut plen = pbuf.len() as u32;
                    if QueryFullProcessImageNameW(hproc, 0, pbuf.as_mut_ptr(), &mut plen) != 0 {
                        let full = String::from_utf16_lossy(&pbuf[..plen as usize]);
                        exe_name = full.rsplit(['\\', '/']).next().unwrap_or(&full).to_string();
                    }
                    let _ = CloseHandle(hproc);
                }
            }
            let win = if !exe_name.is_empty() || !title.is_empty() {
                Some(ActiveWindowInfo {
                    app_id: exe_name.clone(),
                    process_name: exe_name,
                    window_title: title,
                })
            } else {
                None
            };
            (win, lang, hw)
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn native_win32_sample() -> (
        Option<ActiveWindowInfo>,
        Option<u32>,
        Option<HardwareMetrics>,
    ) {
        (None, None, None)
    }

    #[cfg(target_os = "windows")]
    fn system_times() -> Option<SystemTimes> {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::GetSystemTimes;
        let ticks = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
        let (mut idle, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        // SAFETY: out-pointers to locals.
        unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.ok()?;
        Some(SystemTimes {
            idle: ticks(idle),
            kernel: ticks(kernel),
            user: ticks(user),
        })
    }

    #[cfg(not(target_os = "windows"))]
    fn system_times() -> Option<SystemTimes> {
        None
    }

    #[cfg(target_os = "windows")]
    fn network_totals() -> Option<(u64, u64)> {
        use windows::Win32::NetworkManagement::IpHelper::{
            FreeMibTable, GetIfTable2, MIB_IF_ROW2, MIB_IF_TABLE2,
        };
        use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        // SAFETY: GetIfTable2 allocates the table, which is read within its NumEntries and freed
        // with FreeMibTable.
        unsafe {
            if GetIfTable2(&mut table).is_err() || table.is_null() {
                return None;
            }
            let n = (*table).NumEntries as usize;
            let first = std::ptr::addr_of!((*table).Table).cast::<MIB_IF_ROW2>();
            let rows: Vec<InterfaceRow> = std::slice::from_raw_parts(first, n)
                .iter()
                .map(|r| InterfaceRow {
                    in_octets: r.InOctets,
                    out_octets: r.OutOctets,
                    flags: r.InterfaceAndOperStatusFlags._bitfield,
                    oper_up: r.OperStatus == IfOperStatusUp,
                })
                .collect();
            FreeMibTable(table.cast());
            Some(sum_interface_rows(&rows))
        }
    }

    #[cfg(not(target_os = "windows"))]
    fn network_totals() -> Option<(u64, u64)> {
        None
    }

    #[cfg(target_os = "windows")]
    fn spawn_gpu_poller() -> Option<Poller<Option<GpuFigures>>> {
        Some(Poller::spawn(
            "openflux-gpu",
            GPU_POLL,
            crate::bridges::windows_gpu::WindowsGpuSampler::default,
            |s| s.sample(),
        ))
    }

    #[cfg(not(target_os = "windows"))]
    fn spawn_gpu_poller() -> Option<Poller<Option<GpuFigures>>> {
        None
    }

    #[cfg(target_os = "windows")]
    fn spawn_media_poller() -> Option<Poller<MediaSnapshot>> {
        Some(Poller::spawn(
            "openflux-media",
            MEDIA_POLL,
            crate::bridges::smtc::SmtcState::default,
            |s| s.sample(),
        ))
    }

    #[cfg(not(target_os = "windows"))]
    fn spawn_media_poller() -> Option<Poller<MediaSnapshot>> {
        None
    }

    /// Adds the processor, network and GPU figures to a sample of the memory figures.
    fn add_live_figures(&mut self, hw: &mut HardwareMetrics) {
        let now = Instant::now();
        if self
            .rates_read_at
            .is_none_or(|t| now.saturating_duration_since(t) >= RATE_WINDOW)
        {
            self.rates_read_at = Some(now);
            if let Some(cur) = Self::system_times() {
                if let Some(pct) = self.cpu_prev.and_then(|prev| cpu_busy_pct(&prev, &cur)) {
                    self.last_hw.cpu_pct = pct;
                }
                self.cpu_prev = Some(cur);
            }
            if let Some((rx, tx)) = Self::network_totals() {
                if let Some(rates) = self.net.update(now, rx, tx) {
                    self.net_rates = rates;
                }
            }
        }
        hw.cpu_pct = self.last_hw.cpu_pct;
        hw.net_down = format_rate(self.net_rates.0);
        hw.net_up = format_rate(self.net_rates.1);

        if self.gpu.is_none() {
            self.gpu = Self::spawn_gpu_poller();
        }
        if let Some(g) = self
            .gpu
            .as_ref()
            .and_then(|p| p.latest())
            .and_then(|s| s.value)
        {
            hw.gpu_pct = g.util_pct;
            hw.vram_used_gb = g.vram_used_gb;
            hw.vram_total_gb = g.vram_total_gb;
            hw.gpu_unified_mem = g.unified;
        }
    }

    fn refresh_cache_if_stale(&mut self) -> CachedWinSample {
        {
            let g = self.cache.lock().unwrap();
            if g.completed_at
                .map(|t| t.elapsed() < self.poll_ttl)
                .unwrap_or(false)
            {
                return g.clone();
            }
        }
        let (win, lang, hw) = (self.sampler)();
        // completed_at is stamped after the sample returns, so a slow sampler still serves one cached
        // sample to sample_hardware(), active_window() and active_language_code() in the same tick.
        let sample = CachedWinSample {
            completed_at: Some(Instant::now()),
            win,
            lang,
            hw,
        };
        *self.cache.lock().unwrap() = sample.clone();
        sample
    }

    /// Maps a Windows LANGID (the low word of an HKL) to the firmware's language code. Languages
    /// match on their primary language ID (the low 10 bits); English also checks for British English.
    pub fn langid_to_language_code(hkl_low: u16) -> Option<u32> {
        let primary = hkl_low & 0x03FF;
        match primary {
            0x07 => layout_to_language_code("de"),
            0x0C => layout_to_language_code("fr"),
            0x0A => layout_to_language_code("es"),
            0x11 => layout_to_language_code("ja"),
            0x09 => {
                if hkl_low == 0x0809 {
                    layout_to_language_code("en-GB")
                } else {
                    layout_to_language_code("en-US")
                }
            }
            _ => None,
        }
    }
}

impl OsSensorBridge for WindowsBridge {
    fn sample_hardware(&mut self) -> HardwareMetrics {
        let mut hw = self
            .refresh_cache_if_stale()
            .hw
            .unwrap_or_else(|| self.last_hw.clone());
        self.add_live_figures(&mut hw);
        self.last_hw = hw;
        self.last_hw.clone()
    }

    fn active_window(&mut self) -> Option<ActiveWindowInfo> {
        self.refresh_cache_if_stale().win
    }

    fn active_language_code(&mut self) -> Option<u32> {
        self.refresh_cache_if_stale().lang
    }

    fn media_snapshot(&mut self) -> MediaSnapshot {
        if self.media.is_none() {
            self.media = Self::spawn_media_poller();
        }
        self.media
            .as_ref()
            .and_then(|p| p.latest())
            .map(crate::bridges::media_now)
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn maps_windows_keyboard_layouts_to_language_codes() {
        assert_eq!(WindowsBridge::langid_to_language_code(0x0407), Some(334)); // German
        assert_eq!(WindowsBridge::langid_to_language_code(0x0409), Some(471)); // English US
        assert_eq!(WindowsBridge::langid_to_language_code(0x0809), Some(295)); // English UK
        assert_eq!(WindowsBridge::langid_to_language_code(0x040C), Some(329)); // French
        assert_eq!(WindowsBridge::langid_to_language_code(0x0411), Some(226)); // Japanese
        assert_eq!(WindowsBridge::langid_to_language_code(0x0000), None);
    }

    #[test]
    fn processor_load_comes_from_system_time_deltas() {
        let prev = SystemTimes {
            idle: 1_000,
            kernel: 2_000,
            user: 1_000,
        };
        // 400 of 1000 ticks idle: 60 % busy. Kernel time includes the idle time.
        let cur = SystemTimes {
            idle: 1_400,
            kernel: 2_700,
            user: 1_300,
        };
        assert!((cpu_busy_pct(&prev, &cur).unwrap() - 60.0).abs() < 0.01);
        assert_eq!(cpu_busy_pct(&prev, &prev), None);
        assert_eq!(cpu_busy_pct(&cur, &prev), None);
    }

    #[test]
    fn network_totals_count_hardware_interfaces_that_are_up() {
        let row = |flags: u8, up: bool, rx: u64, tx: u64| InterfaceRow {
            in_octets: rx,
            out_octets: tx,
            flags,
            oper_up: up,
        };
        let rows = [
            row(0b01, true, 1_000, 100), // Ethernet
            row(0b01, true, 500, 50),    // Wi-Fi
            row(0b11, true, 1_000, 100), // a filter driver over the Ethernet adapter
            row(0b00, true, 900, 90),    // a VPN or virtual switch
            row(0b01, false, 77, 7),     // a disconnected adapter
        ];
        assert_eq!(sum_interface_rows(&rows), (1_500, 150));
    }

    #[test]
    fn hardware_figures_without_counters_keep_the_memory_sample() {
        let mut bridge = WindowsBridge::with_sampler(
            Duration::from_millis(500),
            Box::new(|| {
                (
                    None,
                    None,
                    Some(HardwareMetrics {
                        cpu_pct: 0.0,
                        mem_used_gb: 6.0,
                        mem_total_gb: 16.0,
                        gpu_pct: 0.0,
                        vram_used_gb: None,
                        vram_total_gb: None,
                        gpu_unified_mem: false,
                        net_down: String::new(),
                        net_up: String::new(),
                    }),
                )
            }),
        );
        let hw = bridge.sample_hardware();
        assert_eq!((hw.mem_used_gb, hw.mem_total_gb), (6.0, 16.0));
        assert_eq!(hw.net_down, "0 KB/s");
        assert_eq!(bridge.media_snapshot(), MediaSnapshot::default());
    }

    #[test]
    fn slow_windows_probe_runs_only_once_per_tick_across_all_three_bridge_queries() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_cl = calls.clone();
        let mut bridge = WindowsBridge::with_sampler(
            Duration::from_millis(500),
            Box::new(move || {
                calls_cl.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(30));
                (
                    Some(ActiveWindowInfo {
                        app_id: "Code.exe".into(),
                        process_name: "Code.exe".into(),
                        window_title: "OpenFlux".into(),
                    }),
                    Some(471),
                    None,
                )
            }),
        );
        let _ = bridge.sample_hardware();
        let win = bridge.active_window().unwrap();
        let lang = bridge.active_language_code();
        assert_eq!(win.process_name, "Code.exe");
        assert_eq!(lang, Some(471));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "expected 1 cached sample per tick"
        );
    }
}
