//! Key code names: the `KC_*` names keymap documents use for keyboard keys and consumer controls,
//! and the HID usages they stand for.
//!
//! Every name is the full QMK-style name, which is what OpenFlux writes. [`KEY_ALIASES`] lists
//! the short and older QMK names that are accepted as well. `tests/fixtures/keycodes.json` holds
//! the same tables for the Python codec and Studio.

/// Keyboard/Keypad Page (`0x07`) usages with a name: every key the page defines from `KC_A` to
/// `KC_EXSEL`, and the eight modifiers.
pub const KEYBOARD_KEYS: [(&str, u32); 169] = [
    ("KC_A", 0x04),
    ("KC_B", 0x05),
    ("KC_C", 0x06),
    ("KC_D", 0x07),
    ("KC_E", 0x08),
    ("KC_F", 0x09),
    ("KC_G", 0x0A),
    ("KC_H", 0x0B),
    ("KC_I", 0x0C),
    ("KC_J", 0x0D),
    ("KC_K", 0x0E),
    ("KC_L", 0x0F),
    ("KC_M", 0x10),
    ("KC_N", 0x11),
    ("KC_O", 0x12),
    ("KC_P", 0x13),
    ("KC_Q", 0x14),
    ("KC_R", 0x15),
    ("KC_S", 0x16),
    ("KC_T", 0x17),
    ("KC_U", 0x18),
    ("KC_V", 0x19),
    ("KC_W", 0x1A),
    ("KC_X", 0x1B),
    ("KC_Y", 0x1C),
    ("KC_Z", 0x1D),
    ("KC_1", 0x1E),
    ("KC_2", 0x1F),
    ("KC_3", 0x20),
    ("KC_4", 0x21),
    ("KC_5", 0x22),
    ("KC_6", 0x23),
    ("KC_7", 0x24),
    ("KC_8", 0x25),
    ("KC_9", 0x26),
    ("KC_0", 0x27),
    ("KC_ENTER", 0x28),
    ("KC_ESCAPE", 0x29),
    ("KC_BACKSPACE", 0x2A),
    ("KC_TAB", 0x2B),
    ("KC_SPACE", 0x2C),
    ("KC_MINUS", 0x2D),
    ("KC_EQUAL", 0x2E),
    ("KC_LEFT_BRACKET", 0x2F),
    ("KC_RIGHT_BRACKET", 0x30),
    ("KC_BACKSLASH", 0x31),
    ("KC_NONUS_HASH", 0x32),
    ("KC_SEMICOLON", 0x33),
    ("KC_QUOTE", 0x34),
    ("KC_GRAVE", 0x35),
    ("KC_COMMA", 0x36),
    ("KC_DOT", 0x37),
    ("KC_SLASH", 0x38),
    ("KC_CAPS_LOCK", 0x39),
    ("KC_F1", 0x3A),
    ("KC_F2", 0x3B),
    ("KC_F3", 0x3C),
    ("KC_F4", 0x3D),
    ("KC_F5", 0x3E),
    ("KC_F6", 0x3F),
    ("KC_F7", 0x40),
    ("KC_F8", 0x41),
    ("KC_F9", 0x42),
    ("KC_F10", 0x43),
    ("KC_F11", 0x44),
    ("KC_F12", 0x45),
    ("KC_PRINT_SCREEN", 0x46),
    ("KC_SCROLL_LOCK", 0x47),
    ("KC_PAUSE", 0x48),
    ("KC_INSERT", 0x49),
    ("KC_HOME", 0x4A),
    ("KC_PAGE_UP", 0x4B),
    ("KC_DELETE", 0x4C),
    ("KC_END", 0x4D),
    ("KC_PAGE_DOWN", 0x4E),
    ("KC_RIGHT", 0x4F),
    ("KC_LEFT", 0x50),
    ("KC_DOWN", 0x51),
    ("KC_UP", 0x52),
    ("KC_NUM_LOCK", 0x53),
    ("KC_KP_SLASH", 0x54),
    ("KC_KP_ASTERISK", 0x55),
    ("KC_KP_MINUS", 0x56),
    ("KC_KP_PLUS", 0x57),
    ("KC_KP_ENTER", 0x58),
    ("KC_KP_1", 0x59),
    ("KC_KP_2", 0x5A),
    ("KC_KP_3", 0x5B),
    ("KC_KP_4", 0x5C),
    ("KC_KP_5", 0x5D),
    ("KC_KP_6", 0x5E),
    ("KC_KP_7", 0x5F),
    ("KC_KP_8", 0x60),
    ("KC_KP_9", 0x61),
    ("KC_KP_0", 0x62),
    ("KC_KP_DOT", 0x63),
    ("KC_NONUS_BACKSLASH", 0x64),
    ("KC_APPLICATION", 0x65),
    ("KC_KB_POWER", 0x66),
    ("KC_KP_EQUAL", 0x67),
    ("KC_F13", 0x68),
    ("KC_F14", 0x69),
    ("KC_F15", 0x6A),
    ("KC_F16", 0x6B),
    ("KC_F17", 0x6C),
    ("KC_F18", 0x6D),
    ("KC_F19", 0x6E),
    ("KC_F20", 0x6F),
    ("KC_F21", 0x70),
    ("KC_F22", 0x71),
    ("KC_F23", 0x72),
    ("KC_F24", 0x73),
    ("KC_EXECUTE", 0x74),
    ("KC_HELP", 0x75),
    ("KC_MENU", 0x76),
    ("KC_SELECT", 0x77),
    ("KC_STOP", 0x78),
    ("KC_AGAIN", 0x79),
    ("KC_UNDO", 0x7A),
    ("KC_CUT", 0x7B),
    ("KC_COPY", 0x7C),
    ("KC_PASTE", 0x7D),
    ("KC_FIND", 0x7E),
    ("KC_KB_MUTE", 0x7F),
    ("KC_KB_VOLUME_UP", 0x80),
    ("KC_KB_VOLUME_DOWN", 0x81),
    ("KC_LOCKING_CAPS_LOCK", 0x82),
    ("KC_LOCKING_NUM_LOCK", 0x83),
    ("KC_LOCKING_SCROLL_LOCK", 0x84),
    ("KC_KP_COMMA", 0x85),
    ("KC_KP_EQUAL_AS400", 0x86),
    ("KC_INTERNATIONAL_1", 0x87),
    ("KC_INTERNATIONAL_2", 0x88),
    ("KC_INTERNATIONAL_3", 0x89),
    ("KC_INTERNATIONAL_4", 0x8A),
    ("KC_INTERNATIONAL_5", 0x8B),
    ("KC_INTERNATIONAL_6", 0x8C),
    ("KC_INTERNATIONAL_7", 0x8D),
    ("KC_INTERNATIONAL_8", 0x8E),
    ("KC_INTERNATIONAL_9", 0x8F),
    ("KC_LANGUAGE_1", 0x90),
    ("KC_LANGUAGE_2", 0x91),
    ("KC_LANGUAGE_3", 0x92),
    ("KC_LANGUAGE_4", 0x93),
    ("KC_LANGUAGE_5", 0x94),
    ("KC_LANGUAGE_6", 0x95),
    ("KC_LANGUAGE_7", 0x96),
    ("KC_LANGUAGE_8", 0x97),
    ("KC_LANGUAGE_9", 0x98),
    ("KC_ALTERNATE_ERASE", 0x99),
    ("KC_SYSTEM_REQUEST", 0x9A),
    ("KC_CANCEL", 0x9B),
    ("KC_CLEAR", 0x9C),
    ("KC_PRIOR", 0x9D),
    ("KC_RETURN", 0x9E),
    ("KC_SEPARATOR", 0x9F),
    ("KC_OUT", 0xA0),
    ("KC_OPER", 0xA1),
    ("KC_CLEAR_AGAIN", 0xA2),
    ("KC_CRSEL", 0xA3),
    ("KC_EXSEL", 0xA4),
    ("KC_LEFT_CTRL", 0xE0),
    ("KC_LEFT_SHIFT", 0xE1),
    ("KC_LEFT_ALT", 0xE2),
    ("KC_LEFT_GUI", 0xE3),
    ("KC_RIGHT_CTRL", 0xE4),
    ("KC_RIGHT_SHIFT", 0xE5),
    ("KC_RIGHT_ALT", 0xE6),
    ("KC_RIGHT_GUI", 0xE7),
];

