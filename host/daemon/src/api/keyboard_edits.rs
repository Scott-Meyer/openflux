//! Keeping edits made on the keyboard itself when the daemon saves configuration documents.
//!
//! The keyboard changes some of its documents on its own: a theme picked with a function key is
//! written to `appearance.json` as `selectedId`. The daemon's cached appearance, settings and
//! mosaic documents each derive from the copy last read from or written to the keyboard, their
//! *base* ([`InnerState::keyboard_documents`]). Right before a save writes one of them, the
//! keyboard's current copy is read again and every top-level field is decided on its own:
//!
//! - a field the cached document hasn't changed since the base takes the keyboard's current value,
//!   or stays absent when the keyboard's copy doesn't have it;
//! - a field the cached document has changed is written as cached: the save changes it
//!   explicitly, even if the keyboard changed it too.
//!
//! In `appearance`, `selectedId` also counts as changed after a theme was picked through the API
//! ([`InnerState::theme_picked`]), even when the pick ended on the base's value. A kept
//! `selectedId` that names a theme the saved document no longer has falls back to the cached one.
//!
//! The cached document is written as it is when there is no base (it didn't come from the
//! keyboard: a reset or an import), when the keyboard reports it has no copy, or when its copy
//! isn't a JSON object. When the keyboard's copy can't be read, the save fails before writing
//! anything and can be tried again.
//!
//! After a successful save the written document becomes the base, and the cached fields that
//! weren't edited while the save ran take the written values, so the cache shows what the keyboard
//! holds.

use crate::api::state::{ApiState, DeviceSink, InnerState};
use crate::device_paths;
use serde_json::Value;
use std::collections::BTreeMap;

/// The documents a save rebases, by save-plan key, with their paths on the keyboard.
pub const REBASED_DOCUMENTS: [(&str, &str); 3] = [
    ("appearance", device_paths::APPEARANCE_JSON),
    ("settings", device_paths::SETTINGS_JSON),
    ("mosaic", device_paths::MOSAIC_JSON),
];

/// The top-level fields a save kept from the keyboard, by save-plan key. Documents with none
/// aren't listed.
pub type KeptFields = BTreeMap<String, Vec<String>>;

/// Three-way merge of the top-level fields of `ours` onto `theirs`: each field `ours` hasn't
/// changed from `base` takes the value in `theirs` (or is absent with it); the others, and the
/// fields in `changed` whatever their value, keep the value in `ours`. Returns the merged document
/// and the fields where it took `theirs` over a different value in `ours`. When any of the three
/// isn't a JSON object, `ours` is returned.
pub fn merge_fields(
    base: &Value,
    ours: &Value,
    theirs: &Value,
    changed: &[&str],
) -> (Value, Vec<String>) {
    let (Some(b), Some(o), Some(t)) = (base.as_object(), ours.as_object(), theirs.as_object())
    else {
        return (ours.clone(), Vec::new());
    };
    let mut merged = serde_json::Map::new();
    let mut kept = Vec::new();
    // A field only the base has was removed on both sides, so it stays absent.
    for key in o.keys().chain(t.keys().filter(|k| !o.contains_key(*k))) {
        let (bv, ov, tv) = (b.get(key), o.get(key), t.get(key));
        let take_theirs = ov == bv && !changed.contains(&key.as_str());
        if take_theirs && ov != tv {
            kept.push(key.clone());
        }
        if let Some(v) = if take_theirs { tv } else { ov } {
            merged.insert(key.clone(), v.clone());
        }
    }
    (Value::Object(merged), kept)
}

/// Whether `appearance` has the theme its `selectedId` names: `default`, the default theme's id,
/// or one of its custom themes. A document without `selectedId` selects the default theme.
fn selected_theme_exists(appearance: &Value) -> bool {
    let Some(id) = appearance.get("selectedId").and_then(|v| v.as_str()) else {
        return true;
    };
    if id.is_empty() || id == "default" {
        return true;
    }
    let default_id = appearance
        .get("defaultTheme")
        .and_then(|t| t.get("id"))
        .and_then(|v| v.as_str());
    default_id == Some(id)
        || appearance
            .get("customThemes")
            .and_then(|v| v.as_array())
            .is_some_and(|themes| {
                themes
                    .iter()
                    .any(|t| t.get("id").and_then(|v| v.as_str()) == Some(id))
            })
}

