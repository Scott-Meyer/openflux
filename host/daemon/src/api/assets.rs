use crate::api::state::{ApiState, ShortcutProfileState};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn err_msg(code: StatusCode, message: &str) -> (StatusCode, Json<serde_json::Value>) {
    (code, Json(serde_json::json!({ "message": message })))
}

pub fn expand_home_path(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    } else if raw == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    PathBuf::from(raw)
}

pub fn resolve_asset_roots(state: &ApiState) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(env_dir) = std::env::var("OPENFLUX_ASSET_DIR") {
        if !env_dir.is_empty() {
            roots.push(PathBuf::from(env_dir));
        }
    }
    if let Some(p) = state.icon_store_dir.parent() {
        let pb = p.to_path_buf();
        if !roots.contains(&pb) {
            roots.push(pb);
        }
    }
    let def = crate::api::state::default_data_dir().join("assets");
    if !roots.contains(&def) {
        roots.push(def);
    }
    roots
}

pub fn resolve_media_local_path(state: &ApiState, name_or_path: &str) -> Option<PathBuf> {
    if name_or_path.is_empty() {
        return None;
    }
    let direct = Path::new(name_or_path);
    if direct.is_absolute() && direct.is_file() {
        return Some(direct.to_path_buf());
    }
    if !is_safe_filename(name_or_path) {
        return None;
    }
    for root in resolve_asset_roots(state) {
        for sub in ["media_themes", "media", "default_backgrounds"] {
            let p = root.join(sub).join(name_or_path);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

fn is_safe_filename(name: &str) -> bool {
    !name.is_empty()
        && !name.contains("..")
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

fn parse_font_family_from_filename(fname: &str) -> String {
    let stem = fname
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(fname);
    stem.split('-').next().unwrap_or(stem).to_string()
}

/// File name of a bundled media tool on this platform (`openflux-ffmpeg`, `openflux-ffprobe`).
fn bundled_tool_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_string()
    }
}

/// A bundled tool installed next to the daemon's executable, where the installers put sidecars.
fn bundled_tool(stem: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let p = exe.parent()?.join(bundled_tool_name(stem));
    p.is_file().then_some(p)
}

fn env_tool(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.is_empty())
}

/// The FFmpeg used for imports: `OPENFLUX_HOST_FFMPEG`, else the bundled `openflux-ffmpeg`.
/// Development builds also accept an FFmpeg installed on the system; release builds use only
/// the bundled one.
pub(crate) fn find_ffmpeg() -> Option<String> {
    if let Some(p) = env_tool("OPENFLUX_HOST_FFMPEG") {
        return Some(p);
    }
    if let Some(p) = bundled_tool("openflux-ffmpeg") {
        return Some(p.to_string_lossy().to_string());
    }
    if cfg!(debug_assertions) {
        for cand in [
            "/opt/homebrew/bin/ffmpeg",
            "/usr/local/bin/ffmpeg",
            "/usr/bin/ffmpeg",
        ] {
            if Path::new(cand).is_file() {
                return Some(cand.to_string());
            }
        }
    }
    None
}

/// The FFprobe used to read durations: `OPENFLUX_HOST_FFPROBE`, else `openflux-ffprobe` next to
/// the FFmpeg in use, else the bundled one. A system FFprobe is never used.
fn find_ffprobe(ff: &str) -> Option<String> {
    if let Some(p) = env_tool("OPENFLUX_HOST_FFPROBE") {
        return Some(p);
    }
    let sibling = Path::new(ff).with_file_name(bundled_tool_name("openflux-ffprobe"));
    if sibling.is_file() {
        return Some(sibling.to_string_lossy().to_string());
    }
    bundled_tool("openflux-ffprobe").map(|p| p.to_string_lossy().to_string())
}

fn run_command_with_timeout_diag(
    mut cmd: Command,
    timeout: Duration,
) -> Result<(bool, String), String> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    let err_pipe = child.stderr.take();
    let err_reader = std::thread::spawn(move || -> String {
        let Some(mut pipe) = err_pipe else {
            return String::new();
        };
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > 8192 {
                        let drop_n = buf.len() - 4096;
                        buf.drain(..drop_n);
                    }
                }
            }
        }
        String::from_utf8_lossy(&buf).trim().to_string()
    });
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let err_tail = err_reader.join().unwrap_or_default();
                return Ok((status.success(), err_tail));
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let err_tail = err_reader.join().unwrap_or_default();
                    let msg = if err_tail.is_empty() {
                        "Subprocess timed out and was terminated".to_string()
                    } else {
                        format!("Subprocess timed out ({err_tail})")
                    };
                    return Err(msg);
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = err_reader.join();
                return Err(format!("wait failed: {e}"));
            }
        }
    }
}

fn run_command_with_timeout(cmd: Command, timeout: Duration) -> Result<bool, String> {
    run_command_with_timeout_diag(cmd, timeout).map(|(ok, _)| ok)
}

fn append_https_tls_args(cmd: &mut Command, src: &Path) {
    if src
        .to_str()
        .map(|s| s.starts_with("https://"))
        .unwrap_or(false)
    {
        cmd.args(["-tls_verify", "1", "-protocol_whitelist", "https,tls,tcp"]);
    }
}

fn stream_sha256_prefix(path: &Path, max_bytes: u64) -> Result<(String, u64), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("stat failed: {e}"))?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > max_bytes {
        return Err("Media file missing, empty, or exceeds maximum allowed size".into());
    }
    let f = std::fs::File::open(path).map_err(|e| format!("open failed: {e}"))?;
    let mut rdr = BufReader::with_capacity(64 * 1024, f);
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = rdr
            .read(&mut buf)
            .map_err(|e| format!("read failed: {e}"))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > max_bytes {
            return Err("Media file exceeds maximum allowed size".into());
        }
        h.update(&buf[..n]);
    }
    let hex = format!("{:x}", h.finalize());
    Ok((hex[..24].to_string(), total))
}

/// Theme presets offered in the catalog, each built on one of OpenFlux's default backgrounds
/// (`assets/default_backgrounds`).
pub fn builtin_theme_presets() -> Vec<serde_json::Value> {
    let preset = |id: &str,
                  name: &str,
                  bg: &str,
                  weight: u32,
                  blur: u32,
                  dim: u32,
                  module_dim: u32,
                  effect: &str,
                  ring: u32,
                  legend: u32,
                  blend: &str| {
        serde_json::json!({
            "id": id,
            "name": name,
            "fontFamily": "Montserrat",
            "fontWeight": weight,
            "fontItalic": false,
            "backgroundImagePath": format!("{bg}.png"),
            "backgroundImageBlurAmount": blur,
            "backgroundImageDimming": dim,
            "moduleBackgroundDimming": module_dim,
            "selectedKeyEffect": effect,
            "ringGlowColor": ring,
            "legendColor": legend,
            "blendingModePreset": blend,
            "thumbnail": format!("{bg}_thumb.jpg"),
        })
    };
    vec![
        preset(
            "preset_aurora_velvet",
            "Aurora Velvet",
            "aurora_velvet",
            300,
            2,
            30,
            70,
            "glowing_rings",
            0xFFC084FC,
            0xFFFFFFFF,
            "lightGlow",
        ),
        preset(
            "preset_cyber_slate",
            "Cyber Slate",
            "cyber_slate",
            500,
            1,
            25,
            65,
            "ripple",
            0xFF38BDF8,
            0xFFE0F2FE,
            "lightGlow",
        ),
        preset(
            "preset_deep_ocean",
            "Deep Ocean",
            "deep_ocean",
            400,
            3,
            35,
            75,
            "glowing_rings",
            0xFF22D3EE,
            0xFFECFEFF,
            "dropShadow",
        ),
        preset(
            "preset_emerald_dusk",
            "Emerald Dusk",
            "emerald_dusk",
            400,
            2,
            35,
            72,
            "solid_highlight",
            0xFF34D399,
            0xFFECFDF5,
            "dropShadow",
        ),
        preset(
            "preset_obsidian_carbon",
            "Obsidian Carbon",
            "obsidian_carbon",
            300,
            0,
            40,
            85,
            "solid_highlight",
            0xFF94A3B8,
            0xFFF8FAFC,
            "off",
        ),
        preset(
            "preset_solar_ember",
            "Solar Ember",
            "solar_ember",
            600,
            2,
            30,
            70,
            "ripple",
            0xFFF59E0B,
            0xFFFEF3C7,
            "dropShadow",
        ),
    ]
}

pub async fn get_assets_catalog(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let res = tokio::task::spawn_blocking(move || {
        let roots = resolve_asset_roots(&state);
        let mut font_families: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut key_icons: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        let mut media_items: BTreeMap<String, serde_json::Value> = BTreeMap::new();

        for root in &roots {
            for font_sub in ["fonts", "bundled_fonts"] {
                let dir = root.join(font_sub);
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for ent in entries.flatten() {
                        let fname = ent.file_name().to_string_lossy().to_string();
                        let lower = fname.to_lowercase();
                        if (lower.ends_with(".ttf") || lower.ends_with(".otf"))
                            && !fname.contains("MaterialIcons")
                        {
                            let fam = parse_font_family_from_filename(&fname);
                            font_families.entry(fam).or_default().push(fname);
                        }
                    }
                }
            }

            for icon_sub in ["key_icons", "weather_icons"] {
                let dir = root.join(icon_sub);
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for ent in entries.flatten() {
                        let fname = ent.file_name().to_string_lossy().to_string();
                        let lower = fname.to_lowercase();
                        if lower.ends_with(".png")
                            || lower.ends_with(".jpg")
                            || lower.ends_with(".svg")
                            || lower.ends_with(".bmp")
                        {
                            key_icons.entry(fname.clone()).or_insert_with(|| {
                                serde_json::json!({
                                    "id": fname,
                                    "category": icon_sub,
                                    "name": fname.rsplit_once('.').map(|(a, _)| a).unwrap_or(&fname),
                                })
                            });
                        }
                    }
                }
            }

            // The same directories that `resolve_media_local_path` and `get_asset_file` search.
            for media_sub in ["media_themes", "default_backgrounds", "media"] {
                let dir = root.join(media_sub);
                if let Ok(entries) = std::fs::read_dir(&dir) {
                    for ent in entries.flatten() {
                        let p = ent.path();
                        if !p.is_file() {
                            continue;
                        }
                        let fname = ent.file_name().to_string_lossy().to_string();
                        let lower = fname.to_lowercase();
                        let is_vid = lower.ends_with(".mp4")
                            || lower.ends_with(".mkv")
                            || lower.ends_with(".webm");
                        let is_img = lower.ends_with(".png")
                            || lower.ends_with(".jpg")
                            || lower.ends_with(".jpeg")
                            || lower.ends_with(".gif");
                        if is_vid || is_img {
                            let stem = fname.rsplit_once('.').map(|(a, _)| a).unwrap_or(&fname);
                            let thumb_dir = dir.join("thumbnails");
                            let thumb = [format!("{stem}_thumb.jpg"), format!("{stem}.jpg")]
                                .into_iter()
                                .find(|c| thumb_dir.join(c).is_file())
                                .or_else(|| {
                                    std::fs::read_dir(&thumb_dir).ok().and_then(|rd| {
                                        rd.flatten()
                                            .map(|e| e.file_name().to_string_lossy().to_string())
                                            .find(|n| n.starts_with(stem))
                                    })
                                });
                            media_items.entry(fname.clone()).or_insert_with(|| {
                                serde_json::json!({
                                    "name": fname,
                                    "path": fname,
                                    "fullPath": p.to_string_lossy(),
                                    "category": media_sub,
                                    "kind": if is_vid { "video" } else { "image" },
                                    "thumbnail": thumb,
                                })
                            });
                        }
                    }
                }
            }
        }

        let mut fonts_list: Vec<serde_json::Value> = Vec::new();
        for (fam, files) in font_families {
            let has_italic = files.iter().any(|f| f.to_lowercase().contains("italic"));
            let has_variable = files.iter().any(|f| f.contains("VariableFont"));
            fonts_list.push(serde_json::json!({
                "family": fam,
                "files": files,
                "hasItalic": has_italic,
                "isVariable": has_variable,
            }));
        }

        let shortcut_packs = crate::api::shortcuts::pack_summaries(
            &roots,
            &crate::api::shortcuts::read_custom(&state),
        );

        serde_json::json!({
            "fonts": fonts_list,
            "keyIcons": key_icons.into_values().collect::<Vec<_>>(),
            "media": media_items.into_values().collect::<Vec<_>>(),
            "presets": builtin_theme_presets(),
            "shortcutPacks": shortcut_packs,
        })
    })
    .await;

    match res {
        Ok(v) => {
            crate::preview::release_process_heap_pages();
            (StatusCode::OK, Json(v)).into_response()
        }
        Err(e) => err_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Catalog error: {e}"),
        )
        .into_response(),
    }
}

#[cfg(test)]
fn count_csv_rows(path: &Path) -> usize {
    let Ok(txt) = std::fs::read_to_string(path) else {
        return 0;
    };
    parse_shortcut_csv_str("", &txt)
        .get("rows")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0)
}

