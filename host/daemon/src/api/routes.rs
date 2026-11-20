//! OpenFlux's own local API, used by OpenFlux Studio, the tray and the daemon's command line.
//! Most routes require a first-party token; `/v1/daemon/lifecycle` also accepts the enrollment
//! secret. The third-party app API lives in [`crate::api::third_party`], which shares the icon
//! import and save handlers defined here.

use crate::api::assets::MAX_KEYBOARD_MEDIA_BYTES;
use crate::api::state::{ApiState, ShortcutProfileState};
use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Json, Router};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

pub const DEFAULT_API_PORT: u16 = 52323;

fn err_msg(code: StatusCode, message: &str) -> (StatusCode, Json<serde_json::Value>) {
    (code, Json(serde_json::json!({ "message": message })))
}

/// 409 for keyboard work that may not run now, with the refusal's code.
fn refusal(r: crate::updates::WriteRefusal) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({ "message": r.message, "code": r.code })),
    )
}

fn detect_icon_ext(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if data.starts_with(b"\xff\xd8\xff") {
        Some("jpg")
    } else if data.starts_with(b"BM") {
        Some("bmp")
    } else {
        let head = std::str::from_utf8(&data[..data.len().min(256)])
            .unwrap_or("")
            .trim_start();
        if head.starts_with("<svg") || head.starts_with("<?xml") {
            Some("svg")
        } else {
            None
        }
    }
}

pub(crate) async fn add_icon(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(r) = state.check_keyboard_write(&[crate::updates::Area::Config]) {
        return refusal(r);
    }
    let Some(list) = body.get("icons").and_then(|v| v.as_array()) else {
        return err_msg(StatusCode::BAD_REQUEST, "icons must be a list");
    };
    const MAX_ICON_BYTES: u64 = 16 * 1024 * 1024;
    let mut validated: Vec<(String, Vec<u8>)> = Vec::new();
    for item in list {
        let Some(fp) = item
            .get("file")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        else {
            return err_msg(
                StatusCode::BAD_REQUEST,
                "Each icon entry must contain a non-empty \"file\"",
            );
        };
        let p = Path::new(fp);
        let Ok(meta) = std::fs::metadata(p) else {
            return err_msg(StatusCode::BAD_REQUEST, "The image data given is invalid");
        };
        if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_ICON_BYTES {
            return err_msg(StatusCode::BAD_REQUEST, "The image data given is invalid");
        }
        let Ok(bytes) = std::fs::read(p) else {
            return err_msg(StatusCode::BAD_REQUEST, "The image data given is invalid");
        };
        let Some(ext) = detect_icon_ext(&bytes) else {
            return err_msg(StatusCode::BAD_REQUEST, "The image data given is invalid");
        };
        let mut h = Sha256::new();
        h.update(&bytes);
        let digest = format!("{:x}", h.finalize());
        let asset_id = format!("{}.{ext}", &digest[..24]);
        validated.push((asset_id, bytes));
    }

    if let Err(e) = std::fs::create_dir_all(&state.icon_store_dir) {
        return err_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Icon dir error: {e}"),
        );
    }

    let mut paths = Vec::with_capacity(validated.len());
    for (asset_id, bytes) in &validated {
        let dst = state.icon_store_dir.join(asset_id);
        if !dst.exists() {
            static ICON_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let seq = ICON_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let tmp = state
                .icon_store_dir
                .join(format!(".tmp_{}_{seq}_{asset_id}", std::process::id()));
            if let Err(e) = std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, &dst)) {
                let _ = std::fs::remove_file(&tmp);
                return err_msg(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("Icon write error: {e}"),
                );
            }
        }
        paths.push(asset_id.clone());
    }

    let mut g = state.inner.lock().unwrap();
    for id in &paths {
        if !g.imported_icons.iter().any(|x| x == id) {
            g.imported_icons.push(id.clone());
        }
    }
    let n = paths.len();
    drop(g);
    state.emit(
        "configChange",
        serde_json::json!({ "kind": "addIcon", "iconPaths": paths }),
    );
    // This route belongs to the third-party API, whose apps receive this exact message.
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "message": format!("{n} Icons imported successfully"),
            "iconPaths": paths
        })),
    )
}

/// The keyboard areas a save plan writes.
pub fn save_areas(plan: &serde_json::Value) -> Vec<crate::updates::Area> {
    use crate::updates::Area;
    let mut areas = vec![Area::Config];
    if plan.get("keymap").is_some() || plan.get("keymapBlobHex").is_some() {
        areas.push(Area::Keymap);
    }
    if plan.get("profiles").is_some() || plan.get("profileSettingsHex").is_some() {
        areas.push(Area::ProfileSettings);
    }
    areas
}