/// Consumer Page (`0x0C`) usages with a name. A consumer key code's low 16 bits are the usage
/// the firmware sends.
pub const CONSUMER_CONTROLS: [(&str, u32); 35] = [
    ("KC_SYSTEM_SLEEP", 0x0032),
    ("KC_BRIGHTNESS_UP", 0x006F),
    ("KC_BRIGHTNESS_DOWN", 0x0070),
    ("KC_MEDIA_NEXT_TRACK", 0x00B5),
    ("KC_MEDIA_PREV_TRACK", 0x00B6),
    ("KC_MEDIA_STOP", 0x00B7),
    ("KC_MEDIA_EJECT", 0x00B8),
    ("KC_MEDIA_PLAY_PAUSE", 0x00CD),
    ("KC_AUDIO_MUTE", 0x00E2),
    ("KC_AUDIO_BASS_BOOST", 0x00E5),
    ("KC_AUDIO_LOUDNESS", 0x00E7),
    ("KC_AUDIO_VOL_UP", 0x00E9),
    ("KC_AUDIO_VOL_DOWN", 0x00EA),
    ("KC_AUDIO_BASS_UP", 0x0152),
    ("KC_AUDIO_BASS_DOWN", 0x0153),
    ("KC_AUDIO_TREBLE_UP", 0x0154),
    ("KC_AUDIO_TREBLE_DOWN", 0x0155),
    ("KC_MEDIA_SELECT", 0x0183),
    ("KC_MAIL", 0x018A),
    ("KC_CALCULATOR", 0x0192),
    ("KC_MY_COMPUTER", 0x0194),
    ("KC_AC_FIND", 0x021F),
    ("KC_WWW_SEARCH", 0x0221),
    ("KC_WWW_HOME", 0x0223),
    ("KC_WWW_BACK", 0x0224),
    ("KC_WWW_FORWARD", 0x0225),
    ("KC_WWW_STOP", 0x0226),
    ("KC_WWW_REFRESH", 0x0227),
    ("KC_WWW_FAVORITES", 0x022A),
    ("KC_SCROLL_UP", 0x0233),
    ("KC_SCROLL_DOWN", 0x0234),
    ("KC_EDIT", 0x023D),
    ("KC_GLOBE", 0x029D),
    ("KC_MISSION_CONTROL", 0x029F),
    ("KC_LAUNCHPAD", 0x02A0),
];

