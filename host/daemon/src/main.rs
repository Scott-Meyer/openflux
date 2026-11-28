#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use keyglassd::api::assets::MAX_KEYBOARD_MEDIA_BYTES;
use keyglassd::api::build_router;
use keyglassd::api::lifecycle_client::{request_exit, RequestOutcome};
use keyglassd::api::routes::DEFAULT_API_PORT;
use keyglassd::api::state::default_data_dir;
use keyglassd::api::state::{ApiState, DeviceSink};
use keyglassd::api::third_party;
use keyglassd::api::tls::{
    ensure_tls_bundle, serve_tls_router, write_keys_discovery_json, write_owner_only_file,
};
use keyglassd::api::worker::{reconcile_tick, UsbWorkerHandle};
use keyglassd::bridges::create_platform_bridge;
use keyglassd::codecs::keymap::decode_keymap_blob;
use keyglassd::device_paths;
use keyglassd::lifecycle::ExitKind;
use keyglassd::tray::{TrayController, TrayMode};
use keyglassd::usb::libusb::KeyboardUsbClient;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Set by SIGINT and SIGTERM.
static SIGNALLED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_sig(_sig: libc::c_int) {
    SIGNALLED.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGINT, on_sig as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_sig as libc::sighandler_t);
    }
}

pub fn host_platform_code() -> u32 {
    #[cfg(target_os = "windows")]
    {
        1
    }
    #[cfg(target_os = "macos")]
    {
        2
    }
    #[cfg(target_os = "linux")]
    {
        3
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        3
    }
}

trait PairedUsbTransport {
    fn stat_file(&self, remote_path: &str) -> Result<bool, String>;
    fn get_file(&self, remote_path: &str) -> Result<Vec<u8>, String>;
    fn put_file(&self, remote_path: &str, data: &[u8]) -> Result<(), String>;
    fn delete_file(&self, remote_path: &str) -> Result<(), String>;
    fn get_keymap(&self) -> Result<Vec<u8>, String>;
    fn send_keymap(&self, blob: &[u8]) -> Result<(), String>;
    fn get_profile_settings(&self) -> Result<Vec<u8>, String>;
    fn send_profile_settings(&self, transfer_bytes: &[u8]) -> Result<(), String>;
}

impl PairedUsbTransport for KeyboardUsbClient {
    fn stat_file(&self, remote_path: &str) -> Result<bool, String> {
        KeyboardUsbClient::stat_file(self, remote_path).map_err(|e| e.to_string())
    }
    fn get_file(&self, remote_path: &str) -> Result<Vec<u8>, String> {
        KeyboardUsbClient::get_file(self, remote_path).map_err(|e| e.to_string())
    }
    fn put_file(&self, remote_path: &str, data: &[u8]) -> Result<(), String> {
        KeyboardUsbClient::put_file(self, remote_path, data).map_err(|e| e.to_string())
    }
    fn delete_file(&self, remote_path: &str) -> Result<(), String> {
        KeyboardUsbClient::delete_file(self, remote_path).map_err(|e| e.to_string())
    }
    fn get_keymap(&self) -> Result<Vec<u8>, String> {
        KeyboardUsbClient::get_keymap(self).map_err(|e| e.to_string())
    }
    fn send_keymap(&self, blob: &[u8]) -> Result<(), String> {
        KeyboardUsbClient::send_keymap(self, blob).map_err(|e| e.to_string())
    }
    fn get_profile_settings(&self) -> Result<Vec<u8>, String> {
        let raw = KeyboardUsbClient::firmware_file_to_host(self, 0).map_err(|e| e.to_string())?;
        let parsed = keyglassd::codecs::keymap::decode_profile_settings(&raw, false)
            .map_err(|e| format!("decode firmware profile settings: {e}"))?;
        parsed
            .to_transfer_bytes()
            .map_err(|e| format!("encode firmware profile settings for transfer: {e}"))
    }
    fn send_profile_settings(&self, transfer_bytes: &[u8]) -> Result<(), String> {
        KeyboardUsbClient::firmware_file_from_host(self, transfer_bytes).map_err(|e| e.to_string())
    }
}

fn parse_nul_listing(listing: &str) -> Vec<&str> {
    listing
        .split('\0')
        .filter(|e| !e.is_empty())
        .map(|entry| entry.split_once(':').map(|(_, n)| n).unwrap_or(entry))
        .collect()
}