pub(crate) async fn save_to_keyboard(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let (
        has_bridge,
        snapshot,
        app_saved,
        set_saved,
        mos_saved,
        km_saved,
        prof_saved,
        app_rev,
        set_rev,
        mos_rev,
        km_rev,
        prof_rev,
        saved_prof_ids,
    ) = {
        let g = state.inner.lock().unwrap();
        let mut icon_files = Vec::new();
        for id in &g.imported_icons {
            let p = state.icon_store_dir.join(id);
            if p.is_file() {
                icon_files.push(serde_json::json!({
                    "name": id,
                    "localPath": p.to_string_lossy()
                }));
            }
        }
        let mut media_files = Vec::new();
        let mut media_seen = std::collections::BTreeSet::new();
        for name in &g.imported_media {
            if media_seen.insert(name.clone()) {
                if let Some(lp) = crate::api::assets::resolve_media_local_path(&state, name) {
                    media_files.push(serde_json::json!({
                        "name": name,
                        "localPath": lp.to_string_lossy()
                    }));
                }
            }
        }
        if g.appearance_dirty {
            if let Some(bg) = g
                .appearance
                .get("defaultTheme")
                .and_then(|t| t.get("backgroundImagePath"))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                if let Some(lp) = crate::api::assets::resolve_media_local_path(&state, bg) {
                    if let Some(fname) = lp.file_name().and_then(|s| s.to_str()) {
                        if media_seen.insert(fname.to_string())
                            && lp
                                .metadata()
                                .map(|m| m.len() <= MAX_KEYBOARD_MEDIA_BYTES)
                                .unwrap_or(false)
                        {
                            media_files.push(serde_json::json!({
                                "name": fname,
                                "localPath": lp.to_string_lossy()
                            }));
                        }
                    }
                }
            }
        }
        // A section edited before the keyboard's own copy was loaded would overwrite it; refuse.
        let mut plan = serde_json::Map::new();
        if g.appearance_dirty {
            if !g.appearance_hydrated {
                return err_msg(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Appearance was edited before the keyboard's configuration was loaded; load it from the keyboard first",
                );
            }
            plan.insert("appearance".into(), g.appearance.clone());
        }
        if g.settings_dirty {
            if !g.settings_hydrated {
                return err_msg(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Settings were edited before the keyboard's configuration was loaded; load it from the keyboard first",
                );
            }
            plan.insert("settings".into(), g.settings.clone());
        }
        if g.mosaic_dirty {
            if !g.mosaic_hydrated {
                return err_msg(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Mosaic was edited before the keyboard's configuration was loaded; load it from the keyboard first",
                );
            }
            plan.insert("mosaic".into(), g.mosaic.clone());
        }
        if !icon_files.is_empty() {
            plan.insert("icons".into(), serde_json::Value::Array(icon_files));
        }
        if !media_files.is_empty() {
            plan.insert("mediaFiles".into(), serde_json::Value::Array(media_files));
        }
        if g.profiles_dirty {
            if g.profiles.len() > crate::codecs::keymap::MAX_PROFILES {
                return err_msg(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    &format!(
                        "The keyboard holds at most {} profiles",
                        crate::codecs::keymap::MAX_PROFILES
                    ),
                );
            }
            let mut ps_settings = Vec::with_capacity(g.profiles.len());
            for p in &g.profiles {
                let layer = u8::try_from(p.default_layer).map_err(|_| ()).and_then(|l| {
                    if l <= 31 {
                        Ok(l)
                    } else {
                        Err(())
                    }
                });
                let Ok(layer_u8) = layer else {
                    return err_msg(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Profile default_layer out of valid range (0..=31)",
                    );
                };
                let Ok(mode_u32) = u32::try_from(p.actuation_mode_index) else {
                    return err_msg(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Profile actuation_mode_index out of u32 range",
                    );
                };
                match crate::codecs::keymap::ProfileSetting::from_layer(mode_u32, layer_u8) {
                    Ok(ps) => ps_settings.push(ps),
                    Err(e) => {
                        return err_msg(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            &format!("Invalid profile setting: {e}"),
                        )
                    }
                }
            }
            let ps_file = crate::codecs::keymap::ProfileSettingsFile {
                settings: ps_settings,
                file_version: 0,
                file_type: 0,
            };
            let ps_bytes = match ps_file.to_transfer_bytes() {
                Ok(b) => b,
                Err(e) => {
                    return err_msg(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        &format!("Failed to encode profile settings: {e}"),
                    )
                }
            };
            let hex = ApiState::encode_hex_bytes(&ps_bytes);
            plan.insert(
                "profiles".into(),
                serde_json::to_value(&g.profiles).unwrap_or(serde_json::json!([])),
            );
            plan.insert("profileSettingsHex".into(), serde_json::Value::String(hex));
        }
        if g.keymap_dirty {
            if !g.keymap_hydrated {
                return err_msg(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "The keymap was edited before the keyboard's keymap was loaded; load it from the keyboard first",
                );
            }
            let Some(ref raw) = g.firmware_keymap_blob else {
                return err_msg(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "The keymap was edited without the keyboard's stored keymap (firmwareKeymapBlobHex); load it from the keyboard first",
                );
            };
            if let Err(e) = ApiState::validate_firmware_blob(raw) {
                return err_msg(
                    StatusCode::BAD_REQUEST,
                    &format!("Invalid firmware keymap blob: {e}"),
                );
            }
            let hex = ApiState::encode_hex_bytes(raw);
            plan.insert("keymap".into(), g.keymap.clone());
            plan.insert("keymapBlobHex".into(), serde_json::Value::String(hex));
        }
        let saved_prof_ids: Vec<String> = g.profiles.iter().map(|p| p.id.clone()).collect();
        (
            g.cached_connected.0,
            serde_json::Value::Object(plan),
            g.appearance_dirty,
            g.settings_dirty,
            g.mosaic_dirty,
            g.keymap_dirty,
            g.profiles_dirty,
            g.appearance_rev,
            g.settings_rev,
            g.mosaic_rev,
            g.keymap_rev,
            g.profiles_rev,
            saved_prof_ids,
        )
    };
    if !has_bridge {
        return err_msg(StatusCode::SERVICE_UNAVAILABLE, "Keyboard is not connected");
    }
    if let Err(r) = state.check_keyboard_write(&save_areas(&snapshot)) {
        return refusal(r);
    }
    let (res, is_worker) = if let Some(w) = state.get_worker() {
        (w.save_config(snapshot.clone()).await, true)
    } else {
        let res = state
            .lifecycle
            .hold("saving to the keyboard")
            .map_err(|e| e.to_string())
            .and_then(|_lease| {
                crate::api::keyboard_edits::save_keeping_keyboard_edits(
                    &state,
                    &*state.device,
                    &snapshot,
                )
            });
        (res, false)
    };
    match res {
        Ok(kept) => {
            // Sections edited while the save was running stay dirty.
            let mut g = state.inner.lock().unwrap();
            if !is_worker {
                ApiState::record_saved_keymap_from_snapshot(&mut g, &snapshot);
            }
            if app_saved && g.appearance_rev == app_rev {
                g.appearance_dirty = false;
            }
            if set_saved && g.settings_rev == set_rev {
                g.settings_dirty = false;
            }
            if mos_saved && g.mosaic_rev == mos_rev {
                g.mosaic_dirty = false;
            }
            if km_saved && g.keymap_rev == km_rev {
                g.keymap_dirty = false;
            }
            if prof_saved {
                if !is_worker {
                    g.committed_profile_ids = Some(saved_prof_ids);
                    g.applied_profile_index = None;
                }
                if g.profiles_rev == prof_rev {
                    g.profiles_dirty = false;
                }
            }
            drop(g);
            if prof_saved {
                if let Err(pe) = state.persist_profiles() {
                    return err_msg(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("Device save succeeded, but host profile store persistence failed: {pe}"),
                    );
                }
            }
            state.emit("configChange", serde_json::json!({ "kind": "save" }));
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "message": "Successfully saved to keyboard",
                    "keptFromKeyboard": kept,
                })),
            )
        }
        Err(e) => {
            if !is_worker && prof_saved {
                {
                    let mut g = state.inner.lock().unwrap();
                    ApiState::invalidate_committed_profiles_on_uncertain_save(
                        &mut g, &snapshot, &e,
                    );
                }
                let _ = state.persist_profiles();
            }
            err_msg(StatusCode::BAD_GATEWAY, &e)
        }
    }
}

fn validate_config_patch(body: &serde_json::Value) -> Result<(), String> {
    if !body.is_object() {
        return Err("Request body must be a JSON object".into());
    }
    if let Some(a) = body.get("appearance") {
        if !a.is_object() {
            return Err("appearance must be a JSON object".into());
        }
        if let Some(dt) = a.get("defaultTheme") {
            if !dt.is_object() {
                return Err("appearance.defaultTheme must be a JSON object".into());
            }
            if let Some(sm) = dt.get("stylemaps") {
                if !sm.is_object() {
                    return Err("appearance.defaultTheme.stylemaps must be a JSON object".into());
                }
            }
        }
    }
    if let Some(s) = body.get("settings") {
        if !s.is_object() {
            return Err("settings must be a JSON object".into());
        }
    }
    if let Some(m) = body.get("mosaic") {
        if !m.is_object() {
            return Err("mosaic must be a JSON object".into());
        }
        if let Some(tt) = m.get("tileTypes") {
            let Some(arr) = tt.as_array() else {
                return Err("mosaic.tileTypes must be an array".into());
            };
            if arr.len() > 4 {
                return Err("mosaic.tileTypes length must be <= 4".into());
            }
        }
    }
    if let Some(k) = body.get("keymap") {
        if !k.is_object() {
            return Err("keymap must be a JSON object".into());
        }
    }
    if let Some(am) = body.get("actuationModes") {
        let Some(arr) = am.as_array() else {
            return Err("actuationModes must be an array".into());
        };
        for item in arr {
            if !item.is_object() || item.get("name").and_then(|v| v.as_str()).is_none() {
                return Err("Each actuationMode must be an object with name".into());
            }
        }
    }
    if let Some(ar) = body.get("appRules") {
        if serde_json::from_value::<Vec<crate::bridges::AppProfileRule>>(ar.clone()).is_err() {
            return Err("appRules must be a valid AppProfileRule array".into());
        }
    }
    if let Some(profs) = body.get("profiles") {
        let Ok(parsed_profs) = serde_json::from_value::<Vec<ShortcutProfileState>>(profs.clone())
        else {
            return Err("profiles must be a valid ShortcutProfileState array".into());
        };
        ApiState::validate_profile_list(&parsed_profs)?;
    }
    Ok(())
}