/// `(alias, name)`: other names accepted for a key code. Documents are written with the name.
pub const KEY_ALIASES: [(&str, &str); 142] = [
    ("KC_ENT", "KC_ENTER"),
    ("KC_ESC", "KC_ESCAPE"),
    ("KC_BSPC", "KC_BACKSPACE"),
    ("KC_SPC", "KC_SPACE"),
    ("KC_MINS", "KC_MINUS"),
    ("KC_EQL", "KC_EQUAL"),
    ("KC_LBRC", "KC_LEFT_BRACKET"),
    ("KC_RBRC", "KC_RIGHT_BRACKET"),
    ("KC_BSLS", "KC_BACKSLASH"),
    ("KC_NUHS", "KC_NONUS_HASH"),
    ("KC_SCLN", "KC_SEMICOLON"),
    ("KC_QUOT", "KC_QUOTE"),
    ("KC_GRV", "KC_GRAVE"),
    ("KC_COMM", "KC_COMMA"),
    ("KC_SLSH", "KC_SLASH"),
    ("KC_CAPS", "KC_CAPS_LOCK"),
    ("KC_PSCR", "KC_PRINT_SCREEN"),
    ("KC_SCRL", "KC_SCROLL_LOCK"),
    ("KC_BRMD", "KC_SCROLL_LOCK"),
    ("KC_PAUS", "KC_PAUSE"),
    ("KC_BRK", "KC_PAUSE"),
    ("KC_BRMU", "KC_PAUSE"),
    ("KC_INS", "KC_INSERT"),
    ("KC_PGUP", "KC_PAGE_UP"),
    ("KC_DEL", "KC_DELETE"),
    ("KC_PGDN", "KC_PAGE_DOWN"),
    ("KC_RGHT", "KC_RIGHT"),
    ("KC_NUM", "KC_NUM_LOCK"),
    ("KC_PSLS", "KC_KP_SLASH"),
    ("KC_PAST", "KC_KP_ASTERISK"),
    ("KC_PMNS", "KC_KP_MINUS"),
    ("KC_PPLS", "KC_KP_PLUS"),
    ("KC_PENT", "KC_KP_ENTER"),
    ("KC_P1", "KC_KP_1"),
    ("KC_P2", "KC_KP_2"),
    ("KC_P3", "KC_KP_3"),
    ("KC_P4", "KC_KP_4"),
    ("KC_P5", "KC_KP_5"),
    ("KC_P6", "KC_KP_6"),
    ("KC_P7", "KC_KP_7"),
    ("KC_P8", "KC_KP_8"),
    ("KC_P9", "KC_KP_9"),
    ("KC_P0", "KC_KP_0"),
    ("KC_PDOT", "KC_KP_DOT"),
    ("KC_NUBS", "KC_NONUS_BACKSLASH"),
    ("KC_APP", "KC_APPLICATION"),
    ("KC_PEQL", "KC_KP_EQUAL"),
    ("KC_EXEC", "KC_EXECUTE"),
    ("KC_SLCT", "KC_SELECT"),
    ("KC_AGIN", "KC_AGAIN"),
    ("KC_PSTE", "KC_PASTE"),
    ("KC_LCAP", "KC_LOCKING_CAPS_LOCK"),
    ("KC_LNUM", "KC_LOCKING_NUM_LOCK"),
    ("KC_LSCR", "KC_LOCKING_SCROLL_LOCK"),
    ("KC_PCMM", "KC_KP_COMMA"),
    ("KC_INT1", "KC_INTERNATIONAL_1"),
    ("KC_INT2", "KC_INTERNATIONAL_2"),
    ("KC_INT3", "KC_INTERNATIONAL_3"),
    ("KC_INT4", "KC_INTERNATIONAL_4"),
    ("KC_INT5", "KC_INTERNATIONAL_5"),
    ("KC_INT6", "KC_INTERNATIONAL_6"),
    ("KC_INT7", "KC_INTERNATIONAL_7"),
    ("KC_INT8", "KC_INTERNATIONAL_8"),
    ("KC_INT9", "KC_INTERNATIONAL_9"),
    ("KC_LNG1", "KC_LANGUAGE_1"),
    ("KC_LNG2", "KC_LANGUAGE_2"),
    ("KC_LNG3", "KC_LANGUAGE_3"),
    ("KC_LNG4", "KC_LANGUAGE_4"),
    ("KC_LNG5", "KC_LANGUAGE_5"),
    ("KC_LNG6", "KC_LANGUAGE_6"),
    ("KC_LNG7", "KC_LANGUAGE_7"),
    ("KC_LNG8", "KC_LANGUAGE_8"),
    ("KC_LNG9", "KC_LANGUAGE_9"),
    ("KC_LANG1", "KC_LANGUAGE_1"),
    ("KC_LANG2", "KC_LANGUAGE_2"),
    ("KC_LANG3", "KC_LANGUAGE_3"),
    ("KC_LANG4", "KC_LANGUAGE_4"),
    ("KC_LANG5", "KC_LANGUAGE_5"),
    ("KC_LANG6", "KC_LANGUAGE_6"),
    ("KC_LANG7", "KC_LANGUAGE_7"),
    ("KC_LANG8", "KC_LANGUAGE_8"),
    ("KC_LANG9", "KC_LANGUAGE_9"),
    ("KC_ERAS", "KC_ALTERNATE_ERASE"),
    ("KC_SYRQ", "KC_SYSTEM_REQUEST"),
    ("KC_CNCL", "KC_CANCEL"),
    ("KC_CLR", "KC_CLEAR"),
    ("KC_PRIR", "KC_PRIOR"),
    ("KC_RETN", "KC_RETURN"),
    ("KC_SEPR", "KC_SEPARATOR"),
    ("KC_CLAG", "KC_CLEAR_AGAIN"),
    ("KC_CRSL", "KC_CRSEL"),
    ("KC_EXSL", "KC_EXSEL"),
    ("KC_LCTL", "KC_LEFT_CTRL"),
    ("KC_LSFT", "KC_LEFT_SHIFT"),
    ("KC_LSHIFT", "KC_LEFT_SHIFT"),
    ("KC_LALT", "KC_LEFT_ALT"),
    ("KC_LOPT", "KC_LEFT_ALT"),
    ("KC_LGUI", "KC_LEFT_GUI"),
    ("KC_LCMD", "KC_LEFT_GUI"),
    ("KC_LWIN", "KC_LEFT_GUI"),
    ("KC_RCTL", "KC_RIGHT_CTRL"),
    ("KC_RSFT", "KC_RIGHT_SHIFT"),
    ("KC_RSHIFT", "KC_RIGHT_SHIFT"),
    ("KC_RALT", "KC_RIGHT_ALT"),
    ("KC_ROPT", "KC_RIGHT_ALT"),
    ("KC_ALGR", "KC_RIGHT_ALT"),
    ("KC_RGUI", "KC_RIGHT_GUI"),
    ("KC_RCMD", "KC_RIGHT_GUI"),
    ("KC_RWIN", "KC_RIGHT_GUI"),
    ("KC_SLEP", "KC_SYSTEM_SLEEP"),
    ("KC_BRIU", "KC_BRIGHTNESS_UP"),
    ("KC_BRID", "KC_BRIGHTNESS_DOWN"),
    ("KC_MNXT", "KC_MEDIA_NEXT_TRACK"),
    ("KC_MPRV", "KC_MEDIA_PREV_TRACK"),
    ("KC_MSTP", "KC_MEDIA_STOP"),
    ("KC_EJCT", "KC_MEDIA_EJECT"),
    ("KC_MPLY", "KC_MEDIA_PLAY_PAUSE"),
    ("KC_MUTE", "KC_AUDIO_MUTE"),
    ("KC_BSBT", "KC_AUDIO_BASS_BOOST"),
    ("KC_LDNS", "KC_AUDIO_LOUDNESS"),
    ("KC_VOLU", "KC_AUDIO_VOL_UP"),
    ("KC_VOLD", "KC_AUDIO_VOL_DOWN"),
    ("KC_BSSU", "KC_AUDIO_BASS_UP"),
    ("KC_BSSD", "KC_AUDIO_BASS_DOWN"),
    ("KC_TRBU", "KC_AUDIO_TREBLE_UP"),
    ("KC_TRBD", "KC_AUDIO_TREBLE_DOWN"),
    ("KC_MSEL", "KC_MEDIA_SELECT"),
    ("KC_CALC", "KC_CALCULATOR"),
    ("KC_MYCM", "KC_MY_COMPUTER"),
    ("KC_WSCH", "KC_WWW_SEARCH"),
    ("KC_WHOM", "KC_WWW_HOME"),
    ("KC_WBAK", "KC_WWW_BACK"),
    ("KC_WFWD", "KC_WWW_FORWARD"),
    ("KC_WSTP", "KC_WWW_STOP"),
    ("KC_WREF", "KC_WWW_REFRESH"),
    ("KC_WFAV", "KC_WWW_FAVORITES"),
    ("KC_SCLU", "KC_SCROLL_UP"),
    ("KC_SCLD", "KC_SCROLL_DOWN"),
    ("KC_GLBE", "KC_GLOBE"),
    ("KC_MCTL", "KC_MISSION_CONTROL"),
    ("KC_LPAD", "KC_LAUNCHPAD"),
    ("KC_TRNS", "KC_TRANSPARENT"),
];

