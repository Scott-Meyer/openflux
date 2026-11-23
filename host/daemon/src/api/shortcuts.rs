//! Shortcut packs: per-application libraries of keyboard shortcuts, which Studio browses and
//! binds to keys and the display shows on the keys while their modifiers are held.
//!
//! Packs come from three places, looked up in this order by id:
//!
//! - files in a `shortcuts` directory of an asset root, either OpenFlux pack documents (`.json`)
//!   or spreadsheet exports (`.csv`, see [`crate::api::assets::parse_shortcut_csv_str`]);
//! - the packs built into the daemon (`assets/shortcuts/*.json`), so a fresh install has a
//!   library;
//! - packs the user made in Studio.
//!
//! The user's own shortcuts ([`CustomShortcuts`]) are kept apart from the packs, in
//! `custom_shortcuts.json` next to the key icon store: shortcuts added to any pack, packs of their
//! own, and the pack shortcuts they hid. Editing a pack's shortcut makes a custom shortcut that
//! `replaces` it, so deleting the custom one brings the original back.
//!
//! # Pack documents
//!
//! ```json
//! {
//!   "name": "Visual Studio Code",
//!   "source": "https://code.visualstudio.com/docs/reference/default-keybindings",
//!   "platforms": ["mac", "windows", "linux"],
//!   "apps": {"macos": ["com.microsoft.VSCode"], "windows": ["Code.exe"], "linux": ["code"]},
//!   "shortcuts": [
//!     {"name": "Show Command Palette", "label": "Palette", "context": "General",
//!      "mac": "Cmd+Shift+P", "windows": "Ctrl+Shift+P"}
//!   ]
//! }
//! ```
//!
//! `mac`, `windows` and `linux` are key combinations ([`parse_combo`]); a shortcut without a
//! `linux` one uses its `windows` one when the pack lists `linux` among its platforms. `label` is
//! the short legend for a key (the name when absent) and `icon` a key icon file name. `apps` are
//! the app matchers (bundle id, executable or process name) a profile made from the pack
//! switches on. `source` names the documentation the shortcuts come from.

use crate::api::state::ApiState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

/// The packs built into the daemon, by id (the file name under `assets/shortcuts`).
pub const BUILT_IN_PACKS: [(&str, &str); 21] = [
    (
        "blender.json",
        include_str!("../../../../assets/shortcuts/blender.json"),
    ),
    (
        "chrome.json",
        include_str!("../../../../assets/shortcuts/chrome.json"),
    ),
    (
        "discord.json",
        include_str!("../../../../assets/shortcuts/discord.json"),
    ),
    (
        "excel.json",
        include_str!("../../../../assets/shortcuts/excel.json"),
    ),
    (
        "figma.json",
        include_str!("../../../../assets/shortcuts/figma.json"),
    ),
    (
        "file-explorer.json",
        include_str!("../../../../assets/shortcuts/file-explorer.json"),
    ),
    (
        "finder.json",
        include_str!("../../../../assets/shortcuts/finder.json"),
    ),
    (
        "firefox.json",
        include_str!("../../../../assets/shortcuts/firefox.json"),
    ),
    (
        "macos.json",
        include_str!("../../../../assets/shortcuts/macos.json"),
    ),
    (
        "macos-terminal.json",
        include_str!("../../../../assets/shortcuts/macos-terminal.json"),
    ),
    (
        "outlook.json",
        include_str!("../../../../assets/shortcuts/outlook.json"),
    ),
    (
        "photoshop.json",
        include_str!("../../../../assets/shortcuts/photoshop.json"),
    ),
    (
        "premiere-pro.json",
        include_str!("../../../../assets/shortcuts/premiere-pro.json"),
    ),
    (
        "safari.json",
        include_str!("../../../../assets/shortcuts/safari.json"),
    ),
    (
        "slack.json",
        include_str!("../../../../assets/shortcuts/slack.json"),
    ),
    (
        "vscode.json",
        include_str!("../../../../assets/shortcuts/vscode.json"),
    ),
    (
        "windows.json",
        include_str!("../../../../assets/shortcuts/windows.json"),
    ),
    (
        "windows-terminal.json",
        include_str!("../../../../assets/shortcuts/windows-terminal.json"),
    ),
    (
        "word.json",
        include_str!("../../../../assets/shortcuts/word.json"),
    ),
    (
        "zoom.json",
        include_str!("../../../../assets/shortcuts/zoom.json"),
    ),
    (
        "gnome-terminal.json",
        include_str!("../../../../assets/shortcuts/gnome-terminal.json"),
    ),
];

/// Modifier bits of a combination, as rows carry them: Ctrl 1, Shift 2, Alt/Option 4,
/// Cmd/Win/Super 8.
pub const MOD_CTRL: u32 = 1;
pub const MOD_SHIFT: u32 = 2;
pub const MOD_ALT: u32 = 4;
pub const MOD_GUI: u32 = 8;

/// The file of the user's own shortcuts, beside the key icon store.
pub const CUSTOM_FILE: &str = "custom_shortcuts.json";

/// One key press of a combination: the modifiers held and the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stroke {
    pub mods: u32,
    /// The key's keymap name, such as `KC_P`.
    pub key: String,
}

fn modifier_bit(token: &str) -> Option<u32> {
    match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "⌃" => Some(MOD_CTRL),
        "shift" | "⇧" => Some(MOD_SHIFT),
        "alt" | "option" | "opt" | "⌥" => Some(MOD_ALT),
        "cmd" | "command" | "⌘" | "win" | "super" | "meta" => Some(MOD_GUI),
        _ => None,
    }
}

