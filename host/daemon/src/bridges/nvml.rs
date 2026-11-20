//! NVIDIA GPU load and memory through NVML, the library the NVIDIA driver installs
//! (`libnvidia-ml.so.1` on Linux, `nvml.dll` on Windows). It's loaded at run time, so machines
//! without an NVIDIA driver need nothing.

/// One NVIDIA GPU's load and dedicated memory.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NvidiaReading {
    pub util_pct: f32,
    pub vram_used_bytes: u64,
    pub vram_total_bytes: u64,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
mod imp {
    use super::NvidiaReading;
    use std::ffi::c_void;

    type NvmlReturn = i32;
    const NVML_SUCCESS: NvmlReturn = 0;
    type Device = *mut c_void;

    #[repr(C)]
    #[derive(Default)]
    struct Utilization {
        gpu: u32,
        memory: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct Memory {
        total: u64,
        free: u64,
        used: u64,
    }

    type InitFn = unsafe extern "C" fn() -> NvmlReturn;
    type CountFn = unsafe extern "C" fn(*mut u32) -> NvmlReturn;
    type HandleFn = unsafe extern "C" fn(u32, *mut Device) -> NvmlReturn;
    type UtilFn = unsafe extern "C" fn(Device, *mut Utilization) -> NvmlReturn;
    type MemFn = unsafe extern "C" fn(Device, *mut Memory) -> NvmlReturn;

    /// An initialized NVML and the GPUs it found.
    pub struct Nvml {
        _lib: Library,
        utilization: UtilFn,
        memory: MemFn,
        devices: Vec<Device>,
    }

    // NVML handles may be used from any thread.
    unsafe impl Send for Nvml {}

    impl Nvml {
        /// Loads and initializes NVML. `None` when the library isn't installed, the driver isn't
        /// running, or it finds no GPU.
        pub fn load() -> Option<Self> {
            let lib = Library::open()?;
            // SAFETY: the symbols are NVML's documented C entry points with these signatures.
            unsafe {
                let init: InitFn = std::mem::transmute(lib.symbol(c"nvmlInit_v2")?);
                let count: CountFn = std::mem::transmute(lib.symbol(c"nvmlDeviceGetCount_v2")?);
                let handle: HandleFn =
                    std::mem::transmute(lib.symbol(c"nvmlDeviceGetHandleByIndex_v2")?);
                let utilization: UtilFn =
                    std::mem::transmute(lib.symbol(c"nvmlDeviceGetUtilizationRates")?);
                let memory: MemFn = std::mem::transmute(lib.symbol(c"nvmlDeviceGetMemoryInfo")?);
                if init() != NVML_SUCCESS {
                    return None;
                }
                let mut n = 0u32;
                if count(&mut n) != NVML_SUCCESS {
                    return None;
                }
                let devices: Vec<Device> = (0..n.min(16))
                    .filter_map(|i| {
                        let mut d: Device = std::ptr::null_mut();
                        (handle(i, &mut d) == NVML_SUCCESS && !d.is_null()).then_some(d)
                    })
                    .collect();
                if devices.is_empty() {
                    return None;
                }
                Some(Self {
                    _lib: lib,
                    utilization,
                    memory,
                    devices,
                })
            }
        }

        /// The current reading of each GPU that answered.
        pub fn sample(&self) -> Vec<NvidiaReading> {
            self.devices
                .iter()
                .filter_map(|&d| {
                    let mut u = Utilization::default();
                    let mut m = Memory::default();
                    // SAFETY: `d` is a handle NVML returned, and the out-structs match NVML's.
                    let ok = unsafe {
                        (self.utilization)(d, &mut u) == NVML_SUCCESS
                            && (self.memory)(d, &mut m) == NVML_SUCCESS
                    };
                    ok.then_some(NvidiaReading {
                        util_pct: u.gpu.min(100) as f32,
                        vram_used_bytes: m.used,
                        vram_total_bytes: m.total,
                    })
                })
                .collect()
        }
    }

    #[cfg(target_os = "linux")]
    struct Library(*mut c_void);

    #[cfg(target_os = "linux")]
    impl Library {
        fn open() -> Option<Self> {
            // SAFETY: dlopen with a constant, NUL-terminated name.
            let h = unsafe {
                libc::dlopen(
                    c"libnvidia-ml.so.1".as_ptr(),
                    libc::RTLD_NOW | libc::RTLD_LOCAL,
                )
            };
            (!h.is_null()).then_some(Self(h))
        }

        fn symbol(&self, name: &std::ffi::CStr) -> Option<*mut c_void> {
            // SAFETY: `self.0` is a live handle from dlopen.
            let p = unsafe { libc::dlsym(self.0, name.as_ptr()) };
            (!p.is_null()).then_some(p)
        }
    }

    #[cfg(target_os = "windows")]
    struct Library(windows::Win32::Foundation::HMODULE);

    #[cfg(target_os = "windows")]
    impl Library {
        fn open() -> Option<Self> {
            use windows::core::w;
            use windows::Win32::System::LibraryLoader::{
                LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
            };
            // Current drivers install nvml.dll in System32; older ones in the NVSMI folder.
            // SAFETY: constant, NUL-terminated paths; no other library is searched.
            unsafe {
                LoadLibraryExW(w!("nvml.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
                    .or_else(|_| {
                        LoadLibraryExW(
                            w!("C:\\Program Files\\NVIDIA Corporation\\NVSMI\\nvml.dll"),
                            None,
                            Default::default(),
                        )
                    })
                    .ok()
                    .map(Self)
            }
        }

        fn symbol(&self, name: &std::ffi::CStr) -> Option<*mut c_void> {
            use windows::core::PCSTR;
            use windows::Win32::System::LibraryLoader::GetProcAddress;
            // SAFETY: `self.0` is a loaded module and `name` is NUL-terminated.
            unsafe { GetProcAddress(self.0, PCSTR(name.as_ptr().cast())) }.map(|f| f as *mut c_void)
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub use imp::Nvml;