/// The name `name` stands for: itself, or the name an alias stands for.
pub fn canonical_name(name: &str) -> &str {
    KEY_ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map_or(name, |(_, n)| n)
}

/// The Keyboard Page usage of a key name or alias.
pub fn keyboard_usage(name: &str) -> Option<u32> {
    let name = canonical_name(name);
    KEYBOARD_KEYS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, u)| *u)
}

/// The name of a Keyboard Page usage.
pub fn keyboard_key_name(usage: u32) -> Option<&'static str> {
    KEYBOARD_KEYS
        .iter()
        .find(|(_, u)| *u == usage)
        .map(|(n, _)| *n)
}

/// The Consumer Page usage of a consumer control name or alias.
pub fn consumer_usage(name: &str) -> Option<u32> {
    let name = canonical_name(name);
    CONSUMER_CONTROLS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, u)| *u)
}

/// The name of a Consumer Page usage.
pub fn consumer_name(usage: u32) -> Option<&'static str> {
    CONSUMER_CONTROLS
        .iter()
        .find(|(_, u)| *u == usage)
        .map(|(n, _)| *n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codecs::keymap::{
        firmware_word_to_action, qmk_action_to_firmware_word, NONE_KEYCODE, OPENFLUX_KEYS,
        TRANSPARENT_KEYCODE,
    };

    fn word(name: &str, mods: Option<u32>) -> u32 {
        qmk_action_to_firmware_word(
            &serde_json::json!({"type": 0, "keyCode": name, "modifiers": mods}),
        )
        .unwrap_or_else(|e| panic!("{name}: {e}"))
    }

    /// Every name a key action can hold: the special keys, the keyboard keys, the consumer
    /// controls and OpenFlux's functions.
    fn names() -> Vec<&'static str> {
        ["KC_TRANSPARENT", "KC_NO"]
            .into_iter()
            .chain(KEYBOARD_KEYS.iter().map(|(n, _)| *n))
            .chain(CONSUMER_CONTROLS.iter().map(|(n, _)| *n))
            .chain(OPENFLUX_KEYS.iter().map(|(n, _)| *n))
            .collect()
    }

    #[test]
    fn every_name_round_trips_through_its_firmware_word() {
        let all = names();
        let mut words = std::collections::HashSet::new();
        for name in &all {
            let w = word(name, None);
            assert!(words.insert(w), "{name} shares 0x{w:08X} with another name");
            assert_eq!(firmware_word_to_action(w).unwrap()["keyCode"], *name);
        }
        for (name, usage) in KEYBOARD_KEYS.iter().chain(CONSUMER_CONTROLS.iter()) {
            let w = word(name, Some(0x05));
            assert_eq!(w & 0xFFFF, *usage, "{name}");
            let action = firmware_word_to_action(w).unwrap();
            assert_eq!(
                (action["keyCode"].as_str(), action["modifiers"].as_u64()),
                (Some(*name), Some(5))
            );
        }
        assert_eq!(word("KC_TRANSPARENT", None), TRANSPARENT_KEYCODE);
        assert_eq!(word("KC_NO", None), NONE_KEYCODE);
    }

    #[test]
    fn every_alias_stands_for_a_name_and_encodes_like_it() {
        let all = names();
        for (alias, name) in KEY_ALIASES {
            assert!(all.contains(&name), "{alias} stands for unknown {name}");
            assert!(!all.contains(&alias), "{alias} is also a name");
            assert_eq!(word(alias, None), word(name, None), "{alias}");
        }
    }

    #[test]
    fn macro_steps_name_every_keyboard_key() {
        use crate::codecs::keymap::{hid_usage_to_qmk_keycode, qmk_keycode_to_hid_usage};
        for (name, usage) in KEYBOARD_KEYS {
            assert_eq!(qmk_keycode_to_hid_usage(name).unwrap(), usage);
            let step = hid_usage_to_qmk_keycode(usage).unwrap();
            assert_eq!(qmk_keycode_to_hid_usage(step).unwrap(), usage, "{name}");
        }
        assert!(qmk_keycode_to_hid_usage("KC_AUDIO_MUTE").is_err());
    }

    /// The tables as `tests/fixtures/keycodes.json` holds them.
    fn keycodes_fixture() -> serde_json::Value {
        let table = |t: &[(&str, u32)]| -> Vec<serde_json::Value> {
            t.iter()
                .map(|(n, u)| serde_json::json!({"name": n, "usage": u}))
                .collect()
        };
        let openflux: Vec<_> = OPENFLUX_KEYS
            .iter()
            .map(|(n, w)| serde_json::json!({"name": n, "word": w}))
            .collect();
        let aliases: serde_json::Map<_, _> = KEY_ALIASES
            .iter()
            .map(|(a, n)| (a.to_string(), serde_json::json!(n)))
            .collect();
        let words: serde_json::Map<_, _> = names()
            .into_iter()
            .chain(KEY_ALIASES.iter().map(|(a, _)| *a))
            .map(|n| (n.to_string(), serde_json::json!(word(n, None))))
            .collect();
        serde_json::json!({
            "keyboard": table(&KEYBOARD_KEYS),
            "consumer": table(&CONSUMER_CONTROLS),
            "openflux": openflux,
            "aliases": aliases,
            "words": words,
        })
    }

    /// `tests/fixtures/keycodes.json` is this module's tables and the firmware word of every name
    /// and alias; `tools/openflux-dev/tests/test_codecs.py` and Studio's tests check their own
    /// tables against the same file. Regenerate it with
    /// `OPENFLUX_REGENERATE_FIXTURES=1 cargo test -p openfluxd keycodes_fixture`.
    #[test]
    fn keycodes_fixture_matches_the_tables() {
        let fixture = keycodes_fixture();
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/keycodes.json");
        if std::env::var("OPENFLUX_REGENERATE_FIXTURES").as_deref() == Ok("1") {
            std::fs::write(
                &path,
                serde_json::to_string_pretty(&fixture).unwrap() + "\n",
            )
            .unwrap();
            return;
        }
        let stored: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/keycodes.json")).unwrap();
        assert_eq!(stored, fixture, "tests/fixtures/keycodes.json is stale");
    }
}