fn key_name(token: &str) -> Option<String> {
    let t = token.trim();
    let mut chars = t.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        let named = match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' => {
                return Some(format!("KC_{}", c.to_ascii_uppercase()))
            }
            '-' => "KC_MINUS",
            '=' => "KC_EQUAL",
            ',' => "KC_COMMA",
            '.' => "KC_DOT",
            '/' => "KC_SLASH",
            '\\' => "KC_BACKSLASH",
            ';' => "KC_SEMICOLON",
            '\'' => "KC_QUOTE",
            '[' => "KC_LEFT_BRACKET",
            ']' => "KC_RIGHT_BRACKET",
            '`' => "KC_GRAVE",
            '↑' => "KC_UP",
            '↓' => "KC_DOWN",
            '←' => "KC_LEFT",
            '→' => "KC_RIGHT",
            _ => return None,
        };
        return Some(named.into());
    }
    let lower = t.to_ascii_lowercase();
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=24).contains(&n).then(|| format!("KC_F{n}"));
    }
    if let Some(n) = lower
        .strip_prefix("numpad")
        .and_then(|n| n.parse::<u32>().ok())
    {
        return (n <= 9).then(|| format!("KC_KP_{n}"));
    }
    let named = match lower.as_str() {
        "enter" | "return" => "KC_ENTER",
        "esc" | "escape" => "KC_ESCAPE",
        "tab" => "KC_TAB",
        "space" | "spacebar" => "KC_SPACE",
        "backspace" => "KC_BACKSPACE",
        "delete" | "del" => "KC_DELETE",
        "insert" | "ins" => "KC_INSERT",
        "home" => "KC_HOME",
        "end" => "KC_END",
        "pageup" | "pgup" => "KC_PAGE_UP",
        "pagedown" | "pgdn" => "KC_PAGE_DOWN",
        "up" => "KC_UP",
        "down" => "KC_DOWN",
        "left" => "KC_LEFT",
        "right" => "KC_RIGHT",
        "printscreen" | "prtsc" => "KC_PRINT_SCREEN",
        "pause" => "KC_PAUSE",
        "capslock" => "KC_CAPS_LOCK",
        "numpadplus" => "KC_KP_PLUS",
        "numpadminus" => "KC_KP_MINUS",
        "numpadmultiply" => "KC_KP_ASTERISK",
        "numpaddivide" => "KC_KP_SLASH",
        "numpadperiod" | "numpaddot" => "KC_KP_DOT",
        "numpadenter" => "KC_KP_ENTER",
        _ => {
            let upper = t.to_ascii_uppercase();
            return (upper.starts_with("KC_")
                && crate::codecs::keycodes::keyboard_usage(&upper).is_some())
            .then(|| crate::codecs::keycodes::canonical_name(&upper).to_string());
        }
    };
    Some(named.into())
}

/// Reads a key combination: modifiers and one key joined by `+` (`Ctrl+Shift+P`, `Cmd+=`), and
/// for a sequence, the presses one after another separated by spaces (`Ctrl+K Ctrl+S`).
/// Modifiers are Ctrl (Control), Shift, Alt (Option, Opt) and Cmd (Command, Win, Super, Meta).
/// Keys are letters, digits and punctuation, F1 to F24, Enter, Esc, Tab, Space, Backspace,
/// Delete, Insert, Home, End, PageUp, PageDown, Up, Down, Left, Right, PrintScreen, Pause,
/// Numpad0 to Numpad9, NumpadPlus, NumpadMinus, NumpadMultiply, NumpadDivide, NumpadPeriod,
/// NumpadEnter, or a keymap name such as `KC_F13`.
pub fn parse_combo(text: &str) -> Result<Vec<Stroke>, String> {
    let mut strokes = Vec::new();
    for part in text.split_whitespace() {
        let tokens: Vec<&str> = if part == "+" {
            vec!["+"]
        } else {
            part.split('+').collect()
        };
        let (key_tok, mod_toks) = tokens.split_last().ok_or("an empty key combination")?;
        let mut mods = 0;
        for m in mod_toks {
            mods |= modifier_bit(m).ok_or_else(|| format!("“{m}” isn't a modifier"))?;
        }
        if key_tok.is_empty() {
            return Err(format!("“{part}” has no key after its modifiers"));
        }
        if modifier_bit(key_tok).is_some() {
            return Err(format!("“{part}” has no key besides its modifiers"));
        }
        let key =
            key_name(key_tok).ok_or_else(|| format!("“{key_tok}” isn't a key OpenFlux knows"))?;
        strokes.push(Stroke { mods, key });
    }
    if strokes.is_empty() {
        return Err("an empty key combination".into());
    }
    if strokes.len() > 2 {
        return Err("a sequence can have at most two presses".into());
    }
    Ok(strokes)
}

/// A shortcut as packs and the custom store hold it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub icon: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linux: Option<String>,
}

/// The app matchers a profile made from a pack switches on, per host platform.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PackApps {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub macos: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub windows: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub linux: Vec<String>,
}

impl PackApps {
    fn is_empty(&self) -> bool {
        self.macos.is_empty() && self.windows.is_empty() && self.linux.is_empty()
    }
}

/// A pack document (see the module documentation).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackDocument {
    pub name: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub platforms: Vec<String>,
    #[serde(default)]
    pub apps: PackApps,
    #[serde(default)]
    pub shortcuts: Vec<ShortcutEntry>,
}

/// A pack the user made in Studio.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CustomPack {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub apps: PackApps,
}

