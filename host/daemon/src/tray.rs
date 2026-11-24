//! The daemon's system tray icon, provided by small native helpers rather than a WebView.

use std::path::PathBuf;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Stdio;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayMode {
    Headless,
    NativeStatus,
}

pub struct TrayController {
    pub mode: TrayMode,
    pub gui_binary: Option<String>,
    helper_child: Arc<Mutex<Option<Child>>>,
}

impl TrayController {
    pub fn resolve_sibling_bin(name: &str) -> Option<PathBuf> {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let p = dir.join(name);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        None
    }

    pub fn new(mode: TrayMode, gui_binary: Option<String>) -> Self {
        let resolved_gui = gui_binary.or_else(|| {
            let name = if cfg!(windows) {
                "openflux-studio.exe"
            } else {
                "openflux-studio"
            };
            Self::resolve_sibling_bin(name).map(|p| p.to_string_lossy().to_string())
        });
        Self {
            mode,
            gui_binary: resolved_gui,
            helper_child: Arc::new(Mutex::new(None)),
        }
    }

    /// Shows the tray icon when the mode is `TrayMode::NativeStatus`:
    /// - macOS: runs the `openflux-tray` helper next to the daemon (an AppKit `NSStatusBar` item).
    /// - Windows: creates a `Shell_NotifyIconW` notification icon, removed when `running_flag`
    ///   clears.
    /// - Linux: runs `zenity --notification` when it is installed (or a helper named by
    ///   `OPENFLUX_TRAY_BIN`); otherwise runs without an icon.
    ///
    /// `quit` is called when the user chooses Quit; it asks the daemon to stop, so keyboard work
    /// finishes first.
    pub fn start(
        &self,
        running_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
        quit: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) {
        if self.mode != TrayMode::NativeStatus {
            let _ = (running_flag, quit);
            return;
        }
        #[cfg(not(target_os = "windows"))]
        let _ = &quit;
        #[cfg(target_os = "macos")]
        {
            let _ = &running_flag;
            self.spawn_helper();
        }
        #[cfg(target_os = "windows")]
        {
            let gui = self.gui_binary.clone();
            std::thread::spawn(move || {
                windows_tray_loop(gui, running_flag, quit);
            });
        }
        #[cfg(target_os = "linux")]
        {
            let _ = &running_flag;
            if !self.spawn_helper() {
                match Command::new("zenity")
                    .args(["--notification", "--text=Keyglass is running"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                {
                    Ok(child) => *self.helper_child.lock().unwrap() = Some(child),
                    Err(_) => {
                        eprintln!("openfluxd: no tray helper found; running without a tray icon")
                    }
                }
            }
        }
    }
}

impl TrayController {
    /// Starts the `openflux-tray` helper (`OPENFLUX_TRAY_BIN`, else next to the daemon). `false`
    /// when there is none or it couldn't be started.
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    fn spawn_helper(&self) -> bool {
        let Some(tray_bin) = std::env::var("OPENFLUX_TRAY_BIN")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| Self::resolve_sibling_bin("openflux-tray"))
        else {
            return false;
        };
        let mut cmd = Command::new(tray_bin);
        cmd.arg("--daemon-pid")
            .arg(std::process::id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(ref gui) = self.gui_binary {
            cmd.arg("--desktop-bin").arg(gui);
        }
        match cmd.spawn() {
            Ok(child) => {
                *self.helper_child.lock().unwrap() = Some(child);
                true
            }
            Err(_) => false,
        }
    }
}

impl Drop for TrayController {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.helper_child.lock() {
            if let Some(mut c) = guard.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn windows_tray_loop(
    gui_binary: Option<String>,
    running_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    quit: std::sync::Arc<dyn Fn() + Send + Sync>,
) {
    // A hidden Win32 message window that owns the Shell_NotifyIconW icon and its menu.
    use std::ffi::c_void;
    const WM_USER: u32 = 0x0400;
    const WM_TRAYICON: u32 = WM_USER + 1;
    const WM_LBUTTONDBLCLK: usize = 0x0203;
    const WM_RBUTTONUP: usize = 0x0205;
    const WM_COMMAND: u32 = 0x0111;
    const WM_CLOSE: u32 = 0x0010;
    const WM_DESTROY: u32 = 0x0002;
    const WM_TIMER: u32 = 0x0113;
    const SM_CXSMICON: i32 = 49;
    const SM_CYSMICON: i32 = 50;
    const RUNNING_TIMER_ID: usize = 1;
    const RUNNING_POLL_MS: u32 = 200;
    const NIM_ADD: u32 = 0x00000000;
    const NIM_DELETE: u32 = 0x00000002;
    const NIF_MESSAGE: u32 = 0x00000001;
    const NIF_ICON: u32 = 0x00000002;
    const NIF_TIP: u32 = 0x00000004;
    const IDI_APPLICATION: usize = 32512;
    const TPM_BOTTOMALIGN: u32 = 0x0020;
    const TPM_LEFTALIGN: u32 = 0x0000;
    const MF_STRING: u32 = 0x00000000;
    const ID_OPEN: usize = 1001;
    const ID_QUIT: usize = 1002;

    #[repr(C)]
    struct WndClassW {
        style: u32,
        lpfn_wnd_proc: Option<unsafe extern "system" fn(isize, u32, usize, isize) -> isize>,
        cb_cls_extra: i32,
        cb_wnd_extra: i32,
        h_instance: isize,
        h_icon: isize,
        h_cursor: isize,
        hbr_background: isize,
        lpsz_menu_name: *const u16,
        lpsz_class_name: *const u16,
    }
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[repr(C)]
    struct Msg {
        hwnd: isize,
        message: u32,
        w_param: usize,
        l_param: isize,
        time: u32,
        pt: Point,
    }
    #[repr(C)]
    struct NotifyIconDataW {
        cb_size: u32,
        h_wnd: isize,
        u_id: u32,
        u_flags: u32,
        u_callback_message: u32,
        h_icon: isize,
        sz_tip: [u16; 128],
        dw_state: u32,
        dw_state_mask: u32,
        sz_info: [u16; 256],
        u_timeout_or_version: u32,
        sz_info_title: [u16; 64],
        dw_info_flags: u32,
    }
    #[link(name = "user32")]
    extern "system" {
        fn LoadIconW(h_instance: isize, lp_icon_name: *const u16) -> isize;
        fn CreateIconFromResourceEx(
            presbits: *const u8,
            dw_res_size: u32,
            f_icon: i32,
            dw_ver: u32,
            cx_desired: i32,
            cy_desired: i32,
            flags: u32,
        ) -> isize;
        fn DestroyIcon(h_icon: isize) -> i32;
        fn GetSystemMetrics(n_index: i32) -> i32;
        fn FindWindowW(lp_class_name: *const u16, lp_window_name: *const u16) -> isize;
        fn PostMessageW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> i32;
        fn RegisterClassW(lp_wnd_class: *const WndClassW) -> u16;
        fn CreateWindowExW(
            dw_ex_style: u32,
            lp_class_name: *const u16,
            lp_window_name: *const u16,
            dw_style: u32,
            x: i32,
            y: i32,
            n_width: i32,
            n_height: i32,
            h_wnd_parent: isize,
            h_menu: isize,
            h_instance: isize,
            lp_param: *mut c_void,
        ) -> isize;
        fn DefWindowProcW(hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> isize;
        fn GetMessageW(lp_msg: *mut Msg, hwnd: isize, min: u32, max: u32) -> i32;
        fn TranslateMessage(lp_msg: *const Msg) -> i32;
        fn DispatchMessageW(lp_msg: *const Msg) -> isize;
        fn PostQuitMessage(exit_code: i32);
        fn CreatePopupMenu() -> isize;
        fn AppendMenuW(
            hmenu: isize,
            uflags: u32,
            uid_new_item: usize,
            lp_new_item: *const u16,
        ) -> i32;
        fn TrackPopupMenu(
            hmenu: isize,
            uflags: u32,
            x: i32,
            y: i32,
            nreserved: i32,
            hwnd: isize,
            prc_rect: *const c_void,
        ) -> i32;
        fn DestroyMenu(hmenu: isize) -> i32;
        fn GetCursorPos(lp_point: *mut Point) -> i32;
        fn SetForegroundWindow(hwnd: isize) -> i32;
        fn SetTimer(hwnd: isize, id: usize, elapse_ms: u32, timer_proc: *const c_void) -> usize;
        fn KillTimer(hwnd: isize, id: usize) -> i32;
        fn DestroyWindow(hwnd: isize) -> i32;
    }
    #[link(name = "shell32")]
    extern "system" {
        fn Shell_NotifyIconW(dw_message: u32, lp_data: *mut NotifyIconDataW) -> i32;
    }

    static GUI_BIN: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    static QUIT: std::sync::Mutex<Option<std::sync::Arc<dyn Fn() + Send + Sync>>> =
        std::sync::Mutex::new(None);
    *GUI_BIN.lock().unwrap() = gui_binary;
    static RUNNING: std::sync::Mutex<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> =
        std::sync::Mutex::new(None);
    *QUIT.lock().unwrap() = Some(quit);
    *RUNNING.lock().unwrap() = Some(running_flag);

    unsafe extern "system" fn wnd_proc(
        hwnd: isize,
        msg: u32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        if msg == WM_TRAYICON {
            let ev = (lparam as usize) & 0xFFFF;
            if ev == WM_LBUTTONDBLCLK {
                if let Some(ref b) = *GUI_BIN.lock().unwrap() {
                    let _ = Command::new(b).spawn();
                }
            } else if ev == WM_RBUTTONUP {
                let hmenu = CreatePopupMenu();
                let open_lbl: Vec<u16> = "Open Keyglass Studio\0".encode_utf16().collect();
                let quit_lbl: Vec<u16> = "Quit Keyglass\0".encode_utf16().collect();
                AppendMenuW(hmenu, MF_STRING, ID_OPEN, open_lbl.as_ptr());
                AppendMenuW(hmenu, MF_STRING, ID_QUIT, quit_lbl.as_ptr());
                let mut pt = Point { x: 0, y: 0 };
                GetCursorPos(&mut pt);
                SetForegroundWindow(hwnd);
                TrackPopupMenu(
                    hmenu,
                    TPM_BOTTOMALIGN | TPM_LEFTALIGN,
                    pt.x,
                    pt.y,
                    0,
                    hwnd,
                    std::ptr::null(),
                );
                DestroyMenu(hmenu);
            }
            return 0;
        }
        if msg == WM_COMMAND {
            let cmd = wparam & 0xFFFF;
            if cmd == ID_OPEN {
                if let Some(ref b) = *GUI_BIN.lock().unwrap() {
                    let _ = Command::new(b).spawn();
                }
            } else if cmd == ID_QUIT {
                // Close OpenFlux Studio cleanly if it is open, then stop the daemon.
                let title: Vec<u16> = "Keyglass Studio\0".encode_utf16().collect();
                let studio_hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
                if studio_hwnd != 0 {
                    PostMessageW(studio_hwnd, WM_CLOSE, 0, 0);
                }
                // The daemon stops once keyboard work finishes; the icon stays until then.
                if let Some(q) = QUIT.lock().unwrap().clone() {
                    q();
                }
            }
            return 0;
        }
        if msg == WM_TIMER && wparam == RUNNING_TIMER_ID {
            // Once the daemon stops running, the window closes and the icon is removed.
            let running = RUNNING
                .lock()
                .unwrap()
                .as_ref()
                .is_none_or(|r| r.load(std::sync::atomic::Ordering::Relaxed));
            if !running {
                KillTimer(hwnd, RUNNING_TIMER_ID);
                DestroyWindow(hwnd);
            }
            return 0;
        }
        if msg == WM_DESTROY {
            PostQuitMessage(0);
            return 0;
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    unsafe {
        let cls: Vec<u16> = "OpenFluxTrayClass\0".encode_utf16().collect();
        let wc = WndClassW {
            style: 0,
            lpfn_wnd_proc: Some(wnd_proc),
            cb_cls_extra: 0,
            cb_wnd_extra: 0,
            h_instance: 0,
            h_icon: 0,
            h_cursor: 0,
            hbr_background: 0,
            lpsz_menu_name: std::ptr::null(),
            lpsz_class_name: cls.as_ptr(),
        };
        if RegisterClassW(&wc) == 0 {
            return;
        }
        let hwnd = CreateWindowExW(
            0,
            cls.as_ptr(),
            cls.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
        );
        if hwnd == 0 {
            return;
        }
        let cx = GetSystemMetrics(SM_CXSMICON).max(16);
        let cy = GetSystemMetrics(SM_CYSMICON).max(16);
        let embedded_icon = select_ico_entry(TRAY_ICO, cx as u32)
            .map(|bytes| {
                CreateIconFromResourceEx(
                    bytes.as_ptr(),
                    bytes.len() as u32,
                    1,
                    0x0003_0000,
                    cx,
                    cy,
                    0,
                )
            })
            .unwrap_or(0);
        let (hicon, owns_icon) = if embedded_icon != 0 {
            (embedded_icon, true)
        } else {
            (LoadIconW(0, IDI_APPLICATION as *const u16), false)
        };
        let mut tip = [0u16; 128];
        for (i, ch) in "Keyglass".encode_utf16().take(127).enumerate() {
            tip[i] = ch;
        }
        let mut nid = NotifyIconDataW {
            cb_size: std::mem::size_of::<NotifyIconDataW>() as u32,
            h_wnd: hwnd,
            u_id: 1,
            u_flags: NIF_MESSAGE | NIF_TIP | if hicon != 0 { NIF_ICON } else { 0 },
            u_callback_message: WM_TRAYICON,
            h_icon: hicon,
            sz_tip: tip,
            dw_state: 0,
            dw_state_mask: 0,
            sz_info: [0; 256],
            u_timeout_or_version: 0,
            sz_info_title: [0; 64],
            dw_info_flags: 0,
        };
        if Shell_NotifyIconW(NIM_ADD, &mut nid) == 0 {
            if owns_icon {
                DestroyIcon(hicon);
            }
            return;
        }
        SetTimer(hwnd, RUNNING_TIMER_ID, RUNNING_POLL_MS, std::ptr::null());
        let mut msg = std::mem::zeroed::<Msg>();
        while GetMessageW(&mut msg, 0, 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        Shell_NotifyIconW(NIM_DELETE, &mut nid);
        if owns_icon {
            DestroyIcon(hicon);
        }
    }
}

#[cfg(any(target_os = "windows", test))]
const TRAY_ICO: &[u8] = include_bytes!("../../studio/src-tauri/icons/icon.ico");

/// Selects the ICO image payload closest to `target_px` (preferring >= `target_px` so icons
/// downscale cleanly rather than upscaling).
#[cfg(any(target_os = "windows", test))]
fn select_ico_entry(ico: &[u8], target_px: u32) -> Option<&[u8]> {
    if ico.len() < 6 {
        return None;
    }
    let typ = u16::from_le_bytes([ico[2], ico[3]]);
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    if typ != 1 || count == 0 || ico.len() < 6 + count * 16 {
        return None;
    }
    let mut best: Option<(bool, u32, &[u8])> = None;
    for i in 0..count {
        let base = 6 + i * 16;
        let w = match ico[base] {
            0 => 256u32,
            n => n as u32,
        };
        let size =
            u32::from_le_bytes([ico[base + 8], ico[base + 9], ico[base + 10], ico[base + 11]])
                as usize;
        let off = u32::from_le_bytes([
            ico[base + 12],
            ico[base + 13],
            ico[base + 14],
            ico[base + 15],
        ]) as usize;
        let Some(end) = off.checked_add(size) else {
            continue;
        };
        if size == 0 || end > ico.len() {
            continue;
        }
        let ge = w >= target_px;
        let diff = w.abs_diff(target_px);
        let replace = match best {
            None => true,
            Some((best_ge, best_diff, _)) => {
                (ge && !best_ge) || (ge == best_ge && diff < best_diff)
            }
        };
        if replace {
            best = Some((ge, diff, &ico[off..end]));
        }
    }
    best.map(|(_, _, slice)| slice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_windows_tray_icon_selects_crisp_small_png() {
        let icon16 = select_ico_entry(TRAY_ICO, 16).expect("16px icon entry");
        assert!(icon16.starts_with(b"\x89PNG"));
        let icon24 = select_ico_entry(TRAY_ICO, 24).expect("24px icon entry");
        assert!(icon24.starts_with(b"\x89PNG"));
        assert_ne!(icon16.len(), icon24.len());
    }
}