fn parse_csv_records(txt: &str) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = txt.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    cur.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
        } else if c == ',' {
            fields.push(std::mem::take(&mut cur));
        } else if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            fields.push(std::mem::take(&mut cur));
            if fields.iter().any(|f| !f.trim().is_empty()) {
                records.push(std::mem::take(&mut fields));
            } else {
                fields.clear();
            }
        } else if c == '\n' {
            fields.push(std::mem::take(&mut cur));
            if fields.iter().any(|f| !f.trim().is_empty()) {
                records.push(std::mem::take(&mut fields));
            } else {
                fields.clear();
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() || !fields.is_empty() {
        fields.push(cur);
        if fields.iter().any(|f| !f.trim().is_empty()) {
            records.push(fields);
        }
    }
    records
}

fn csv_key_to_qmk(raw: &str, is_mac: bool, mac_fn: bool) -> Option<&'static str> {
    let u = raw.trim().to_ascii_uppercase();
    match u.as_str() {
        "A" | "VK_A" => Some("KC_A"),
        "B" | "VK_B" => Some("KC_B"),
        "C" | "VK_C" => Some("KC_C"),
        "D" | "VK_D" => Some("KC_D"),
        "E" | "VK_E" => Some("KC_E"),
        "F" | "VK_F" => Some("KC_F"),
        "G" | "VK_G" => Some("KC_G"),
        "H" | "VK_H" => Some("KC_H"),
        "I" | "VK_I" => Some("KC_I"),
        "J" | "VK_J" => Some("KC_J"),
        "K" | "VK_K" => Some("KC_K"),
        "L" | "VK_L" => Some("KC_L"),
        "M" | "VK_M" => Some("KC_M"),
        "N" | "VK_N" => Some("KC_N"),
        "O" | "VK_O" => Some("KC_O"),
        "P" | "VK_P" => Some("KC_P"),
        "Q" | "VK_Q" => Some("KC_Q"),
        "R" | "VK_R" => Some("KC_R"),
        "S" | "VK_S" => Some("KC_S"),
        "T" | "VK_T" => Some("KC_T"),
        "U" | "VK_U" => Some("KC_U"),
        "V" | "VK_V" => Some("KC_V"),
        "W" | "VK_W" => Some("KC_W"),
        "X" | "VK_X" => Some("KC_X"),
        "Y" | "VK_Y" => Some("KC_Y"),
        "Z" | "VK_Z" => Some("KC_Z"),
        "0" | "VK_0" => Some("KC_0"),
        "1" | "VK_1" => Some("KC_1"),
        "2" | "VK_2" => Some("KC_2"),
        "3" | "VK_3" => Some("KC_3"),
        "4" | "VK_4" => Some("KC_4"),
        "5" | "VK_5" => Some("KC_5"),
        "6" | "VK_6" => Some("KC_6"),
        "7" | "VK_7" => Some("KC_7"),
        "8" | "VK_8" => Some("KC_8"),
        "9" | "VK_9" => Some("KC_9"),
        "SPACE" | "SPACEBAR" | "VK_SPACE" => Some("KC_SPACE"),
        "TAB" | "VK_TAB" => Some("KC_TAB"),
        "RETURN" | "ENTER" | "VK_RETURN" => Some("KC_ENTER"),
        "ESC" | "ESCAPE" | "VK_ESCAPE" => Some("KC_ESCAPE"),
        "BACKSPACE" | "VK_BACK" => Some("KC_BACKSPACE"),
        "DELETE" | "DEL" | "VK_DELETE" => {
            if is_mac && !mac_fn {
                Some("KC_BACKSPACE")
            } else {
                Some("KC_DELETE")
            }
        }
        "HOME" | "VK_HOME" => Some("KC_HOME"),
        "END" | "VK_END" => Some("KC_END"),
        "PAGE UP" | "PGUP" | "VK_PRIOR" => Some("KC_PAGE_UP"),
        "PAGE DOWN" | "PGDN" | "VK_NEXT" => Some("KC_PAGE_DOWN"),
        "UP" | "UP ARROW" | "VK_UP" => Some("KC_UP"),
        "DOWN" | "DOWN ARROW" | "VK_DOWN" => Some("KC_DOWN"),
        "LEFT" | "LEFT ARROW" | "VK_LEFT" => Some("KC_LEFT"),
        "RIGHT" | "RIGHT ARROW" | "VK_RIGHT" => Some("KC_RIGHT"),
        "-" | "MINUS" | "VK_OEM_MINUS" => Some("KC_MINUS"),
        "=" | "EQUAL" | "VK_OEM_PLUS" => Some("KC_EQUAL"),
        "," | "COMMA" | "VK_OEM_COMMA" => Some("KC_COMMA"),
        "." | "PERIOD" | "VK_OEM_PERIOD" => Some("KC_DOT"),
        "/" | "SLASH" | "VK_OEM_2" => Some("KC_SLASH"),
        ";" | "SEMICOLON" | "VK_OEM_1" => Some("KC_SEMICOLON"),
        "'" | "QUOTE" | "VK_OEM_7" => Some("KC_QUOTE"),
        "[" | "VK_OEM_4" => Some("KC_LEFT_BRACKET"),
        "]" | "VK_OEM_6" => Some("KC_RIGHT_BRACKET"),
        "\\" | "VK_OEM_5" => Some("KC_BACKSLASH"),
        "`" | "VK_OEM_3" => Some("KC_GRAVE"),
        "F1" | "VK_F1" => Some("KC_F1"),
        "F2" | "VK_F2" => Some("KC_F2"),
        "F3" | "VK_F3" => Some("KC_F3"),
        "F4" | "VK_F4" => Some("KC_F4"),
        "F5" | "VK_F5" => Some("KC_F5"),
        "F6" | "VK_F6" => Some("KC_F6"),
        "F7" | "VK_F7" => Some("KC_F7"),
        "F8" | "VK_F8" => Some("KC_F8"),
        "F9" | "VK_F9" => Some("KC_F9"),
        "F10" | "VK_F10" => Some("KC_F10"),
        "F11" | "VK_F11" => Some("KC_F11"),
        "F12" | "VK_F12" => Some("KC_F12"),
        _ => None,
    }
}

pub fn parse_shortcut_csv_str(pack: &str, txt: &str) -> serde_json::Value {
    let records = parse_csv_records(txt);
    let mut rec_iter = records.into_iter();
    let headers: Vec<String> = rec_iter
        .next()
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.trim().to_string())
        .collect();
    let idx_of = |name: &str| headers.iter().position(|h| h.eq_ignore_ascii_case(name));
    let i_inc = idx_of("Include this shortcut?");
    let i_app = idx_of("Application");
    let i_ctx = idx_of("Application Context");
    let i_sc = idx_of("Shortcut");
    let i_lbl = idx_of("Shortcut label");
    let i_icon = idx_of("Icon");
    let i_mstr = idx_of("Mac String");
    let i_wstr = idx_of("Win String");
    let i_mcmd = idx_of("Mac-Cmd");
    let i_mctl = idx_of("Mac-Ctrl");
    let i_msft = idx_of("Mac-Shift");
    let i_mopt = idx_of("Mac-Opt");
    let i_mfn = idx_of("Mac-Fn");
    let i_mkey = idx_of("Mac-US Key 1");
    let i_mkey2 = idx_of("Mac-US Key 2");
    let i_mconst = idx_of("Mac-US Key 1 constant");
    let i_mconst2 = idx_of("Mac-US Key 2 constant");
    let i_wctl = idx_of("Win-Ctrl");
    let i_wsft = idx_of("Win-Shift");
    let i_walt = idx_of("Win-Alt");
    let i_wwin = idx_of("Win-Winkey");
    let i_wkey = idx_of("Win-US Key 1");
    let i_wkey2 = idx_of("Win-US Key 2");
    let i_wconst = idx_of("Win-US Key 1 constant");
    let i_wconst2 = idx_of("Win-US Key 2 constant");

    let mut rows = Vec::new();
    let mut app_name = pack
        .strip_suffix(".csv")
        .unwrap_or(pack)
        .replace("%20", " ");
    for cols in rec_iter {
        let get_col = |idx: Option<usize>| -> String {
            idx.and_then(|i| cols.get(i))
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        };
        if get_col(i_inc).eq_ignore_ascii_case("no") {
            continue;
        }
        let row_app = get_col(i_app);
        if !row_app.is_empty() {
            app_name = row_app;
        }
        let title = get_col(i_sc);
        if title.is_empty() {
            continue;
        }
        let label = {
            let l = get_col(i_lbl);
            if l.is_empty() {
                title.clone()
            } else {
                l
            }
        };
        let is_one =
            |idx: Option<usize>| get_col(idx) == "1" || get_col(idx).eq_ignore_ascii_case("true");
        let mut mac_mods = 0u32;
        if is_one(i_mctl) {
            mac_mods |= 0x01;
        }
        if is_one(i_msft) {
            mac_mods |= 0x02;
        }
        if is_one(i_mopt) {
            mac_mods |= 0x04;
        }
        if is_one(i_mcmd) {
            mac_mods |= 0x08;
        }
        let mut win_mods = 0u32;
        if is_one(i_wctl) {
            win_mods |= 0x01;
        }
        if is_one(i_wsft) {
            win_mods |= 0x02;
        }
        if is_one(i_walt) {
            win_mods |= 0x04;
        }
        if is_one(i_wwin) {
            win_mods |= 0x08;
        }

        let mac_raw_1 = get_col(i_mkey);
        let mac_const_1 = get_col(i_mconst);
        let mac_raw_2 = get_col(i_mkey2);
        let mac_const_2 = get_col(i_mconst2);
        let win_raw_1 = get_col(i_wkey);
        let win_const_1 = get_col(i_wconst);
        let win_raw_2 = get_col(i_wkey2);
        let win_const_2 = get_col(i_wconst2);

        let mac_fn = is_one(i_mfn);
        let mac_kc = csv_key_to_qmk(&mac_raw_1, true, mac_fn)
            .or_else(|| csv_key_to_qmk(&mac_const_1, true, mac_fn));
        let win_kc = csv_key_to_qmk(&win_raw_1, false, false)
            .or_else(|| csv_key_to_qmk(&win_const_1, false, false));
        let mac_has_chord2 = !mac_raw_2.is_empty() || !mac_const_2.is_empty();
        let win_has_chord2 = !win_raw_2.is_empty() || !win_const_2.is_empty();
        let mac_kc2 = csv_key_to_qmk(&mac_raw_2, true, mac_fn)
            .or_else(|| csv_key_to_qmk(&mac_const_2, true, mac_fn));
        let win_kc2 = csv_key_to_qmk(&win_raw_2, false, false)
            .or_else(|| csv_key_to_qmk(&win_const_2, false, false));

        let icon_name = {
            let ic = get_col(i_icon);
            if ic.is_empty() || ic.ends_with(".png") {
                ic
            } else {
                format!("{ic}.png")
            }
        };

        rows.push(serde_json::json!({
            "context": get_col(i_ctx),
            "shortcut": title,
            "label": label,
            "icon": icon_name,
            "macString": get_col(i_mstr),
            "winString": get_col(i_wstr),
            "macKeyCode": mac_kc,
            "macKeyCode2": mac_kc2,
            "macHasSecondKey": mac_has_chord2,
            "macModifiers": mac_mods,
            "winKeyCode": win_kc,
            "winKeyCode2": win_kc2,
            "winHasSecondKey": win_has_chord2,
            "winModifiers": win_mods,
        }));
    }
    serde_json::json!({
        "pack": pack,
        "application": app_name,
        "rows": rows,
    })
}

pub async fn get_asset_file(
    State(state): State<Arc<ApiState>>,
    Query(q): Query<BTreeMap<String, String>>,
) -> impl IntoResponse {
    let Some(cat) = q
        .get("cat")
        .filter(|s| {
            matches!(
                s.as_str(),
                "key_icons"
                    | "weather_icons"
                    | "shortcuts"
                    | "presets"
                    | "media_themes"
                    | "media"
                    | "default_backgrounds"
                    | "thumbnails"
                    | "fonts"
            )
        })
        .cloned()
    else {
        return err_msg(StatusCode::BAD_REQUEST, "Unsupported asset category").into_response();
    };
    let Some(name) = q.get("name").filter(|s| is_safe_filename(s)).cloned() else {
        return err_msg(StatusCode::BAD_REQUEST, "Invalid asset name").into_response();
    };
    let res = tokio::task::spawn_blocking(move || {
        let mut found: Option<PathBuf> = None;
        for root in resolve_asset_roots(&state) {
            let p = if cat == "thumbnails" {
                let p1 = root.join("media_themes/thumbnails").join(&name);
                let p2 = root.join("default_backgrounds/thumbnails").join(&name);
                let p3 = root.join("media/thumbnails").join(&name);
                if p1.is_file() {
                    p1
                } else if p2.is_file() {
                    p2
                } else {
                    p3
                }
            } else if cat == "media_themes" || cat == "media" || cat == "default_backgrounds" {
                let p1 = root.join("media_themes").join(&name);
                let p2 = root.join("default_backgrounds").join(&name);
                let p3 = root.join("media").join(&name);
                if p1.is_file() {
                    p1
                } else if p2.is_file() {
                    p2
                } else {
                    p3
                }
            } else if cat == "presets" {
                // Theme presets are built on the default backgrounds and use their thumbnails.
                root.join("default_backgrounds/thumbnails").join(&name)
            } else if cat == "fonts" {
                let p1 = root.join("fonts").join(&name);
                let p2 = root.join("bundled_fonts").join(&name);
                if p1.is_file() {
                    p1
                } else {
                    p2
                }
            } else {
                root.join(&cat).join(&name)
            };
            if p.is_file() {
                found = Some(p);
                break;
            }
        }
        let Some(path) = found else {
            return Err((StatusCode::NOT_FOUND, "Asset not found"));
        };
        let Ok(bytes) = std::fs::read(&path) else {
            return Err((StatusCode::INTERNAL_SERVER_ERROR, "Failed to read asset"));
        };
        let lower = name.to_lowercase();
        let ctype = if lower.ends_with(".png") {
            "image/png"
        } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
            "image/jpeg"
        } else if lower.ends_with(".svg") {
            "image/svg+xml"
        } else if lower.ends_with(".ttf") {
            "font/ttf"
        } else if lower.ends_with(".otf") {
            "font/otf"
        } else {
            "application/octet-stream"
        };
        Ok((ctype, bytes))
    })
    .await;

    match res {
        Ok(Ok((ctype, bytes))) => (
            StatusCode::OK,
            [(axum::http::header::CONTENT_TYPE, ctype)],
            bytes,
        )
            .into_response(),
        Ok(Err((code, msg))) => err_msg(code, msg).into_response(),
        Err(e) => err_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Asset error: {e}"),
        )
        .into_response(),
    }
}