/// A shortcut the user added to a pack.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CustomShortcut {
    pub id: String,
    /// The id of the pack it belongs to.
    pub pack: String,
    #[serde(flatten)]
    pub shortcut: ShortcutEntry,
    /// The id of the pack shortcut it stands in for, which is hidden while this one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
}

/// The user's own shortcuts, packs and hidden pack shortcuts (`custom_shortcuts.json`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CustomShortcuts {
    #[serde(default)]
    pub packs: Vec<CustomPack>,
    #[serde(default)]
    pub shortcuts: Vec<CustomShortcut>,
    /// Pack shortcut ids the user hid, by pack id.
    #[serde(default)]
    pub hidden: BTreeMap<String, Vec<String>>,
}

/// Where a pack came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackOrigin {
    /// A file in a `shortcuts` asset directory.
    File,
    BuiltIn,
    /// Made in Studio.
    Custom,
}

impl PackOrigin {
    fn as_str(self) -> &'static str {
        match self {
            PackOrigin::File => "file",
            PackOrigin::BuiltIn => "builtIn",
            PackOrigin::Custom => "custom",
        }
    }
}

/// A pack with its rows as the API gives them.
#[derive(Debug, Clone)]
pub struct LoadedPack {
    pub id: String,
    pub name: String,
    pub source: String,
    pub origin: PackOrigin,
    pub apps: PackApps,
    /// Shortcuts without a Linux combination use their Windows one.
    pub linux_follows_windows: bool,
    /// Every row, the hidden ones flagged with `hidden: true`.
    pub rows: Vec<Value>,
}

impl LoadedPack {
    pub fn to_json(&self) -> Value {
        json!({
            "pack": self.id,
            "application": self.name,
            "source": self.source,
            "origin": self.origin.as_str(),
            "apps": self.apps,
            "rows": self.rows,
        })
    }

    /// The rows that aren't hidden.
    pub fn visible_rows(&self) -> Vec<Value> {
        self.rows
            .iter()
            .filter(|r| r.get("hidden").and_then(|v| v.as_bool()) != Some(true))
            .cloned()
            .collect()
    }

    fn summary(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "rowCount": self.visible_rows().len(),
            "origin": self.origin.as_str(),
            "source": self.source,
            "apps": self.apps,
        })
    }
}

/// The row fields of one platform's combination (`mac`, `win` or `linux` prefixed): the text,
/// the first key, the second key of a sequence and the first press's modifiers.
fn platform_fields(row: &mut serde_json::Map<String, Value>, prefix: &str, combo: Option<&str>) {
    let strokes = combo.and_then(|c| parse_combo(c).ok());
    let first = strokes.as_ref().and_then(|s| s.first());
    let second = strokes.as_ref().and_then(|s| s.get(1));
    row.insert(format!("{prefix}String"), json!(combo.unwrap_or_default()));
    row.insert(
        format!("{prefix}KeyCode"),
        json!(first.map(|s| s.key.clone())),
    );
    row.insert(
        format!("{prefix}KeyCode2"),
        json!(second.map(|s| s.key.clone())),
    );
    row.insert(format!("{prefix}HasSecondKey"), json!(second.is_some()));
    row.insert(
        format!("{prefix}Modifiers"),
        json!(first.map(|s| s.mods).unwrap_or(0)),
    );
}

/// A shortcut as an API row. `linux_follows_windows` gives a shortcut without a Linux
/// combination its Windows one.
fn entry_row(id: &str, e: &ShortcutEntry, linux_follows_windows: bool, custom: bool) -> Value {
    let mut row = serde_json::Map::new();
    row.insert("id".into(), json!(id));
    row.insert("context".into(), json!(e.context));
    row.insert("shortcut".into(), json!(e.name));
    let label = e
        .label
        .clone()
        .filter(|l| !l.trim().is_empty())
        .unwrap_or_else(|| e.name.clone());
    row.insert("label".into(), json!(label));
    row.insert("icon".into(), json!(e.icon));
    platform_fields(&mut row, "mac", e.mac.as_deref());
    platform_fields(&mut row, "win", e.windows.as_deref());
    let linux = e.linux.as_deref().or(if linux_follows_windows {
        e.windows.as_deref()
    } else {
        None
    });
    platform_fields(&mut row, "linux", linux);
    row.insert("custom".into(), json!(custom));
    Value::Object(row)
}

fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

/// Ids for a pack's shortcuts: their own `id`, else context and name, made unique in the pack.
fn entry_ids(entries: &[ShortcutEntry]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    entries
        .iter()
        .map(|e| {
            let base =
                e.id.clone()
                    .unwrap_or_else(|| slug(&format!("{} {}", e.context, e.name)));
            let mut id = base.clone();
            let mut n = 2;
            while !seen.insert(id.clone()) {
                id = format!("{base}-{n}");
                n += 1;
            }
            id
        })
        .collect()
}

fn pack_from_document(id: &str, doc: &PackDocument, origin: PackOrigin) -> LoadedPack {
    let linux = doc.platforms.iter().any(|p| p == "linux");
    let rows = entry_ids(&doc.shortcuts)
        .iter()
        .zip(&doc.shortcuts)
        .map(|(rid, e)| entry_row(rid, e, linux, false))
        .collect();
    LoadedPack {
        id: id.into(),
        name: doc.name.clone(),
        source: doc.source.clone(),
        origin,
        apps: doc.apps.clone(),
        linux_follows_windows: linux,
        rows,
    }
}

