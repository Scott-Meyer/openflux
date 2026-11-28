//! Keyglass's default configuration: the built-in keymap and the defaults bundled in
//! `assets/default_config.json`.
//!
//! The keyboard has no printed legends; the display draws every label, so the default layout is
//! free to place keys where they are most useful.

/// Bundled default appearance, settings, mosaic and actuation modes.
pub const DEFAULT_CONFIG_JSON: &str = include_str!("../../../assets/default_config.json");

/// Number of keys per physical row, top row first.
pub const ANSI_ROWS: [usize; 6] = [16, 15, 15, 14, 14, 11];
pub const ISO_ROWS: [usize; 6] = [16, 15, 15, 15, 15, 11];

/// Parsed `assets/default_config.json`, or an empty object if it can't be parsed.
pub fn default_config() -> serde_json::Value {
    serde_json::from_str(DEFAULT_CONFIG_JSON).unwrap_or_else(|_| serde_json::json!({}))
}

const BOTTOM_ROW: [&str; 11] = [
    "KC_LEFT_CTRL",
    "KC_LEFT_GUI",
    "KC_LEFT_ALT",
    "KC_GLOBE",
    "KC_SPACE",
    "KC_RIGHT_ALT",
    "FN",
    "KC_RIGHT_CTRL",
    "KC_LEFT",
    "KC_DOWN",
    "KC_RIGHT",
];

fn ansi_base() -> Vec<Vec<&'static str>> {
    vec![
        vec![
            "KC_ESCAPE",
            "KC_F1",
            "KC_F2",
            "KC_F3",
            "KC_F4",
            "KC_F5",
            "KC_F6",
            "KC_F7",
            "KC_F8",
            "KC_F9",
            "KC_F10",
            "KC_F11",
            "KC_F12",
            "KC_F13",
            "KC_F14",
            "KC_DELETE",
        ],
        vec![
            "KC_GRAVE",
            "KC_1",
            "KC_2",
            "KC_3",
            "KC_4",
            "KC_5",
            "KC_6",
            "KC_7",
            "KC_8",
            "KC_9",
            "KC_0",
            "KC_MINUS",
            "KC_EQUAL",
            "KC_BACKSPACE",
            "KC_HOME",
        ],
        vec![
            "KC_TAB",
            "KC_Q",
            "KC_W",
            "KC_E",
            "KC_R",
            "KC_T",
            "KC_Y",
            "KC_U",
            "KC_I",
            "KC_O",
            "KC_P",
            "KC_LEFT_BRACKET",
            "KC_RIGHT_BRACKET",
            "KC_BACKSLASH",
            "KC_END",
        ],
        vec![
            "KC_CAPS_LOCK",
            "KC_A",
            "KC_S",
            "KC_D",
            "KC_F",
            "KC_G",
            "KC_H",
            "KC_J",
            "KC_K",
            "KC_L",
            "KC_SEMICOLON",
            "KC_QUOTE",
            "KC_ENTER",
            "KC_PAGE_UP",
        ],
        vec![
            "KC_LEFT_SHIFT",
            "KC_Z",
            "KC_X",
            "KC_C",
            "KC_V",
            "KC_B",
            "KC_N",
            "KC_M",
            "KC_COMMA",
            "KC_DOT",
            "KC_SLASH",
            "KC_RIGHT_SHIFT",
            "KC_UP",
            "KC_PAGE_DOWN",
        ],
        BOTTOM_ROW.to_vec(),
    ]
}

fn iso_base() -> Vec<Vec<&'static str>> {
    let mut rows = ansi_base();
    // ISO: Enter spans rows 2 and 3, the key left of Enter is #, and the key right of the left
    // Shift is \.
    rows[2][13] = "KC_ENTER";
    rows[3].insert(12, "KC_NONUS_HASH");
    rows[4].insert(1, "KC_BACKSLASH");
    rows
}

