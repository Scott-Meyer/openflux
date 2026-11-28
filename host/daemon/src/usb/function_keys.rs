//! Keyglass function keys the computer acts on.
//!
//! The firmware reports which `0x5000_00xx` function keys are held in a vendor-defined HID
//! collection, usage page `0xFF00`, usage `0x01` (see `docs/protocol/usb.md`): a 4-byte
//! little-endian mask in which bit `c` is set while `0x5000_0000 | c` is held. The collection is
//! found by its usage, on an interface of its own with no report ID or on the keyboard's
//! interface as report ID 3. The listener reads it on its own thread and acts on each key as it
//! goes down; `KC_OF_AUTO_PROFILE_LOCK` toggles the auto-profile lock.

use crate::api::state::ApiState;
use crate::codecs::keymap::KEYGLASS_KEYS;
use crate::usb::protocol::{FIRMWARE_PID, KEYBOARD_VID};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The report ID of the function-key report when it shares the keyboard's interface.
pub const REPORT_ID: u8 = 3;
/// The report ID of generic QMK/Vial Flux module actions (`{ slot_le16, pressed, mods, layer }`).
pub const REPORT_ID_FLUX_ACTION: u8 = 4;
/// The usage page and usage of the function-key collection.
pub const USAGE_PAGE: u16 = 0xFF00;
pub const USAGE: u16 = 0x01;

/// How long one read waits before the listener checks whether it should stop.
const READ_TIMEOUT_MS: i32 = 250;
/// How long the listener waits before looking for the keyboard again.
const RETRY_INTERVAL: Duration = Duration::from_secs(2);

/// The function code (`c` of `0x5000_0000 | c`) of an Keyglass key name.
pub fn function_code(name: &str) -> Option<u32> {
    KEYGLASS_KEYS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, w)| w & 0xFF)
}

/// The held-function mask of a function-key report, or `None` for any other report. A report on
/// the collection's own interface is the 4-byte mask alone; on the keyboard's interface, where
/// every report starts with its ID, it is report ID 3 followed by the mask. No other report of
/// either interface is 4 bytes long or starts with ID 3.
pub fn parse_report(report: &[u8]) -> Option<u32> {
    match report {
        [a, b, c, d] | [REPORT_ID, a, b, c, d, ..] => Some(u32::from_le_bytes([*a, *b, *c, *d])),
        _ => None,
    }
}

/// Parses a Report ID 4 generic Flux/module action event: `(slot, pressed, mods, layer)`.
pub fn parse_flux_action_report(report: &[u8]) -> Option<(u16, bool, u8, u8)> {
    match report {
        [REPORT_ID_FLUX_ACTION, slo, shi, pressed, mods, layer, ..] => {
            Some((u16::from_le_bytes([*slo, *shi]), *pressed != 0, *mods, *layer))
        }
        _ => None,
    }
}

/// The function keys held according to the reports read so far.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HeldFunctions {
    mask: u32,
}

impl HeldFunctions {
    /// Takes the mask of a new report and returns the codes of the keys that went down since the
    /// last one, lowest first. A key that stays held, or is released, is not reported.
    pub fn update(&mut self, mask: u32) -> Vec<u32> {
        let pressed = mask & !self.mask;
        self.mask = mask;
        (0..32).filter(|c| pressed & (1 << c) != 0).collect()
    }

    /// Forgets every held key, as when the keyboard goes away.
    pub fn clear(&mut self) {
        self.mask = 0;
    }
}

/// What the computer did for a function key that went down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionAction {
    /// The auto-profile lock was toggled; `locked` is its new state.
    AutoProfileLock { locked: bool },
    /// The keyboard handles this function itself.
    None,
}

/// Acts on a function key that went down.
pub fn on_function_pressed(state: &ApiState, code: u32) -> FunctionAction {
    if Some(code) == function_code("KC_OF_AUTO_PROFILE_LOCK") {
        let locked = state.toggle_auto_profile_lock();
        return FunctionAction::AutoProfileLock { locked };
    }
    FunctionAction::None
}