/// `next` (the actuation modes a `PUT /v1/config` carries) with each mode's `firmwareTable`: the
/// position of the mode with the same `id` in `current`, whose tables the cached firmware keymap
/// holds, or null for a mode `current` doesn't have. Without ids to go by, modes keep their
/// positions.
fn modes_with_firmware_tables(
    next: &serde_json::Value,
    current: &serde_json::Value,
) -> serde_json::Value {
    let (Some(arr), Some(cur)) = (next.as_array(), current.as_array()) else {
        return next.clone();
    };
    let id_of = |m: &serde_json::Value| m.get("id").and_then(|v| v.as_str()).map(str::to_owned);
    let ids: Vec<Option<String>> = cur.iter().map(id_of).collect();
    let distinct: std::collections::BTreeSet<&Option<String>> = ids.iter().collect();
    if distinct.len() != ids.len() || ids.contains(&None) || arr.iter().any(|m| id_of(m).is_none())
    {
        return next.clone();
    }
    let out = arr
        .iter()
        .map(|m| {
            let mut m = m.clone();
            if m.get("firmwareTable").is_none() {
                let id = id_of(&m);
                m["firmwareTable"] = match ids.iter().position(|i| *i == id) {
                    Some(p) => serde_json::json!(p),
                    None => serde_json::Value::Null,
                };
            }
            m
        })
        .collect();
    serde_json::Value::Array(out)
}

/// `PUT /v1/config`: replaces the sections the body carries. `"themeSelection": true` marks
/// `appearance.selectedId` as a theme the user picked, which a save writes over the keyboard's
/// even when it is unchanged; a `selectedId` other than the cached one counts as picked too.
async fn put_config(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(msg) = validate_config_patch(&body) {
        return err_msg(StatusCode::BAD_REQUEST, &msg);
    }

    // Everything is decoded and validated before the state lock is taken, so a bad field changes nothing.
    let parsed_blob: Option<Vec<u8>> = if let Some(v) = body.get("firmwareKeymapBlobHex") {
        let Some(hex) = v.as_str() else {
            return err_msg(
                StatusCode::BAD_REQUEST,
                "firmwareKeymapBlobHex must be a string",
            );
        };
        let bytes = match ApiState::decode_hex_bytes(hex) {
            Ok(b) => b,
            Err(e) => return err_msg(StatusCode::BAD_REQUEST, &e),
        };
        if let Err(e) = ApiState::validate_firmware_blob(&bytes) {
            return err_msg(
                StatusCode::BAD_REQUEST,
                &format!("Invalid firmware keymap blob: {e}"),
            );
        }
        if body.get("keymap").is_none() {
            return err_msg(
                StatusCode::BAD_REQUEST,
                "firmwareKeymapBlobHex must be paired with keymap JSON in the same snapshot",
            );
        }
        Some(bytes)
    } else {
        None
    };

    let explicit_reset = body
        .get("explicitReset")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut g = state.inner.lock().unwrap();
    // The keymap and actuation encoding is checked before any section changes.
    let next_act_modes = body.get("actuationModes").unwrap_or(&g.actuation_modes);
    let act_explicit = body.get("actuationModes").is_some();
    let mut derived_act_modes: Option<serde_json::Value> = None;
    let encoded_blob_opt = if let Some(blob) = parsed_blob {
        if let Some(k) = body.get("keymap") {
            if let Err(e) = crate::codecs::keymap::verify_keymap_matches_blob(k, &blob) {
                return err_msg(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    &format!("keymap and firmwareKeymapBlobHex do not match: {e}"),
                );
            }
            let blob_modes = crate::codecs::keymap::decode_actuation_modes_from_blob(&blob)
                .map_err(|e| format!("Invalid actuation tables in firmwareKeymapBlobHex: {e}"));
            let blob_modes = match blob_modes {
                Ok(m) => m,
                Err(e) => return err_msg(StatusCode::UNPROCESSABLE_ENTITY, &e),
            };
            if act_explicit {
                // actuationModes must agree with the tables in firmwareKeymapBlobHex.
                let reencoded = match crate::codecs::keymap::encode_keymap_json_to_firmware_blob(
                    k,
                    Some(next_act_modes),
                    Some(&blob),
                ) {
                    Ok(b) => b,
                    Err(e) => {
                        return err_msg(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            &format!("Invalid actuationModes for paired blob: {e}"),
                        )
                    }
                };
                if reencoded != blob {
                    return err_msg(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Contradictory actuationModes and firmwareKeymapBlobHex in same PUT",
                    );
                }
            }
            derived_act_modes = Some(crate::codecs::keymap::name_actuation_modes(blob_modes, k));
        }
        Some(blob)
    } else if let Some(k) = body.get("keymap") {
        let am_arg =
            act_explicit.then(|| modes_with_firmware_tables(next_act_modes, &g.actuation_modes));
        match crate::codecs::keymap::encode_keymap_json_to_firmware_blob(
            k,
            am_arg.as_ref(),
            g.firmware_keymap_blob.as_deref(),
        ) {
            Ok(b) => Some(b),
            Err(e) => {
                return err_msg(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    &format!("Keymap cannot be encoded for the firmware without loss: {e}"),
                )
            }
        }
    } else if act_explicit && (g.keymap_hydrated || explicit_reset) {
        match crate::codecs::keymap::encode_keymap_json_to_firmware_blob(
            &g.keymap,
            Some(&modes_with_firmware_tables(
                next_act_modes,
                &g.actuation_modes,
            )),
            g.firmware_keymap_blob.as_deref(),
        ) {
            Ok(b) => Some(b),
            Err(e) => {
                return err_msg(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    &format!("Actuation update could not be encoded for the firmware: {e}"),
                )
            }
        }
    } else {
        None
    };

    if let Some(a) = body.get("appearance") {
        // A theme pick: `themeSelection`, or a `selectedId` other than the cached one. Other
        // appearance edits send the whole document but leave `selectedId` as it was.
        if body.get("themeSelection").and_then(|v| v.as_bool()) == Some(true)
            || a.get("selectedId") != g.appearance.get("selectedId")
        {
            g.theme_picked = true;
        }
        g.appearance = a.clone();
        g.appearance_dirty = true;
        g.appearance_rev = g.appearance_rev.wrapping_add(1);
        if explicit_reset {
            g.appearance_hydrated = true;
        }
    }
    if let Some(s) = body.get("settings") {
        g.settings = s.clone();
        g.settings_dirty = true;
        g.settings_rev = g.settings_rev.wrapping_add(1);
        if explicit_reset {
            g.settings_hydrated = true;
        }
    }
    if let Some(m) = body.get("mosaic") {
        g.mosaic = m.clone();
        g.mosaic_dirty = true;
        g.mosaic_rev = g.mosaic_rev.wrapping_add(1);
        if explicit_reset {
            g.mosaic_hydrated = true;
        }
    }
    if let Some(am) = body.get("actuationModes") {
        g.actuation_modes = am.clone();
        g.keymap_dirty = true;
        g.keymap_rev = g.keymap_rev.wrapping_add(1);
        if let Some(b) = encoded_blob_opt.as_ref() {
            g.firmware_keymap_blob = Some(b.clone());
        }
    } else if let Some(dm) = derived_act_modes {
        g.actuation_modes = dm;
    }
    if let Some(profs) = body.get("profiles") {
        if let Ok(parsed_profs) = serde_json::from_value::<Vec<ShortcutProfileState>>(profs.clone())
        {
            let sync_matchers = body.get("appRules").is_none();
            ApiState::replace_profiles_and_remap(&mut g, parsed_profs, true, sync_matchers);
        }
    }
    if let Some(ar) = body.get("appRules") {
        if let Ok(rules) = serde_json::from_value::<Vec<crate::bridges::AppProfileRule>>(ar.clone())
        {
            g.app_rules = rules;
        }
    }
    if let Some(pidx) = body.get("desiredProfileIndex").and_then(|v| v.as_u64()) {
        ApiState::apply_profile_selection(&mut g, pidx as usize);
        let selected = g.desired_profile_index;
        g.choose_profile_theme(selected, true);
        // Picking a profile picks its theme, also for the save.
        let profile_theme = g.profiles.get(selected).map(|p| p.theme_id.as_str());
        if profile_theme.is_some()
            && g.appearance.get("selectedId").and_then(|v| v.as_str()) == profile_theme
        {
            g.theme_picked = true;
        }
        g.applied_profile_index = None;
    }
    if explicit_reset {
        g.profiles_dirty = true;
        g.profiles_rev = g.profiles_rev.wrapping_add(1);
    }
    let persist_needed = body.get("profiles").is_some()
        || body.get("appRules").is_some()
        || body.get("desiredProfileIndex").is_some()
        || explicit_reset;
    if let Some(k) = body.get("keymap") {
        g.keymap = k.clone();
        g.keymap_dirty = true;
        g.keymap_rev = g.keymap_rev.wrapping_add(1);
        g.firmware_keymap_blob = encoded_blob_opt;
        if explicit_reset || body.get("firmwareKeymapBlobHex").is_some() {
            g.keymap_hydrated = true;
        }
    }
    if body.get("keymap").is_some() || body.get("actuationModes").is_some() {
        // The modes' names are kept in keymap.json, which the firmware keymap travels with.
        let modes = g.actuation_modes.clone();
        crate::codecs::keymap::keep_actuation_mode_names(&mut g.keymap, &modes);
    }
    drop(g);
    if persist_needed {
        if let Err(pe) = state.persist_profiles() {
            return err_msg(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("Failed to persist profile state: {pe}"),
            );
        }
    }
    state.emit(
        "configChange",
        serde_json::json!({ "kind": "configUpdate" }),
    );
    (
        StatusCode::OK,
        Json(serde_json::json!({ "message": "Configuration updated" })),
    )
}