/// The function layer, held with the Fn key. Unlisted keys are transparent.
fn fn_layer(rows: &[usize; 6]) -> Vec<Vec<&'static str>> {
    let mut out: Vec<Vec<&'static str>> = rows.iter().map(|&n| vec!["KC_TRANSPARENT"; n]).collect();
    let top = [
        (1, "KC_BRIGHTNESS_DOWN"),
        (2, "KC_BRIGHTNESS_UP"),
        (3, "KC_MISSION_CONTROL"),
        (4, "KC_LAUNCHPAD"),
        (7, "KC_MEDIA_PREV_TRACK"),
        (8, "KC_MEDIA_PLAY_PAUSE"),
        (9, "KC_MEDIA_NEXT_TRACK"),
        (10, "KC_AUDIO_MUTE"),
        (11, "KC_AUDIO_VOL_DOWN"),
        (12, "KC_AUDIO_VOL_UP"),
    ];
    let numbers = [
        (1, "KC_OF_BRIGHTNESS_DOWN"),
        (2, "KC_OF_BRIGHTNESS_UP"),
        (3, "KC_OF_PROFILE_CYCLE_PREV"),
        (4, "KC_OF_PROFILE_CYCLE_NEXT"),
        (5, "KC_OF_ACTUATION_MODE_PREV"),
        (6, "KC_OF_ACTUATION_MODE_NEXT"),
        (7, "KC_OF_THEME_CYCLE_PREV"),
        (8, "KC_OF_THEME_CYCLE_NEXT"),
        (9, "KC_OF_START_PAUSE_POMODORO"),
        (10, "KC_OF_RESET_POMODORO"),
    ];
    let letters = [
        (1, "KC_OF_PLATFORM_WINDOWS"),
        (2, "KC_OF_PLATFORM_MACOS"),
        (3, "KC_OF_PLATFORM_LINUX"),
        (4, "KC_OF_AUTO_PROFILE_LOCK"),
    ];
    for (i, kc) in top {
        out[0][i] = kc;
    }
    for (i, kc) in numbers {
        out[1][i] = kc;
    }
    for (i, kc) in letters {
        out[2][i] = kc;
    }
    out
}

fn key_actions(rows: Vec<Vec<&'static str>>) -> Vec<serde_json::Value> {
    rows.into_iter()
        .flatten()
        .map(|kc| {
            if kc == "FN" {
                serde_json::json!({"layerControlType": "momentaryLayer", "targetLayer": 1, "type": 2})
            } else {
                serde_json::json!({"keyCode": kc, "modifiers": null, "type": 0})
            }
        })
        .collect()
}

fn layout(
    name: &str,
    frame: &str,
    base: Vec<Vec<&'static str>>,
    rows: &[usize; 6],
    ids: [&str; 2],
) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "defaultLayer": 0,
        "frameLayoutType": frame,
        "layers": [
            {"frameLayoutType": frame, "id": ids[0], "name": "Base", "keyActions": key_actions(base)},
            {"frameLayoutType": frame, "id": ids[1], "name": "FN", "keyActions": key_actions(fn_layer(rows))},
        ],
    })
}

/// Keyglass's default keymap document for both physical layouts.
pub fn default_keymap() -> serde_json::Value {
    serde_json::json!({
        "keymaps": {
            "ansi": layout(
                "Keyglass ANSI",
                "ansi",
                ansi_base(),
                &ANSI_ROWS,
                ["6f0c3a52-8e1d-4b7a-9c24-31d5e07a1b01", "6f0c3a52-8e1d-4b7a-9c24-31d5e07a1b02"],
            ),
            "iso": layout(
                "Keyglass ISO",
                "iso",
                iso_base(),
                &ISO_ROWS,
                ["6f0c3a52-8e1d-4b7a-9c24-31d5e07a1b11", "6f0c3a52-8e1d-4b7a-9c24-31d5e07a1b12"],
            ),
        },
        "macros": {},
        "selectedFrameLayoutType": "ansi",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layouts_have_one_action_per_physical_key() {
        let km = default_keymap();
        for (name, rows) in [("ansi", ANSI_ROWS), ("iso", ISO_ROWS)] {
            for layer in km["keymaps"][name]["layers"].as_array().unwrap() {
                assert_eq!(
                    layer["keyActions"].as_array().unwrap().len(),
                    rows.iter().sum::<usize>(),
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn default_keymap_encodes_for_the_firmware() {
        let blob = crate::codecs::keymap::encode_keymap_json_to_firmware_blob(
            &default_keymap(),
            None,
            None,
        )
        .unwrap();
        let dec = crate::codecs::keymap::decode_keymap_blob(&blob).unwrap();
        assert_eq!(dec.keymaps.len(), 2);
        assert_eq!(dec.keymaps[0].num_keys, 85);
        assert_eq!(dec.keymaps[1].num_keys, 87);
    }
}