/// Saves a plan to the keyboard as one transaction.
///
/// Every document, icon, media file, keymap blob and profile settings blob is validated, and the
/// current contents of everything about to be replaced are read back, before anything is written.
/// The keymap and profile settings go first (firmware, then the bridge's copy); icons, media and the
/// remaining documents follow. A failure rolls back what was written. Errors that leave the two
/// copies out of step start with `PARTIAL_SAVE_MISMATCH` (known mismatch) or `PARTIAL_SAVE_UNKNOWN`
/// (state couldn't be confirmed).
fn execute_atomic_save(
    transport: &dyn PairedUsbTransport,
    save_plan: &serde_json::Value,
) -> Result<(), String> {
    // 1. Validate everything in memory.
    let mut non_keymap_files: Vec<(&'static str, Vec<u8>)> = Vec::new();
    for (fname, key) in [
        (device_paths::APPEARANCE_JSON, "appearance"),
        (device_paths::SETTINGS_JSON, "settings"),
        (device_paths::MOSAIC_JSON, "mosaic"),
    ] {
        if let Some(obj) = save_plan.get(key) {
            let bytes = serde_json::to_vec_pretty(obj).map_err(|e| e.to_string())?;
            non_keymap_files.push((fname, bytes));
        }
    }

    let paired_profiles = match (
        save_plan.get("profiles"),
        save_plan.get("profileSettingsHex").and_then(|v| v.as_str()),
    ) {
        (Some(prof_obj), Some(hex_ps)) => {
            let prof_bytes = serde_json::to_vec_pretty(prof_obj).map_err(|e| e.to_string())?;
            let ps_bytes = ApiState::decode_hex_bytes(hex_ps)
                .map_err(|e| format!("invalid profileSettingsHex: {e}"))?;
            keyglassd::codecs::keymap::decode_profile_settings(&ps_bytes, true)
                .map_err(|e| format!("invalid profileSettings: {e}"))?;
            Some((prof_bytes, ps_bytes))
        }
        (Some(_), None) => return Err("profiles present without paired profileSettingsHex".into()),
        (None, Some(_)) => return Err("profileSettingsHex present without paired profiles".into()),
        (None, None) => None,
    };

    let mut icon_uploads: Vec<(String, Vec<u8>)> = Vec::new();
    if let Some(icons) = save_plan.get("icons").and_then(|v| v.as_array()) {
        for ic in icons {
            let Some(name) = ic.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(lp) = ic.get("localPath").and_then(|v| v.as_str()) else {
                continue;
            };
            let data = std::fs::read(lp).map_err(|e| format!("read icon {lp}: {e}"))?;
            icon_uploads.push((device_paths::key_icon_path(name), data));
        }
    }

    let mut media_uploads: Vec<(String, String)> = Vec::new();
    if let Some(mfiles) = save_plan.get("mediaFiles").and_then(|v| v.as_array()) {
        for mf in mfiles {
            let Some(name) = mf.get("name").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(lp) = mf.get("localPath").and_then(|v| v.as_str()) else {
                continue;
            };
            let meta = std::fs::metadata(lp).map_err(|e| format!("stat media {lp}: {e}"))?;
            if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_KEYBOARD_MEDIA_BYTES {
                return Err(format!(
                    "media file is empty or exceeds the {} MiB keyboard media limit: {lp}",
                    MAX_KEYBOARD_MEDIA_BYTES / (1024 * 1024)
                ));
            }
            media_uploads.push((device_paths::media_path(name), lp.to_string()));
        }
    }

    let paired_keymap = match (
        save_plan.get("keymap"),
        save_plan.get("keymapBlobHex").and_then(|v| v.as_str()),
    ) {
        (Some(km_obj), Some(hex_blob)) => {
            let km_bytes = serde_json::to_vec_pretty(km_obj).map_err(|e| e.to_string())?;
            let blob_bytes = ApiState::decode_hex_bytes(hex_blob)
                .map_err(|e| format!("invalid keymapBlobHex: {e}"))?;
            decode_keymap_blob(&blob_bytes).map_err(|e| format!("invalid keymap blob: {e}"))?;
            keyglassd::codecs::keymap::verify_keymap_matches_blob(km_obj, &blob_bytes)
                .map_err(|e| format!("keymap JSON and blob in the save plan don't match: {e}"))?;
            Some((km_bytes, blob_bytes))
        }
        (Some(_), None) => return Err("keymap present without paired keymapBlobHex".into()),
        (None, Some(_)) => return Err("keymapBlobHex present without paired keymap JSON".into()),
        (None, None) => None,
    };

    // 2. Read back the current contents of everything about to be replaced, on both the firmware and
    //    the bridge, before writing either.
    let snapshot_prior = |remote: &str| -> Result<Option<Vec<u8>>, String> {
        match transport.get_file(remote) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) => {
                // A file counts as absent only when STAT_FILE says so (`absent\0`).
                if matches!(transport.stat_file(remote), Ok(false)) {
                    return Ok(None);
                }
                Err(format!(
                    "Preflight backup read {remote} failed (refusing unsafe overwrite): {e}"
                ))
            }
        }
    };
    let prev_fw_km = if paired_keymap.is_some() {
        Some(
            transport
                .get_keymap()
                .map_err(|e| format!("Reading the current firmware keymap failed: {e}"))?,
        )
    } else {
        None
    };
    let prev_bridge_km = if paired_keymap.is_some() {
        snapshot_prior(device_paths::KEYMAP_JSON)?
    } else {
        None
    };
    let prev_fw_ps =
        if paired_profiles.is_some() {
            Some(transport.get_profile_settings().map_err(|e| {
                format!("Reading the current firmware profile settings failed: {e}")
            })?)
        } else {
            None
        };
    let prev_bridge_profiles = if paired_profiles.is_some() {
        snapshot_prior(device_paths::PROFILES_JSON)?
    } else {
        None
    };
    let mut prev_non_keymap: Vec<(&'static str, Option<Vec<u8>>)> =
        Vec::with_capacity(non_keymap_files.len());
    for (fname, _) in &non_keymap_files {
        prev_non_keymap.push((*fname, snapshot_prior(fname)?));
    }
    let mut prev_icons: Vec<(String, Option<Vec<u8>>)> = Vec::with_capacity(icon_uploads.len());
    for (remote, _) in &icon_uploads {
        prev_icons.push((remote.clone(), snapshot_prior(remote)?));
    }
    let mut prev_media: Vec<(String, Option<Vec<u8>>)> = Vec::with_capacity(media_uploads.len());
    for (remote, _) in &media_uploads {
        prev_media.push((remote.clone(), snapshot_prior(remote)?));
    }

    // 3. Keymap and profile settings first, so a failure there leaves appearance and media untouched.
    let mut committed_keymap = false;
    if let Some((km_bytes, blob_bytes)) = paired_keymap {
        let prev_fw = prev_fw_km.as_ref().unwrap();
        if let Err(e) = transport.send_keymap(&blob_bytes) {
            match transport.get_keymap() {
                Ok(cur) if cur == blob_bytes => {
                    // The firmware stored the keymap although the acknowledgement was lost.
                }
                Ok(cur) if cur == *prev_fw => {
                    return Err(format!("send_keymap: {e} (firmware unchanged)"));
                }
                Ok(_) | Err(_) => {
                    return Err(format!(
                        "PARTIAL_SAVE_UNKNOWN: firmware send_keymap failed ({e}) and readback could not confirm its state"
                    ));
                }
            }
        }

        if let Err(e) = transport.put_file(device_paths::KEYMAP_JSON, &km_bytes) {
            match transport.get_file(device_paths::KEYMAP_JSON) {
                Ok(readback) if readback == km_bytes => {
                    committed_keymap = true;
                }
                Ok(readback) if prev_bridge_km.as_deref() == Some(readback.as_slice()) => {
                    if let Err(rb_err) = transport.send_keymap(prev_fw) {
                        return Err(format!(
                            "PARTIAL_SAVE_MISMATCH: bridge keymap.json failed ({e}) and the firmware rollback failed ({rb_err}); pull or save again"
                        ));
                    }
                    return Err(format!(
                        "put_file({}): {e} (firmware keymap rolled back cleanly)",
                        device_paths::KEYMAP_JSON
                    ));
                }
                Ok(_) | Err(_) => {
                    return Err(format!(
                        "PARTIAL_SAVE_UNKNOWN: bridge keymap.json write failed ({e}) and readback failed or differed; pull or save again"
                    ));
                }
            }
        } else {
            committed_keymap = true;
        }
    }

    let rollback_prior_keymap = |reason: &str| -> String {
        if !committed_keymap {
            return reason.to_string();
        }
        let mut rb_errs = Vec::new();
        if let Some(ref po) = prev_fw_km {
            if let Err(re) = transport.send_keymap(po) {
                rb_errs.push(format!("firmware_keymap: {re}"));
            }
        }
        match prev_bridge_km {
            Some(ref pg) => {
                if let Err(re) = transport.put_file(device_paths::KEYMAP_JSON, pg) {
                    rb_errs.push(format!("bridge_keymap: {re}"));
                }
            }
            None => {
                if let Err(re) = transport.delete_file(device_paths::KEYMAP_JSON) {
                    rb_errs.push(format!("bridge_keymap delete: {re}"));
                }
            }
        }
        if rb_errs.is_empty() {
            format!("{reason} (keymap rolled back cleanly)")
        } else {
            format!(
                "PARTIAL_SAVE_MISMATCH: {reason}; rollback errors: {}",
                rb_errs.join(", ")
            )
        }
    };

    let mut committed_profiles = false;
    if let Some((prof_bytes, ps_bytes)) = paired_profiles {
        let prev_ps = prev_fw_ps.as_ref().unwrap();
        if let Err(e) = transport.send_profile_settings(&ps_bytes) {
            match transport.get_profile_settings() {
                Ok(cur) if cur == ps_bytes => {}
                Ok(cur) if cur == *prev_ps => {
                    return Err(rollback_prior_keymap(&format!(
                        "send_profile_settings: {e} (firmware profile settings unchanged)"
                    )));
                }
                Ok(_) | Err(_) => {
                    return Err(format!(
                        "PARTIAL_SAVE_UNKNOWN: firmware send_profile_settings failed ({e}) and readback could not confirm its state"
                    ));
                }
            }
        }

        if let Err(e) = transport.put_file(device_paths::PROFILES_JSON, &prof_bytes) {
            match transport.get_file(device_paths::PROFILES_JSON) {
                Ok(readback) if readback == prof_bytes => {
                    committed_profiles = true;
                }
                Ok(readback) if prev_bridge_profiles.as_deref() == Some(readback.as_slice()) => {
                    if let Err(rb_err) = transport.send_profile_settings(prev_ps) {
                        return Err(format!(
                            "PARTIAL_SAVE_MISMATCH: bridge profiles.json failed ({e}) and the firmware profile settings rollback failed ({rb_err})"
                        ));
                    }
                    return Err(rollback_prior_keymap(&format!(
                        "put_file({}): {e} (firmware profile settings rolled back cleanly)",
                        device_paths::PROFILES_JSON
                    )));
                }
                Ok(_) if prev_bridge_profiles.is_none() => {
                    let mut rb_errs = Vec::new();
                    if let Err(de) = transport.delete_file(device_paths::PROFILES_JSON) {
                        rb_errs.push(format!("bridge_profiles delete: {de}"));
                    }
                    if let Err(rb_err) = transport.send_profile_settings(prev_ps) {
                        rb_errs.push(format!("firmware_profiles: {rb_err}"));
                    }
                    let base_msg = rollback_prior_keymap(&format!(
                        "put_file({}): {e}",
                        device_paths::PROFILES_JSON
                    ));
                    if rb_errs.is_empty() {
                        return Err(format!(
                            "{base_msg} (partial profiles.json removed and firmware profile settings rolled back cleanly)"
                        ));
                    }
                    return Err(format!(
                        "PARTIAL_SAVE_MISMATCH: {base_msg}; rollback issues: {}",
                        rb_errs.join(", ")
                    ));
                }
                Ok(_) | Err(_) => {
                    return Err(format!(
                        "PARTIAL_SAVE_UNKNOWN: bridge profiles.json failed ({e}) and readback failed or differed"
                    ));
                }
            }
        } else {
            committed_profiles = true;
        }
    }

    // 4. Icons, media and the remaining documents; on failure, everything overwritten is restored
    //    and every file the save created is removed.
    let restore = |remote: &str, prev: Option<&[u8]>, rb_errs: &mut Vec<String>| {
        let res = match prev {
            Some(bytes) => transport.put_file(remote, bytes),
            None => transport
                .delete_file(remote)
                .map_err(|e| format!("couldn't remove the new file: {e}")),
        };
        if let Err(re) = res {
            rb_errs.push(format!("{remote}: {re}"));
        }
    };
    let rollback_all_committed = |reason: &str,
                                  written_icons: &[(String, Option<&[u8]>)],
                                  written_media: &[(String, Option<&[u8]>)],
                                  written_nk: &[(&'static str, Option<&[u8]>)]|
     -> String {
        let mut rb_errs = Vec::new();
        for (fname, prev) in written_nk.iter().rev() {
            restore(fname, *prev, &mut rb_errs);
        }
        for (remote, prev) in written_media.iter().rev() {
            restore(remote, *prev, &mut rb_errs);
        }
        for (remote, prev) in written_icons.iter().rev() {
            restore(remote, *prev, &mut rb_errs);
        }
        if committed_profiles {
            if let Some(ref ps) = prev_fw_ps {
                if let Err(re) = transport.send_profile_settings(ps) {
                    rb_errs.push(format!("firmware_profiles: {re}"));
                }
            }
            restore(
                device_paths::PROFILES_JSON,
                prev_bridge_profiles.as_deref(),
                &mut rb_errs,
            );
        }
        let base_msg = rollback_prior_keymap(reason);
        if rb_errs.is_empty() {
            base_msg
        } else {
            format!(
                "PARTIAL_SAVE_MISMATCH: {base_msg}; rollback issues: {}",
                rb_errs.join(", ")
            )
        }
    };

    let mut written_icons: Vec<(String, Option<&[u8]>)> = Vec::new();
    let mut written_media: Vec<(String, Option<&[u8]>)> = Vec::new();
    for (idx, (remote, data)) in icon_uploads.iter().enumerate() {
        if let Err(e) = transport.put_file(remote, data) {
            written_icons.push((remote.clone(), prev_icons[idx].1.as_deref()));
            return Err(rollback_all_committed(
                &format!("put_file({remote}): {e}"),
                &written_icons,
                &written_media,
                &[],
            ));
        }
        written_icons.push((remote.clone(), prev_icons[idx].1.as_deref()));
    }
    for (idx, (remote, lp)) in media_uploads.iter().enumerate() {
        let data = match std::fs::read(lp) {
            Ok(d) => d,
            Err(e) => {
                return Err(rollback_all_committed(
                    &format!("read media {lp}: {e}"),
                    &written_icons,
                    &written_media,
                    &[],
                ))
            }
        };
        if let Err(e) = transport.put_file(remote, &data) {
            written_media.push((remote.clone(), prev_media[idx].1.as_deref()));
            return Err(rollback_all_committed(
                &format!("put_file({remote}): {e}"),
                &written_icons,
                &written_media,
                &[],
            ));
        }
        written_media.push((remote.clone(), prev_media[idx].1.as_deref()));
    }
    let mut written_nk: Vec<(&'static str, Option<&[u8]>)> = Vec::new();
    for (idx, (fname, bytes)) in non_keymap_files.iter().enumerate() {
        if let Err(e) = transport.put_file(fname, bytes) {
            match transport.get_file(fname) {
                Ok(rb) if rb == *bytes => {
                    written_nk.push((*fname, prev_non_keymap[idx].1.as_deref()));
                    return Err(rollback_all_committed(
                        &format!(
                            "put_file({fname}) reported an error after the file was written: {e}"
                        ),
                        &written_icons,
                        &written_media,
                        &written_nk,
                    ));
                }
                Ok(rb) if prev_non_keymap[idx].1.as_deref() == Some(rb.as_slice()) => {
                    return Err(rollback_all_committed(
                        &format!("put_file({fname}): {e}"),
                        &written_icons,
                        &written_media,
                        &written_nk,
                    ));
                }
                _ => {
                    written_nk.push((*fname, prev_non_keymap[idx].1.as_deref()));
                    let rb_summary = rollback_all_committed(
                        &format!("put_file({fname}) failed ({e}) and readback could not confirm prior state"),
                        &written_icons,
                        &written_media,
                        &written_nk,
                    );
                    return Err(if rb_summary.starts_with("PARTIAL_SAVE_") {
                        rb_summary
                    } else {
                        format!("PARTIAL_SAVE_UNKNOWN: {rb_summary}")
                    });
                }
            }
        }
        written_nk.push((*fname, prev_non_keymap[idx].1.as_deref()));
    }
    Ok(())
}

fn execute_device_backup(
    transport: &dyn PairedUsbTransport,
    out_dir_raw: &std::path::Path,
    versions: &serde_json::Value,
) -> Result<String, String> {
    let out_path = keyglassd::api::assets::expand_home_path(&out_dir_raw.to_string_lossy());
    std::fs::create_dir_all(&out_path).map_err(|e| format!("mkdir backup: {e}"))?;
    let mut errors: Vec<String> = Vec::new();
    for (remote, key) in device_paths::CONFIG_DOCUMENTS {
        let fname = format!("{key}.json");
        match transport.get_file(remote) {
            Ok(raw) => match serde_json::from_slice::<serde_json::Value>(&raw) {
                Ok(v) => {
                    let pretty = serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?;
                    std::fs::write(out_path.join(&fname), pretty)
                        .map_err(|e| format!("write {fname}: {e}"))?;
                }
                Err(e) => errors.push(format!("{fname} parse: {e}")),
            },
            Err(e) => errors.push(format!("{fname} pull: {e}")),
        }
    }
    match transport.get_keymap() {
        Ok(raw_km) => {
            decode_keymap_blob(&raw_km)
                .map_err(|e| format!("firmware keymap decode error: {e}"))?;
            std::fs::write(out_path.join("firmware_keymap.bin"), raw_km)
                .map_err(|e| format!("write firmware_keymap.bin: {e}"))?;
        }
        Err(e) => errors.push(format!("firmware_keymap.bin pull: {e}")),
    }
    match transport.get_profile_settings() {
        Ok(ps_xfer) => {
            keyglassd::codecs::keymap::decode_profile_settings(&ps_xfer, true)
                .map_err(|e| format!("firmware profile settings decode error: {e}"))?;
            std::fs::write(out_path.join("firmware_profile_settings.bin"), ps_xfer)
                .map_err(|e| format!("write firmware_profile_settings.bin: {e}"))?;
        }
        Err(e) => errors.push(format!("firmware_profile_settings.bin pull: {e}")),
    }
    std::fs::write(
        out_path.join("versions.json"),
        serde_json::to_vec_pretty(versions).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("write versions.json: {e}"))?;
    if !errors.is_empty() {
        return Err(format!(
            "Incomplete device backup ({}): {}",
            out_path.display(),
            errors.join("; ")
        ));
    }
    Ok(format!(
        "Backed up the configuration, firmware keymap and profile settings to {}",
        out_path.display()
    ))
}

struct UsbDeviceSink {
    client: Option<KeyboardUsbClient>,
    v2_mux: std::sync::Mutex<Option<Arc<keyglassd::usb::v2_mux::V2UsbMux>>>,
}

impl UsbDeviceSink {
    fn new() -> Self {
        Self {
            client: KeyboardUsbClient::new().ok(),
            v2_mux: std::sync::Mutex::new(None),
        }
    }

    fn attach_v2_mux(&self, mux: Arc<keyglassd::usb::v2_mux::V2UsbMux>) {
        *self.v2_mux.lock().unwrap() = Some(mux);
    }

    fn active_mux(&self) -> Option<Arc<keyglassd::usb::v2_mux::V2UsbMux>> {
        self.v2_mux
            .lock()
            .unwrap()
            .as_ref()
            .filter(|m| m.is_connected())
            .cloned()
    }

    fn client(&self) -> Result<&KeyboardUsbClient, String> {
        self.client
            .as_ref()
            .ok_or_else(|| "The USB library couldn't be initialised".to_string())
    }
}

impl DeviceSink for UsbDeviceSink {
    fn is_v2_mux_active(&self) -> bool {
        self.active_mux().is_some()
    }

    fn set_gamepad_config(
        &self,
        mode: u8,
        deadzone_um: u16,
        saturation_um: u16,
        curve: u8,
        radial_clamp: bool,
    ) -> Result<(), String> {
        self.client()?
            .set_gamepad_config(mode, deadzone_um, saturation_um, curve, radial_clamp)
            .map_err(|e| e.to_string())
    }

    fn set_layer_override(&self, layer: Option<u8>) -> Result<(), String> {
        self.client()?
            .set_layer_override(layer)
            .map_err(|e| e.to_string())
    }

    fn send_media_frame(
        &self,
        stream_name: &str,
        codec: keyglass_bus::MediaCodec,
        data: &[u8],
        flags: u8,
    ) -> bool {
        self.active_mux()
            .map(|m| m.send_media(stream_name, codec, data, flags))
            .unwrap_or(false)
    }

    fn is_connected(&self) -> (bool, bool) {
        let (has_bridge_probe, has_fw) = self
            .client
            .as_ref()
            .map(|c| c.probe_attached())
            .unwrap_or((false, false));
        (self.active_mux().is_some() || has_bridge_probe, has_fw)
    }

    fn versions(&self) -> serde_json::Value {
        if let Some(m) = self.active_mux() {
            if let Ok(mut v) = m.get_versions() {
                if let Some(obj) = v.as_object_mut() {
                    if !obj.contains_key("firmware") {
                        let fw = self
                            .client
                            .as_ref()
                            .and_then(|c| c.get_firmware_version().ok());
                        obj.insert("firmware".into(), serde_json::json!(fw));
                    }
                    // Read real installed display version stamp and ensure canonical `keyglass-display <ver>` prefix
                    let raw_disp = m
                        .get_file("/home/root/.keyglass/versions/display")
                        .ok()
                        .and_then(|b| String::from_utf8(b).ok())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .or_else(|| {
                            obj.get("display")
                                .and_then(|x| x.as_str())
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                        });
                    if let Some(disp) = raw_disp {
                        let canon_disp = if disp.starts_with("keyglass-display ") {
                            disp
                        } else {
                            format!("keyglass-display {disp}")
                        };
                        obj.insert("display".into(), serde_json::json!(canon_disp));
                    }
                }
                return v;
            }
        }
        self.client
            .as_ref()
            .map(|c| c.get_versions())
            .unwrap_or_else(keyglassd::api::state::empty_versions)
    }

    fn send_live_json(&self, _payload: &serde_json::Value) -> Result<(), String> {
        // Pure OFV2 mode: legacy v1 live_json is disabled (never contend on bridge flock!).
        Err("legacy v1 live_json disabled in OFV2 mode".into())
    }

    fn send_bus_frame(&self, _frame: &keyglass_bus::WireFrame) -> Result<(), String> {
        Err("legacy v1 bus frame disabled in OFV2 mode".into())
    }

    fn poll_bus_frames(&self) -> Result<Vec<keyglass_bus::WireFrame>, String> {
        Ok(Vec::new())
    }

    fn set_system_time(&self, _utc_timestamp: &str, zone: &str) -> Result<(), String> {
        let Some(m) = self.active_mux() else {
            return Err("OFV2 USB bridge not connected yet".into());
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        m.set_clock(now, zone)
    }

    fn set_active_profile(&self, profile_idx: u32) -> Result<(), String> {
        let client = self.client()?;
        client
            .set_active_profile(profile_idx)
            .map_err(|e| e.to_string())
    }

    fn set_language(&self, lang_code: u32) -> Result<(), String> {
        let client = self.client()?;
        client.set_language(lang_code).map_err(|e| e.to_string())
    }

    fn set_platform(&self, platform_code: u32) -> Result<(), String> {
        let client = self.client()?;
        client
            .set_platform(platform_code)
            .map_err(|e| e.to_string())
    }

    fn read_firmware_runtime(&self) -> Result<keyglassd::api::state::FirmwareRuntime, String> {
        let raw = self
            .client()?
            .get_runtime_state()
            .map_err(|e| e.to_string())?;
        keyglassd::api::state::FirmwareRuntime::decode(&raw)
    }

    fn save_config_dir(&self, save_plan: &serde_json::Value) -> Result<(), String> {
        let client = self.client()?;
        execute_atomic_save(client, save_plan)
    }

    fn pull_config_dir(&self) -> Result<serde_json::Value, String> {
        let mut out = serde_json::Map::new();
        if let Some(m) = self.active_mux() {
            for (fname, key) in device_paths::CONFIG_DOCUMENTS {
                if let Ok(raw) = m.get_file(fname) {
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&raw) {
                        out.insert(key.into(), v);
                    }
                }
            }
            if let Ok(client) = self.client() {
                if let Ok(raw_km) = client.get_keymap() {
                    if decode_keymap_blob(&raw_km).is_ok() {
                        out.insert(
                            "firmwareKeymapBlobHex".into(),
                            serde_json::Value::String(ApiState::encode_hex_bytes(&raw_km)),
                        );
                    }
                }
            }
            return Ok(serde_json::Value::Object(out));
        }
        let client = self.client()?;
        for (fname, key) in device_paths::CONFIG_DOCUMENTS {
            if let Ok(raw) = client.get_file(fname) {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&raw) {
                    out.insert(key.into(), v);
                }
            }
        }
        if let Ok(raw_km) = client.get_keymap() {
            decode_keymap_blob(&raw_km)
                .map_err(|e| format!("firmware keymap decode error: {e}"))?;
            let hex: String = ApiState::encode_hex_bytes(&raw_km);
            out.insert(
                "firmwareKeymapBlobHex".into(),
                serde_json::Value::String(hex),
            );
        }
        if let Ok(ps_xfer) = PairedUsbTransport::get_profile_settings(client) {
            let hex: String = ApiState::encode_hex_bytes(&ps_xfer);
            out.insert(
                "firmwareProfileSettingsHex".into(),
                serde_json::Value::String(hex),
            );
        }
        if out.is_empty() {
            return Err("No config pulled from attached keyboard".into());
        }
        Ok(serde_json::Value::Object(out))
    }

    fn backup_device(&self, out_dir: &str) -> Result<String, String> {
        let versions = self.versions();
        if let Some(m) = self.active_mux() {
            // Backup directly over the live OFV2 mux without contending for the legacy v1 USB lock!
            let out_path = std::path::Path::new(out_dir);
            std::fs::create_dir_all(out_path).map_err(|e| format!("mkdir backup: {e}"))?;
            let mut saved = 0usize;
            for name in ["state.json", "display_prefs.json", "sync_meta.json", "resolved_keys.json"] {
                let remote = format!("/home/root/keyglass/v2/{name}");
                if let Ok(bytes) = m.get_file(&remote) {
                    if std::fs::write(out_path.join(name), bytes).is_ok() {
                        saved += 1;
                    }
                }
            }
            if let Ok(vj) = serde_json::to_vec_pretty(&versions) {
                let _ = std::fs::write(out_path.join("versions.json"), vj);
            }
            return Ok(format!(
                "Backed up {saved} Keyglass keyboard config files + versions to {}",
                out_path.display()
            ));
        }
        let client = self.client()?;
        execute_device_backup(client, std::path::Path::new(out_dir), &versions)
    }

    fn flash_uf2(&self, uf2_path: &str) -> Result<String, String> {
        let expanded = keyglassd::api::assets::expand_home_path(uf2_path);
        let bytes =
            std::fs::read(&expanded).map_err(|e| format!("read {}: {e}", expanded.display()))?;
        let client = self.client()?;
        keyglassd::usb::picoboot::flash_uf2_live(&bytes, || {
            client
                .enter_firmware_bootloader()
                .map_err(|e| e.to_string())
        })
    }

    fn get_file(&self, path: &str) -> Result<Vec<u8>, String> {
        if let Some(m) = self.active_mux() {
            return m.get_file(path);
        }
        // Fallback to stock/v1 USB bridge so Stock -> Keyglass conversion backup & rescue staging
        // work natively on a stock Flux keyboard before OFV2 is installed!
        let client = self.client()?;
        client.get_file(path).map_err(|e| e.to_string())
    }

    fn put_file(&self, path: &str, data: &[u8]) -> Result<(), String> {
        if let Some(m) = self.active_mux() {
            return m.put_file(path, data);
        }
        let client = self.client()?;
        client.put_file(path, data).map_err(|e| e.to_string())
    }

    fn stat_file(&self, path: &str) -> Result<bool, String> {
        if let Some(m) = self.active_mux() {
            return m.stat_file(path);
        }
        let client = self.client()?;
        client.stat_file(path).map_err(|e| e.to_string())
    }

    fn list_files(&self, dir: &str) -> Result<Vec<(String, u64)>, String> {
        if let Some(m) = self.active_mux() {
            let names = m.list_files(dir)?;
            return Ok(names.into_iter().map(|n| (n, 0)).collect());
        }
        let client = self.client()?;
        let raw = client.get_file_list(dir).map_err(|e| e.to_string())?;
        Ok(raw
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|name| (name.to_string(), 0))
            .collect())
    }

    fn read_firmware_keymap(&self) -> Result<Vec<u8>, String> {
        let client = self.client()?;
        client.get_keymap().map_err(|e| e.to_string())
    }

    fn write_firmware_keymap(&self, blob: &[u8]) -> Result<(), String> {
        let client = self.client()?;
        client.send_keymap(blob).map_err(|e| e.to_string())
    }

    fn press_refinement_status(&self) -> Result<keyglassd::usb::press_refinement::Status, String> {
        let client = self.client()?;
        let raw = client
            .press_refinement_status()
            .map_err(|e| e.to_string())?;
        keyglassd::usb::press_refinement::parse_status(&raw)
    }

    fn put_press_refinement(&self, record: &[u8]) -> Result<(), String> {
        let client = self.client()?;
        client
            .put_press_refinement(record)
            .map_err(|e| e.to_string())
    }

    fn get_press_refinement(&self) -> Result<Vec<u8>, String> {
        let client = self.client()?;
        client.get_press_refinement().map_err(|e| e.to_string())
    }

    fn bridge_armed_sentinel_present(&self) -> Result<bool, String> {
        if let Some(m) = self.active_mux() {
            return m.stat_file(device_paths::BRIDGE_ROLLBACK_ARMED);
        }
        let client = self.client()?;
        let listing = client
            .get_file_list(device_paths::BRIDGE_ROLLBACK_DIR)
            .map_err(|e| e.to_string())?;
        Ok(parse_nul_listing(&listing).contains(&"armed"))
    }

    fn confirm_bridge_install(&self) -> Result<(), String> {
        if let Some(m) = self.active_mux() {
            let _ = m.delete_file(device_paths::BRIDGE_ROLLBACK_ARMED);
            return Ok(());
        }
        let client = self.client()?;
        // DELETE_FILE succeeds when the file is already gone and fails only on real errors.
        client
            .delete_file(device_paths::BRIDGE_ROLLBACK_ARMED)
            .map_err(|e| format!("delete rollback armed sentinel: {e}"))?;
        Ok(())
    }

    fn sideload_ipk(&self, ipk_path: &str) -> Result<String, String> {
        if let Some(m) = self.active_mux() {
            return m.sideload_ipk(std::path::Path::new(ipk_path));
        }
        let client = self.client()?;
        let p = PathBuf::from(ipk_path);
        let fname = p
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| "Invalid IPK filename".to_string())?;
        let data = std::fs::read(&p).map_err(|e| format!("read {ipk_path}: {e}"))?;
        let remote = format!("{}/{fname}", device_paths::PACKAGE_UPLOAD_DIR);
        client
            .put_file(&remote, &data)
            .map_err(|e| format!("put_file({remote}): {e}"))?;
        Ok(format!(
            "Uploaded {fname} ({} B) to {remote}. The keyboard installs it and reconnects; check the versions afterwards.",
            data.len()
        ))
    }

    fn read_device_pomodoro_state(&self) -> Option<serde_json::Value> {
        if self.active_mux().is_some() {
            return None; // In OFV2 mode, Pomodoro state flows over the unified bus!
        }
        let client = self.client.as_ref()?;
        let raw = client.get_file(device_paths::POMODORO_STATE_JSON).ok()?;
        serde_json::from_slice::<serde_json::Value>(&raw).ok()
    }

    fn read_device_soc_stats(&self) -> Option<serde_json::Value> {
        let m = self.active_mux()?;
        if let Ok(raw) = m.get_file(device_paths::SOC_STATS_JSON) {
            if let Ok(val) = serde_json::from_slice::<serde_json::Value>(&raw) {
                return Some(val);
            }
        }
        m.get_soc_stats().ok()
    }
}