/// A spreadsheet pack's rows, with ids, and the Windows combination standing in for Linux.
fn pack_from_csv(id: &str, txt: &str) -> LoadedPack {
    let parsed = crate::api::assets::parse_shortcut_csv_str(id, txt);
    let mut seen = BTreeSet::new();
    let rows = parsed["rows"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut r| {
            let base = slug(&format!(
                "{} {}",
                r["context"].as_str().unwrap_or_default(),
                r["shortcut"].as_str().unwrap_or_default()
            ));
            let mut rid = base.clone();
            let mut n = 2;
            while !seen.insert(rid.clone()) {
                rid = format!("{base}-{n}");
                n += 1;
            }
            r["id"] = json!(rid);
            for f in ["String", "KeyCode", "KeyCode2", "HasSecondKey", "Modifiers"] {
                r[format!("linux{f}")] = r[format!("win{f}")].clone();
            }
            r["custom"] = json!(false);
            r
        })
        .collect();
    LoadedPack {
        id: id.into(),
        name: parsed["application"].as_str().unwrap_or(id).to_string(),
        source: String::new(),
        origin: PackOrigin::File,
        apps: PackApps::default(),
        linux_follows_windows: true,
        rows,
    }
}

fn built_in_pack(id: &str) -> Option<LoadedPack> {
    let (_, txt) = BUILT_IN_PACKS.iter().find(|(pid, _)| *pid == id)?;
    let doc: PackDocument = serde_json::from_str(txt).ok()?;
    Some(pack_from_document(id, &doc, PackOrigin::BuiltIn))
}

fn file_pack(roots: &[PathBuf], id: &str) -> Option<LoadedPack> {
    let lower = id.to_ascii_lowercase();
    if !(lower.ends_with(".json") || lower.ends_with(".csv")) || !is_plain_name(id) {
        return None;
    }
    let path = roots
        .iter()
        .map(|r| r.join("shortcuts").join(id))
        .find(|p| p.is_file())?;
    let txt = std::fs::read_to_string(path).ok()?;
    if lower.ends_with(".csv") {
        return Some(pack_from_csv(id, &txt));
    }
    let doc: PackDocument = serde_json::from_str(&txt).ok()?;
    Some(pack_from_document(id, &doc, PackOrigin::File))
}

fn is_plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains("..")
        && !name.contains(['/', '\\', '\0'])
        && name != CUSTOM_FILE
}

/// The pack `id` before the user's shortcuts are applied: a file, else built in, else one the
/// user made.
fn base_pack(roots: &[PathBuf], custom: &CustomShortcuts, id: &str) -> Option<LoadedPack> {
    file_pack(roots, id)
        .or_else(|| built_in_pack(id))
        .or_else(|| {
            let p = custom.packs.iter().find(|p| p.id == id)?;
            Some(LoadedPack {
                id: p.id.clone(),
                name: p.name.clone(),
                source: String::new(),
                origin: PackOrigin::Custom,
                apps: p.apps.clone(),
                linux_follows_windows: true,
                rows: Vec::new(),
            })
        })
}

/// Applies the user's shortcuts to `pack`: hidden and replaced rows are flagged `hidden`, and the
/// custom shortcuts of the pack follow its own, with `custom: true`.
fn apply_custom(mut pack: LoadedPack, custom: &CustomShortcuts) -> LoadedPack {
    let mut hidden: BTreeSet<&str> = custom
        .hidden
        .get(&pack.id)
        .map(|ids| ids.iter().map(String::as_str).collect())
        .unwrap_or_default();
    let mine: Vec<&CustomShortcut> = custom
        .shortcuts
        .iter()
        .filter(|s| s.pack == pack.id)
        .collect();
    hidden.extend(mine.iter().filter_map(|s| s.replaces.as_deref()));
    for row in &mut pack.rows {
        if row["id"].as_str().is_some_and(|id| hidden.contains(id)) {
            row["hidden"] = json!(true);
        }
    }
    for s in mine {
        let mut row = entry_row(&s.id, &s.shortcut, pack.linux_follows_windows, true);
        if let Some(r) = &s.replaces {
            row["replaces"] = json!(r);
        }
        pack.rows.push(row);
    }
    pack
}

/// The pack `id` with the user's shortcuts applied, or none when no pack has that id.
pub fn load_pack(roots: &[PathBuf], custom: &CustomShortcuts, id: &str) -> Option<LoadedPack> {
    base_pack(roots, custom, id).map(|p| apply_custom(p, custom))
}

/// Every pack, sorted by name, with the user's shortcuts applied.
pub fn all_packs(roots: &[PathBuf], custom: &CustomShortcuts) -> Vec<LoadedPack> {
    let mut ids: BTreeSet<String> = BUILT_IN_PACKS
        .iter()
        .map(|(id, _)| id.to_string())
        .collect();
    for root in roots {
        if let Ok(entries) = std::fs::read_dir(root.join("shortcuts")) {
            for ent in entries.flatten() {
                let name = ent.file_name().to_string_lossy().to_string();
                let lower = name.to_ascii_lowercase();
                if lower.ends_with(".csv") || lower.ends_with(".json") {
                    ids.insert(name);
                }
            }
        }
    }
    ids.extend(custom.packs.iter().map(|p| p.id.clone()));
    let mut packs: Vec<LoadedPack> = ids
        .iter()
        .filter_map(|id| load_pack(roots, custom, id))
        .collect();
    packs.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    packs
}

/// The catalog entries of every pack: id, name, the count of rows that aren't hidden, origin,
/// source and apps.
pub fn pack_summaries(roots: &[PathBuf], custom: &CustomShortcuts) -> Vec<Value> {
    all_packs(roots, custom)
        .iter()
        .map(LoadedPack::summary)
        .collect()
}