/// Applications installed in the standard macOS application folders, one per bundle ID.
#[cfg(target_os = "macos")]
fn installed_apps() -> Vec<serde_json::Value> {
    let mut apps: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut search_dirs = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        search_dirs.push(PathBuf::from(home).join("Applications"));
    }
    for dir in search_dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for ent in entries.flatten() {
            let p = ent.path();
            if p.extension().and_then(|s| s.to_str()) != Some("app") {
                continue;
            }
            let plist = p.join("Contents/Info.plist");
            if !plist.is_file() {
                continue;
            }
            let Ok(out) = Command::new("plutil")
                .args(["-convert", "json", "-o", "-"])
                .arg(&plist)
                .output()
            else {
                continue;
            };
            if !out.status.success() {
                continue;
            }
            let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
                continue;
            };
            let Some(bid) = v
                .get("CFBundleIdentifier")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let name = v
                .get("CFBundleDisplayName")
                .or_else(|| v.get("CFBundleName"))
                .and_then(|x| x.as_str())
                .unwrap_or_else(|| p.file_stem().and_then(|s| s.to_str()).unwrap_or(bid));
            apps.entry(bid.to_string()).or_insert_with(|| {
                serde_json::json!({
                    "bundleId": bid,
                    "name": name,
                    "path": p.to_string_lossy(),
                })
            });
        }
    }
    apps.into_values().collect()
}

/// Installed applications are listed on macOS only.
#[cfg(not(target_os = "macos"))]
fn installed_apps() -> Vec<serde_json::Value> {
    Vec::new()
}

pub async fn get_installed_apps() -> impl IntoResponse {
    let res = tokio::task::spawn_blocking(installed_apps).await;

    match res {
        Ok(apps) => {
            crate::preview::release_process_heap_pages();
            (StatusCode::OK, Json(serde_json::json!({ "apps": apps }))).into_response()
        }
        Err(e) => err_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Couldn't list installed applications: {e}"),
        )
        .into_response(),
    }
}

struct TempCleanupGuard(Option<PathBuf>);
impl Drop for TempCleanupGuard {
    fn drop(&mut self) {
        if let Some(ref p) = self.0 {
            let _ = std::fs::remove_file(p);
        }
    }
}

static MEDIA_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static AERIAL_IMPORT_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

/// Duration of a video in seconds, or `None` when it can't be read. Uses the bundled FFprobe, and
/// the container duration FFmpeg reports when no FFprobe is available.
fn probe_video_duration_secs(ff: &str, src: &Path) -> Option<f64> {
    probe_video_duration_with(ff, find_ffprobe(ff).as_deref(), src)
}

fn probe_video_duration_with(ff: &str, ffprobe: Option<&str>, src: &Path) -> Option<f64> {
    if let Some(probe) = ffprobe {
        let mut cmd = Command::new(probe);
        cmd.args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ]);
        append_https_tls_args(&mut cmd, src);
        cmd.arg(src);
        if let Some(out) = run_capturing(cmd, true, Duration::from_secs(8)) {
            if let Some(d) = out
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|&v| v.is_finite() && v > 0.1)
            {
                return Some(d);
            }
        }
    }
    // `ffmpeg -i <file>` prints the container duration and exits with an error (no output given).
    let mut cmd = Command::new(ff);
    cmd.args(["-hide_banner", "-nostdin"]);
    append_https_tls_args(&mut cmd, src);
    cmd.arg("-i").arg(src);
    let text = run_capturing(cmd, false, Duration::from_secs(8))?;
    parse_ffmpeg_duration(&text)
}

/// Runs `cmd` and returns its stdout (`stdout == true`) or stderr, or `None` if it couldn't run in
/// time. With `stdout`, a failing exit status also gives `None`.
fn run_capturing(mut cmd: Command, stdout: bool, timeout: Duration) -> Option<String> {
    cmd.stdin(Stdio::null());
    if stdout {
        cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    } else {
        cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    }
    let mut child = cmd.spawn().ok()?;
    let pipe: Option<Box<dyn Read + Send>> = if stdout {
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>)
    } else {
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>)
    };
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(p) = pipe {
            let _ = p.take(1024 * 1024).read_to_end(&mut buf);
        }
        String::from_utf8_lossy(&buf).to_string()
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let text = reader.join().ok()?;
    if stdout && !status.success() {
        return None;
    }
    Some(text)
}

/// Reads `Duration: HH:MM:SS.ss` from FFmpeg's description of its input.
fn parse_ffmpeg_duration(text: &str) -> Option<f64> {
    let rest = &text[text.find("Duration: ")? + "Duration: ".len()..];
    let stamp = rest.split(|c: char| c == ',' || c.is_whitespace()).next()?;
    let mut parts = stamp.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let sec: f64 = parts.next()?.parse().ok()?;
    let d = h * 3600.0 + m * 60.0 + sec;
    (d.is_finite() && d > 0.1).then_some(d)
}

/// Largest media file sent to the keyboard. The keyboard has gigabytes free; this bounds transfer
/// time and the memory a save needs.
pub const MAX_KEYBOARD_MEDIA_BYTES: u64 = 96 * 1024 * 1024;
const MAX_VIDEO_KBPS: f64 = 8000.0;
/// Version of the transcode rules, part of the name of every transcoded import.
const MEDIA_TRANSCODE_FORMAT: &str = "tc3";
/// Shortest time a transcode may take before it is abandoned; longer videos get proportionally more.
const MIN_TRANSCODE_TIMEOUT: Duration = Duration::from_secs(240);
/// Time allowed for a video whose length couldn't be read.
const UNKNOWN_LENGTH_TIMEOUT: Duration = Duration::from_secs(1800);
/// Length of the pieces a loop-reverse video is reversed in. FFmpeg's `reverse` filter keeps every
/// frame of its input in memory, so reversing piece by piece bounds memory by this length (about
/// 150 MB of decoded frames at 1080p and 24 fps) whatever the video's length.
const REVERSE_SEGMENT_SECS: u32 = 2;
/// Frame rate of loop-reverse videos, so the pieces line up.
const LOOP_REVERSE_FPS: u32 = 24;

/// One output size the transcoder may use, and the lowest bitrate it is used at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rung {
    pub label: &'static str,
    pub width: u32,
    pub height: u32,
    pub min_kbps: f64,
}

/// Output sizes from best to the floor. A video that doesn't fit at the floor is refused.
pub const RUNGS: [Rung; 3] = [
    Rung {
        label: "1080p",
        width: 1920,
        height: 1080,
        min_kbps: 2500.0,
    },
    Rung {
        label: "720p",
        width: 1280,
        height: 720,
        min_kbps: 1200.0,
    },
    Rung {
        label: "540p",
        width: 960,
        height: 540,
        min_kbps: 400.0,
    },
];

/// The size limit a transcode must meet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaLimits {
    /// Largest file accepted.
    pub max_bytes: u64,
    /// Size aimed for, leaving headroom for bitrate overshoot.
    pub target_bytes: f64,
}

impl MediaLimits {
    pub const KEYBOARD: MediaLimits = MediaLimits {
        max_bytes: MAX_KEYBOARD_MEDIA_BYTES,
        target_bytes: 90.0 * 1024.0 * 1024.0,
    };

    /// Longest output, in seconds, that fits at the floor.
    pub fn longest_fitting_secs(&self) -> f64 {
        self.target_bytes * 8.0 / 1000.0 / RUNGS[RUNGS.len() - 1].min_kbps
    }
}

fn mib(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}

/// The encodes to try, best first: each a size and a bitrate. The plan only lowers bitrate and
/// size; it never trims the video, and a video that can't fit at the floor is refused. With an
/// unknown length, only the floor is tried and the result must still meet the limit.
pub fn plan_encodes(
    output_secs: Option<f64>,
    limits: &MediaLimits,
) -> Result<Vec<(Rung, u32)>, String> {
    let floor = RUNGS[RUNGS.len() - 1];
    let Some(d) = output_secs else {
        return Ok(vec![(floor, floor.min_kbps as u32)]);
    };
    let target = (limits.target_bytes * 8.0 / 1000.0 / d.max(1.0)).min(MAX_VIDEO_KBPS);
    if target < floor.min_kbps {
        return Err(format!(
            "This video plays for {d:.0}s. The keyboard holds {} MiB per video, which at the lowest quality OpenFlux uses ({}x{}) is about {:.0}s.",
            mib(limits.max_bytes),
            floor.width,
            floor.height,
            limits.longest_fitting_secs()
        ));
    }
    let first = RUNGS
        .iter()
        .position(|r| target >= r.min_kbps)
        .unwrap_or(RUNGS.len() - 1);
    // A lower size is tried only if the encode overshoots, with a little less bitrate.
    Ok(RUNGS[first..]
        .iter()
        .enumerate()
        .map(|(n, r)| (*r, (target * 0.85f64.powi(n as i32)).max(r.min_kbps) as u32))
        .collect())
}

/// Result of transcoding a video for the keyboard.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscodeOutcome {
    /// Duration of the produced file (twice the source when loop-reversed), read from the file.
    /// `None` when it couldn't be read; it is never an estimate.
    pub output_duration_s: Option<f64>,
    /// Duration of the source, when it could be probed.
    pub source_duration_s: Option<f64>,
    pub resolution: String,
    pub bitrate_kbps: u32,
}

/// Transcodes all of `src` to H.264 for the keyboard (see [`transcode_with_limits`]).
fn transcode_video_for_keyboard(
    ff: &str,
    src: &Path,
    dst: &Path,
    loop_reverse: bool,
) -> Result<TranscodeOutcome, (StatusCode, String)> {
    transcode_with_limits(ff, src, dst, loop_reverse, &MediaLimits::KEYBOARD)
}

fn scale_filter(r: &Rung) -> String {
    // force_divisible_by=2 keeps odd-dimension sources valid for yuv420p.
    format!(
        "scale=min({}\\,iw):min({}\\,ih):force_original_aspect_ratio=decrease:force_divisible_by=2",
        r.width, r.height
    )
}

fn encode_args(cmd: &mut Command, kbps: u32) {
    let maxrate = kbps * 13 / 10;
    cmd.args([
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-b:v",
        &format!("{kbps}k"),
        "-maxrate",
        &format!("{maxrate}k"),
        "-bufsize",
        &format!("{}k", maxrate * 2),
    ])
    .args(["-pix_fmt", "yuv420p", "-an"]);
}

fn run_ffmpeg(cmd: Command, timeout: Duration) -> Result<(), (StatusCode, String)> {
    match run_command_with_timeout_diag(cmd, timeout) {
        Ok((true, _)) => Ok(()),
        Ok((false, tail)) => Err((
            StatusCode::BAD_GATEWAY,
            format!("ffmpeg transcode failed: {tail}"),
        )),
        Err(e) => Err((
            StatusCode::GATEWAY_TIMEOUT,
            format!("ffmpeg transcode aborted: {e}"),
        )),
    }
}

/// Encodes the whole of `src` forward, then the whole of it reversed.
///
/// The source is first encoded at the output size with a key frame every second and cut into
/// [`REVERSE_SEGMENT_SECS`] pieces; each piece is reversed on its own and the pieces are joined in
/// reverse order after the forward video. Memory stays bounded by one piece.
fn encode_loop_reverse(
    ff: &str,
    src: &Path,
    out: &Path,
    rung: &Rung,
    kbps: u32,
    timeout: Duration,
    work: &Path,
) -> Result<(), (StatusCode, String)> {
    let io = |e: std::io::Error| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Media work directory: {e}"),
        )
    };
    let _ = std::fs::remove_dir_all(work);
    std::fs::create_dir_all(work).map_err(io)?;
    let fwd = work.join("forward.mp4");
    let mut cmd = Command::new(ff);
    cmd.args(["-y", "-loglevel", "error"]);
    append_https_tls_args(&mut cmd, src);
    cmd.arg("-i").arg(src).args([
        "-vf",
        &format!("{},fps={LOOP_REVERSE_FPS}", scale_filter(rung)),
    ]);
    cmd.args([
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "14",
        "-g",
        &LOOP_REVERSE_FPS.to_string(),
        "-pix_fmt",
        "yuv420p",
        "-an",
    ]);
    cmd.arg(&fwd);
    run_ffmpeg(cmd, timeout)?;

    let mut cmd = Command::new(ff);
    cmd.args(["-y", "-loglevel", "error", "-i"]).arg(&fwd);
    cmd.args([
        "-c",
        "copy",
        "-f",
        "segment",
        "-segment_time",
        &REVERSE_SEGMENT_SECS.to_string(),
        "-reset_timestamps",
        "1",
    ]);
    cmd.arg(work.join("piece_%05d.mp4"));
    run_ffmpeg(cmd, timeout)?;

    let mut pieces: Vec<PathBuf> = std::fs::read_dir(work)
        .map_err(io)?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("piece_"))
        })
        .collect();
    pieces.sort();
    // Entries name files in the list's own directory, so no path is quoted into the list.
    let mut list = String::from("file 'forward.mp4'\n");
    for (n, piece) in pieces.iter().enumerate().rev() {
        let rev_name = format!("reversed_{n:05}.mp4");
        let rev = work.join(&rev_name);
        let mut cmd = Command::new(ff);
        cmd.args(["-y", "-loglevel", "error", "-i"]).arg(piece);
        cmd.args([
            "-vf", "reverse", "-c:v", "libx264", "-preset", "veryfast", "-crf", "14", "-pix_fmt",
            "yuv420p", "-an",
        ]);
        cmd.arg(&rev);
        run_ffmpeg(cmd, timeout)?;
        list.push_str(&format!("file '{rev_name}'\n"));
    }
    std::fs::write(work.join("list.txt"), list).map_err(io)?;
    // The join runs in the work directory; the output path must not depend on it.
    let out = std::path::absolute(out).map_err(io)?;
    let mut cmd = Command::new(ff);
    cmd.current_dir(work);
    cmd.args([
        "-y",
        "-loglevel",
        "error",
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
        "list.txt",
    ]);
    encode_args(&mut cmd, kbps);
    cmd.arg(&out);
    run_ffmpeg(cmd, timeout)
}