/// The document a save writes for `key`: `ours` rebased onto the keyboard's current copy
/// `theirs`, as the module documentation describes, with the fields in `changed` counted as
/// changed. Returns it with the fields kept from the keyboard.
pub fn rebase_document(
    key: &str,
    base: Option<&Value>,
    ours: &Value,
    theirs: Option<&Value>,
    changed: &[&str],
) -> (Value, Vec<String>) {
    let (Some(base), Some(theirs)) = (base, theirs) else {
        return (ours.clone(), Vec::new());
    };
    let (mut merged, mut kept) = merge_fields(base, ours, theirs, changed);
    if key == "appearance" && !selected_theme_exists(&merged) {
        if let (Some(obj), Some(sel)) = (merged.as_object_mut(), ours.get("selectedId")) {
            obj.insert("selectedId".into(), sel.clone());
        }
        kept.retain(|k| k != "selectedId");
    }
    (merged, kept)
}

/// The keyboard's current copy of a document: `None` when the keyboard reports it has none, or
/// when its copy is damaged (the same bytes that aren't JSON on two reads), which the save then
/// replaces. A copy that can't be read, or reads differently twice without being JSON, is an
/// error: the save fails before writing anything.
fn read_keyboard_document(device: &dyn DeviceSink, path: &str) -> Result<Option<Value>, String> {
    let unreadable = |e: &str| {
        format!(
            "The keyboard's {path} couldn't be read to keep changes made on the keyboard ({e}). \
             Nothing was saved; try saving again."
        )
    };
    let read = || match device.get_file(path) {
        Ok(raw) => Ok(Some(raw)),
        Err(e) => match device.stat_file(path) {
            Ok(false) => Ok(None),
            Ok(true) => Err(unreadable(&e)),
            Err(stat_err) => Err(unreadable(&format!("{e}; {stat_err}"))),
        },
    };
    let Some(raw) = read()? else {
        return Ok(None);
    };
    if let Ok(doc) = serde_json::from_slice(&raw) {
        return Ok(Some(doc));
    }
    match read()? {
        None => Ok(None),
        Some(again) => match serde_json::from_slice(&again) {
            Ok(doc) => Ok(Some(doc)),
            Err(_) if again == raw => Ok(None),
            Err(e) => Err(unreadable(&format!("not JSON: {e}"))),
        },
    }
}

/// `plan` with its appearance, settings and mosaic documents rebased onto the keyboard's current
/// copies, and the fields kept from the keyboard. Fails when a copy can't be read.
pub fn rebase_plan(
    state: &ApiState,
    device: &dyn DeviceSink,
    plan: &Value,
) -> Result<(Value, KeptFields), String> {
    let mut out = plan.clone();
    let mut kept_all = KeptFields::new();
    for (key, path) in REBASED_DOCUMENTS {
        let Some(ours) = plan.get(key) else { continue };
        let (base, theme_picked) = {
            let g = state.inner.lock().unwrap();
            (g.keyboard_documents.get(key).cloned(), g.theme_picked)
        };
        // Without a base nothing tells the keyboard's edits apart, so there is nothing to read.
        let theirs = match base {
            Some(_) => read_keyboard_document(device, path)?,
            None => None,
        };
        let changed: &[&str] = if key == "appearance" && theme_picked {
            &["selectedId"]
        } else {
            &[]
        };
        let (doc, kept) = rebase_document(key, base.as_ref(), ours, theirs.as_ref(), changed);
        if !kept.is_empty() {
            kept_all.insert(key.to_string(), kept);
        }
        if let Some(obj) = out.as_object_mut() {
            obj.insert(key.to_string(), doc);
        }
    }
    Ok((out, kept_all))
}

fn cached_document_mut<'a>(g: &'a mut InnerState, key: &str) -> Option<&'a mut Value> {
    match key {
        "appearance" => Some(&mut g.appearance),
        "settings" => Some(&mut g.settings),
        "mosaic" => Some(&mut g.mosaic),
        _ => None,
    }
}