const USAGE: &str = "\
Usage: keyglassd [options]

  --help, -h           Print this help and exit.
  --version            Print the version and exit.
  --request-restart    Ask the running daemon to restart. Exit status: 0 restarted, 3 deferred
                       (it restarts by itself once keyboard work finishes), 4 not running.
  --request-stop       Ask the running daemon to stop. Exit status: 0 stopped, 3 deferred
                       (it stops by itself once keyboard work finishes), 4 not running.
  --once               Run one reconcile pass and exit.
  --no-tray            Don't start the tray.
  --no-usb             Don't open the keyboard.
  --plaintext          Serve the local API over plain HTTP.
";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version") {
        println!("keyglassd {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{USAGE}");
        return;
    }
    for (flag, kind) in [
        ("--request-restart", ExitKind::Restart),
        ("--request-stop", ExitKind::Stop),
    ] {
        if args.iter().any(|a| a == flag) {
            let data_dir = default_data_dir();
            let outcome = request_exit(&data_dir, kind, Duration::from_secs(20));
            match &outcome {
                RequestOutcome::Error(e) => eprintln!("{e}"),
                other => println!("{}", other.label(kind)),
            }
            std::process::exit(outcome.exit_code());
        }
    }
    if args.iter().any(|a| a == "--preview-worker") {
        if let Err(e) = keyglassd::preview::run_preview_worker_stdio() {
            eprintln!("keyglassd preview-worker error: {e}");
            std::process::exit(1);
        }
        return;
    }
    std::process::exit(run_daemon(&args));
}