/// Transcodes all of `src` to H.264. The video is never shortened: to meet `limits` the bitrate
/// and then the size are lowered (see [`plan_encodes`]), and a video that still doesn't fit is
/// refused with an explanation. A result that comes out shorter than its source is discarded.
fn transcode_with_limits(
    ff: &str,
    src: &Path,
    dst: &Path,
    loop_reverse: bool,
    limits: &MediaLimits,
) -> Result<TranscodeOutcome, (StatusCode, String)> {
    let seq = MEDIA_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let stem = dst
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("vid")
        .to_string();
    let tmp_out = dst.with_file_name(format!(".tmp_tc_{}_{}_{stem}.mp4", std::process::id(), seq));
    let work = dst.with_file_name(format!(".tmp_rev_{}_{}_{stem}", std::process::id(), seq));
    let _guard = TempCleanupGuard(Some(tmp_out.clone()));
    let _work_guard = TempDirCleanupGuard(work.clone());

    let src_dur = probe_video_duration_secs(ff, src);
    let factor = if loop_reverse { 2.0 } else { 1.0 };
    let out_dur = src_dur.map(|d| d * factor);
    let advice = if loop_reverse {
        "Loop-reverse doubles its length: trim the video or turn off loop-reverse, and import it again."
    } else {
        "Trim the video and import it again."
    };
    let plan = plan_encodes(out_dur, limits)
        .map_err(|m| (StatusCode::PAYLOAD_TOO_LARGE, format!("{m} {advice}")))?;
    let timeout = out_dur.map_or(UNKNOWN_LENGTH_TIMEOUT, |d| {
        MIN_TRANSCODE_TIMEOUT.max(Duration::from_secs_f64(d * 1.5))
    });

    let mut last_size = 0;
    for (rung, kbps) in &plan {
        let _ = std::fs::remove_file(&tmp_out);
        if loop_reverse {
            encode_loop_reverse(ff, src, &tmp_out, rung, *kbps, timeout, &work)?;
        } else {
            let mut cmd = Command::new(ff);
            cmd.args(["-y", "-loglevel", "error"]);
            append_https_tls_args(&mut cmd, src);
            cmd.arg("-i").arg(src).args(["-vf", &scale_filter(rung)]);
            encode_args(&mut cmd, *kbps);
            cmd.arg(&tmp_out);
            run_ffmpeg(cmd, timeout)?;
        }
        last_size = tmp_out.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
        if last_size > limits.max_bytes {
            continue;
        }
        let output_duration_s = probe_video_duration_secs(ff, &tmp_out);
        if let (Some(want), Some(got)) = (out_dur, output_duration_s) {
            if got + 0.5 < want {
                return Err((
                    StatusCode::BAD_GATEWAY,
                    format!("The converted video came out {got:.1}s long instead of {want:.1}s; it was discarded"),
                ));
            }
        }
        std::fs::rename(&tmp_out, dst).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Media rename failed: {e}"),
            )
        })?;
        return Ok(TranscodeOutcome {
            output_duration_s,
            source_duration_s: src_dur,
            resolution: rung.label.to_string(),
            bitrate_kbps: *kbps,
        });
    }
    let floor = RUNGS[RUNGS.len() - 1];
    Err((
        StatusCode::PAYLOAD_TOO_LARGE,
        if out_dur.is_some() {
            format!(
                "This video doesn't fit the keyboard's {} MiB per video even at {}x{}. {advice}",
                mib(limits.max_bytes),
                floor.width,
                floor.height
            )
        } else {
            format!(
                "This video's length couldn't be read, and at {}x{} it comes to {} MiB, more than the keyboard's {} MiB per video. {advice}",
                floor.width,
                floor.height,
                mib(last_size),
                mib(limits.max_bytes)
            )
        },
    ))
}

/// Removes a directory when dropped.
struct TempDirCleanupGuard(PathBuf);

impl Drop for TempDirCleanupGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn make_thumbnail(dst: &Path, thumb_dst: &Path) {
    if thumb_dst.is_file() {
        return;
    }
    if let Some(ff) = find_ffmpeg() {
        let mut tcmd = Command::new(&ff);
        tcmd.args(["-y", "-loglevel", "error", "-i"])
            .arg(dst)
            .args(["-vframes", "1", "-vf", "scale=320:180"])
            .arg(thumb_dst);
        let _ = run_command_with_timeout(tcmd, Duration::from_secs(15));
    }
}

fn media_dirs(state: &ApiState) -> Result<(PathBuf, PathBuf), (StatusCode, String)> {
    let asset_root = state
        .icon_store_dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| state.icon_store_dir.clone());
    let media_dir = asset_root.join("media_themes");
    let thumb_dir = media_dir.join("thumbnails");
    std::fs::create_dir_all(&thumb_dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Media dir error: {e}"),
        )
    })?;
    Ok((media_dir, thumb_dir))
}

fn remember_imported_media(state: &ApiState, name: &str) {
    let mut g = state.inner.lock().unwrap();
    if !g.imported_media.iter().any(|x| x == name) {
        g.imported_media.push(name.to_string());
    }
}

/// `POST /v1/media/import` with `{"file": path, "loopReverse": bool}`.
///
/// Images, and MP4s under the media limit, are copied as they are; other videos are transcoded at
/// full length (see [`transcode_with_limits`]).
pub async fn import_media_file(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(src_str) = body
        .get("file")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
    else {
        return err_msg(StatusCode::BAD_REQUEST, "file path is required").into_response();
    };
    let loop_reverse = body
        .get("loopReverse")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let state_clone = state.clone();
    let res = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, (StatusCode, String)> {
        let src_buf = expand_home_path(&src_str);
        let src = src_buf.as_path();
        if !src.is_file() {
            return Err((StatusCode::BAD_REQUEST, "Media file does not exist".into()));
        }
        let ext = src
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| s.to_ascii_lowercase())
            .unwrap_or_default();
        let is_video = matches!(ext.as_str(), "mp4" | "mov" | "mkv" | "webm" | "m4v" | "avi");
        let is_image = matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "gif");
        if !is_video && !is_image {
            return Err((StatusCode::BAD_REQUEST, "Unsupported media format".into()));
        }

        const MAX_INPUT_MEDIA_BYTES: u64 = 4 * 1024 * 1024 * 1024;
        let (raw_stem, input_len) = stream_sha256_prefix(src, MAX_INPUT_MEDIA_BYTES)
            .map_err(|e| (StatusCode::BAD_REQUEST, e))?;
        let needs_transcode = is_video && (loop_reverse || ext != "mp4" || input_len > MAX_KEYBOARD_MEDIA_BYTES);
        // A transcoded file is named after the transcode rules that made it, so a file made
        // under other rules is never taken for this import.
        let stem = match (needs_transcode, loop_reverse) {
            (true, true) => format!("{raw_stem}_{MEDIA_TRANSCODE_FORMAT}_loopreverse"),
            (true, false) => format!("{raw_stem}_{MEDIA_TRANSCODE_FORMAT}"),
            (false, _) => raw_stem,
        };
        let (media_dir, thumb_dir) = media_dirs(&state_clone)?;

        let mut transcoded = false;
        let out_ext = if is_video { "mp4" } else { ext.as_str() };
        let out_name = format!("{stem}.{out_ext}");
        let dst = media_dir.join(&out_name);

        let mut outcome: Option<TranscodeOutcome> = None;
        if needs_transcode {
            let Some(ff) = find_ffmpeg() else {
                return Err((StatusCode::BAD_REQUEST, "ffmpeg is required to convert this video for the keyboard".into()));
            };
            if !dst.is_file() {
                outcome = Some(transcode_video_for_keyboard(&ff, src, &dst, loop_reverse)?);
            }
            transcoded = true;
        } else if !is_video && input_len > MAX_KEYBOARD_MEDIA_BYTES {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("Image exceeds the keyboard's {} MiB media limit", mib(MAX_KEYBOARD_MEDIA_BYTES)),
            ));
        } else if !dst.exists() {
            let seq = MEDIA_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let tmp = media_dir.join(format!(".tmp_{}_{}_{out_name}", std::process::id(), seq));
            let _tg = TempCleanupGuard(Some(tmp.clone()));
            std::fs::copy(src, &tmp)
                .and_then(|_| std::fs::rename(&tmp, &dst))
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Media copy error: {e}")))?;
        }

        if dst.metadata().map(|m| m.len() > MAX_KEYBOARD_MEDIA_BYTES).unwrap_or(true) {
            let _ = std::fs::remove_file(&dst);
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("Imported media exceeds the keyboard's {} MiB media limit", mib(MAX_KEYBOARD_MEDIA_BYTES)),
            ));
        }

        let thumb_name = format!("{stem}_thumb.jpg");
        let thumb_dst = thumb_dir.join(&thumb_name);
        make_thumbnail(&dst, &thumb_dst);
        remember_imported_media(&state_clone, &out_name);
        let actual_duration = match &outcome {
            Some(o) => o.output_duration_s,
            None if is_video => find_ffmpeg().and_then(|ff| probe_video_duration_secs(&ff, &dst)),
            None => None,
        };

        Ok(serde_json::json!({
            "assetPath": out_name,
            "fullPath": dst.to_string_lossy(),
            "thumbnail": if thumb_dst.is_file() { Some(thumb_name) } else { None::<String> },
            "isVideo": is_video,
            "transcoded": transcoded,
            "loopReverse": is_video && loop_reverse,
            "sourceDurationS": outcome.as_ref().and_then(|o| o.source_duration_s),
            "durationS": actual_duration,
            "durationKnown": actual_duration.is_some(),
            "bitrateKbps": outcome.as_ref().map(|o| o.bitrate_kbps),
            "resolution": outcome.map(|o| o.resolution).unwrap_or_else(|| "original".to_string()),
        }))
    })
    .await;

    match res {
        Ok(Ok(v)) => (StatusCode::OK, Json(v)).into_response(),
        Ok(Err((code, msg))) => err_msg(code, &msg).into_response(),
        Err(e) => err_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Media import failed unexpectedly: {e}"),
        )
        .into_response(),
    }
}

#[derive(Clone)]
struct AerialCache {
    fetched_at: Instant,
    items: Vec<serde_json::Value>,
}

static AERIAL_CACHE: std::sync::OnceLock<std::sync::Mutex<Option<AerialCache>>> =
    std::sync::OnceLock::new();

/// Reads the video cards of the Aerial catalog page: an index, kept by a third party, of the
/// screensaver videos hosted at `sylvan.apple.com`. The page's markup isn't a stable format, so a
/// card that doesn't parse is skipped.
fn parse_aerial_cards(html: &str) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut rest = html;
    while let Some(pos) = rest.find("thumbnails/") {
        let after_th = &rest[pos..];
        let Some(end_q) = after_th.find('"') else {
            break;
        };
        let thumb_rel = &after_th[..end_q];
        // Each card is read only up to the next card, so URLs are never taken from a neighbour.
        let next_card = after_th[end_q + 1..]
            .find("thumbnails/")
            .map(|i| end_q + 1 + i)
            .unwrap_or(after_th.len());
        let chunk = &after_th[..next_card];

        let title = if let Some(h3s) = chunk.find("<h3") {
            let sub = &chunk[h3s..];
            if let (Some(gt), Some(endh3)) = (sub.find('>'), sub.find("</h3>")) {
                sub[gt + 1..endh3].trim().to_string()
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let mut urls: Vec<(String, String)> = Vec::new();
        let mut scan = chunk;
        while let Some(hp) = scan.find("https://sylvan.apple.com/") {
            let u_start = &scan[hp..];
            let Some(u_end) = u_start.find('"') else {
                break;
            };
            let url = &u_start[..u_end];
            if url.ends_with(".mov") || url.ends_with(".mp4") {
                let kind = if url.contains("2K_AVC") {
                    "1080p H.264"
                } else if url.contains("SDR_2K_HEVC") {
                    "1080p HEVC"
                } else if url.contains("SDR_4K_HEVC") {
                    "4K HEVC"
                } else {
                    "Video"
                };
                urls.push((kind.to_string(), url.to_string()));
            }
            scan = &u_start[u_end + 1..];
        }
        // 1080p SDR H.264 is preferred, then 1080p SDR HEVC.
        let preferred = urls
            .iter()
            .find(|(k, _)| k == "1080p H.264")
            .or_else(|| urls.iter().find(|(k, _)| k == "1080p HEVC"))
            .or_else(|| urls.first());
        if let Some((qlabel, vurl)) = preferred {
            let id = thumb_rel
                .trim_start_matches("thumbnails/")
                .trim_end_matches(".webp")
                .to_string();
            if !title.is_empty() && seen.insert(id.clone()) {
                out.push(serde_json::json!({
                    "id": id,
                    "title": title,
                    "quality": qlabel,
                    "thumbnailUrl": format!("https://aerial-videos.netlify.app/{thumb_rel}"),
                    "videoUrl": vurl,
                }));
            }
        }
        rest = &after_th[next_card..];
    }
    out
}

pub async fn get_aerial_videos() -> impl IntoResponse {
    let cache = AERIAL_CACHE.get_or_init(|| std::sync::Mutex::new(None));
    {
        let g = cache.lock().unwrap();
        if let Some(ref c) = *g {
            if c.fetched_at.elapsed() < Duration::from_secs(3600) && !c.items.is_empty() {
                return (
                    StatusCode::OK,
                    Json(serde_json::json!({ "videos": c.items })),
                )
                    .into_response();
            }
        }
    }
    let res = tokio::task::spawn_blocking(|| -> Result<Vec<serde_json::Value>, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| e.to_string())?;
        let html = client
            .get("https://aerial-videos.netlify.app/")
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.text())
            .map_err(|e| format!("Failed to fetch Aerial catalog: {e}"))?;
        let items = parse_aerial_cards(&html);
        if items.is_empty() {
            return Err("No Aerial videos found in catalog".into());
        }
        Ok(items)
    })
    .await;

    match res {
        Ok(Ok(items)) => {
            *cache.lock().unwrap() = Some(AerialCache {
                fetched_at: Instant::now(),
                items: items.clone(),
            });
            (StatusCode::OK, Json(serde_json::json!({ "videos": items }))).into_response()
        }
        Ok(Err(e)) => err_msg(StatusCode::BAD_GATEWAY, &e).into_response(),
        Err(e) => err_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Fetching the aerial video list failed unexpectedly: {e}"),
        )
        .into_response(),
    }
}