async fn get_full_config(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    state.prune_expired_overrides();
    let g = state.inner.lock().unwrap();
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "appearance": g.appearance,
            "settings": g.settings,
            "mosaic": g.mosaic,
            "keymap": g.keymap,
            "actuationModes": g.actuation_modes,
            "keymapLimits": {
                "maxLayers": crate::codecs::keymap::MAX_LAYERS,
                "maxActuationModes": crate::codecs::keymap::MAX_ACTUATION_MODES,
            },
            "appRules": g.app_rules,
            "profiles": g.profiles,
            "desiredProfileIndex": g.desired_profile_index,
            "appliedProfileIndex": g.applied_profile_index,
            "hydrated": {
                "appearance": g.appearance_hydrated,
                "settings": g.settings_hydrated,
                "mosaic": g.mosaic_hydrated,
                "keymap": g.keymap_hydrated && g.firmware_keymap_blob.is_some(),
            },
            "detectionOverrides": {
                "process": g.detection_override_process,
                "processes": g.detection_override_processes,
                "layout": g.detection_override_layout,
            }
        })),
    )
}

/// `GET /v1/device/status`. Besides connection, versions and the profile and language in effect:
/// `autoProfileSwitch` (`{"locked", "heldUntilAppChange"}`), `actuationMode`
/// (`{"index", "name", "source"}` or null), `platform` (`{"code", "source"}` or null) and
/// `firmwareRuntime`, the firmware's own report (`{"activeProfileSlot", "actuationModeIndex",
/// "platformCode"}`), null while the firmware doesn't report it.
async fn device_status(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    state.prune_expired_overrides();
    let compatibility = state.updates().map(|u| u.compatibility());
    let g = state.inner.lock().unwrap();
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "connected": { "bridge": g.cached_connected.0, "firmware": g.cached_connected.1 },
            "keymapSync": g.keymap_sync(),
            "versions": g.cached_versions,
            "conversion": crate::conversion::classify_versions(&g.cached_versions),
            "compatibility": compatibility,
            "desiredProfileIndex": g.desired_profile_index,
            "appliedProfileIndex": g.applied_profile_index,
            "autoProfileSwitch": g.auto_profile_switch_status(),
            "actuationMode": g.actuation_mode_status(),
            "platform": g.platform_status(),
            "firmwareRuntime": g.firmware_runtime,
            "desiredLanguageCode": g.desired_language_code,
            "appliedLanguageCode": g.applied_language_code,
            "hardware": g.hardware,
            "media": g.media,
        })),
    )
}

struct PreviewEpochGuard {
    state: Arc<ApiState>,
    epoch: u64,
}

impl Drop for PreviewEpochGuard {
    fn drop(&mut self) {
        if self
            .state
            .preview_epoch
            .load(std::sync::atomic::Ordering::SeqCst)
            != self.epoch
        {
            if let Ok(mut g) = self.state.preview.lock() {
                *g = None;
            }
            if let Ok(mut lu) = self.state.preview_last_used.lock() {
                *lu = None;
            }
            crate::preview::release_process_heap_pages();
        } else if let Ok(mut lu) = self.state.preview_last_used.lock() {
            *lu = Some(Instant::now());
        }
    }
}

async fn preview_rects(
    State(state): State<Arc<ApiState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    // The epoch is read before waiting for the preview, so /preview/close also cancels queued requests.
    let render_epoch = state
        .preview_epoch
        .load(std::sync::atomic::Ordering::SeqCst);
    let Ok(permit) = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        state.preview_sem.clone().acquire_owned(),
    )
    .await
    .map_err(|_| ())
    .and_then(|r| r.map_err(|_| ())) else {
        return err_msg(StatusCode::SERVICE_UNAVAILABLE, "Preview worker busy").into_response();
    };
    if state
        .preview_epoch
        .load(std::sync::atomic::Ordering::SeqCst)
        != render_epoch
    {
        return err_msg(StatusCode::CONFLICT, "Preview closed").into_response();
    }
    let layout_override = params.get("layout").cloned();
    let (app_s, set_s, km_s, mos_s) = {
        let g = state.inner.lock().unwrap();
        let mut km_val = g.keymap.clone();
        if let Some(ref lay) = layout_override {
            if lay.eq_ignore_ascii_case("iso") || lay.eq_ignore_ascii_case("ansi") {
                km_val["selectedFrameLayoutType"] = serde_json::json!(lay.to_ascii_lowercase());
            }
        }
        (
            g.appearance.to_string(),
            g.settings.to_string(),
            km_val.to_string(),
            g.mosaic.to_string(),
        )
    };
    let asset_root = state
        .icon_store_dir
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| state.icon_store_dir.to_string_lossy().to_string());
    *state.preview_last_used.lock().unwrap() = Some(Instant::now());
    let prev_arc = state.preview.clone();
    let state_clone = state.clone();
    let res = tokio::task::spawn_blocking(move || {
        // Held for the whole blocking job, even when the request is cancelled.
        let _permit = permit;
        let _epoch_guard = PreviewEpochGuard {
            state: state_clone.clone(),
            epoch: render_epoch,
        };
        if state_clone
            .preview_epoch
            .load(std::sync::atomic::Ordering::SeqCst)
            != render_epoch
        {
            return Err("CLOSED_DURING_PREVIEW".to_string());
        }
        let mut guard = prev_arc.lock().unwrap();
        if guard.is_none() {
            *guard = Some(crate::preview::PreviewBackend::open_auto(
                state_clone.preview_child.clone(),
                Some((&state_clone.preview_epoch, render_epoch)),
            )?);
        }
        let barrier = state_clone.preview_barrier.lock().unwrap().clone();
        if let Some((entered, release)) = barrier {
            entered.notify_waiters();
            let (m, cv) = &*release;
            let mut g = m.lock().unwrap();
            while !*g {
                g = cv.wait(g).unwrap();
            }
        }
        let prev = guard.as_mut().unwrap();
        if let Err(e) = prev.set_config_json(
            Some(&app_s),
            Some(&set_s),
            Some(&km_s),
            Some(&mos_s),
            Some(&asset_root),
        ) {
            *guard = None;
            return Err(e);
        }
        match prev.get_key_rects_json() {
            Ok(rects) => Ok::<Vec<serde_json::Value>, String>(rects),
            Err(e) => {
                *guard = None;
                Err(e)
            }
        }
    })
    .await;
    match res {
        Ok(Ok(rects)) => {
            (StatusCode::OK, Json(serde_json::json!({ "rects": rects }))).into_response()
        }
        Ok(Err(e)) if e == "CLOSED_DURING_PREVIEW" || e == "CLOSED_DURING_LAUNCH" => {
            err_msg(StatusCode::CONFLICT, "Preview closed").into_response()
        }
        Ok(Err(e)) => err_msg(StatusCode::INTERNAL_SERVER_ERROR, &e).into_response(),
        Err(e) => err_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("The preview failed unexpectedly: {e}"),
        )
        .into_response(),
    }
}