/// Runs the daemon until it is signalled or asked to exit; returns the process exit status.
fn run_daemon(args: &[String]) -> i32 {
    install_signal_handlers();
    let once = args.iter().any(|a| a == "--once");
    let no_tray = args.iter().any(|a| a == "--no-tray");
    let plaintext = args.iter().any(|a| a == "--plaintext")
        || std::env::var("KEYGLASS_API_PLAINTEXT").as_deref() == Ok("1");

    // One daemon per data directory. Nothing is read or written, and no device is opened, before
    // this process holds the lock. The lock also tells `--request-stop` whether a daemon that
    // doesn't answer is really gone.
    let data_dir = default_data_dir();
    let _instance_lock = match keyglassd::lifecycle::InstanceLock::acquire(&data_dir) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            eprintln!(
                "keyglassd: another keyglassd is already running for {}; exiting",
                data_dir.display()
            );
            return 0;
        }
        Err(e) => {
            eprintln!("keyglassd: {e}");
            return 1;
        }
    };

    let tray = TrayController::new(
        if no_tray {
            TrayMode::Headless
        } else {
            TrayMode::NativeStatus
        },
        std::env::var("KEYGLASS_GUI_BIN").ok(),
    );

    let cfg_dir = data_dir.join("config");
    let _ = std::fs::create_dir_all(&cfg_dir);
    let secret_path = cfg_dir.join("enrollment.secret");
    let enrollment_secret = match enrollment_secret(&secret_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("keyglassd: {e}");
            return 1;
        }
    };

    let no_usb = args.iter().any(|a| a == "--no-usb")
        || std::env::var("KEYGLASS_DISABLE_USB").as_deref() == Ok("1");
    let usb_sink = if no_usb {
        None
    } else {
        Some(Arc::new(UsbDeviceSink::new()))
    };
    let device: Arc<dyn DeviceSink> = match usb_sink {
        Some(ref s) => s.clone(),
        None => Arc::new(keyglassd::api::state::NoopDeviceSink),
    };
    let local_token = std::env::var("KEYGLASS_API_TOKEN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| ApiState::generate_csprng_token().ok());
    let api_state = ApiState::with_persistent_stores(
        device.clone(),
        local_token,
        Duration::from_secs(20),
        data_dir.join("assets/key_icons"),
        cfg_dir.join("api_tokens.json"),
        enrollment_secret,
    );
    let updates = keyglassd::updates::UpdateService::new(
        keyglassd::updates::UpdateConfig::installed(data_dir.clone()),
        device.clone(),
        api_state.lifecycle.clone(),
    );
    api_state.set_updates(updates.clone());

    if once {
        let mut bridge = create_platform_bridge();
        reconcile_tick(&api_state, &mut *bridge, &*device, host_platform_code());
        return 0;
    }

    // A signal or the tray's Quit asks for a stop like `--request-stop`: keyboard work finishes
    // first. The runtime, the API and the worker keep running until the exit is due.
    let running_flag = Arc::new(AtomicBool::new(true));
    if let Some(ref sink) = usb_sink {
        let mux = keyglassd::usb::v2_mux::V2UsbMux::start(
            api_state.v2.broker.clone(),
            running_flag.clone(),
        );
        sink.attach_v2_mux(mux);
    }
    {
        let lifecycle = api_state.lifecycle.clone();
        let running = running_flag.clone();
        std::thread::spawn(move || {
            keyglassd::lifecycle::watch_exit(&lifecycle, &SIGNALLED, &running)
        });
    }
    {
        let lifecycle = api_state.lifecycle.clone();
        tray.start(
            running_flag.clone(),
            Arc::new(move || {
                lifecycle.request_exit(ExitKind::Stop);
            }),
        );
    }

    let bridge = create_platform_bridge();
    let worker = UsbWorkerHandle::spawn(
        api_state.clone(),
        device.clone(),
        bridge,
        host_platform_code(),
        running_flag.clone(),
    );
    api_state.set_worker(worker.clone());
    if !no_usb {
        keyglassd::usb::function_keys::spawn(api_state.clone(), running_flag.clone());
    }
    updates.start();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(4)
        .thread_keep_alive(Duration::from_secs(2))
        .build()
        .expect("tokio runtime");

    let api_addr: SocketAddr = std::env::var("KEYGLASS_API_ADDR")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], DEFAULT_API_PORT)));

    let api_router = build_router(api_state.clone());
    let state_for_server = api_state.clone();

    rt.block_on(async move {
        let listener = match tokio::net::TcpListener::bind(api_addr).await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("keyglassd: couldn't bind {api_addr} ({e}); using a free port");
                tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("ephemeral bind")
            }
        };
        let bound_addr = listener.local_addr().expect("local_addr");

        if plaintext {
            write_plaintext_discovery(&data_dir.join("config"), bound_addr);
            let server = axum::serve(listener, api_router).with_graceful_shutdown(async move {
                while running_flag.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            });
            let _ = server.await;
        } else {
            let bundle = ensure_tls_bundle(&data_dir.join("tls")).expect("tls bundle");
            let _ = write_keys_discovery_json(
                &data_dir.join("config"),
                bound_addr,
                &bundle.cert_path,
                &bundle.cert_sha256,
            );
            match third_party::DiscoveryEnv::for_daemon() {
                Some(env) => {
                    for (path, err) in third_party::start(
                        &state_for_server,
                        &env,
                        third_party::DiscoveryPlatform::current(),
                        bound_addr.port(),
                        &bundle.cert_pem,
                        &bundle.cert_sha256,
                    ) {
                        eprintln!("keyglassd: couldn't write third-party discovery file {}: {err}", path.display());
                    }
                }
                None => {
                    eprintln!(
                        "keyglassd: third-party discovery is off because KEYGLASS_DATA_DIR names a custom data directory; set KEYGLASS_HOME_OVERRIDE to enable it in another location"
                    );
                    third_party::start_without_discovery(&state_for_server);
                }
            }
            // Plain loopback HTTP listener (`http://127.0.0.1:52324`) for `/v2/*` so 3rd-party
            // host scripts (Python/Node/curl/Go) work cleanly without self-signed TLS workarounds!
            let v2_plain_router = keyglassd::api::v2::router(state_for_server.clone());
            let v2_plain_running = running_flag.clone();
            tokio::spawn(async move {
                if let Ok(plain_listener) =
                    tokio::net::TcpListener::bind("127.0.0.1:52324").await
                {
                    let _ = axum::serve(plain_listener, v2_plain_router)
                        .with_graceful_shutdown(async move {
                            while v2_plain_running.load(Ordering::Relaxed) {
                                tokio::time::sleep(Duration::from_millis(200)).await;
                            }
                        })
                        .await;
                }
            });
            serve_tls_router(listener, bundle.acceptor, api_router, running_flag).await;
            third_party::stop(&state_for_server);
        }
    });
    worker.shutdown_and_join();
    api_state.v2.shutdown();
    keyglassd::preview::kill_and_reap_child(&api_state.preview_child);
    api_state
        .lifecycle
        .exit_due()
        .map(ExitKind::exit_code)
        .unwrap_or(0)
}