/// Version of the Aerial cache layout and transcode settings. It is part of every cache file name
/// and recorded in each cache entry, so a change here never serves files produced under other
/// rules.
const AERIAL_CACHE_FORMAT: &str = "aerial-cache-3";

/// Cache file stem for one Aerial request.
pub fn aerial_cache_stem(url: &str, loop_reverse: bool) -> String {
    let mut h = Sha256::new();
    h.update(format!("{AERIAL_CACHE_FORMAT}\n{url}\n{loop_reverse}").as_bytes());
    format!("aerial_{}", &format!("{:x}", h.finalize())[..16])
}

/// The details of a cached Aerial import, if the cache holds one made under the current rules.
fn cached_aerial(dst: &Path, meta_path: &Path) -> Option<serde_json::Value> {
    if !dst.is_file() {
        return None;
    }
    let details: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(meta_path).ok()?).ok()?;
    (details.get("cacheFormat").and_then(|v| v.as_str()) == Some(AERIAL_CACHE_FORMAT))
        .then_some(details)
}

/// `POST /v1/media/aerial-import` with `{"url", "title", "loopReverse"}`.
///
/// The whole video is imported (see [`transcode_with_limits`]). With `loopReverse` the result is
/// the video forward and then reversed.
pub async fn import_aerial_video(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(url) = body
        .get("url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
    else {
        return err_msg(StatusCode::BAD_REQUEST, "url is required").into_response();
    };
    let loop_reverse = body
        .get("loopReverse")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !url.starts_with("https://sylvan.apple.com/")
        || !(url.ends_with(".mov") || url.ends_with(".mp4"))
    {
        return err_msg(
            StatusCode::BAD_REQUEST,
            "Only Aerial video URLs from sylvan.apple.com can be imported",
        )
        .into_response();
    }
    let state_clone = state.clone();
    let res =
        tokio::task::spawn_blocking(move || -> Result<serde_json::Value, (StatusCode, String)> {
            let Ok(_imp_guard) = AERIAL_IMPORT_LOCK
                .get_or_init(|| std::sync::Mutex::new(()))
                .try_lock()
            else {
                return Err((
                    StatusCode::TOO_MANY_REQUESTS,
                    "Another Aerial download/transcode is already in progress".into(),
                ));
            };
            let (media_dir, thumb_dir) = media_dirs(&state_clone)?;
            let stem = aerial_cache_stem(&url, loop_reverse);
            let out_name = format!("{stem}.mp4");
            let dst = media_dir.join(&out_name);
            let meta_path = media_dir.join(format!("{stem}.json"));
            let thumb_name = format!("{stem}_thumb.jpg");
            let thumb_dst = thumb_dir.join(&thumb_name);

            let details = match cached_aerial(&dst, &meta_path) {
                Some(d) => d,
                None => {
                    let details = fetch_and_transcode_aerial(&url, &dst, &stem, loop_reverse)?;
                    let _ = std::fs::write(
                        &meta_path,
                        serde_json::to_vec_pretty(&details).unwrap_or_default(),
                    );
                    details
                }
            };

            make_thumbnail(&dst, &thumb_dst);
            remember_imported_media(&state_clone, &out_name);

            let mut out = serde_json::json!({
                "assetPath": out_name,
                "fullPath": dst.to_string_lossy(),
                "thumbnail": if thumb_dst.is_file() { Some(thumb_name) } else { None::<String> },
                "isVideo": true,
                "transcoded": true,
                "loopReverse": loop_reverse,
            });
            if let (Some(o), Some(d)) = (out.as_object_mut(), details.as_object()) {
                for (k, v) in d {
                    o.insert(k.clone(), v.clone());
                }
            }
            Ok(out)
        })
        .await;

    match res {
        Ok(Ok(v)) => (StatusCode::OK, Json(v)).into_response(),
        Ok(Err((code, msg))) => err_msg(code, &msg).into_response(),
        Err(e) => err_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Aerial video import failed unexpectedly: {e}"),
        )
        .into_response(),
    }
}

fn fetch_and_transcode_aerial(
    url: &str,
    dst: &Path,
    stem: &str,
    loop_reverse: bool,
) -> Result<serde_json::Value, (StatusCode, String)> {
    const MAX_AERIAL_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;
    let Some(ff) = find_ffmpeg() else {
        return Err((
            StatusCode::BAD_REQUEST,
            "ffmpeg is required to transcode Aerial videos for the keyboard".into(),
        ));
    };
    let finish = |source: &Path| -> Result<serde_json::Value, (StatusCode, String)> {
        let o = transcode_video_for_keyboard(&ff, source, dst, loop_reverse)?;
        Ok(serde_json::json!({
            "cacheFormat": AERIAL_CACHE_FORMAT,
            "sourceDurationS": o.source_duration_s,
            "durationS": o.output_duration_s,
            "durationKnown": o.output_duration_s.is_some(),
            "bitrateKbps": o.bitrate_kbps,
            "resolution": o.resolution,
        }))
    };
    // ffmpeg reads the URL directly.
    let direct_err = match finish(Path::new(url)) {
        Ok(v) => return Ok(v),
        Err((code, msg))
            if code == StatusCode::PAYLOAD_TOO_LARGE || code == StatusCode::BAD_REQUEST =>
        {
            return Err((code, msg));
        }
        Err((_, msg)) => msg,
    };
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("{e} (streaming the video failed: {direct_err})"),
            )
        })?;
    let mut resp = client
        .get(url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("Aerial download failed: {e} (streaming the video failed: {direct_err})"),
            )
        })?;
    if resp
        .content_length()
        .is_some_and(|clen| clen > MAX_AERIAL_DOWNLOAD_BYTES)
    {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Aerial source exceeds the {} MiB download limit",
                mib(MAX_AERIAL_DOWNLOAD_BYTES)
            ),
        ));
    }
    let seq = MEDIA_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp_in = std::env::temp_dir().join(format!(
        ".openflux_dl_{}_{}_{}.mov",
        std::process::id(),
        seq,
        stem
    ));
    let _dl_guard = TempCleanupGuard(Some(tmp_in.clone()));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_in)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Temp create failed: {e}"),
            )
        })?;
    let mut buf = [0u8; 64 * 1024];
    let mut dl_bytes = 0u64;
    loop {
        let n = resp.read(&mut buf).map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("Aerial download stream error: {e}"),
            )
        })?;
        if n == 0 {
            break;
        }
        dl_bytes += n as u64;
        if dl_bytes > MAX_AERIAL_DOWNLOAD_BYTES {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "Aerial source exceeds the {} MiB download limit",
                    mib(MAX_AERIAL_DOWNLOAD_BYTES)
                ),
            ));
        }
        f.write_all(&buf[..n]).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Aerial temp write error: {e}"),
            )
        })?;
    }
    drop(f);
    finish(&tmp_in)
}

/// Merged profiles, the remapped app rules if the bundle had any, and the imported profile names.
type ProfileMerge = (
    Vec<ShortcutProfileState>,
    Option<Vec<crate::bridges::AppProfileRule>>,
    Vec<String>,
);

/// File extension of an OpenFlux bundle (a ZIP of the configuration, keymap blob, icons and media).
pub const BUNDLE_EXTENSION: &str = "openflux";
pub const BUNDLE_EXTENSION_DOTTED: &str = ".openflux";
/// Largest single entry in a bundle: the keyboard's media limit.
pub const MAX_BUNDLE_ENTRY_BYTES: u64 = MAX_KEYBOARD_MEDIA_BYTES;
/// Largest total uncompressed size of a bundle.
pub const MAX_BUNDLE_UNCOMPRESSED_BYTES: u64 = 512 * 1024 * 1024;

/// `POST /v1/config/export` with `{"outPath"}`: writes an OpenFlux bundle.
pub async fn export_bundle(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let Some(out_str) = body
        .get("outPath")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
    else {
        return err_msg(StatusCode::BAD_REQUEST, "outPath is required").into_response();
    };
    let state_clone = state.clone();
    let res = tokio::task::spawn_blocking(move || -> Result<serde_json::Value, (StatusCode, String)> {
        let expanded = expand_home_path(&out_str);
        let out_path = if expanded.extension().and_then(|s| s.to_str()) == Some(BUNDLE_EXTENSION) {
            expanded
        } else {
            PathBuf::from(format!("{}{BUNDLE_EXTENSION_DOTTED}", expanded.to_string_lossy()))
        };
        if let Some(parent) = out_path.parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }

        let (appearance, settings, mosaic, keymap, actuation_modes, profiles, app_rules, icons, blob_opt) = {
            let g = state_clone.inner.lock().unwrap();
            (
                g.appearance.clone(),
                g.settings.clone(),
                g.mosaic.clone(),
                g.keymap.clone(),
                g.actuation_modes.clone(),
                g.profiles.clone(),
                g.app_rules.clone(),
                g.imported_icons.clone(),
                g.firmware_keymap_blob.clone(),
            )
        };

        let paired_blob = match blob_opt {
            Some(b) => Some(b),
            None => crate::codecs::keymap::encode_keymap_json_to_firmware_blob(
                &keymap,
                Some(&actuation_modes),
                None,
            )
            .ok(),
        };
        let export_actuation = if let Some(ref b) = paired_blob {
            crate::codecs::keymap::decode_actuation_modes_from_blob(b)
                .map(|m| crate::codecs::keymap::name_actuation_modes(m, &keymap))
                .unwrap_or_else(|_| actuation_modes.clone())
        } else {
            actuation_modes.clone()
        };

        // Every entry is checked against the import limits before out_path is created.

        let mut export_appearance = appearance.clone();
        let mut bundled_media: Vec<(String, Vec<u8>)> = Vec::new();
        let mut seen_media: BTreeMap<String, String> = BTreeMap::new();
        let mut used_bundle_names: BTreeSet<String> = BTreeSet::new();
        let mut media_err: Option<(StatusCode, String)> = None;
        let mut rewrite_theme_media = |theme_val: &mut serde_json::Value| {
            if media_err.is_some() {
                return;
            }
            let Some(bg_str) = theme_val
                .get("backgroundImagePath")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
            else {
                return;
            };
            let Some(mp) = resolve_media_local_path(&state_clone, &bg_str) else { return; };
            let Ok(meta) = mp.metadata() else { return; };
            if meta.len() > MAX_BUNDLE_ENTRY_BYTES {
                media_err = Some((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!("Media '{}' exceeds the {} MiB bundle entry limit", bg_str, mib(MAX_BUNDLE_ENTRY_BYTES)),
                ));
                return;
            }
            let src_key = mp.canonicalize().unwrap_or_else(|_| mp.clone()).to_string_lossy().to_string();
            if let Some(existing_name) = seen_media.get(&src_key) {
                if let Some(obj) = theme_val.as_object_mut() {
                    obj.insert("backgroundImagePath".into(), serde_json::Value::String(existing_name.clone()));
                }
                return;
            }
            let Ok(mbytes) = std::fs::read(&mp) else {
                media_err = Some((StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to read media '{}'", bg_str)));
                return;
            };
            let Some(orig_name) = mp.file_name().and_then(|s| s.to_str()).filter(|s| is_safe_filename(s)) else {
                return;
            };
            let bundle_name = if used_bundle_names.insert(orig_name.to_string()) {
                orig_name.to_string()
            } else {
                let mut h = Sha256::new();
                h.update(&mbytes);
                let tag = format!("{:x}", h.finalize());
                let disamb = format!("{}_{orig_name}", &tag[..8]);
                used_bundle_names.insert(disamb.clone());
                disamb
            };
            seen_media.insert(src_key, bundle_name.clone());
            bundled_media.push((bundle_name.clone(), mbytes));
            if let Some(obj) = theme_val.as_object_mut() {
                obj.insert("backgroundImagePath".into(), serde_json::Value::String(bundle_name));
            }
        };
        if let Some(dt) = export_appearance.get_mut("defaultTheme") {
            rewrite_theme_media(dt);
        }
        if let Some(ct) = export_appearance.get_mut("customThemes").and_then(|v| v.as_array_mut()) {
            for th in ct.iter_mut() {
                rewrite_theme_media(th);
            }
        }
        if let Some(err) = media_err {
            return Err(err);
        }

        let mut all_entries: Vec<(String, Vec<u8>)> = vec![
            ("appearance.json".into(), serde_json::to_vec_pretty(&export_appearance).unwrap_or_default()),
            ("settings.json".into(), serde_json::to_vec_pretty(&settings).unwrap_or_default()),
            ("mosaic.json".into(), serde_json::to_vec_pretty(&mosaic).unwrap_or_default()),
            ("keymap.json".into(), serde_json::to_vec_pretty(&keymap).unwrap_or_default()),
            ("actuation_modes.json".into(), serde_json::to_vec_pretty(&export_actuation).unwrap_or_default()),
            ("profiles.json".into(), serde_json::to_vec_pretty(&profiles).unwrap_or_default()),
            ("app_rules.json".into(), serde_json::to_vec_pretty(&app_rules).unwrap_or_default()),
        ];
        if let Some(b) = paired_blob {
            all_entries.push(("firmware_keymap.bin".into(), b));
        }

        let mut included_icons = BTreeSet::new();
        for id in icons {
            if is_safe_filename(&id) && included_icons.insert(id.clone()) {
                let p = state_clone.icon_store_dir.join(&id);
                if let Ok(bytes) = std::fs::read(&p) {
                    all_entries.push((format!("key_icons/{id}"), bytes));
                }
            }
        }
        for (fname, mbytes) in bundled_media {
            all_entries.push((format!("media/{fname}"), mbytes));
        }

        let total_uncompressed: u64 = all_entries.iter().map(|(_, d)| d.len() as u64).sum();
        if total_uncompressed > MAX_BUNDLE_UNCOMPRESSED_BYTES {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "Export bundle uncompressed size ({total_uncompressed} bytes) exceeds the {} MiB bundle limit",
                    mib(MAX_BUNDLE_UNCOMPRESSED_BYTES)
                ),
            ));
        }

        let file = std::fs::File::create(&out_path)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Create export failed: {e}")))?;
        let mut zw = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        for (name, data) in all_entries {
            zw.start_file(&name, opts)
                .and_then(|_| zw.write_all(&data).map_err(zip::result::ZipError::Io))
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("ZIP write failed: {e}")))?;
        }

        zw.finish()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Finalize bundle failed: {e}")))?;

        Ok(serde_json::json!({
            "message": format!("Exported OpenFlux bundle to {}", out_path.display()),
            "outPath": out_path.to_string_lossy(),
        }))
    })
    .await;

    match res {
        Ok(Ok(v)) => {
            crate::preview::release_process_heap_pages();
            (StatusCode::OK, Json(v)).into_response()
        }
        Ok(Err((code, msg))) => err_msg(code, &msg).into_response(),
        Err(e) => err_msg(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Export failed unexpectedly: {e}"),
        )
        .into_response(),
    }
}