/// After `written` (the rebased `plan`) was saved: its documents become the base, and cached
/// fields not edited since `plan` was taken take the written values.
pub fn record_saved_documents(state: &ApiState, plan: &Value, written: &Value) {
    let mut g = state.inner.lock().unwrap();
    for (key, _) in REBASED_DOCUMENTS {
        let (Some(sent), Some(doc)) = (plan.get(key), written.get(key)) else {
            continue;
        };
        if let Some(cached) = cached_document_mut(&mut g, key) {
            let (updated, _) = merge_fields(sent, cached, doc, &[]);
            *cached = updated;
        }
        // The keyboard has the picked theme now, unless another was picked while saving.
        if key == "appearance" && g.appearance.get("selectedId") == doc.get("selectedId") {
            g.theme_picked = false;
        }
        g.keyboard_documents.insert(key.to_string(), doc.clone());
    }
}

/// Records `doc`, just read from the keyboard into the cache, as the base of `key`.
pub fn record_pulled_document(g: &mut InnerState, key: &str, doc: &Value) {
    if key == "appearance" {
        g.theme_picked = false;
    }
    g.keyboard_documents.insert(key.to_string(), doc.clone());
}

/// Saves `plan` to the keyboard, keeping edits made on the keyboard since its documents were last
/// read or written. Returns the fields kept from the keyboard.
pub fn save_keeping_keyboard_edits(
    state: &ApiState,
    device: &dyn DeviceSink,
    plan: &Value,
) -> Result<KeptFields, String> {
    let (written, kept) = rebase_plan(state, device, plan)?;
    device.save_config_dir(&written)?;
    record_saved_documents(state, plan, &written);
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unchanged_fields_take_the_keyboards_value_and_changed_fields_are_written() {
        let base = json!({"selectedId": "a", "customThemes": [{"id": "a"}, {"id": "b"}], "x": 1});
        let ours = json!({"selectedId": "a", "customThemes": [{"id": "a"}, {"id": "b"}, {"id": "c"}], "x": 1});
        let theirs = json!({"selectedId": "b", "customThemes": [{"id": "a"}, {"id": "b"}], "y": 2});
        let (merged, kept) = merge_fields(&base, &ours, &theirs, &[]);
        assert_eq!(
            merged,
            json!({"selectedId": "b", "customThemes": [{"id": "a"}, {"id": "b"}, {"id": "c"}], "y": 2})
        );
        assert_eq!(kept, vec!["selectedId".to_string(), "x".into(), "y".into()]);
    }

    #[test]
    fn an_explicit_change_wins_over_the_keyboards() {
        let base = json!({"selectedId": "a"});
        let ours = json!({"selectedId": "c"});
        let theirs = json!({"selectedId": "b"});
        let (merged, kept) = rebase_document("appearance", Some(&base), &ours, Some(&theirs), &[]);
        assert_eq!(merged["selectedId"], "c");
        assert!(kept.is_empty());
    }

    #[test]
    fn a_picked_theme_is_written_even_when_it_is_the_bases() {
        let base = json!({"selectedId": "a", "x": 1});
        let theirs = json!({"selectedId": "b", "x": 1});
        let (merged, kept) = rebase_document(
            "appearance",
            Some(&base),
            &base,
            Some(&theirs),
            &["selectedId"],
        );
        assert_eq!(merged["selectedId"], "a");
        assert!(kept.is_empty());
    }

    #[test]
    fn a_kept_theme_the_save_removes_falls_back_to_the_cached_selection() {
        let base = json!({"selectedId": "a", "customThemes": [{"id": "a"}, {"id": "b"}]});
        let ours = json!({"selectedId": "a", "customThemes": [{"id": "a"}]});
        let theirs = json!({"selectedId": "b", "customThemes": [{"id": "a"}, {"id": "b"}]});
        let (merged, kept) = rebase_document("appearance", Some(&base), &ours, Some(&theirs), &[]);
        assert_eq!(merged["selectedId"], "a");
        assert!(kept.is_empty());
    }

    #[test]
    fn without_a_base_or_a_keyboard_copy_the_cached_document_is_written() {
        let ours = json!({"selectedId": "a"});
        let theirs = json!({"selectedId": "b"});
        assert_eq!(
            rebase_document("appearance", None, &ours, Some(&theirs), &[]).0,
            ours
        );
        assert_eq!(
            rebase_document("appearance", Some(&theirs), &ours, None, &[]).0,
            ours
        );
    }
}
