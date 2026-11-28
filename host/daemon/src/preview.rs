//! Minimal preview lifecycle stub (v1 software `DisplayPreview` has been removed;
//! Keyglass v2 live preview is rendered directly by `keyglass-compositor` via `GET /v2/preview/frame.png`).

use std::process::Child;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

pub struct PreviewBackend;

impl PreviewBackend {
    pub fn open_auto(
        _child_slot: Arc<Mutex<Option<Child>>>,
        _cancel: Option<(&Arc<AtomicU64>, u64)>,
    ) -> Result<Self, String> {
        Err("v1 preview has been removed; use Keyglass v2 /v2/preview/frame.png".into())
    }

    pub fn set_config_json(
        &mut self,
        _appearance_json: Option<&str>,
        _settings_json: Option<&str>,
        _keymap_json: Option<&str>,
        _mosaic_json: Option<&str>,
        _media_dir: Option<&str>,
    ) -> Result<(), String> {
        Ok(())
    }

    pub fn apply_live_json(&mut self, _json: &str, _now_ms: u64) -> Result<(), String> {
        Ok(())
    }

    pub fn get_key_rects_json(&mut self) -> Result<Vec<serde_json::Value>, String> {
        Err("v1 preview removed".into())
    }

    pub fn render_rgba_stream_to_sized<F: FnMut(&[u8]) -> Result<(), String>>(
        &mut self,
        _now_sec: i64,
        _now_ms: u64,
        _suspended: bool,
        _half_res: bool,
        _cb: F,
    ) -> Result<(), String> {
        Err("v1 preview removed".into())
    }

    pub fn render_ppm_bytes(
        &mut self,
        _now_sec: i64,
        _now_ms: u64,
        _suspended: bool,
    ) -> Result<Vec<u8>, String> {
        Err("v1 preview removed".into())
    }
}

pub fn kill_and_reap_child(slot: &Arc<Mutex<Option<Child>>>) {
    if let Ok(mut g) = slot.lock() {
        if let Some(mut child) = g.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn release_process_heap_pages() {
    #[cfg(target_os = "linux")]
    unsafe {
        extern "C" {
            fn malloc_trim(pad: usize) -> i32;
        }
        let _ = malloc_trim(0);
    }
}

pub fn run_preview_worker_stdio() -> Result<(), String> {
    Err("v1 preview worker removed; use /v2/preview/frame.png".into())
}