/// Imports an OpenFlux bundle. Every entry is validated and every file staged before any state
/// changes; then files and state are committed together.
pub fn import_bundle_archive(
    state: &ApiState,
    path: &Path,
) -> Result<Vec<String>, (StatusCode, String)> {
    const MAX_UNCOMPRESSED: u64 = MAX_BUNDLE_UNCOMPRESSED_BYTES;
    const MAX_ENTRY_BYTES: u64 = MAX_BUNDLE_ENTRY_BYTES;
    let file = std::fs::File::open(path)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Open bundle failed: {e}")))?;
    let mut za = zip::ZipArchive::new(BufReader::new(file))
        .map_err(|_| (StatusCode::BAD_REQUEST, "Corrupt OpenFlux bundle".into()))?;
    let mut total_uncompressed: u64 = 0;

    let mut appearance_opt: Option<serde_json::Value> = None;
    let mut settings_opt: Option<serde_json::Value> = None;
    let mut mosaic_opt: Option<serde_json::Value> = None;
    let mut keymap_opt: Option<serde_json::Value> = None;
    let mut actuation_opt: Option<serde_json::Value> = None;
    let mut firmware_blob_opt: Option<Vec<u8>> = None;
    let mut profiles_opt: Option<Vec<serde_json::Value>> = None;
    let mut app_rules_opt: Option<Vec<crate::bridges::AppProfileRule>> = None;
    let mut extracted_icons: Vec<(String, Vec<u8>)> = Vec::new();
    let mut extracted_media: Vec<(String, Vec<u8>)> = Vec::new();
    let mut seen_icon_names: BTreeSet<String> = BTreeSet::new();
    let mut seen_media_names: BTreeSet<String> = BTreeSet::new();

    for i in 0..za.len() {
        let mut f = za
            .by_index(i)
            .map_err(|_| (StatusCode::BAD_REQUEST, "ZIP entry read error".into()))?;
        if f.is_dir() {
            continue;
        }
        let Some(enclosed) = f.enclosed_name().map(|p| p.to_path_buf()) else {
            return Err((
                StatusCode::BAD_REQUEST,
                "Unsafe path in OpenFlux bundle".into(),
            ));
        };
        if f.size() > MAX_ENTRY_BYTES {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "Bundle entry exceeds the {} MiB entry limit",
                    mib(MAX_ENTRY_BYTES)
                ),
            ));
        }
        let too_large = || {
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "Bundle exceeds the {} MiB uncompressed limit",
                    mib(MAX_UNCOMPRESSED)
                ),
            )
        };
        // The declared size refuses an oversized bundle early; the bytes actually read are what
        // count, since the declared size comes from the archive itself.
        if total_uncompressed.saturating_add(f.size()) > MAX_UNCOMPRESSED {
            return Err(too_large());
        }
        let mut buf = Vec::new();
        (&mut f)
            .take(MAX_ENTRY_BYTES + 1)
            .read_to_end(&mut buf)
            .map_err(|_| (StatusCode::BAD_REQUEST, "Failed reading ZIP entry".into()))?;
        if buf.len() as u64 > MAX_ENTRY_BYTES {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "Bundle entry exceeds the {} MiB entry limit",
                    mib(MAX_ENTRY_BYTES)
                ),
            ));
        }
        total_uncompressed = total_uncompressed.saturating_add(buf.len() as u64);
        if total_uncompressed > MAX_UNCOMPRESSED {
            return Err(too_large());
        }

        let name_str = enclosed.to_string_lossy();
        if name_str == "appearance.json" {
            let v: serde_json::Value = serde_json::from_slice(&buf).map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("Invalid appearance.json in bundle: {e}"),
                )
            })?;
            if !v.is_object() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "appearance.json must be a JSON object".into(),
                ));
            }
            appearance_opt = Some(v);
        } else if name_str == "settings.json" {
            let v: serde_json::Value = serde_json::from_slice(&buf).map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("Invalid settings.json in bundle: {e}"),
                )
            })?;
            if !v.is_object() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "settings.json must be a JSON object".into(),
                ));
            }
            settings_opt = Some(v);
        } else if name_str == "mosaic.json" {
            let v: serde_json::Value = serde_json::from_slice(&buf).map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("Invalid mosaic.json in bundle: {e}"),
                )
            })?;
            if !v.is_object() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "mosaic.json must be a JSON object".into(),
                ));
            }
            mosaic_opt = Some(v);
        } else if name_str == "keymap.json" {
            let v: serde_json::Value = serde_json::from_slice(&buf).map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("Invalid keymap.json in bundle: {e}"),
                )
            })?;
            if !v.is_object() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "keymap.json must be a JSON object".into(),
                ));
            }
            keymap_opt = Some(v);
        } else if name_str == "actuation_modes.json" {
            let v: serde_json::Value = serde_json::from_slice(&buf).map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("Invalid actuation_modes.json in bundle: {e}"),
                )
            })?;
            if !v.is_array() {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "actuation_modes.json must be a JSON array".into(),
                ));
            }
            actuation_opt = Some(v);
        } else if name_str == "firmware_keymap.bin" {
            firmware_blob_opt = Some(buf);
        } else if name_str == "profiles.json" {
            let v: Vec<serde_json::Value> = serde_json::from_slice(&buf).map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    format!("Invalid profiles.json in bundle: {e}"),
                )
            })?;
            profiles_opt = Some(v);
        } else if name_str == "app_rules.json" {
            let v: Vec<crate::bridges::AppProfileRule> =
                serde_json::from_slice(&buf).map_err(|e| {
                    (
                        StatusCode::BAD_REQUEST,
                        format!("Invalid app_rules.json in bundle: {e}"),
                    )
                })?;
            app_rules_opt = Some(v);
        } else if let Ok(rel_icon) = enclosed.strip_prefix("key_icons") {
            let comps: Vec<_> = rel_icon.components().collect();
            if comps.len() != 1 {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Nested paths under key_icons/ are not allowed".into(),
                ));
            }
            if let Some(icon_name) = rel_icon.file_name().and_then(|s| s.to_str()) {
                if !is_safe_filename(icon_name) {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "Invalid icon filename in bundle".into(),
                    ));
                }
                if !seen_icon_names.insert(icon_name.to_string()) {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "Duplicate key_icons/ entry in bundle".into(),
                    ));
                }
                extracted_icons.push((icon_name.to_string(), buf));
            }
        } else if let Ok(rel_media) = enclosed.strip_prefix("media") {
            let comps: Vec<_> = rel_media.components().collect();
            if comps.len() != 1 {
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Nested paths under media/ are not allowed".into(),
                ));
            }
            if let Some(media_name) = rel_media.file_name().and_then(|s| s.to_str()) {
                if !is_safe_filename(media_name) {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "Invalid media filename in bundle".into(),
                    ));
                }
                if !seen_media_names.insert(media_name.to_string()) {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "Duplicate media/ entry in bundle".into(),
                    ));
                }
                extracted_media.push((media_name.to_string(), buf));
            }
        }
    }

    // The keymap JSON and the firmware blob are validated as a pair before any state changes.
    let mut validated_blob: Option<Vec<u8>> = None;
    let mut derived_actuation: Option<serde_json::Value> = None;
    if let Some(ref raw_blob) = firmware_blob_opt {
        let Some(ref k) = keymap_opt else {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                "firmware_keymap.bin present without paired keymap.json".into(),
            ));
        };
        ApiState::validate_firmware_blob(raw_blob).map_err(|e| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("Invalid firmware_keymap.bin in bundle: {e}"),
            )
        })?;
        crate::codecs::keymap::verify_keymap_matches_blob(k, raw_blob).map_err(|e| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("Bundle keymap.json and firmware_keymap.bin don't match: {e}"),
            )
        })?;
        let blob_modes = crate::codecs::keymap::decode_actuation_modes_from_blob(raw_blob)
            .map_err(|e| {
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("Invalid actuation tables in the bundle's firmware_keymap.bin: {e}"),
                )
            })?;
        if let Some(ref am) = actuation_opt {
            let reenc = crate::codecs::keymap::encode_keymap_json_to_firmware_blob(
                k,
                Some(am),
                Some(raw_blob),
            )
            .map_err(|e| {
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!(
                        "Invalid actuation_modes.json for the bundle's firmware_keymap.bin: {e}"
                    ),
                )
            })?;
            if &reenc != raw_blob {
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "actuation_modes.json and firmware_keymap.bin in the bundle disagree".into(),
                ));
            }
        } else {
            derived_actuation = Some(crate::codecs::keymap::name_actuation_modes(blob_modes, k));
        }
        validated_blob = Some(raw_blob.clone());
    } else if let Some(ref k) = keymap_opt {
        let enc = crate::codecs::keymap::encode_keymap_json_to_firmware_blob(
            k,
            actuation_opt.as_ref(),
            None,
        )
        .map_err(|e| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("The bundle's keymap.json can't be encoded for the keyboard: {e}"),
            )
        })?;
        validated_blob = Some(enc);
    }

    // Profiles and app rules are validated before anything is written.
    let prevalidated_incoming_profiles: Option<(
        Vec<crate::api::state::ShortcutProfileState>,
        Vec<String>,
        Vec<String>,
    )> = if let Some(profs) = profiles_opt {
        let mut incoming = Vec::with_capacity(profs.len());
        let mut archive_ids: Vec<String> = Vec::with_capacity(profs.len());
        let mut seen_archive_ids = std::collections::HashSet::new();
        let mut names = Vec::new();
        for (i, pval) in profs.iter().enumerate() {
            // Rules refer to profiles by position, so every profile must be kept.
            let Some(name) = pval
                .get("name")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
            else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("Imported profile {i} has no name"),
                ));
            };
            let id = pval
                .get("id")
                .and_then(|x| x.as_str())
                .unwrap_or(name)
                .to_string();
            if !seen_archive_ids.insert(id.clone()) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("Imported profiles contain duplicate id '{id}'"),
                ));
            }
            let theme_id = pval
                .get("theme_id")
                .or_else(|| pval.get("themeId"))
                .and_then(|x| x.as_str())
                .unwrap_or("default")
                .to_string();
            let contexts: Vec<String> = pval
                .get("contexts")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .filter(|v: &Vec<String>| !v.is_empty())
                .unwrap_or_else(|| vec!["Main".into()]);
            let active_context = pval
                .get("active_context")
                .or_else(|| pval.get("activeContext"))
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty() && contexts.iter().any(|c| c == s))
                .map(|s| s.to_string())
                .unwrap_or_else(|| contexts[0].clone());
            let default_layer = pval
                .get("default_layer")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            if default_layer > 31 {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("Imported profile {name} default_layer {default_layer} exceeds 31"),
                ));
            }
            let am_raw = pval
                .get("actuation_mode_index")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let Ok(am_idx) = u32::try_from(am_raw) else {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("Imported profile {name} actuation_mode_index out of range"),
                ));
            };
            archive_ids.push(id.clone());
            names.push(name.to_string());
            incoming.push(ShortcutProfileState {
                id,
                name: name.to_string(),
                theme_id,
                contexts,
                active_context,
                actuation_mode_index: am_idx as usize,
                default_layer,
                shortcut_pack: pval
                    .get("shortcut_pack")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                app_matcher: pval
                    .get("app_matcher")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
            });
        }
        if incoming.len() > crate::codecs::keymap::MAX_PROFILES {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "Imported profiles exceed the keyboard's {} profile limit",
                    crate::codecs::keymap::MAX_PROFILES
                ),
            ));
        }
        if let Some(ref ar) = app_rules_opt {
            for r in ar {
                if archive_ids.get(r.profile_index as usize).is_none() {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        "Imported app_rules profile_index out of archive bounds".into(),
                    ));
                }
            }
        }
        Some((incoming, archive_ids, names))
    } else {
        None
    };

    // Icons and media are staged next to their destinations before any is renamed into place.
    static IMPORT_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let seq = IMPORT_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let asset_root = state
        .icon_store_dir
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| state.icon_store_dir.clone());
    let media_dir = asset_root.join("media_themes");

    let mut staged_ops: Vec<(PathBuf, PathBuf)> = Vec::new();
    let stage_dir_files = |dir: &Path,
                           files: &[(String, Vec<u8>)],
                           ops: &mut Vec<(PathBuf, PathBuf)>|
     -> Result<(), (StatusCode, String)> {
        if files.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(dir).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Asset dir {} error: {e}", dir.display()),
            )
        })?;
        for (name, bytes) in files {
            let tmp = dir.join(format!(".tmp_import_{}_{}_{name}", std::process::id(), seq));
            let mut f = match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
            {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        format!("Duplicate or aliased asset filename in the bundle: {name}"),
                    ));
                }
                Err(e) => {
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Asset stage {} failed: {e}", tmp.display()),
                    ));
                }
            };
            if let Err(e) = f.write_all(bytes) {
                drop(f);
                let _ = std::fs::remove_file(&tmp);
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Asset write {} failed: {e}", tmp.display()),
                ));
            }
            ops.push((tmp, dir.join(name)));
        }
        Ok(())
    };

    if let Err(e) = stage_dir_files(&state.icon_store_dir, &extracted_icons, &mut staged_ops)
        .and_then(|_| stage_dir_files(&media_dir, &extracted_media, &mut staged_ops))
    {
        for (tmp, _) in &staged_ops {
            let _ = std::fs::remove_file(tmp);
        }
        return Err(e);
    }

    // Lets tests pause between staging and commit to exercise concurrent edits.
    let imp_barrier = state.import_barrier.lock().unwrap().clone();
    if let Some((entered, release)) = imp_barrier {
        entered.notify_one();
        let (m, cv) = &*release;
        let mut bg = m.lock().unwrap();
        while !*bg {
            bg = cv.wait(bg).unwrap();
        }
    }

    // The state lock is held across the profile merge, the renames and the state commit, so a
    // concurrent profile edit during staging is merged rather than overwritten.
    let mut g = state.inner.lock().unwrap();
    let prepared_profile_merge: Option<ProfileMerge> =
        if let Some((incoming, archive_ids, names)) = prevalidated_incoming_profiles {
            let mut merged = g.profiles.clone();
            for np in incoming {
                // Profiles match by stable ID first, then by name.
                if let Some(pos) = merged.iter().position(|ep| ep.id == np.id) {
                    merged[pos] = np;
                } else if let Some(pos) = merged.iter().position(|ep| ep.name == np.name) {
                    merged[pos] = np;
                } else {
                    merged.push(np);
                }
            }
            let mut seen_ids = std::collections::HashSet::new();
            if !merged.iter().all(|p| seen_ids.insert(p.id.as_str())) {
                for (tmp, _) in &staged_ops {
                    let _ = std::fs::remove_file(tmp);
                }
                return Err((
                    StatusCode::BAD_REQUEST,
                    "Imported profiles create duplicate stable profile IDs".into(),
                ));
            }
            if merged.len() > crate::codecs::keymap::MAX_PROFILES {
                for (tmp, _) in &staged_ops {
                    let _ = std::fs::remove_file(tmp);
                }
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!(
                        "Imported profiles exceed the keyboard's {} profile limit",
                        crate::codecs::keymap::MAX_PROFILES
                    ),
                ));
            }
            let remapped_rules = if let Some(ar) = app_rules_opt.as_ref() {
                let mut out = Vec::with_capacity(ar.len());
                for r in ar {
                    let Some(aid) = archive_ids.get(r.profile_index as usize) else {
                        for (tmp, _) in &staged_ops {
                            let _ = std::fs::remove_file(tmp);
                        }
                        return Err((
                            StatusCode::BAD_REQUEST,
                            "Imported app_rules profile_index out of archive bounds".into(),
                        ));
                    };
                    let Some(merged_idx) = merged.iter().position(|p| p.id == *aid) else {
                        for (tmp, _) in &staged_ops {
                            let _ = std::fs::remove_file(tmp);
                        }
                        return Err((
                            StatusCode::BAD_REQUEST,
                            "Imported app_rules target profile lost in name collision".into(),
                        ));
                    };
                    let mut nr = r.clone();
                    nr.profile_index = merged_idx as u32;
                    out.push(nr);
                }
                Some(out)
            } else {
                None
            };
            Some((merged, remapped_rules, names))
        } else {
            None
        };

    // Commit staged files, keeping backups to roll back if any rename fails.
    let mut committed_rollbacks: Vec<(PathBuf, Option<PathBuf>)> =
        Vec::with_capacity(staged_ops.len());
    for (idx, (tmp, dst)) in staged_ops.iter().enumerate() {
        let bak_opt = if dst.exists() {
            let bak = dst.with_file_name(format!(
                ".bak_import_{}_{}_{}",
                std::process::id(),
                seq,
                dst.file_name().and_then(|s| s.to_str()).unwrap_or("asset")
            ));
            if let Err(e) = std::fs::rename(dst, &bak) {
                for (rem_tmp, _) in &staged_ops[idx..] {
                    let _ = std::fs::remove_file(rem_tmp);
                }
                for (c_dst, c_bak) in committed_rollbacks.into_iter().rev() {
                    let _ = std::fs::remove_file(&c_dst);
                    if let Some(b) = c_bak {
                        let _ = std::fs::rename(b, c_dst);
                    }
                }
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Asset backup {} failed: {e}", dst.display()),
                ));
            }
            Some(bak)
        } else {
            None
        };
        if let Err(e) = std::fs::rename(tmp, dst) {
            if let Some(ref b) = bak_opt {
                let _ = std::fs::rename(b, dst);
            }
            for (rem_tmp, _) in &staged_ops[idx..] {
                let _ = std::fs::remove_file(rem_tmp);
            }
            for (c_dst, c_bak) in committed_rollbacks.into_iter().rev() {
                let _ = std::fs::remove_file(&c_dst);
                if let Some(b) = c_bak {
                    let _ = std::fs::rename(b, c_dst);
                }
            }
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Asset commit {} failed: {e}", dst.display()),
            ));
        }
        committed_rollbacks.push((dst.clone(), bak_opt));
    }
    for (_, bak_opt) in committed_rollbacks {
        if let Some(b) = bak_opt {
            let _ = std::fs::remove_file(b);
        }
    }

    let mut imported_names = Vec::new();
    if let Some(app) = appearance_opt {
        if app.get("defaultTheme").is_some() {
            g.appearance = app;
            g.appearance_dirty = true;
            g.appearance_hydrated = true;
            g.appearance_rev = g.appearance_rev.wrapping_add(1);
        } else if let Some(ct) = g
            .appearance
            .get_mut("customThemes")
            .and_then(|v| v.as_array_mut())
        {
            ct.push(app);
            g.appearance_dirty = true;
            g.appearance_rev = g.appearance_rev.wrapping_add(1);
        }
    }
    if let Some(s) = settings_opt {
        g.settings = s;
        g.settings_dirty = true;
        g.settings_hydrated = true;
        g.settings_rev = g.settings_rev.wrapping_add(1);
    }
    if let Some(m) = mosaic_opt {
        g.mosaic = m;
        g.mosaic_dirty = true;
        g.mosaic_hydrated = true;
        g.mosaic_rev = g.mosaic_rev.wrapping_add(1);
    }
    if let Some(mut k) = keymap_opt {
        let has_macros_in_json = k
            .get("macros")
            .map(|v| {
                v.is_array()
                    || (v.is_object() && v.as_object().map(|o| !o.is_empty()).unwrap_or(false))
            })
            .unwrap_or(false);
        if !has_macros_in_json {
            if let Some(ref b) = validated_blob {
                if let Ok(decoded_m) = crate::codecs::keymap::decode_macros_from_blob(b) {
                    let non_empty = decoded_m.as_array().map(|a| !a.is_empty()).unwrap_or(false)
                        || decoded_m
                            .as_object()
                            .map(|o| {
                                o.values()
                                    .any(|v| v.as_array().map(|a| !a.is_empty()).unwrap_or(false))
                            })
                            .unwrap_or(false);
                    if non_empty {
                        if let Some(obj) = k.as_object_mut() {
                            obj.insert("macros".into(), decoded_m);
                        }
                    }
                }
            }
        }
        g.keymap = k;
        g.firmware_keymap_blob = validated_blob;
        g.keymap_dirty = true;
        g.keymap_hydrated = true;
        g.keymap_rev = g.keymap_rev.wrapping_add(1);
    }
    if let Some(am) = actuation_opt.or(derived_actuation) {
        g.actuation_modes = am;
    }
    let mut has_merged_profiles = false;
    let mut has_app_rules = false;
    if let Some((merged_profs, remapped_rules, names)) = prepared_profile_merge {
        has_merged_profiles = true;
        has_app_rules = remapped_rules.is_some();
        ApiState::replace_profiles_and_remap(&mut g, merged_profs, true, remapped_rules.is_none());
        if let Some(ar) = remapped_rules {
            g.app_rules = ar;
        }
        imported_names.extend(names);
    } else if let Some(ar) = app_rules_opt {
        has_app_rules = true;
        g.app_rules = ar;
    }
    for (ic, _) in extracted_icons {
        if !g.imported_icons.contains(&ic) {
            g.imported_icons.push(ic);
        }
    }
    for (mn, _) in extracted_media {
        if !g.imported_media.contains(&mn) {
            g.imported_media.push(mn);
        }
    }
    if imported_names.is_empty() {
        imported_names.push("OpenFlux bundle".into());
    }
    let had_profile_or_rule_update = has_merged_profiles || has_app_rules;
    drop(g);
    if had_profile_or_rule_update {
        state.persist_profiles().map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to persist imported profile store: {e}"),
            )
        })?;
    }
    Ok(imported_names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiline_quoted_csv_and_separate_mac_win_bindings() {
        let csv = concat!(
            "\"Include this shortcut?\",\"Application \",\"Application Context\",\"Shortcut\",\"Shortcut label\",\"Icon\",\"Mac String\",\"Win String\",\"Win-Ctrl\",\"Win-Shift\",\"Win-Alt \",\"Win-Winkey\",\"Win-US Key 1\",\"Win-US Key 2\",\"Win-US Key 1 constant\",\"Win-US Key 2 constant\",\"Mac-Cmd\",\"Mac-Ctrl\",\"Mac-Shift\",\"Mac-Opt\",\"Mac-Fn\",\"Mac-US Key 1\",\"Mac-US Key 2\",\"Mac-US Key 1 constant\",\"Mac-US Key 2 constant\"\n",
            "\"Yes\",\"Cubase\",\"Track\",\"Add Down:\nMove selection\",\"Add Down\",\"down.png\",\"Down\",\"Down\",\"\",\"\",\"\",\"\",\"Down Arrow\",\"\",\"VK_DOWN\",\"\",\"\",\"\",\"\",\"\",\"\",\"Down Arrow\",\"\",\"VK_DOWN\",\"\"\n",
            "\"Yes\",\"Excel\",\"File\",\"Save As\",\"Save As\",\"save.png\",\"Cmd+Shift+S\",\"F12\",\"\",\"\",\"\",\"\",\"F12\",\"\",\"VK_F12\",\"\",\"1\",\"\",\"1\",\"\",\"\",\"S\",\"\",\"S\",\"\"\n",
            "\"Yes\",\"Excel\",\"Edit\",\"Chord Play\",\"Play\",\"\",\"Cmd+K S\",\"Ctrl+Space\",\"1\",\"\",\"\",\"\",\"Spacebar\",\"\",\"VK_SPACE\",\"\",\"1\",\"\",\"\",\"\",\"\",\"K\",\"S\",\"K\",\"S\"\n",
            "\"Yes\",\"Visual Studio Code\",\"Search\",\"Delete File Results\",\"Delete File\",\"\",\"Cmd+Shift+Backspace\",\"Delete\",\"\",\"\",\"\",\"\",\"Delete\",\"\",\"VK_DELETE\",\"\",\"1\",\"\",\"1\",\"\",\"\",\"Delete\",\"\",\"Delete\",\"\"\n",
            "\"Yes\",\"Visual Studio Code\",\"Search\",\"Forward Delete\",\"Fwd Del\",\"\",\"Fn+Delete\",\"Delete\",\"\",\"\",\"\",\"\",\"Delete\",\"\",\"VK_DELETE\",\"\",\"\",\"\",\"\",\"\",\"1\",\"Delete\",\"\",\"Delete\",\"\"\n"
        );
        let parsed = parse_shortcut_csv_str("Test.csv", csv);
        let rows = parsed["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 5, "multiline quoted CSV must remain 5 rows");
        assert_eq!(rows[0]["label"], "Add Down");
        assert_eq!(rows[0]["macKeyCode"], "KC_DOWN");
        // Excel Save As: Windows F12 vs Mac Cmd+Shift+S
        assert_eq!(rows[1]["winKeyCode"], "KC_F12");
        assert_eq!(rows[1]["winModifiers"], 0);
        assert_eq!(rows[1]["macKeyCode"], "KC_S");
        // Cmd (8) | Shift (2)
        assert_eq!(rows[1]["macModifiers"], 0x0A);
        // Space, and a two-key chord
        assert_eq!(rows[2]["winKeyCode"], "KC_SPACE");
        assert_eq!(rows[2]["winModifiers"], 0x01);
        assert_eq!(rows[2]["macKeyCode"], "KC_K");
        assert_eq!(rows[2]["macKeyCode2"], "KC_S");
        assert_eq!(rows[2]["macHasSecondKey"], true);
        // Mac Delete without Fn -> KC_BACKSPACE; Win Delete -> KC_DELETE; Mac Fn+Delete -> KC_DELETE
        assert_eq!(rows[3]["macKeyCode"], "KC_BACKSPACE");
        assert_eq!(rows[3]["winKeyCode"], "KC_DELETE");
        assert_eq!(rows[4]["macKeyCode"], "KC_DELETE");

        // count_csv_rows counts the rows parse_shortcut_csv_str includes ("No" rows are skipped).
        let td = std::env::temp_dir().join(format!("openfluxd_csv_cnt_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&td);
        let p_nl = td.join("nl.csv");
        let p_nonl = td.join("nonl.csv");
        let hdr = "Include this shortcut?,Application ,Application Context,Shortcut,Shortcut label,Icon,Mac String,Win String\n";
        std::fs::write(&p_nl, format!("{hdr}Yes,App,Edit,Copy,Copy,,Cmd+C,Ctrl+C\nNo,App,Edit,SkipMe,Skip,,Cmd+X,Ctrl+X\nYes,App,Edit,Paste,Paste,,Cmd+V,Ctrl+V\n")).unwrap();
        std::fs::write(
            &p_nonl,
            format!("{hdr}Yes,App,Edit,Copy,Copy,,Cmd+C,Ctrl+C"),
        )
        .unwrap();
        assert_eq!(count_csv_rows(&p_nl), 2);
        assert_eq!(count_csv_rows(&p_nonl), 1);
        let _ = std::fs::remove_dir_all(&td);
    }

    #[test]
    fn parses_aerial_cards_safely_across_utf8_and_adjacent_boundaries() {
        let html = format!(
            r#"<div class="card"><img src="thumbnails/AAA-111.webp"/><h3>{}</h3><a href="https://sylvan.apple.com/a_SDR_2K_AVC.mov" aria-label="1080p SDR — {}">1080p</a></div><div class="card"><img src="thumbnails/BBB-222.webp"/><h3>Antarctica</h3><a href="https://sylvan.apple.com/b_SDR_2K_HEVC.mov">HEVC</a></div>"#,
            "Korea — Japan",
            "x".repeat(3500)
        );
        let cards = parse_aerial_cards(&html);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0]["id"], "AAA-111");
        assert_eq!(
            cards[0]["videoUrl"],
            "https://sylvan.apple.com/a_SDR_2K_AVC.mov"
        );
        assert_eq!(cards[1]["id"], "BBB-222");
        assert_eq!(
            cards[1]["videoUrl"],
            "https://sylvan.apple.com/b_SDR_2K_HEVC.mov"
        );
    }

    /// Synthesizes a video with FFmpeg's test sources.
    fn synth(ff: &str, out: &Path, lavfi: &str, extra: &[&str]) {
        let st = Command::new(ff)
            .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", lavfi])
            .args(extra)
            .arg(out)
            .status()
            .unwrap();
        assert!(st.success(), "ffmpeg failed to synthesize {lavfi}");
    }

    #[test]
    fn whole_videos_are_transcoded_forward_and_loop_reversed_and_never_shorter() {
        let Some(ff) = find_ffmpeg() else { return };
        let td = std::env::temp_dir().join(format!("openfluxd_vid_full_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&td);
        let odd_src = td.join("odd_src.mp4");
        synth(
            &ff,
            &odd_src,
            "testsrc=duration=1.0:size=162x92:rate=10",
            &[
                "-vf",
                "crop=161:91:0:0",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv444p",
            ],
        );
        let src_dur = probe_video_duration_secs(&ff, &odd_src)
            .expect("the synthesized video's duration is readable");
        let ffmpeg_only = probe_video_duration_with(&ff, None, &odd_src)
            .expect("FFmpeg alone reads the duration");
        assert!(
            (ffmpeg_only - src_dur).abs() < 0.1,
            "{ffmpeg_only} vs {src_dur}"
        );

        let norm =
            transcode_video_for_keyboard(&ff, &odd_src, &td.join("out_norm.mp4"), false).unwrap();
        assert!(norm.output_duration_s.unwrap() + 0.1 >= src_dur, "{norm:?}");
        assert_eq!(norm.resolution, "1080p");
        assert!(norm.bitrate_kbps > 0);

        // A long loop-reverse video is reversed piece by piece and keeps its whole length twice.
        let long_src = td.join("long_src.mp4");
        synth(
            &ff,
            &long_src,
            "testsrc=duration=26:size=64x64:rate=2",
            &[
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ],
        );
        let long_dur = probe_video_duration_secs(&ff, &long_src).unwrap();
        let rev =
            transcode_video_for_keyboard(&ff, &long_src, &td.join("out_rev.mp4"), true).unwrap();
        let rev_dur = rev.output_duration_s.unwrap();
        assert!(
            rev_dur + 0.1 >= 2.0 * long_dur,
            "forward and reverse of the whole source: {rev:?} from {long_dur}"
        );
        assert!(rev_dur < 2.0 * long_dur + 1.0, "{rev:?}");
        let full =
            transcode_video_for_keyboard(&ff, &long_src, &td.join("out_full.mp4"), false).unwrap();
        assert!(
            full.output_duration_s.unwrap() + 0.1 >= long_dur,
            "{full:?}"
        );
        let _ = std::fs::remove_dir_all(&td);
    }

    /// The frame numbers of a video whose frame n has luma 16 + 2n. The luma plane is read as
    /// stored, without any range conversion.
    fn frame_numbers(ff: &str, video: &Path, side: usize) -> Vec<usize> {
        let out = Command::new(ff)
            .args(["-loglevel", "error", "-i"])
            .arg(video)
            .args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
            .output()
            .unwrap();
        assert!(out.status.success());
        out.stdout
            .chunks(side * side * 3 / 2)
            .map(|f| {
                let luma = &f[..side * side];
                let mean = luma.iter().map(|b| *b as f64).sum::<f64>() / luma.len() as f64;
                ((mean - 16.0) / 2.0).round() as usize
            })
            .collect()
    }

    #[test]
    fn loop_reverse_plays_every_frame_forward_then_every_frame_backward_across_piece_joins() {
        let Some(ff) = find_ffmpeg() else { return };
        let td = std::env::temp_dir().join(format!("openfluxd_vid_frames_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&td);
        // 4.5 s at the loop-reverse frame rate (three reversal pieces): 108 frames, each with its
        // own luma, losslessly encoded.
        let frames = 108;
        let src = td.join("numbered.mp4");
        synth(
            &ff,
            &src,
            &format!("color=c=black:s=32x32:r={LOOP_REVERSE_FPS}:d=4.5"),
            &[
                "-vf",
                "geq=lum=16+N*2:cb=128:cr=128",
                "-c:v",
                "libx264",
                "-qp",
                "0",
                "-pix_fmt",
                "yuv420p",
            ],
        );
        let forward: Vec<usize> = (0..frames).collect();
        assert_eq!(
            frame_numbers(&ff, &src, 32),
            forward,
            "the source is numbered frame by frame"
        );

        let out = td.join("looped.mp4");
        let o = transcode_video_for_keyboard(&ff, &src, &out, true).unwrap();
        let expected: Vec<usize> = forward
            .iter()
            .copied()
            .chain(forward.iter().rev().copied())
            .collect();
        assert_eq!(
            frame_numbers(&ff, &out, 32),
            expected,
            "every frame forward, then every frame in exact reverse"
        );
        let src_dur = probe_video_duration_secs(&ff, &src).unwrap();
        let d = o.output_duration_s.unwrap();
        assert!(
            d + 0.05 >= 2.0 * src_dur && d <= 2.0 * src_dur + 1.0 / LOOP_REVERSE_FPS as f64 + 0.05,
            "{d} from {src_dur}"
        );
        let _ = std::fs::remove_dir_all(&td);
    }

    #[test]
    fn loop_reverse_works_in_a_directory_whose_path_has_an_apostrophe() {
        let Some(ff) = find_ffmpeg() else { return };
        let td = std::env::temp_dir().join(format!(
            "openfluxd_vid_quote_{}/Artist's media",
            std::process::id()
        ));
        let _ = std::fs::create_dir_all(&td);
        let src = td.join("src.mp4");
        synth(
            &ff,
            &src,
            "testsrc=duration=5:size=64x64:rate=4",
            &[
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ],
        );
        let src_dur = probe_video_duration_secs(&ff, &src).unwrap();
        let out = td.join("out.mp4");
        let rev = transcode_video_for_keyboard(&ff, &src, &out, true).unwrap();
        assert!(
            rev.output_duration_s.unwrap() + 0.1 >= 2.0 * src_dur,
            "{rev:?}"
        );
        assert!(out.is_file());
        let _ = std::fs::remove_dir_all(td.parent().unwrap());
    }

    #[test]
    fn a_video_that_cant_fit_is_refused_and_nothing_is_written() {
        let Some(ff) = find_ffmpeg() else { return };
        let td = std::env::temp_dir().join(format!("openfluxd_vid_refuse_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&td);
        let src = td.join("src.mp4");
        synth(
            &ff,
            &src,
            "testsrc=duration=6:size=320x240:rate=10",
            &[
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ],
        );
        // Too long for the limit even at the floor: refused before any encode.
        let tiny = MediaLimits {
            max_bytes: 20_000,
            target_bytes: 18_000.0,
        };
        for loop_reverse in [false, true] {
            let dst = td.join(format!("out_{loop_reverse}.mp4"));
            let (code, msg) =
                transcode_with_limits(&ff, &src, &dst, loop_reverse, &tiny).unwrap_err();
            assert_eq!(code, StatusCode::PAYLOAD_TOO_LARGE);
            assert!(msg.to_lowercase().contains("trim the video"), "{msg}");
            assert_eq!(msg.contains("turn off loop-reverse"), loop_reverse, "{msg}");
            assert!(!dst.exists(), "no shortened file is produced");
        }
        // The plan fits but every encode overshoots the limit: refused, not trimmed.
        let overshoot = MediaLimits {
            max_bytes: 1_000,
            target_bytes: 4_000_000.0,
        };
        let dst = td.join("out_overshoot.mp4");
        let (code, msg) = transcode_with_limits(&ff, &src, &dst, false, &overshoot).unwrap_err();
        assert_eq!(code, StatusCode::PAYLOAD_TOO_LARGE);
        assert!(msg.contains("960x540"), "{msg}");
        assert!(!dst.exists());
        let leftovers: Vec<_> = std::fs::read_dir(&td)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp_"))
            .collect();
        assert!(leftovers.is_empty(), "work files are removed");
        let _ = std::fs::remove_dir_all(&td);
    }

    #[test]
    fn plans_lower_bitrate_then_smaller_size_and_refuses_below_the_floor() {
        let k = MediaLimits::KEYBOARD;
        assert!(
            k.longest_fitting_secs() > 1800.0,
            "the keyboard holds more than half an hour at the floor"
        );
        let short = plan_encodes(Some(30.0), &k).unwrap();
        assert_eq!(short[0].0.label, "1080p");
        assert_eq!(
            short[0].1, MAX_VIDEO_KBPS as u32,
            "a short video gets the best bitrate"
        );
        assert_eq!(short.last().unwrap().0.label, "540p");
        let long = plan_encodes(Some(600.0), &k).unwrap();
        assert_eq!(
            long[0].0.label, "720p",
            "a longer video starts at a smaller size"
        );
        assert!(plan_encodes(Some(k.longest_fitting_secs() + 60.0), &k)
            .unwrap_err()
            .contains("plays for"));
        assert_eq!(
            plan_encodes(None, &k).unwrap(),
            vec![(RUNGS[2], 400)],
            "an unknown length is tried only at the floor"
        );
    }

    #[test]
    fn ffmpeg_reports_the_container_duration() {
        let text = "Input #0, mov,mp4, from 'a.mp4':\n  Duration: 00:01:02.50, start: 0.000000, bitrate: 12 kb/s\n";
        assert_eq!(parse_ffmpeg_duration(text), Some(62.5));
        assert_eq!(parse_ffmpeg_duration("  Duration: N/A, bitrate: N/A"), None);
        assert_eq!(parse_ffmpeg_duration("no input"), None);
    }

    #[test]
    fn durations_come_from_the_bundled_tools_or_are_unknown() {
        let td = std::env::temp_dir().join(format!("openfluxd_probe_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&td);
        let clip = td.join("clip.mp4");
        std::fs::write(&clip, b"not a video").unwrap();
        // No FFprobe and an FFmpeg that can't run: the length is unknown, never assumed.
        assert_eq!(
            probe_video_duration_with("/nonexistent/openflux-ffmpeg", None, &clip),
            None
        );
        // An FFprobe that fails falls back to FFmpeg, which can't run either.
        assert_eq!(
            probe_video_duration_with(
                "/nonexistent/openflux-ffmpeg",
                Some("/nonexistent/openflux-ffprobe"),
                &clip
            ),
            None
        );
        let _ = std::fs::remove_dir_all(&td);
    }

    #[test]
    fn aerial_cache_entries_are_reused_only_under_this_cache_format() {
        let url = "https://sylvan.apple.com/a_SDR_2K_AVC.mov";
        let stem = aerial_cache_stem(url, false);
        assert_ne!(stem, aerial_cache_stem(url, true));
        assert_eq!(stem, aerial_cache_stem(url, false));
        // An entry is reused only when it records this cache format.
        let td =
            std::env::temp_dir().join(format!("openfluxd_aerial_cache_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&td);
        let dst = td.join(format!("{stem}.mp4"));
        let meta = td.join(format!("{stem}.json"));
        std::fs::write(&dst, b"clip").unwrap();
        std::fs::write(&meta, "{}").unwrap();
        assert!(cached_aerial(&dst, &meta).is_none());
        std::fs::write(
            &meta,
            format!(r#"{{"cacheFormat": "{AERIAL_CACHE_FORMAT}", "durationS": 120.0}}"#),
        )
        .unwrap();
        assert!(cached_aerial(&dst, &meta).is_some());
        let _ = std::fs::remove_dir_all(&td);
    }

    #[tokio::test]
    async fn a_cached_local_import_made_under_other_rules_isnt_reused() {
        let Some(ff) = find_ffmpeg() else { return };
        let td = std::env::temp_dir().join(format!("openfluxd_local_cache_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&td);
        let icons = td.join("assets/key_icons");
        std::fs::create_dir_all(&icons).unwrap();
        let state = ApiState::with_dirs(
            Arc::new(crate::api::state::NoopDeviceSink),
            None,
            Duration::from_secs(20),
            icons,
        );
        let src = td.join("clip.mov");
        synth(
            &ff,
            &src,
            "testsrc=duration=4:size=64x64:rate=4",
            &[
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ],
        );
        let src_dur = probe_video_duration_secs(&ff, &src).unwrap();
        // A shorter file whose name lacks this transcode format isn't taken for this import.
        let (raw_stem, _) = stream_sha256_prefix(&src, u64::MAX).unwrap();
        let media = td.join("assets/media_themes");
        std::fs::create_dir_all(&media).unwrap();
        let other = media.join(format!("{raw_stem}_loopreverse.mp4"));
        synth(
            &ff,
            &other,
            "testsrc=duration=1:size=64x64:rate=4",
            &[
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
            ],
        );

        let resp = import_media_file(
            State(state),
            Json(serde_json::json!({"file": src.to_string_lossy(), "loopReverse": true})),
        )
        .await
        .into_response();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        let asset = body["assetPath"].as_str().unwrap();
        assert_ne!(
            asset,
            other.file_name().unwrap().to_str().unwrap(),
            "the other file isn't reused"
        );
        let got = probe_video_duration_secs(&ff, &media.join(asset)).unwrap();
        assert!(
            got + 0.3 >= 2.0 * src_dur,
            "the whole source forward and reversed: {got} from {src_dur}"
        );
        assert_eq!(
            body["durationS"].as_f64().map(|d| (d - got).abs() < 0.01),
            Some(true),
            "{body}"
        );
        assert!(
            body["bitrateKbps"].as_u64().is_some_and(|b| b > 0),
            "{body}"
        );
        let _ = std::fs::remove_dir_all(&td);
    }
}