/// Profile templates: one per pack that names the apps it is for.
pub fn profile_templates(roots: &[PathBuf], custom: &CustomShortcuts) -> Vec<Value> {
    all_packs(roots, custom)
        .iter()
        .filter(|p| !p.apps.is_empty())
        .map(|p| {
            json!({
                "id": format!("template-{}", p.id),
                "name": p.name,
                "shortcutPack": p.id,
                "apps": p.apps,
                "shortcutCount": p.visible_rows().len(),
            })
        })
        .collect()
}

/// Where the user's shortcuts are kept.
pub fn custom_path(state: &ApiState) -> PathBuf {
    state
        .icon_store_dir
        .parent()
        .unwrap_or(&state.icon_store_dir)
        .join(CUSTOM_FILE)
}

/// The user's shortcuts; none when the file is missing or unreadable.
pub fn read_custom(state: &ApiState) -> CustomShortcuts {
    std::fs::read(custom_path(state))
        .ok()
        .and_then(|raw| serde_json::from_slice(&raw).ok())
        .unwrap_or_default()
}

/// What is wrong with a custom shortcut store, naming the shortcut or pack and the field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreProblem {
    pub message: String,
    /// The custom shortcut or pack the problem is in.
    pub id: Option<String>,
    pub field: Option<String>,
}

fn problem(message: String, id: Option<&str>, field: Option<&str>) -> StoreProblem {
    StoreProblem {
        message,
        id: id.map(str::to_owned),
        field: field.map(str::to_owned),
    }
}

/// Checks a store before it is kept: unique ids, names, packs that exist and combinations that
/// parse, with at least one per shortcut.
pub fn validate_custom(roots: &[PathBuf], store: &CustomShortcuts) -> Result<(), StoreProblem> {
    let mut pack_ids = BTreeSet::new();
    for p in &store.packs {
        if p.id.trim().is_empty() || !is_plain_name(&p.id) {
            return Err(problem(
                format!("The pack id “{}” can't be used", p.id),
                Some(&p.id),
                Some("id"),
            ));
        }
        if !pack_ids.insert(p.id.as_str())
            || file_pack(roots, &p.id).is_some()
            || built_in_pack(&p.id).is_some()
        {
            return Err(problem(
                format!("Another pack already has the id “{}”", p.id),
                Some(&p.id),
                Some("id"),
            ));
        }
        if p.name.trim().is_empty() {
            return Err(problem(
                "A pack needs a name".into(),
                Some(&p.id),
                Some("name"),
            ));
        }
    }
    let mut ids = BTreeSet::new();
    for s in &store.shortcuts {
        let id = Some(s.id.as_str());
        if s.id.trim().is_empty() || !ids.insert(s.id.as_str()) {
            return Err(problem(
                format!("The shortcut id “{}” is empty or used twice", s.id),
                id,
                Some("id"),
            ));
        }
        if s.shortcut.name.trim().is_empty() {
            return Err(problem("A shortcut needs a name".into(), id, Some("name")));
        }
        if !pack_ids.contains(s.pack.as_str()) && base_pack(roots, store, &s.pack).is_none() {
            return Err(problem(
                format!("There is no pack “{}”", s.pack),
                id,
                Some("pack"),
            ));
        }
        let combos = [
            ("mac", &s.shortcut.mac),
            ("windows", &s.shortcut.windows),
            ("linux", &s.shortcut.linux),
        ];
        let mut any = false;
        for (field, combo) in combos {
            if let Some(c) = combo.as_deref().filter(|c| !c.trim().is_empty()) {
                parse_combo(c).map_err(|e| {
                    problem(
                        format!("The {field} combination of “{}”: {e}", s.shortcut.name),
                        id,
                        Some(field),
                    )
                })?;
                any = true;
            }
        }
        if !any {
            return Err(problem(
                format!(
                    "“{}” needs a key combination for at least one platform",
                    s.shortcut.name
                ),
                id,
                Some("mac"),
            ));
        }
    }
    Ok(())
}

/// `store` with blank combinations dropped, so an absent one means "not on this platform".
fn normalized(mut store: CustomShortcuts) -> CustomShortcuts {
    for s in &mut store.shortcuts {
        for combo in [
            &mut s.shortcut.mac,
            &mut s.shortcut.windows,
            &mut s.shortcut.linux,
        ] {
            if combo.as_deref().is_some_and(|c| c.trim().is_empty()) {
                *combo = None;
            } else if let Some(c) = combo {
                *c = c.trim().to_string();
            }
        }
        s.shortcut.id = None;
    }
    store.hidden.retain(|_, ids| !ids.is_empty());
    store
}

/// The rows the display's shortcut overlays come from for the pack `id`: its rows that aren't
/// hidden; none for an empty or unknown id.
pub fn overlay_rows(state: &ApiState, id: &str) -> Vec<Value> {
    if id.is_empty() {
        return Vec::new();
    }
    let roots = crate::api::assets::resolve_asset_roots(state);
    let custom = read_custom(state);
    load_pack(&roots, &custom, id)
        .or_else(|| {
            if !id.ends_with(".json") {
                load_pack(&roots, &custom, &format!("{id}.json"))
            } else {
                None
            }
        })
        .map(|p| p.visible_rows())
        .unwrap_or_default()
}