/// Where the listener reads reports from.
pub trait ReportSource {
    /// Reads one input report into `buf`, waiting at most `timeout_ms`. `Ok(0)` when none came.
    fn read_report(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, String>;
}

impl ReportSource for hidapi::HidDevice {
    fn read_report(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, String> {
        self.read_timeout(buf, timeout_ms)
            .map_err(|e| e.to_string())
    }
}

/// Reads reports from `source` until it fails or `running` is cleared, and calls `on_press` for
/// every function key that goes down and `on_flux` for Report ID 4 QMK/Vial module actions.
pub fn listen_with_actions(
    source: &mut dyn ReportSource,
    running: &AtomicBool,
    on_press: &mut dyn FnMut(u32),
    on_flux: &mut dyn FnMut(u16, bool, u8, u8),
) -> Option<String> {
    let mut held = HeldFunctions::default();
    // Large enough for any report of the interface, including the keyboard's.
    let mut buf = [0u8; 64];
    while running.load(Ordering::SeqCst) {
        match source.read_report(&mut buf, READ_TIMEOUT_MS) {
            Ok(0) => {}
            Ok(n) => {
                let slice = &buf[..n.min(buf.len())];
                if let Some((slot, pressed, mods, layer)) = parse_flux_action_report(slice) {
                    on_flux(slot, pressed, mods, layer);
                } else if let Some(mask) = parse_report(slice) {
                    held.update(mask).into_iter().for_each(&mut *on_press);
                }
            }
            Err(e) => return Some(e),
        }
    }
    None
}

pub fn listen(
    source: &mut dyn ReportSource,
    running: &AtomicBool,
    on_press: &mut dyn FnMut(u32),
) -> Option<String> {
    let mut ignore_flux = |_: u16, _: bool, _: u8, _: u8| {};
    listen_with_actions(source, running, on_press, &mut ignore_flux)
}

/// Why the function-key collection couldn't be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// No Keyglass firmware is connected.
    NoKeyboard,
    /// The firmware is connected but its function keys can't be read; the text says why.
    Unavailable(String),
}

/// Starts the listener thread. It runs until `running` is cleared, opening the keyboard's
/// function-key collection whenever the keyboard is connected. When the collection can't be
/// read, it says so once and keeps trying.
pub fn spawn(state: Arc<ApiState>, running: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("keyglass-function-keys".into())
        .spawn(move || run(&state, &running))
        .expect("function-key listener thread")
}

fn run(state: &ApiState, running: &AtomicBool) {
    let mut api: Option<hidapi::HidApi> = None;
    let mut reported_unavailable = false;
    let mut reported_listening = false;
    while running.load(Ordering::SeqCst) {
        match open(&mut api) {
            Ok(mut device) => {
                if !reported_listening {
                    eprintln!("keyglassd: listening for Keyglass function keys");
                    reported_listening = true;
                }
                let mut on_press = |code: u32| match on_function_pressed(state, code) {
                    FunctionAction::AutoProfileLock { locked } => eprintln!(
                        "keyglassd: the Auto-Profile Lock key {} automatic profile switching",
                        if locked { "locked" } else { "unlocked" }
                    ),
                    FunctionAction::None => {}
                };
                let mut on_flux = |slot: u16, pressed: bool, mods: u8, layer: u8| {
                    state.v2.dispatch_flux_action(slot, pressed, mods, layer);
                };
                // A read error means the keyboard went away; it is looked for again below.
                let _ = listen_with_actions(&mut device, running, &mut on_press, &mut on_flux);
            }
            Err(OpenError::NoKeyboard) => {}
            Err(OpenError::Unavailable(why)) => {
                if !reported_unavailable {
                    eprintln!("keyglassd: Keyglass function keys can't be read: {why}");
                    reported_unavailable = true;
                }
            }
        }
        sleep_while_running(running, RETRY_INTERVAL);
    }
}

fn sleep_while_running(running: &AtomicBool, total: Duration) {
    let step = Duration::from_millis(100);
    let mut slept = Duration::ZERO;
    while slept < total && running.load(Ordering::SeqCst) {
        std::thread::sleep(step);
        slept += step;
    }
}

fn open(api: &mut Option<hidapi::HidApi>) -> Result<hidapi::HidDevice, OpenError> {
    if api.is_none() {
        let created = hidapi::HidApi::new()
            .map_err(|e| OpenError::Unavailable(format!("the HID library didn't start ({e})")))?;
        *api = Some(created);
    }
    let api = api.as_mut().expect("created above");
    api.reset_devices()
        .and_then(|()| api.add_devices(KEYBOARD_VID, FIRMWARE_PID))
        .map_err(|_| OpenError::NoKeyboard)?;
    let mut firmware_seen = false;
    let mut collection = None;
    for info in api.device_list() {
        if info.vendor_id() != KEYBOARD_VID || info.product_id() != FIRMWARE_PID {
            continue;
        }
        firmware_seen = true;
        if info.usage_page() == USAGE_PAGE && info.usage() == USAGE {
            collection = Some(info);
            break;
        }
    }
    let Some(info) = collection else {
        return Err(if firmware_seen {
            OpenError::Unavailable(
                "the keyboard firmware has no function-key collection (usage page 0xFF00)".into(),
            )
        } else {
            OpenError::NoKeyboard
        });
    };
    // The keyboard's own collection is Generic Desktop (0x01) usage Keyboard (0x06).
    let shares_keyboard_interface = api
        .device_list()
        .any(|d| d.path() == info.path() && d.usage_page() == 0x01 && d.usage() == 0x06);
    if shares_keyboard_interface {
        keyboard_interface_access()?;
    }
    info.open_device(api).map_err(|e| {
        OpenError::Unavailable(format!("the keyboard's HID interface didn't open ({e})"))
    })
}

/// macOS asks the person for Input Monitoring the first time a process opens a keyboard's HID
/// interface, which the function-key collection can share. The listener opens such an interface
/// only once that access was decided, so it never brings up the prompt itself; when access was
/// denied the open is still tried, which can't prompt.
#[cfg(target_os = "macos")]
fn keyboard_interface_access() -> Result<(), OpenError> {
    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOHIDCheckAccess(request_type: u32) -> u32;
    }
    const LISTEN_EVENT: u32 = 1;
    const ACCESS_UNKNOWN: u32 = 2;
    // SAFETY: IOHIDCheckAccess takes a request type by value and only reads the access state.
    if unsafe { IOHIDCheckAccess(LISTEN_EVENT) } == ACCESS_UNKNOWN {
        return Err(OpenError::Unavailable(
            "this keyboard firmware reports function keys on the keyboard's HID interface, which macOS opens only with Input Monitoring access".into(),
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn keyboard_interface_access() -> Result<(), OpenError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_function_key_report_carries_a_little_endian_mask() {
        assert_eq!(
            parse_report(&[0x00, 0x08, 0, 0]),
            Some(1 << 11),
            "own interface"
        );
        assert_eq!(parse_report(&[0x04, 0, 0, 0x80]), Some(0x8000_0004));
        assert_eq!(
            parse_report(&[3, 0x00, 0x08, 0, 0]),
            Some(1 << 11),
            "report ID 3"
        );
        assert_eq!(parse_report(&[3, 0x04, 0, 0, 0x80]), Some(0x8000_0004));
        assert_eq!(parse_report(&[3, 1, 2, 3, 4, 0, 0]), Some(0x0403_0201));
        assert_eq!(parse_report(&[1, 0, 0, 0x04, 0]), None, "keyboard report");
        assert_eq!(parse_report(&[2, 0xE9, 0x00]), None, "consumer report");
        assert_eq!(parse_report(&[3, 0, 8]), None, "short report");
        assert_eq!(
            parse_report(&[1, 0, 0, 0x04, 0, 0, 0, 0, 0]),
            None,
            "boot keyboard report"
        );
        assert_eq!(parse_report(&[]), None);
    }

    #[test]
    fn a_key_is_reported_once_when_it_goes_down() {
        let mut held = HeldFunctions::default();
        assert_eq!(held.update(1 << 11), vec![11]);
        assert_eq!(held.update(1 << 11), Vec::<u32>::new(), "still held");
        assert_eq!(held.update(1 << 11 | 1 << 4), vec![4]);
        assert_eq!(held.update(1 << 4), Vec::<u32>::new(), "released");
        assert_eq!(held.update(0), Vec::<u32>::new());
        assert_eq!(held.update(1 << 11 | 1 << 2), vec![2, 11]);
        held.clear();
        assert_eq!(held.update(1 << 11), vec![11], "held across a reconnect");
    }

    #[test]
    fn the_auto_profile_lock_code_comes_from_the_keymap_codec() {
        assert_eq!(function_code("KC_OF_AUTO_PROFILE_LOCK"), Some(0x0B));
        assert_eq!(function_code("KC_A"), None);
    }

    /// Reports to read, then an error.
    struct Scripted(std::collections::VecDeque<Result<Vec<u8>, String>>);

    impl ReportSource for Scripted {
        fn read_report(&mut self, buf: &mut [u8], _timeout_ms: i32) -> Result<usize, String> {
            match self.0.pop_front() {
                Some(Ok(r)) => {
                    buf[..r.len()].copy_from_slice(&r);
                    Ok(r.len())
                }
                Some(Err(e)) => Err(e),
                None => Err("gone".into()),
            }
        }
    }

    #[test]
    fn listening_acts_on_press_edges_and_skips_other_reports() {
        let lock = 1u8 << 3; // bit 11 is bit 3 of the second byte
        let mut source = Scripted(
            [
                Ok(vec![1, 0, 0, 0x04, 0, 0, 0, 0, 0]),
                Ok(vec![3, 0, lock, 0, 0]),
                Ok(vec![]),
                Ok(vec![3, 0, lock, 0, 0]),
                Ok(vec![2, 0xE9, 0]),
                Ok(vec![3, 0, 0, 0, 0]),
                Ok(vec![3, 0, lock, 0, 0]),
                Err("device disconnected".into()),
            ]
            .into(),
        );
        let running = AtomicBool::new(true);
        let mut presses = Vec::new();
        let err = listen(&mut source, &running, &mut |c| presses.push(c));
        assert_eq!(err.as_deref(), Some("device disconnected"));
        assert_eq!(presses, vec![11, 11]);
    }

    #[test]
    fn listening_stops_when_asked() {
        let mut source = Scripted([Ok(vec![3, 0, 8, 0, 0])].into());
        let running = AtomicBool::new(false);
        let mut presses = Vec::new();
        assert_eq!(
            listen(&mut source, &running, &mut |c| presses.push(c)),
            None
        );
        assert!(presses.is_empty());
    }

    #[test]
    fn the_auto_profile_lock_key_toggles_the_lock() {
        let state = ApiState::new(Arc::new(crate::api::state::NoopDeviceSink), None);
        let code = function_code("KC_OF_AUTO_PROFILE_LOCK").unwrap();
        assert_eq!(
            on_function_pressed(&state, code),
            FunctionAction::AutoProfileLock { locked: true }
        );
        assert!(state.inner.lock().unwrap().auto_profile_lock);
        assert_eq!(
            on_function_pressed(&state, code),
            FunctionAction::AutoProfileLock { locked: false }
        );
        let theme = function_code("KC_OF_THEME_CYCLE_NEXT").unwrap();
        assert_eq!(on_function_pressed(&state, theme), FunctionAction::None);
        assert!(!state.inner.lock().unwrap().auto_profile_lock);
    }
}