struct ChanBody {
    rx: tokio::sync::mpsc::Receiver<Result<bytes::Bytes, std::io::Error>>,
    expected_len: u64,
}

impl http_body::Body for ChanBody {
    type Data = bytes::Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        match self.rx.poll_recv(cx) {
            std::task::Poll::Ready(Some(Ok(b))) => {
                std::task::Poll::Ready(Some(Ok(http_body::Frame::data(b))))
            }
            std::task::Poll::Ready(Some(Err(e))) => std::task::Poll::Ready(Some(Err(e))),
            std::task::Poll::Ready(None) => std::task::Poll::Ready(None),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }

    fn size_hint(&self) -> http_body::SizeHint {
        http_body::SizeHint::with_exact(self.expected_len)
    }
}

/// The clock for previews that don't pass `nowSec`, so their renders are reproducible.
const DEFAULT_PREVIEW_UNIX_SECS: i64 = 1_763_414_400;

async fn preview_render(
    State(state): State<Arc<ApiState>>,
    body: Option<Json<serde_json::Value>>,
) -> impl IntoResponse {
    let req = body.map(|b| b.0).unwrap_or_else(|| serde_json::json!({}));
    let format = req
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("ppm")
        .to_string();
    let now_sec = req
        .get("nowSec")
        .and_then(|v| v.as_i64())
        .unwrap_or(DEFAULT_PREVIEW_UNIX_SECS);
    let now_ms = req.get("nowMs").and_then(|v| v.as_u64()).unwrap_or(0);
    let suspended = req
        .get("suspended")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let half_res = req
        .get("halfRes")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let live_str = req.get("liveJson").map(|live| {
        if let Some(str_v) = live.as_str() {
            str_v.to_string()
        } else {
            live.to_string()
        }
    });

    // The epoch is read before waiting for the preview, so /preview/close also cancels queued renders.
    let render_epoch = state
        .preview_epoch
        .load(std::sync::atomic::Ordering::SeqCst);
    let Ok(permit) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        state.preview_sem.clone().acquire_owned(),
    )
    .await
    .map_err(|_| ())
    .and_then(|r| r.map_err(|_| ())) else {
        return err_msg(StatusCode::SERVICE_UNAVAILABLE, "Preview worker busy").into_response();
    };
    if state
        .preview_epoch
        .load(std::sync::atomic::Ordering::SeqCst)
        != render_epoch
    {
        return err_msg(StatusCode::CONFLICT, "Preview closed").into_response();
    }

    let (app_s, set_s, km_s, mos_s, cached_live) = {
        let g = state.inner.lock().unwrap();
        let sanitized_live = g.cached_live_data_json.as_ref().map(|raw| {
            if let Ok(mut lv) = serde_json::from_str::<serde_json::Value>(raw) {
                if let Some(obj) = lv.as_object_mut() {
                    obj.remove("activeThemeId");
                    obj.remove("active_theme_id");
                    obj.remove("activeThemeProfileSlot");
                    obj.remove("themeRev");
                }
                lv.to_string()
            } else {
                raw.clone()
            }
        });
        (
            g.appearance.to_string(),
            g.settings.to_string(),
            g.keymap.to_string(),
            g.mosaic.to_string(),
            sanitized_live,
        )
    };

    let asset_root = state
        .icon_store_dir
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| state.icon_store_dir.to_string_lossy().to_string());
    *state.preview_last_used.lock().unwrap() = Some(Instant::now());
    let is_rgba = format.eq_ignore_ascii_case("rgba");
    if is_rgba {
        let expected_len: usize = if half_res {
            960 * 540 * 4
        } else {
            1920 * 1080 * 4
        };
        // RGBA is streamed from the preview worker in 64 KB chunks so the daemon never holds a whole
        // 8.3 MB frame.
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<bytes::Bytes, std::io::Error>>(4);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
        let prev_arc = state.preview.clone();
        let state_clone = state.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _epoch_guard = PreviewEpochGuard {
                state: state_clone.clone(),
                epoch: render_epoch,
            };
            let mut ready_tx = Some(ready_tx);
            if state_clone
                .preview_epoch
                .load(std::sync::atomic::Ordering::SeqCst)
                != render_epoch
            {
                if let Some(tx_r) = ready_tx.take() {
                    let _ = tx_r.send(Err("CLOSED_DURING_RENDER".into()));
                }
                return;
            }
            let mut guard = prev_arc.lock().unwrap();
            if guard.is_none() {
                match crate::preview::PreviewBackend::open_auto(
                    state_clone.preview_child.clone(),
                    Some((&state_clone.preview_epoch, render_epoch)),
                ) {
                    Ok(p) => *guard = Some(p),
                    Err(e) => {
                        if let Some(tx_r) = ready_tx.take() {
                            let _ = tx_r.send(Err(e));
                        }
                        return;
                    }
                }
            }
            let barrier = state_clone.preview_barrier.lock().unwrap().clone();
            if let Some((entered, release)) = barrier {
                entered.notify_waiters();
                let (m, cv) = &*release;
                let mut g = m.lock().unwrap();
                while !*g {
                    g = cv.wait(g).unwrap();
                }
            }
            let prev = guard.as_mut().unwrap();
            if let Err(e) = prev.set_config_json(
                Some(&app_s),
                Some(&set_s),
                Some(&km_s),
                Some(&mos_s),
                Some(&asset_root),
            ) {
                *guard = None;
                if let Some(tx_r) = ready_tx.take() {
                    let _ = tx_r.send(Err(e));
                }
                return;
            }
            if let Some(ref cl) = cached_live {
                let _ = prev.apply_live_json(cl, now_ms);
            }
            if let Some(ref ls) = live_str {
                if let Err(e) = prev.apply_live_json(ls, now_ms) {
                    *guard = None;
                    if let Some(tx_r) = ready_tx.take() {
                        let _ = tx_r.send(Err(format!("BAD_LIVE:{e}")));
                    }
                    return;
                }
            }
            let stream_deadline = Instant::now() + std::time::Duration::from_secs(2);
            let mut streamed_bytes: usize = 0;
            let res =
                prev.render_rgba_stream_to_sized(now_sec, now_ms, suspended, half_res, |chunk| {
                    streamed_bytes += chunk.len();
                    if let Some(tx_r) = ready_tx.take() {
                        let _ = tx_r.send(Ok(()));
                    }
                    let data = bytes::Bytes::copy_from_slice(chunk);
                    let mut msg = Some(Ok(data));
                    loop {
                        if state_clone
                            .preview_epoch
                            .load(std::sync::atomic::Ordering::SeqCst)
                            != render_epoch
                        {
                            return Err("CLOSED_DURING_RENDER".into());
                        }
                        let payload = msg.take().unwrap();
                        match tx.try_send(payload) {
                            Ok(()) => return Ok(()),
                            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                                return Err("client closed stream".into());
                            }
                            Err(tokio::sync::mpsc::error::TrySendError::Full(returned)) => {
                                state_clone.preview_stream_full.notify_waiters();
                                if Instant::now() >= stream_deadline {
                                    return Err("client stream backpressure timeout".into());
                                }
                                msg = Some(returned);
                                std::thread::sleep(std::time::Duration::from_millis(5));
                            }
                        }
                    }
                });
            let final_res = match res {
                Ok(()) if streamed_bytes != expected_len => Err(format!(
                    "truncated RGBA stream: got {streamed_bytes} != {expected_len}"
                )),
                other => other,
            };
            if let Err(e) = final_res {
                *guard = None;
                if let Some(tx_r) = ready_tx.take() {
                    let _ = tx_r.send(Err(e));
                } else {
                    // A mid-stream error aborts the HTTP response rather than ending it early and cleanly.
                    let _ = tx.try_send(Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        e,
                    )));
                }
            } else if let Some(tx_r) = ready_tx.take() {
                let _ = tx_r.send(Ok(()));
            }
        });
        return match ready_rx.await {
            Ok(Ok(())) => (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
                axum::body::Body::new(ChanBody {
                    rx,
                    expected_len: expected_len as u64,
                }),
            )
                .into_response(),
            Ok(Err(e)) if e == "CLOSED_DURING_RENDER" || e == "CLOSED_DURING_LAUNCH" => {
                err_msg(StatusCode::CONFLICT, "Preview closed").into_response()
            }
            Ok(Err(e)) if e.starts_with("BAD_LIVE:") => {
                err_msg(StatusCode::BAD_REQUEST, &e["BAD_LIVE:".len()..]).into_response()
            }
            Ok(Err(e)) => err_msg(StatusCode::INTERNAL_SERVER_ERROR, &e).into_response(),
            Err(e) => err_msg(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("The preview failed unexpectedly: {e}"),
            )
            .into_response(),
        };
    }

    let prev_arc = state.preview.clone();
    let state_clone = state.clone();
    let res = tokio::task::spawn_blocking(move || {
        // Held until the blocking render completes, even when the request is cancelled.
        let _permit = permit;
        // Declared before `guard` so `guard` drops first and `_epoch_guard` always cleans up.
        let _epoch_guard = PreviewEpochGuard {
            state: state_clone.clone(),
            epoch: render_epoch,
        };
        if state_clone
            .preview_epoch
            .load(std::sync::atomic::Ordering::SeqCst)
            != render_epoch
        {
            return Err("CLOSED_DURING_RENDER".to_string());
        }
        let mut guard = prev_arc.lock().unwrap();
        if guard.is_none() {
            *guard = Some(crate::preview::PreviewBackend::open_auto(
                state_clone.preview_child.clone(),
                Some((&state_clone.preview_epoch, render_epoch)),
            )?);
        }
        let barrier = state_clone.preview_barrier.lock().unwrap().clone();
        if let Some((entered, release)) = barrier {
            entered.notify_waiters();
            let (m, cv) = &*release;
            let mut g = m.lock().unwrap();
            while !*g {
                g = cv.wait(g).unwrap();
            }
        }
        let prev = guard.as_mut().unwrap();
        if let Err(e) = prev.set_config_json(
            Some(&app_s),
            Some(&set_s),
            Some(&km_s),
            Some(&mos_s),
            Some(&asset_root),
        ) {
            *guard = None;
            return Err(e);
        }
        if let Some(ref cl) = cached_live {
            let _ = prev.apply_live_json(cl, now_ms);
        }
        if let Some(ref ls) = live_str {
            if let Err(e) = prev.apply_live_json(ls, now_ms) {
                *guard = None;
                return Err(format!("BAD_LIVE:{e}"));
            }
        }
        let res = prev
            .render_ppm_bytes(now_sec, now_ms, suspended)
            .map(|bytes| ("image/x-portable-pixmap", bytes));
        if res.is_err() {
            *guard = None;
        }
        res
    })
    .await;

    match res {
        Ok(Ok((ctype, bytes))) => {
            crate::preview::release_process_heap_pages();
            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, ctype)],
                bytes,
            )
                .into_response()
        }
        Ok(Err(e)) if e == "CLOSED_DURING_RENDER" || e == "CLOSED_DURING_LAUNCH" => {
            err_msg(StatusCode::CONFLICT, "Preview closed").into_response()
        }
        Ok(Err(e)) if e.starts_with("BAD_LIVE:") => {
            err_msg(StatusCode::BAD_REQUEST, &e["BAD_LIVE:".len()..]).into_response()
        }
        Ok(Err(e)) => err_msg(StatusCode::INTERNAL_SERVER_ERROR, &e).into_response(),
        Err(e) => err_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("The preview failed unexpectedly: {e}"),
        )
        .into_response(),
    }
}