/// Finds the best matching shortcut pack (built-in or user custom) whose `apps` metadata matches
/// the foreground application's bundle ID / process name for the current OS platform.
pub fn match_pack_for_app(
    state: &ApiState,
    app_id: &str,
    process_name: &str,
    platform_code: u32,
) -> Option<String> {
    let app_l = app_id.trim().to_ascii_lowercase();
    let proc_l = process_name.trim().to_ascii_lowercase();
    if app_l.is_empty() && proc_l.is_empty() {
        return None;
    }
    let roots = crate::api::assets::resolve_asset_roots(state);
    let custom = read_custom(state);
    let packs = all_packs(&roots, &custom);
    for pack in packs {
        let targets: &[String] = match platform_code {
            2 => &pack.apps.macos,
            1 => &pack.apps.windows,
            _ => &pack.apps.linux,
        };
        for target in targets {
            let t = target.trim().to_ascii_lowercase();
            if t.is_empty() {
                continue;
            }
            if (!app_l.is_empty() && (app_l == t || app_l.contains(&t)))
                || (!proc_l.is_empty() && (proc_l == t || proc_l.contains(&t)))
            {
                return Some(pack.id);
            }
        }
    }
    None
}

fn err(code: StatusCode, message: &str) -> axum::response::Response {
    (code, Json(json!({ "message": message }))).into_response()
}

/// `GET /v1/shortcuts?pack=<id>`: the pack with every row, hidden ones flagged.
pub async fn get_pack(
    State(state): State<Arc<ApiState>>,
    Query(q): Query<BTreeMap<String, String>>,
) -> impl IntoResponse {
    let Some(id) = q.get("pack").filter(|s| is_plain_name(s)).cloned() else {
        return err(StatusCode::BAD_REQUEST, "A valid pack id is required");
    };
    let res = tokio::task::spawn_blocking(move || {
        let roots = crate::api::assets::resolve_asset_roots(&state);
        load_pack(&roots, &read_custom(&state), &id).map(|p| p.to_json())
    })
    .await;
    match res {
        Ok(Some(v)) => (StatusCode::OK, Json(v)).into_response(),
        Ok(None) => err(
            StatusCode::NOT_FOUND,
            "There is no shortcut pack with that id",
        ),
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Shortcut pack error: {e}"),
        ),
    }
}

/// `GET /v1/shortcuts/custom`: the user's shortcuts.
pub async fn get_custom(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    (StatusCode::OK, Json(json!(read_custom(&state))))
}

/// `PUT /v1/shortcuts/custom`: replaces the user's shortcuts with the body, once it is valid.
/// A refusal is 422 with `message`, and `id` and `field` naming where the problem is.
pub async fn put_custom(
    State(state): State<Arc<ApiState>>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let store: CustomShortcuts = match serde_json::from_value(body) {
        Ok(s) => s,
        Err(e) => {
            return err(
                StatusCode::BAD_REQUEST,
                &format!("Invalid custom shortcuts: {e}"),
            )
        }
    };
    let store = normalized(store);
    let st = state.clone();
    let res = tokio::task::spawn_blocking(move || {
        let roots = crate::api::assets::resolve_asset_roots(&st);
        validate_custom(&roots, &store).map_err(Ok)?;
        let path = custom_path(&st);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| Err(e.to_string()))?;
        }
        let raw = serde_json::to_vec_pretty(&store).map_err(|e| Err(e.to_string()))?;
        crate::api::tls::write_owner_only_file(&path, &raw, false).map_err(Err)?;
        Ok::<_, Result<StoreProblem, String>>(store)
    })
    .await;
    match res {
        Ok(Ok(store)) => {
            state.emit("configChange", json!({ "kind": "shortcuts" }));
            (StatusCode::OK, Json(json!(store))).into_response()
        }
        Ok(Err(Ok(p))) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({ "message": p.message, "id": p.id, "field": p.field })),
        )
            .into_response(),
        Ok(Err(Err(e))) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("Couldn't keep the custom shortcuts: {e}"),
        ),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// `GET /v1/shortcuts/templates`: profile templates, one per pack that names its apps.