/// The enrollment secret: `KEYGLASS_ENROLLMENT_SECRET`, else the one saved at `path`. A missing
/// or empty secret is replaced by a new one, saved for the next start; one that can't be read is
/// replaced for this run only. A secret is never empty, since an empty one would match a request
/// that sends an empty secret.
fn enrollment_secret(path: &std::path::Path) -> Result<String, String> {
    if let Some(s) = std::env::var("KEYGLASS_ENROLLMENT_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
    {
        return Ok(s);
    }
    match std::fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => return Ok(s.trim().to_string()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            eprintln!(
                "keyglassd: couldn't read the enrollment secret {}: {e}; using a new one until the next start",
                path.display()
            );
            return ApiState::generate_csprng_token();
        }
    }
    let secret = ApiState::generate_csprng_token()?;
    if let Err(e) = write_owner_only_file(path, secret.as_bytes(), true) {
        eprintln!(
            "keyglassd: couldn't save the enrollment secret {}: {e}; it changes at the next start",
            path.display()
        );
    }
    Ok(secret)
}

/// In plaintext mode the discovery file names the port and scheme; there is no certificate.
fn write_plaintext_discovery(cfg_dir: &std::path::Path, bound_addr: SocketAddr) {
    let doc = serde_json::json!({
        "openPort": bound_addr.port(),
        "host": bound_addr.ip().to_string(),
        "scheme": "http",
    });
    let _ = std::fs::create_dir_all(cfg_dir);
    let _ = write_owner_only_file(
        &cfg_dir.join("keys.json"),
        serde_json::to_string_pretty(&doc)
            .unwrap_or_default()
            .as_bytes(),
        true,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    struct MockTransport {
        files: RefCell<BTreeMap<String, Vec<u8>>>,
        firmware_blob: RefCell<Vec<u8>>,
        firmware_profile_settings: RefCell<Vec<u8>>,
        send_count: std::cell::Cell<u32>,
        put_count: std::cell::Cell<u32>,
        fail_get_keymap: bool,
        fail_get_file_after_write: bool,
        nak_get_file_paths: Vec<String>,
        fail_put_paths: Vec<String>,
        fail_stat_file: bool,
        fail_send_keymap: bool,
        commit_then_timeout_send_keymap: bool,
        fail_rollback_send: bool,
        fail_put_keymap_json: bool,
        commit_then_timeout_put_keymap: bool,
        fail_send_profile_settings: bool,
        fail_put_profiles_json: bool,
    }

    impl PairedUsbTransport for MockTransport {
        fn stat_file(&self, remote_path: &str) -> Result<bool, String> {
            if self.fail_stat_file {
                return Err("Device returned NAK (stat unsupported or I/O error)".into());
            }
            if self.nak_get_file_paths.iter().any(|p| p == remote_path) {
                return Ok(true);
            }
            Ok(self.files.borrow().contains_key(remote_path))
        }
        fn get_file(&self, remote_path: &str) -> Result<Vec<u8>, String> {
            if self.nak_get_file_paths.iter().any(|p| p == remote_path) {
                return Err("Device returned NAK".into());
            }
            if self.fail_get_file_after_write
                && ((remote_path == device_paths::KEYMAP_JSON && self.send_count.get() >= 1)
                    || (remote_path == device_paths::SETTINGS_JSON && self.put_count.get() >= 1))
            {
                return Err("Simulated bridge readback failure".into());
            }
            self.files
                .borrow()
                .get(remote_path)
                .cloned()
                .ok_or_else(|| "missing".into())
        }
        fn put_file(&self, remote_path: &str, data: &[u8]) -> Result<(), String> {
            self.put_count.set(self.put_count.get() + 1);
            if self.fail_put_paths.iter().any(|p| p == remote_path) {
                return Err(format!("Simulated write NAK for {remote_path}"));
            }
            if remote_path == device_paths::KEYMAP_JSON {
                if self.commit_then_timeout_put_keymap {
                    self.files
                        .borrow_mut()
                        .insert(remote_path.to_string(), data.to_vec());
                    return Err("USB write ACK timed out after commit".into());
                }
                if self.fail_put_keymap_json {
                    return Err("Simulated bridge write NAK".into());
                }
            }
            if remote_path == device_paths::PROFILES_JSON && self.fail_put_profiles_json {
                return Err("Simulated bridge profiles.json NAK".into());
            }
            self.files
                .borrow_mut()
                .insert(remote_path.to_string(), data.to_vec());
            Ok(())
        }
        fn delete_file(&self, remote_path: &str) -> Result<(), String> {
            self.files.borrow_mut().remove(remote_path);
            Ok(())
        }
        fn get_keymap(&self) -> Result<Vec<u8>, String> {
            if self.fail_get_keymap {
                return Err("Simulated get_keymap failure".into());
            }
            Ok(self.firmware_blob.borrow().clone())
        }
        fn send_keymap(&self, blob: &[u8]) -> Result<(), String> {
            let n = self.send_count.get() + 1;
            self.send_count.set(n);
            if self.commit_then_timeout_send_keymap && n == 1 {
                *self.firmware_blob.borrow_mut() = blob.to_vec();
                return Err("Simulated firmware ACK timeout after commit".into());
            }
            if self.fail_send_keymap {
                return Err("Simulated firmware NAK".into());
            }
            if self.fail_rollback_send && n >= 2 {
                return Err("Simulated firmware rollback NAK".into());
            }
            *self.firmware_blob.borrow_mut() = blob.to_vec();
            Ok(())
        }
        fn get_profile_settings(&self) -> Result<Vec<u8>, String> {
            // The firmware returns the raw data (no 16-byte file header); the transport adds the header.
            let raw_data = self.firmware_profile_settings.borrow().clone();
            let parsed = keyglassd::codecs::keymap::decode_profile_settings(&raw_data, false)
                .map_err(|e| format!("mock decode raw profile_settings: {e}"))?;
            parsed
                .to_transfer_bytes()
                .map_err(|e| format!("mock normalize profile_settings: {e}"))
        }
        fn send_profile_settings(&self, transfer_bytes: &[u8]) -> Result<(), String> {
            if self.fail_send_profile_settings {
                return Err("Simulated firmware profile settings NAK".into());
            }
            // The firmware accepts transfer bytes and stores the data after the 16-byte header.
            keyglassd::codecs::keymap::decode_profile_settings(transfer_bytes, true)
                .map_err(|e| format!("mock reject non-transfer profile_settings: {e}"))?;
            *self.firmware_profile_settings.borrow_mut() = transfer_bytes[16..].to_vec();
            Ok(())
        }
    }

    fn base_mock(files: BTreeMap<String, Vec<u8>>, firmware_blob: Vec<u8>) -> MockTransport {
        let default_ps_xfer = keyglassd::codecs::keymap::ProfileSettingsFile {
            settings: vec![keyglassd::codecs::keymap::ProfileSetting::from_layer(0, 0).unwrap()],
            file_version: 0,
            file_type: 0,
        }
        .to_transfer_bytes()
        .unwrap();
        MockTransport {
            files: RefCell::new(files),
            firmware_blob: RefCell::new(firmware_blob),
            firmware_profile_settings: RefCell::new(default_ps_xfer[16..].to_vec()),
            send_count: std::cell::Cell::new(0),
            put_count: std::cell::Cell::new(0),
            fail_get_keymap: false,
            fail_get_file_after_write: false,
            nak_get_file_paths: Vec::new(),
            fail_put_paths: Vec::new(),
            fail_stat_file: false,
            fail_send_keymap: false,
            commit_then_timeout_send_keymap: false,
            fail_rollback_send: false,
            fail_put_keymap_json: false,
            commit_then_timeout_put_keymap: false,
            fail_send_profile_settings: false,
            fail_put_profiles_json: false,
        }
    }

    #[test]
    fn save_rolls_back_or_reports_partial_state_on_every_failure() {
        let default_bytes = include_bytes!("../tests/fixtures/default_keymap.bin").to_vec();
        let mut default_km: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/default_keymap.json")).unwrap();
        default_km["keymaps"]["ansi"]["layers"][0]["keyActions"][0]["keyCode"] =
            serde_json::json!("KC_B");
        let mut modified_blob = decode_keymap_blob(&default_bytes).unwrap();
        modified_blob.keymaps[0].layers[0][0] = 0x05;
        let new_bytes = modified_blob.to_bytes().unwrap();
        let new_hex: String = ApiState::encode_hex_bytes(&new_bytes);

        let plan = serde_json::json!({
            "keymap": default_km,
            "keymapBlobHex": new_hex,
        });

        // 1. Reading the current firmware keymap fails: nothing is written.
        let mut t_backup_fail = base_mock(
            BTreeMap::from([(device_paths::KEYMAP_JSON.into(), b"OLD_KEYMAP".to_vec())]),
            default_bytes.clone(),
        );
        t_backup_fail.fail_get_keymap = true;
        assert!(execute_atomic_save(&t_backup_fail, &plan).is_err());
        assert_eq!(
            t_backup_fail
                .files
                .borrow()
                .get(device_paths::KEYMAP_JSON)
                .unwrap(),
            b"OLD_KEYMAP"
        );
        assert_eq!(*t_backup_fail.firmware_blob.borrow(), default_bytes);

        // 2. The firmware rejects the keymap: the bridge copy is not written.
        let mut t_fw_fail = base_mock(
            BTreeMap::from([(device_paths::KEYMAP_JSON.into(), b"OLD_KEYMAP".to_vec())]),
            default_bytes.clone(),
        );
        t_fw_fail.fail_send_keymap = true;
        assert!(execute_atomic_save(&t_fw_fail, &plan).is_err());
        assert_eq!(
            t_fw_fail
                .files
                .borrow()
                .get(device_paths::KEYMAP_JSON)
                .unwrap(),
            b"OLD_KEYMAP"
        );
        assert_eq!(*t_fw_fail.firmware_blob.borrow(), default_bytes);

        // 3. The bridge write fails after the firmware write: the firmware is rolled back.
        let mut t_bridge_fail = base_mock(
            BTreeMap::from([(device_paths::KEYMAP_JSON.into(), b"OLD_KEYMAP".to_vec())]),
            default_bytes.clone(),
        );
        t_bridge_fail.fail_put_keymap_json = true;
        let err3 = execute_atomic_save(&t_bridge_fail, &plan).unwrap_err();
        assert!(err3.contains("rolled back cleanly"), "{err3}");
        assert_eq!(
            t_bridge_fail
                .files
                .borrow()
                .get(device_paths::KEYMAP_JSON)
                .unwrap(),
            b"OLD_KEYMAP"
        );
        assert_eq!(*t_bridge_fail.firmware_blob.borrow(), default_bytes);

        // 4. The bridge write and the firmware rollback both fail: PARTIAL_SAVE_MISMATCH.
        let mut t_rb_fail = base_mock(
            BTreeMap::from([(device_paths::KEYMAP_JSON.into(), b"OLD_KEYMAP".to_vec())]),
            default_bytes.clone(),
        );
        t_rb_fail.fail_put_keymap_json = true;
        t_rb_fail.fail_rollback_send = true;
        let err4 = execute_atomic_save(&t_rb_fail, &plan).unwrap_err();
        assert!(err4.contains("PARTIAL_SAVE_MISMATCH"), "{err4}");

        // 5. The bridge stores keymap.json but the acknowledgement times out: readback confirms it, no rollback.
        let mut t_commit_to = base_mock(
            BTreeMap::from([(device_paths::KEYMAP_JSON.into(), b"OLD_KEYMAP".to_vec())]),
            default_bytes.clone(),
        );
        t_commit_to.commit_then_timeout_put_keymap = true;
        assert!(execute_atomic_save(&t_commit_to, &plan).is_ok());
        assert_eq!(*t_commit_to.firmware_blob.borrow(), new_bytes);

        // 6. The firmware stores the keymap but the acknowledgement times out: readback confirms it and the save continues.
        let mut t_fw_to = base_mock(
            BTreeMap::from([(device_paths::KEYMAP_JSON.into(), b"OLD_KEYMAP".to_vec())]),
            default_bytes.clone(),
        );
        t_fw_to.commit_then_timeout_send_keymap = true;
        assert!(execute_atomic_save(&t_fw_to, &plan).is_ok());
        assert_eq!(*t_fw_to.firmware_blob.borrow(), new_bytes);
        assert_ne!(
            t_fw_to
                .files
                .borrow()
                .get(device_paths::KEYMAP_JSON)
                .unwrap(),
            b"OLD_KEYMAP"
        );

        // 7. The bridge write and its readback both fail: PARTIAL_SAVE_UNKNOWN, never a clean rollback.
        let mut t_bridge_rb_fail = base_mock(
            BTreeMap::from([(device_paths::KEYMAP_JSON.into(), b"OLD_KEYMAP".to_vec())]),
            default_bytes.clone(),
        );
        t_bridge_rb_fail.fail_put_keymap_json = true;
        t_bridge_rb_fail.fail_get_file_after_write = true;
        let err7 = execute_atomic_save(&t_bridge_rb_fail, &plan).unwrap_err();
        assert!(err7.contains("PARTIAL_SAVE_UNKNOWN"), "{err7}");

        // 8. Keymap and profile settings together: if either profile settings write fails, the firmware
        //    keymap, the bridge keymap.json and the firmware profile settings all roll back.
        let new_ps = keyglassd::codecs::keymap::ProfileSettingsFile {
            settings: vec![
                keyglassd::codecs::keymap::ProfileSetting::from_layer(0, 0).unwrap(),
                keyglassd::codecs::keymap::ProfileSetting::from_layer(1, 2).unwrap(),
            ],
            file_version: 0,
            file_type: 0,
        }
        .to_transfer_bytes()
        .unwrap();
        let new_ps_hex: String = ApiState::encode_hex_bytes(&new_ps);
        let plan_with_prof = serde_json::json!({
            "keymap": default_km,
            "keymapBlobHex": new_hex,
            "profiles": [{"id":"default","name":"Default"},{"id":"fps","name":"FPS"}],
            "profileSettingsHex": new_ps_hex,
        });

        let mut t_ps_fail = base_mock(
            BTreeMap::from([
                (device_paths::KEYMAP_JSON.into(), b"OLD_KEYMAP".to_vec()),
                (device_paths::PROFILES_JSON.into(), b"OLD_PROF".to_vec()),
            ]),
            default_bytes.clone(),
        );
        t_ps_fail.fail_send_profile_settings = true;
        let err_ps = execute_atomic_save(&t_ps_fail, &plan_with_prof).unwrap_err();
        assert!(err_ps.contains("rolled back cleanly"), "{err_ps}");
        assert_eq!(*t_ps_fail.firmware_blob.borrow(), default_bytes);
        assert_eq!(
            t_ps_fail
                .files
                .borrow()
                .get(device_paths::KEYMAP_JSON)
                .unwrap(),
            b"OLD_KEYMAP"
        );
        assert_eq!(
            t_ps_fail
                .files
                .borrow()
                .get(device_paths::PROFILES_JSON)
                .unwrap(),
            b"OLD_PROF"
        );

        let mut t_gp_fail = base_mock(
            BTreeMap::from([
                (device_paths::KEYMAP_JSON.into(), b"OLD_KEYMAP".to_vec()),
                (device_paths::PROFILES_JSON.into(), b"OLD_PROF".to_vec()),
            ]),
            default_bytes.clone(),
        );
        let orig_ps = t_gp_fail.firmware_profile_settings.borrow().clone();
        t_gp_fail.fail_put_profiles_json = true;
        let err_gp = execute_atomic_save(&t_gp_fail, &plan_with_prof).unwrap_err();
        assert!(err_gp.contains("rolled back cleanly"), "{err_gp}");
        assert_eq!(*t_gp_fail.firmware_profile_settings.borrow(), orig_ps);
        assert_eq!(*t_gp_fail.firmware_blob.borrow(), default_bytes);
        assert_eq!(
            t_gp_fail
                .files
                .borrow()
                .get(device_paths::KEYMAP_JSON)
                .unwrap(),
            b"OLD_KEYMAP"
        );
        assert_eq!(
            t_gp_fail
                .files
                .borrow()
                .get(device_paths::PROFILES_JSON)
                .unwrap(),
            b"OLD_PROF"
        );

        // 9. No previous bridge keymap or profiles and a later failure: newly created files are
        //    deleted cleanly on rollback instead of being left behind.
        let mut t_no_bridge_bak = base_mock(
            BTreeMap::from([(device_paths::PROFILES_JSON.into(), b"OLD_PROF".to_vec())]),
            default_bytes.clone(),
        );
        t_no_bridge_bak.fail_send_profile_settings = true;
        let err_no_bak = execute_atomic_save(&t_no_bridge_bak, &plan_with_prof).unwrap_err();
        assert!(
            err_no_bak.contains("keymap rolled back cleanly"),
            "{err_no_bak}"
        );
        assert!(
            !t_no_bridge_bak
                .files
                .borrow()
                .contains_key(device_paths::KEYMAP_JSON),
            "newly created keymap.json should be deleted on rollback"
        );

        let mut t_no_prof_bak = base_mock(
            BTreeMap::from([(device_paths::KEYMAP_JSON.into(), b"OLD_KEYMAP".to_vec())]),
            default_bytes.clone(),
        );
        t_no_prof_bak
            .fail_put_paths
            .push(device_paths::APPEARANCE_JSON.into());
        let plan_with_prof_and_app = serde_json::json!({
            "keymap": default_km,
            "keymapBlobHex": new_hex,
            "profiles": [{"id": "p1", "name": "P1", "actuation_mode_index": 0, "default_layer": 0}],
            "profileSettingsHex": new_ps_hex,
            "appearance": {"selectedId": "t1"},
        });
        let err_no_prof = execute_atomic_save(&t_no_prof_bak, &plan_with_prof_and_app).unwrap_err();
        assert!(
            !err_no_prof.contains("PARTIAL_SAVE_MISMATCH"),
            "{err_no_prof}"
        );
        assert!(
            !t_no_prof_bak
                .files
                .borrow()
                .contains_key(device_paths::PROFILES_JSON),
            "newly created profiles.json should be deleted on rollback"
        );

        // 10. A device backup holds all five documents and the firmware profile settings.
        let t_bak = base_mock(
            BTreeMap::from([
                (device_paths::APPEARANCE_JSON.into(), b"{}".to_vec()),
                (device_paths::SETTINGS_JSON.into(), b"{}".to_vec()),
                (device_paths::MOSAIC_JSON.into(), b"{}".to_vec()),
                (device_paths::KEYMAP_JSON.into(), serde_json::to_vec(&default_km).unwrap()),
                (
                    device_paths::PROFILES_JSON.into(),
                    br#"[{"id":"default","name":"Default","actuation_mode_index":0,"default_layer":0},{"id":"fps","name":"FPS","actuation_mode_index":1,"default_layer":2}]"#.to_vec(),
                ),
            ]),
            default_bytes.clone(),
        );
        t_bak.send_profile_settings(&new_ps).unwrap();
        let bak_dir =
            std::env::temp_dir().join(format!("keyglassd_bak_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&bak_dir);
        let bak_msg = execute_device_backup(
            &t_bak,
            &bak_dir,
            &serde_json::json!({"firmware":"keyglass-firmware 1.0.0"}),
        )
        .unwrap();
        assert!(bak_msg.contains("profile settings"), "{bak_msg}");
        let bak_profs: serde_json::Value =
            serde_json::from_slice(&std::fs::read(bak_dir.join("profiles.json")).unwrap()).unwrap();
        assert_eq!(bak_profs[1]["id"], "fps");
        assert_eq!(bak_profs[1]["name"], "FPS");
        let bak_ps_raw = std::fs::read(bak_dir.join("firmware_profile_settings.bin")).unwrap();
        let bak_ps_dec =
            keyglassd::codecs::keymap::decode_profile_settings(&bak_ps_raw, true).unwrap();
        assert_eq!(bak_ps_dec.settings.len(), 2);
        assert_eq!(bak_ps_dec.settings[1].actuation_mode_index, 1);
        assert_eq!(bak_ps_dec.settings[1].default_layer_bitfield, 1 << 2);
        let _ = std::fs::remove_dir_all(&bak_dir);

        // 11. Appearance, an icon and a keymap: if the firmware rejects the keymap, appearance.json and
        //     the icon are not overwritten.
        let icon_tmp =
            std::env::temp_dir().join(format!("keyglassd_icon_rb_{}.png", std::process::id()));
        std::fs::write(&icon_tmp, b"NEW_ICON").unwrap();
        let mixed_plan = serde_json::json!({
            "appearance": {"selectedId": "new_theme"},
            "icons": [{"name": "a.png", "localPath": icon_tmp.to_string_lossy()}],
            "keymap": default_km,
            "keymapBlobHex": new_hex,
        });
        let mut t_mix_km_fail = base_mock(
            BTreeMap::from([
                (device_paths::KEYMAP_JSON.into(), b"OLD_KEYMAP".to_vec()),
                (device_paths::APPEARANCE_JSON.into(), b"OLD_APP".to_vec()),
                (
                    "/home/root/keyglass/media/key_icons/a.png".into(),
                    b"OLD_ICON".to_vec(),
                ),
            ]),
            default_bytes.clone(),
        );
        t_mix_km_fail.fail_send_keymap = true;
        assert!(execute_atomic_save(&t_mix_km_fail, &mixed_plan).is_err());
        assert_eq!(
            t_mix_km_fail
                .files
                .borrow()
                .get(device_paths::APPEARANCE_JSON)
                .unwrap(),
            b"OLD_APP"
        );
        assert_eq!(
            t_mix_km_fail
                .files
                .borrow()
                .get("/home/root/keyglass/media/key_icons/a.png")
                .unwrap(),
            b"OLD_ICON"
        );

        // 12. An existing file that can't be read back stops the save before any write.
        let mut t_nak_prior = base_mock(
            BTreeMap::from([
                (device_paths::KEYMAP_JSON.into(), b"OLD_KEYMAP".to_vec()),
                (device_paths::APPEARANCE_JSON.into(), b"OLD_APP".to_vec()),
                (
                    "/home/root/keyglass/media/key_icons/a.png".into(),
                    b"OLD_ICON".to_vec(),
                ),
            ]),
            default_bytes.clone(),
        );
        t_nak_prior
            .nak_get_file_paths
            .push(device_paths::APPEARANCE_JSON.into());
        let err_nak = execute_atomic_save(&t_nak_prior, &mixed_plan).unwrap_err();
        assert!(err_nak.contains("Preflight backup read"), "{err_nak}");
        assert_eq!(*t_nak_prior.firmware_blob.borrow(), default_bytes);
        assert_eq!(
            t_nak_prior
                .files
                .borrow()
                .get(device_paths::KEYMAP_JSON)
                .unwrap(),
            b"OLD_KEYMAP"
        );
        assert_eq!(
            t_nak_prior
                .files
                .borrow()
                .get(device_paths::APPEARANCE_JSON)
                .unwrap(),
            b"OLD_APP"
        );

        // 13. Media overwritten before a later settings failure is restored.
        let media_tmp =
            std::env::temp_dir().join(format!("keyglassd_media_rb_{}.mp4", std::process::id()));
        std::fs::write(&media_tmp, b"NEW_VIDEO_BYTES").unwrap();
        let media_plan = serde_json::json!({
            "mediaFiles": [{"name": "bg.mp4", "localPath": media_tmp.to_string_lossy()}],
            "settings": {"displayOffSeconds": 300},
        });
        let mut t_med_fail = base_mock(
            BTreeMap::from([
                (
                    "/home/root/keyglass/media/bg.mp4".into(),
                    b"OLD_VIDEO_BYTES".to_vec(),
                ),
                (device_paths::SETTINGS_JSON.into(), b"OLD_SETTINGS".to_vec()),
            ]),
            default_bytes.clone(),
        );
        t_med_fail
            .fail_put_paths
            .push(device_paths::SETTINGS_JSON.into());
        assert!(execute_atomic_save(&t_med_fail, &media_plan).is_err());
        assert_eq!(
            t_med_fail
                .files
                .borrow()
                .get("/home/root/keyglass/media/bg.mp4")
                .unwrap(),
            b"OLD_VIDEO_BYTES"
        );
        assert_eq!(
            t_med_fail
                .files
                .borrow()
                .get(device_paths::SETTINGS_JSON)
                .unwrap(),
            b"OLD_SETTINGS"
        );

        // 14. Listings are NUL-delimited; a first upload into an empty directory succeeds; when STAT_FILE
        //     fails, a file that can't be read back stops the save before any write.
        assert_eq!(
            parse_nul_listing("12:bridge.bin\x0024:armed\0"),
            vec!["bridge.bin", "armed"]
        );
        let t_empty_dir = base_mock(
            BTreeMap::from([(device_paths::KEYMAP_JSON.into(), b"OLD_KEYMAP".to_vec())]),
            default_bytes.clone(),
        );
        let first_icon_plan = serde_json::json!({
            "icons": [{"name": "first.png", "localPath": icon_tmp.to_string_lossy()}],
        });
        assert!(execute_atomic_save(&t_empty_dir, &first_icon_plan).is_ok());
        assert_eq!(
            t_empty_dir
                .files
                .borrow()
                .get("/home/root/keyglass/media/key_icons/first.png")
                .unwrap(),
            b"NEW_ICON"
        );

        let mut t_stat_fails = base_mock(
            BTreeMap::from([
                (device_paths::KEYMAP_JSON.into(), b"OLD_KEYMAP".to_vec()),
                (
                    "/home/root/keyglass/config/other.json".into(),
                    b"OTHER".to_vec(),
                ),
            ]),
            default_bytes.clone(),
        );
        t_stat_fails.fail_stat_file = true;
        t_stat_fails
            .nak_get_file_paths
            .push(device_paths::APPEARANCE_JSON.into());
        assert!(execute_atomic_save(&t_stat_fails, &mixed_plan).is_err());
        assert_eq!(*t_stat_fails.firmware_blob.borrow(), default_bytes);

        // 15. A settings write failure with an unreadable result still restores the overwritten media.
        let mut t_med_unk = base_mock(
            BTreeMap::from([
                (
                    "/home/root/keyglass/media/bg.mp4".into(),
                    b"OLD_VIDEO_BYTES".to_vec(),
                ),
                (device_paths::SETTINGS_JSON.into(), b"OLD_SETTINGS".to_vec()),
            ]),
            default_bytes.clone(),
        );
        t_med_unk
            .fail_put_paths
            .push(device_paths::SETTINGS_JSON.into());
        t_med_unk.fail_get_file_after_write = true;
        let err_unk = execute_atomic_save(&t_med_unk, &media_plan).unwrap_err();
        assert!(err_unk.starts_with("PARTIAL_SAVE_"), "{err_unk}");
        assert_eq!(
            t_med_unk
                .files
                .borrow()
                .get("/home/root/keyglass/media/bg.mp4")
                .unwrap(),
            b"OLD_VIDEO_BYTES"
        );

        // 16. Media the save created is removed when a later write fails, and the save is reported
        //     as rolled back rather than out of step.
        let mut t_new_media = base_mock(
            BTreeMap::from([(device_paths::SETTINGS_JSON.into(), b"OLD_SETTINGS".to_vec())]),
            default_bytes.clone(),
        );
        t_new_media
            .fail_put_paths
            .push(device_paths::SETTINGS_JSON.into());
        let err_new = execute_atomic_save(&t_new_media, &media_plan).unwrap_err();
        assert!(!err_new.starts_with("PARTIAL_SAVE_"), "{err_new}");
        assert!(!t_new_media
            .files
            .borrow()
            .contains_key("/home/root/keyglass/media/bg.mp4"));

        let _ = std::fs::remove_file(&icon_tmp);
        let _ = std::fs::remove_file(&media_tmp);
    }
}