async fn preview_close(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    // Bumping the epoch cancels queued requests, and killing and reaping the preview child breaks
    // any blocked pipe, so even a stalled preview worker unblocks and its memory is released.
    state
        .preview_epoch
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    *state.preview_last_used.lock().unwrap() = None;
    crate::preview::kill_and_reap_child(&state.preview_child);
    if let Ok(permit) = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        state.preview_sem.clone().acquire_owned(),
    )
    .await
    .map_err(|_| ())
    .and_then(|r| r.map_err(|_| ()))
    {
        let prev_arc = state.preview.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            *prev_arc.lock().unwrap() = None;
            crate::preview::release_process_heap_pages();
        })
        .await;
    }
    crate::preview::release_process_heap_pages();
    (
        StatusCode::OK,
        Json(serde_json::json!({ "message": "Preview unloaded" })),
    )
        .into_response()
}

/// `{"error": {"code", "message"}}` with `status`.
fn api_error(
    status: StatusCode,
    code: &str,
    message: &str,
) -> (StatusCode, Json<serde_json::Value>) {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
}

/// `POST /v1/keymap/adopt-firmware`: makes the keyboard's firmware keymap, what the keyboard
/// actually does, the keymap. `keymap.json` is rebuilt from the firmware keymap last read from the
/// keyboard, keeping its layout and layer names, and saved to the keyboard with it. Answers the
/// new `GET /v1/config` body.
///
/// 409 when keyboard writes are refused (`busy`, `conversionRequired`, `readOnly`,
/// `unsupported`, `unknown`, as in [`crate::updates::WriteRefusal`]), when no firmware keymap has been read (`noFirmwareKeymap`), or when it has key
/// codes a keymap document can't express (`firmwareKeymapUnreadable`). 502 when the save fails.
async fn adopt_firmware_keymap(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    use crate::updates::Area;
    if let Err(r) = state.check_keyboard_write(&[Area::Config, Area::Keymap]) {
        return api_error(StatusCode::CONFLICT, r.code, &r.message).into_response();
    }
    let (blob, mut base) = {
        let g = state.inner.lock().unwrap();
        (g.device_keymap_blob.clone(), g.keymap.clone())
    };
    let Some(blob) = blob else {
        return api_error(
            StatusCode::CONFLICT,
            "noFirmwareKeymap",
            "The keyboard's firmware keymap hasn't been read; connect the keyboard and wait for it to load",
        )
        .into_response();
    };
    crate::codecs::keycode_names::to_openflux(&mut base);
    if base.get("keymaps").and_then(|v| v.as_object()).is_none() {
        base = crate::defaults::default_keymap();
    }
    let doc = match crate::codecs::keymap::keymap_json_from_blob(&blob, &base) {
        Ok(d) => d,
        Err(e) => {
            return api_error(
                StatusCode::CONFLICT,
                "firmwareKeymapUnreadable",
                &format!("The keyboard's firmware keymap can't be turned into a keymap: {e}"),
            )
            .into_response()
        }
    };
    let plan = serde_json::json!({
        "keymap": doc,
        "keymapBlobHex": ApiState::encode_hex_bytes(&blob),
    });
    let res = if let Some(w) = state.get_worker() {
        w.save_config(plan.clone()).await
    } else {
        state
            .lifecycle
            .hold("saving to the keyboard")
            .map_err(|e| e.to_string())
            .and_then(|_lease| {
                crate::api::keyboard_edits::save_keeping_keyboard_edits(
                    &state,
                    &*state.device,
                    &plan,
                )
            })
    };
    if let Err(e) = res {
        // A job that took the keyboard while the save waited for the worker is a refusal too.
        if let Err(r) = state.check_keyboard_write(&[Area::Config, Area::Keymap]) {
            return api_error(StatusCode::CONFLICT, r.code, &r.message).into_response();
        }
        return api_error(StatusCode::BAD_GATEWAY, "saveFailed", &e).into_response();
    }
    {
        let mut g = state.inner.lock().unwrap();
        if let Ok(modes) = crate::codecs::keymap::decode_actuation_modes_from_blob(&blob) {
            g.actuation_modes = crate::codecs::keymap::name_actuation_modes(modes, &doc);
        }
        g.keymap = doc;
        g.keymap_dirty = false;
        g.keymap_rev = g.keymap_rev.wrapping_add(1);
        g.firmware_keymap_blob = Some(blob.clone());
        g.device_keymap_blob = Some(blob);
        g.keymap_hydrated = true;
        g.record_keymap_pull(crate::api::state::KeymapPull::Ok);
    }
    state.emit(
        "configChange",
        serde_json::json!({ "kind": "adoptFirmwareKeymap" }),
    );
    get_full_config(State(state)).await.into_response()
}