pub async fn get_templates(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let res = tokio::task::spawn_blocking(move || {
        let roots = crate::api::assets::resolve_asset_roots(&state);
        profile_templates(&roots, &read_custom(&state))
    })
    .await
    .unwrap_or_default();
    (StatusCode::OK, Json(json!({ "templates": res })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(mods: u32, key: &str) -> Stroke {
        Stroke {
            mods,
            key: key.into(),
        }
    }

    #[test]
    fn combinations_read_modifiers_keys_and_sequences() {
        assert_eq!(
            parse_combo("Cmd+Shift+P").unwrap(),
            [stroke(MOD_GUI | MOD_SHIFT, "KC_P")]
        );
        assert_eq!(
            parse_combo("ctrl+alt+delete").unwrap(),
            [stroke(MOD_CTRL | MOD_ALT, "KC_DELETE")]
        );
        assert_eq!(
            parse_combo("Option+Cmd+Esc").unwrap(),
            [stroke(MOD_ALT | MOD_GUI, "KC_ESCAPE")]
        );
        assert_eq!(
            parse_combo("Win+Shift+S").unwrap(),
            [stroke(MOD_GUI | MOD_SHIFT, "KC_S")]
        );
        assert_eq!(
            parse_combo("Ctrl+=").unwrap(),
            [stroke(MOD_CTRL, "KC_EQUAL")]
        );
        assert_eq!(
            parse_combo("Cmd+[").unwrap(),
            [stroke(MOD_GUI, "KC_LEFT_BRACKET")]
        );
        assert_eq!(parse_combo("F5").unwrap(), [stroke(0, "KC_F5")]);
        assert_eq!(
            parse_combo("Shift+F12").unwrap(),
            [stroke(MOD_SHIFT, "KC_F12")]
        );
        assert_eq!(parse_combo("Numpad7").unwrap(), [stroke(0, "KC_KP_7")]);
        assert_eq!(
            parse_combo("Ctrl+NumpadPlus").unwrap(),
            [stroke(MOD_CTRL, "KC_KP_PLUS")]
        );
        assert!(parse_combo("⌘⇧").is_err());
        assert_eq!(
            parse_combo("Ctrl+KC_F13").unwrap(),
            [stroke(MOD_CTRL, "KC_F13")]
        );
        assert_eq!(
            parse_combo("Ctrl+K Ctrl+S").unwrap(),
            [stroke(MOD_CTRL, "KC_K"), stroke(MOD_CTRL, "KC_S")]
        );
        assert_eq!(
            parse_combo("Ctrl+K  V").unwrap(),
            [stroke(MOD_CTRL, "KC_K"), stroke(0, "KC_V")]
        );
    }

    #[test]
    fn combinations_that_name_no_key_are_refused_with_the_reason() {
        assert!(parse_combo("").unwrap_err().contains("empty"));
        assert!(parse_combo("Ctrl+Shift")
            .unwrap_err()
            .contains("no key besides"));
        assert!(parse_combo("Ctrl+").unwrap_err().contains("no key after"));
        assert!(parse_combo("Hyper+A")
            .unwrap_err()
            .contains("“Hyper” isn't a modifier"));
        assert!(parse_combo("Ctrl+Banana")
            .unwrap_err()
            .contains("“Banana” isn't a key"));
        assert!(parse_combo("F25").is_err());
        assert!(parse_combo("A B C").unwrap_err().contains("at most two"));
    }

    #[test]
    fn every_built_in_pack_reads_and_every_combination_parses() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/shortcuts");
        let mut files: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".json"))
            .collect();
        files.sort();
        let mut listed: Vec<String> = BUILT_IN_PACKS
            .iter()
            .map(|(id, _)| id.to_string())
            .collect();
        listed.sort();
        assert_eq!(
            files, listed,
            "every file in assets/shortcuts is built in, and only those"
        );

        for (id, txt) in BUILT_IN_PACKS {
            let doc: PackDocument =
                serde_json::from_str(txt).unwrap_or_else(|e| panic!("{id} doesn't read: {e}"));
            assert!(!doc.name.is_empty(), "{id}");
            assert!(doc.source.starts_with("https://"), "{id} names its source");
            assert!(
                doc.shortcuts.len() >= 15,
                "{id} has {} shortcuts",
                doc.shortcuts.len()
            );
            assert!(!doc.platforms.is_empty(), "{id}");
            for p in &doc.platforms {
                assert!(
                    ["mac", "windows", "linux"].contains(&p.as_str()),
                    "{id}: {p}"
                );
            }
            let ids = entry_ids(&doc.shortcuts);
            let base: BTreeSet<String> = doc
                .shortcuts
                .iter()
                .map(|e| slug(&format!("{} {}", e.context, e.name)))
                .collect();
            assert_eq!(
                base.len(),
                ids.len(),
                "{id}: each shortcut has its own context and name"
            );
            for e in &doc.shortcuts {
                assert!(!e.name.trim().is_empty(), "{id}");
                let combos = [
                    ("mac", &e.mac),
                    ("windows", &e.windows),
                    ("linux", &e.linux),
                ];
                let mut any = false;
                for (platform, combo) in combos {
                    if let Some(c) = combo {
                        assert!(
                            doc.platforms.iter().any(|p| p == platform),
                            "{id}: “{}” has a {platform} combination the pack isn't for",
                            e.name
                        );
                        let strokes = parse_combo(c).unwrap_or_else(|err| {
                            panic!("{id}: “{}” {platform} “{c}”: {err}", e.name)
                        });
                        for s in strokes {
                            assert!(
                                crate::codecs::keycodes::keyboard_usage(&s.key).is_some(),
                                "{id}: {}",
                                s.key
                            );
                        }
                        any = true;
                    }
                }
                assert!(any, "{id}: “{}” has no combination", e.name);
            }
        }
    }

    fn custom_store() -> CustomShortcuts {
        CustomShortcuts {
            packs: vec![CustomPack {
                id: "custom-tool".into(),
                name: "My Tool".into(),
                apps: PackApps {
                    macos: vec!["com.example.tool".into()],
                    ..Default::default()
                },
            }],
            shortcuts: vec![
                CustomShortcut {
                    id: "c1".into(),
                    pack: "vscode.json".into(),
                    shortcut: ShortcutEntry {
                        name: "Toggle Copilot".into(),
                        context: "Editor".into(),
                        icon: "spark.png".into(),
                        mac: Some("Cmd+Shift+I".into()),
                        windows: Some("Ctrl+Shift+I".into()),
                        ..Default::default()
                    },
                    replaces: None,
                },
                CustomShortcut {
                    id: "c2".into(),
                    pack: "vscode.json".into(),
                    shortcut: ShortcutEntry {
                        name: "Show Command Palette".into(),
                        context: "General".into(),
                        mac: Some("Cmd+K Cmd+P".into()),
                        windows: Some("Ctrl+K Ctrl+P".into()),
                        ..Default::default()
                    },
                    replaces: Some("navigation-show-all-commands".into()),
                },
                CustomShortcut {
                    id: "c3".into(),
                    pack: "custom-tool".into(),
                    shortcut: ShortcutEntry {
                        name: "Run".into(),
                        windows: Some("F9".into()),
                        ..Default::default()
                    },
                    replaces: None,
                },
            ],
            hidden: BTreeMap::from([(
                "vscode.json".to_string(),
                vec!["navigation-go-to-file-quick-open".to_string()],
            )]),
        }
    }

    #[test]
    fn custom_shortcuts_join_their_pack_and_hide_what_they_replace() {
        let store = custom_store();
        validate_custom(&[], &store).unwrap();
        let pack = load_pack(&[], &store, "vscode.json").unwrap();
        let row = |id: &str| pack.rows.iter().find(|r| r["id"] == id).cloned().unwrap();
        assert_eq!(row("navigation-show-all-commands")["hidden"], true);
        assert_eq!(row("navigation-go-to-file-quick-open")["hidden"], true);
        let c1 = row("c1");
        assert_eq!(c1["custom"], true);
        assert_eq!(c1["macKeyCode"], "KC_I");
        assert_eq!(c1["macModifiers"], MOD_GUI | MOD_SHIFT);
        assert_eq!(c1["winModifiers"], MOD_CTRL | MOD_SHIFT);
        // VS Code is for Linux too, so the Windows combination stands in.
        assert_eq!(c1["linuxString"], "Ctrl+Shift+I");
        assert_eq!(c1["icon"], "spark.png");
        let c2 = row("c2");
        assert_eq!(c2["replaces"], "navigation-show-all-commands");
        assert_eq!(
            (c2["macHasSecondKey"].as_bool(), c2["macKeyCode2"].as_str()),
            (Some(true), Some("KC_P"))
        );
        let base = load_pack(&[], &CustomShortcuts::default(), "vscode.json").unwrap();
        assert_eq!(pack.visible_rows().len(), base.rows.len() - 2 + 2);
        assert!(pack
            .visible_rows()
            .iter()
            .all(|r| r["id"] != "navigation-show-all-commands"));

        let mine = load_pack(&[], &store, "custom-tool").unwrap();
        assert_eq!(mine.origin, PackOrigin::Custom);
        assert_eq!(mine.rows.len(), 1);
        assert_eq!(mine.rows[0]["winKeyCode"], "KC_F9");
        assert_eq!(mine.rows[0]["macKeyCode"], Value::Null);

        let summaries = pack_summaries(&[], &store);
        assert_eq!(summaries.len(), BUILT_IN_PACKS.len() + 1);
        let templates = profile_templates(&[], &store);
        let t = templates
            .iter()
            .find(|t| t["shortcutPack"] == "custom-tool")
            .unwrap();
        assert_eq!(t["apps"]["macos"], json!(["com.example.tool"]));
        assert!(templates.iter().any(|t| t["shortcutPack"] == "vscode.json"));
    }

    #[test]
    fn an_invalid_store_is_refused_naming_the_shortcut_and_field() {
        let mut store = custom_store();
        store.shortcuts[0].shortcut.mac = Some("Cmd+Banana".into());
        let p = validate_custom(&[], &store).unwrap_err();
        assert_eq!(
            (p.id.as_deref(), p.field.as_deref()),
            (Some("c1"), Some("mac"))
        );
        assert!(p.message.contains("Banana"), "{}", p.message);

        let mut store = custom_store();
        store.shortcuts[2].shortcut.windows = None;
        let p = validate_custom(&[], &store).unwrap_err();
        assert!(p.message.contains("at least one platform"), "{}", p.message);

        let mut store = custom_store();
        store.shortcuts[1].pack = "nope.json".into();
        assert_eq!(
            validate_custom(&[], &store).unwrap_err().field.as_deref(),
            Some("pack")
        );

        let mut store = custom_store();
        store.packs[0].id = "vscode.json".into();
        assert!(validate_custom(&[], &store)
            .unwrap_err()
            .message
            .contains("already"));

        let mut store = custom_store();
        store.shortcuts[1].id = "c1".into();
        assert_eq!(
            validate_custom(&[], &store).unwrap_err().field.as_deref(),
            Some("id")
        );
    }

    #[test]
    fn a_file_pack_shadows_a_built_in_one_and_spreadsheets_get_ids() {
        let root = std::env::temp_dir().join(format!("openflux_sc_{}", std::process::id()));
        std::fs::create_dir_all(root.join("shortcuts")).unwrap();
        std::fs::write(
            root.join("shortcuts/vscode.json"),
            r#"{"name": "My VS Code", "platforms": ["mac"], "shortcuts": [{"name": "Save", "mac": "Cmd+S"}]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("shortcuts/Tool.csv"),
            "Include this shortcut?,Application ,Application Context,Shortcut,Shortcut label,Icon,Mac String,Win String,Win-Ctrl,Win-Shift,Win-Alt ,Win-Winkey,Win-US Key 1\nYes,Tool,Main,Save,Save,,,Ctrl+S,1,0,0,0,S\n",
        )
        .unwrap();
        let roots = vec![root.clone()];
        let custom = CustomShortcuts::default();
        let mine = load_pack(&roots, &custom, "vscode.json").unwrap();
        assert_eq!(
            (mine.name.as_str(), mine.origin, mine.rows.len()),
            ("My VS Code", PackOrigin::File, 1)
        );
        let csv = load_pack(&roots, &custom, "Tool.csv").unwrap();
        assert_eq!(csv.rows[0]["id"], "main-save");
        assert_eq!(csv.rows[0]["linuxKeyCode"], "KC_S");
        assert_eq!(csv.rows[0]["linuxModifiers"], MOD_CTRL);
        assert!(all_packs(&roots, &custom)
            .iter()
            .any(|p| p.id == "Tool.csv"));
        assert!(load_pack(&roots, &custom, "../etc.json").is_none());
        std::fs::remove_dir_all(root).ok();
    }
}