async fn pull_config(
    State(state): State<Arc<ApiState>>,
    body: Option<Json<serde_json::Value>>,
) -> impl IntoResponse {
    let discard_unsaved = body
        .as_ref()
        .and_then(|b| b.get("discardUnsaved"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let res = if let Some(w) = state.get_worker() {
        w.pull_config(discard_unsaved).await
    } else {
        crate::api::worker::apply_device_pull(&state, &*state.device, discard_unsaved)
    };
    match res {
        Ok(_) => {
            if let Err(pe) = state.persist_profiles() {
                return err_msg(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!(
                        "Device pull succeeded, but host profile store persistence failed: {pe}"
                    ),
                )
                .into_response();
            }
            get_full_config(State(state)).await.into_response()
        }
        Err(e) if e.starts_with("Conflict:") => err_msg(StatusCode::CONFLICT, &e).into_response(),
        Err(e) => err_msg(StatusCode::BAD_GATEWAY, &e).into_response(),
    }
}

async fn maintenance_backup(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(out_dir) = body
        .get("outDir")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    else {
        return err_msg(StatusCode::BAD_REQUEST, "outDir is required").into_response();
    };
    if let Err(r) = state.check_keyboard_free() {
        return refusal(r).into_response();
    }
    let res = if let Some(w) = state.get_worker() {
        w.backup_device(out_dir.to_string()).await
    } else {
        state
            .lifecycle
            .hold("backing up the keyboard")
            .map_err(|e| e.to_string())
            .and_then(|_lease| state.device.backup_device(out_dir))
    };
    match res {
        Ok(msg) => (StatusCode::OK, Json(serde_json::json!({ "message": msg }))).into_response(),
        Err(e) => err_msg(StatusCode::BAD_GATEWAY, &e).into_response(),
    }
}

async fn maintenance_flash_uf2(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(r) = state.check_keyboard_write(&[crate::updates::Area::FirmwareUsb]) {
        return refusal(r).into_response();
    }
    let Some(uf2) = body
        .get("uf2Path")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    else {
        return err_msg(StatusCode::BAD_REQUEST, "uf2Path is required").into_response();
    };
    let Ok(bytes) = std::fs::read(uf2) else {
        return err_msg(StatusCode::BAD_REQUEST, "UF2 file does not exist").into_response();
    };
    if let Err(e) = crate::usb::picoboot::parse_uf2_image(&bytes) {
        return err_msg(StatusCode::BAD_REQUEST, &e).into_response();
    }
    let res = if let Some(w) = state.get_worker() {
        w.flash_uf2(uf2.to_string()).await
    } else {
        state
            .lifecycle
            .hold("flashing the keyboard firmware")
            .map_err(|e| e.to_string())
            .and_then(|_lease| state.device.flash_uf2(uf2))
    };
    match res {
        Ok(msg) => (StatusCode::OK, Json(serde_json::json!({ "message": msg }))).into_response(),
        Err(e) => err_msg(StatusCode::BAD_GATEWAY, &e).into_response(),
    }
}

async fn get_soc_stats(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let stats = if let Some(w) = state.get_worker() {
        w.read_device_soc_stats().await
    } else {
        state.device.read_device_soc_stats()
    };
    match stats {
        Some(v) => (StatusCode::OK, Json(v)).into_response(),
        None => (StatusCode::OK, Json(serde_json::json!({ "offline": true }))).into_response(),
    }
}

async fn pomodoro_action(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let act = body
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("toggle");
    if act != "toggle" && act != "reset" {
        return err_msg(StatusCode::BAD_REQUEST, "action must be toggle or reset").into_response();
    }
    let dev_pomo = if let Some(w) = state.get_worker() {
        w.read_device_pomodoro_state().await
    } else {
        state.device.read_device_pomodoro_state()
    };
    crate::api::worker::reconcile_pomodoro_from_device(&state, dev_pomo.as_ref());
    let (default_s, prev_state, next_running, next_rem, next_rev, cmd_seq) = {
        let mut g = state.inner.lock().unwrap();
        let def_s = {
            let types = g.mosaic.get("tileTypes").and_then(|v| v.as_array());
            let cfgs = g.mosaic.get("tileConfigs").and_then(|v| v.as_array());
            let mut found_min = None;
            if let (Some(ts), Some(cs)) = (types, cfgs) {
                for (i, t) in ts.iter().enumerate() {
                    if t.as_u64() == Some(13) {
                        if let Some(m) = cs.get(i).and_then(|c| c.get("main")).and_then(|m| {
                            m.as_u64()
                                .or_else(|| m.as_str().and_then(|s| s.parse::<u64>().ok()))
                        }) {
                            if m > 0 {
                                found_min = Some(m);
                                break;
                            }
                        }
                    }
                }
            }
            found_min.unwrap_or(25) as u32 * 60
        };
        let snapshot = (
            g.pomo_running,
            g.pomo_anchor_rem_s,
            g.pomo_anchor_at,
            g.pomo_rev,
            g.pomo_expected_cmd_seq,
        );
        let cur_rem = if g.pomo_rev == 0 && g.pomo_last_dev_rev == 0 {
            def_s
        } else if g.pomo_running {
            let elapsed = g
                .pomo_anchor_at
                .map(|t| t.elapsed().as_secs() as u32)
                .unwrap_or(0);
            g.pomo_anchor_rem_s.saturating_sub(elapsed)
        } else {
            g.pomo_anchor_rem_s
        };
        if act == "reset" {
            g.pomo_running = false;
            g.pomo_anchor_rem_s = def_s;
            g.pomo_anchor_at = None;
        } else {
            let nr = !g.pomo_running;
            let nrem = if nr && cur_rem == 0 { def_s } else { cur_rem };
            g.pomo_running = nr;
            g.pomo_anchor_rem_s = nrem;
            g.pomo_anchor_at = if nr { Some(Instant::now()) } else { None };
        }
        g.pomo_rev = g.pomo_rev.saturating_add(1);
        g.pomo_cmd_seq = g.pomo_cmd_seq.saturating_add(1);
        g.pomo_expected_cmd_seq = g.pomo_cmd_seq;
        (
            def_s,
            snapshot,
            g.pomo_running,
            g.pomo_anchor_rem_s,
            g.pomo_rev,
            g.pomo_cmd_seq,
        )
    };
    // The keyboard owns the timer; the host sends the intent (`action`, `defaultS`, `cmdSeq`).
    let payload = serde_json::json!({
        "pomodoro": {
            "action": act,
            "defaultS": default_s,
            "cmdSeq": cmd_seq,
        }
    });
    let pomo_obj = serde_json::json!({
        "running": next_running,
        "remainingS": next_rem,
        "defaultS": default_s,
        "cmdSeq": cmd_seq,
    });
    let res = if let Some(w) = state.get_worker() {
        w.send_live_json(payload).await
    } else {
        state.device.send_live_json(&payload)
    };
    match res {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "message": format!("Pomodoro {act} applied"),
                "pomodoro": pomo_obj
            })),
        )
            .into_response(),
        Err(e) => {
            // The keyboard rejected the command: restore the previous timer state and expected sequence.
            let mut g = state.inner.lock().unwrap();
            if g.pomo_rev == next_rev {
                g.pomo_running = prev_state.0;
                g.pomo_anchor_rem_s = prev_state.1;
                g.pomo_anchor_at = prev_state.2;
                g.pomo_rev = prev_state.3;
                g.pomo_expected_cmd_seq = prev_state.4;
            }
            err_msg(StatusCode::BAD_GATEWAY, &e).into_response()
        }
    }
}

/// `PUT /v1/profiles/autoLock` with `{"locked": bool}`: locks or unlocks app-rule profile
/// switching (see [`ApiState::set_auto_profile_lock`]). Answers with the new `autoProfileSwitch`
/// of the device status, `{"locked", "heldUntilAppChange"}`.
async fn put_auto_profile_lock(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(locked) = body.get("locked").and_then(|v| v.as_bool()) else {
        return err_msg(StatusCode::BAD_REQUEST, "locked must be true or false");
    };
    state.set_auto_profile_lock(locked);
    let status = state.inner.lock().unwrap().auto_profile_switch_status();
    (StatusCode::OK, Json(status))
}

async fn get_live_data(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let raw = {
        let g = state.inner.lock().unwrap();
        g.cached_live_data_json.clone()
    };
    let mut val = raw
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = val.as_object_mut() {
        obj.remove("activeThemeId");
        obj.remove("active_theme_id");
        obj.remove("activeThemeProfileSlot");
        obj.remove("themeRev");
    }
    (StatusCode::OK, Json(val))
}

async fn maintenance_sideload_ipk(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(r) = state.check_keyboard_write(&[crate::updates::Area::BridgeUsb]) {
        return refusal(r).into_response();
    }
    let Some(ipk) = body
        .get("ipkPath")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
    else {
        return err_msg(StatusCode::BAD_REQUEST, "ipkPath is required").into_response();
    };
    if !Path::new(ipk).is_file() {
        return err_msg(StatusCode::BAD_REQUEST, "IPK file does not exist").into_response();
    }
    let res = if let Some(w) = state.get_worker() {
        w.sideload_ipk(ipk.to_string()).await
    } else {
        state
            .lifecycle
            .hold("installing a keyboard package")
            .map_err(|e| e.to_string())
            .and_then(|_lease| state.device.sideload_ipk(ipk))
    };
    match res {
        Ok(msg) => (StatusCode::OK, Json(serde_json::json!({ "message": msg }))).into_response(),
        Err(e) => err_msg(StatusCode::BAD_GATEWAY, &e).into_response(),
    }
}

async fn get_default_config() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(crate::api::state::default_config_snapshot()),
    )
}

/// `POST /v1/daemon/lifecycle` with `{"action": "status" | "restart" | "stop"}`.
///
/// Callers authenticate with a first-party token or with `enrollmentSecret` in the body (the
/// daemon's own command line). `restart` and `stop` answer `{"outcome": "exiting"}` when the daemon
/// is idle and exits now, or `{"outcome": "deferred", "busy": [...]}` while keyboard work is running;
/// the daemon then exits by itself when the work finishes.
async fn daemon_lifecycle(
    State(state): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    if !ApiState::is_trusted_origin(origin) {
        return err_msg(StatusCode::FORBIDDEN, "Untrusted Origin").into_response();
    }
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let by_token = matches!(
        state.verify_bearer(bearer),
        Ok(crate::api::state::TokenKind::FirstParty)
    );
    let by_secret = body
        .get("enrollmentSecret")
        .and_then(|v| v.as_str())
        .map(|s| state.inner.lock().unwrap().registration_secret.as_deref() == Some(s))
        .unwrap_or(false);
    if !by_token && !by_secret {
        return err_msg(
            StatusCode::UNAUTHORIZED,
            "A first-party token or the enrollment secret is required",
        )
        .into_response();
    }
    let instance = state.instance_id.clone();
    let action = body
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("status");
    if action == "status" {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "outcome": "running",
                "instanceId": instance,
                "busy": state.lifecycle.busy(),
            })),
        )
            .into_response();
    }
    let Some(kind) = crate::lifecycle::ExitKind::parse(action) else {
        return err_msg(
            StatusCode::BAD_REQUEST,
            "action must be status, restart or stop",
        )
        .into_response();
    };
    let reply = match state.lifecycle.request_exit(kind) {
        crate::lifecycle::ExitDecision::Exiting(k) => serde_json::json!({
            "outcome": "exiting",
            "exit": k.as_str(),
            "instanceId": instance,
        }),
        crate::lifecycle::ExitDecision::Deferred { kind, busy } => serde_json::json!({
            "outcome": "deferred",
            "exit": kind.as_str(),
            "busy": busy,
            "instanceId": instance,
        }),
    };
    (StatusCode::OK, Json(reply)).into_response()
}

pub fn router(state: Arc<ApiState>) -> Router {
    let first_party = Router::new()
        .route("/v1/config", get(get_full_config).put(put_config))
        .route("/v1/config/defaults", get(get_default_config))
        .route("/v1/config/pull", post(pull_config))
        .route("/v1/keymap/adopt-firmware", post(adopt_firmware_keymap))
        .route("/v1/config/export", post(crate::api::assets::export_bundle))
        .route("/v1/assets", get(crate::api::assets::get_assets_catalog))
        .route("/v1/shortcuts", get(crate::api::shortcuts::get_pack))
        .route(
            "/v1/shortcuts/custom",
            get(crate::api::shortcuts::get_custom).put(crate::api::shortcuts::put_custom),
        )
        .route(
            "/v1/shortcuts/templates",
            get(crate::api::shortcuts::get_templates),
        )
        .route("/v1/asset-file", get(crate::api::assets::get_asset_file))
        .route("/v1/apps", get(crate::api::assets::get_installed_apps))
        .route(
            "/v1/media/import",
            post(crate::api::assets::import_media_file),
        )
        .route(
            "/v1/media/aerials",
            get(crate::api::assets::get_aerial_videos),
        )
        .route(
            "/v1/media/aerial-import",
            post(crate::api::assets::import_aerial_video),
        )
        .route("/v1/maintenance/backup", post(maintenance_backup))
        .route("/v1/maintenance/flash-uf2", post(maintenance_flash_uf2))
        .route(
            "/v1/maintenance/sideload-ipk",
            post(maintenance_sideload_ipk),
        )
        .route("/v1/device/status", get(device_status))
        .route("/v1/profiles/autoLock", put(put_auto_profile_lock))
        .route("/v1/live-data", get(get_live_data))
        .route("/v1/device/soc-stats", get(get_soc_stats))
        .route("/v1/pomodoro", post(pomodoro_action))
        .route("/v1/preview/rects", get(preview_rects))
        .route("/v1/preview/render", post(preview_render))
        .route("/v1/preview/close", post(preview_close))
        .merge(crate::api::updates::router())
        .merge(crate::api::press_refinement::router())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::api::third_party::first_party_token,
        ));

    Router::new()
        .route("/v1/daemon/lifecycle", post(daemon_lifecycle))
        .merge(first_party)
        .with_state(state)
}
