//! Keycode names that differ between Keyglass and the keyboard's factory software.
//!
//! Keyglass names its own keyboard functions `KC_OF_*`. The factory software, and the third-party
//! apps written against it, name the same functions `KC_FF_*`; the binary values are the same.
//! The factory names also include `KC_WWW` for the WWW Home consumer control, which Keyglass
//! names with QMK's `KC_WWW_HOME`.
//! Documents that come from outside Keyglass (a factory keyboard's settings, third-party requests)
//! are translated to Keyglass names on the way in, and third-party replies are translated back.

/// `(Keyglass name, factory name)` for every keycode whose names differ.
pub const KEYCODE_NAMES: [(&str, &str); 15] = [
    ("KC_OF_BRIGHTNESS_UP", "KC_FF_BRIGHTNESS_UP"),
    ("KC_OF_BRIGHTNESS_DOWN", "KC_FF_BRIGHTNESS_DOWN"),
    ("KC_OF_PROFILE_CYCLE_NEXT", "KC_FF_PROFILE_CYCLE_NEXT"),
    ("KC_OF_PROFILE_CYCLE_PREV", "KC_FF_PROFILE_CYCLE_PREV"),
    ("KC_OF_ACTUATION_MODE_NEXT", "KC_FF_ACTUATION_MODE_NEXT"),
    ("KC_OF_ACTUATION_MODE_PREV", "KC_FF_ACTUATION_MODE_PREV"),
    ("KC_OF_PLATFORM_LINUX", "KC_FF_PLATFORM_LINUX"),
    ("KC_OF_PLATFORM_MACOS", "KC_FF_PLATFORM_MACOS"),
    ("KC_OF_PLATFORM_WINDOWS", "KC_FF_PLATFORM_WINDOWS"),
    ("KC_OF_AUTO_PROFILE_LOCK", "KC_FF_AUTO_PROFILE_LOCK"),
    ("KC_OF_START_PAUSE_POMODORO", "KC_FF_START_PAUSE_POMODORO"),
    ("KC_OF_RESET_POMODORO", "KC_FF_RESET_POMODORO"),
    ("KC_OF_THEME_CYCLE_NEXT", "KC_FF_THEME_CYCLE_NEXT"),
    ("KC_OF_THEME_CYCLE_PREV", "KC_FF_THEME_CYCLE_PREV"),
    ("KC_WWW_HOME", "KC_WWW"),
];

/// The Keyglass name of a factory keycode name.
pub fn keyglass_name(factory: &str) -> Option<&'static str> {
    KEYCODE_NAMES
        .iter()
        .find(|(_, f)| *f == factory)
        .map(|(of, _)| *of)
}

/// The factory name of an Keyglass keycode name.
pub fn factory_name(keyglass: &str) -> Option<&'static str> {
    KEYCODE_NAMES
        .iter()
        .find(|(of, _)| *of == keyglass)
        .map(|(_, f)| *f)
}

fn rename(v: &mut serde_json::Value, map: fn(&str) -> Option<&'static str>) -> usize {
    match v {
        serde_json::Value::String(s) => match map(s) {
            Some(name) => {
                *s = name.to_string();
                1
            }
            None => 0,
        },
        serde_json::Value::Array(a) => a.iter_mut().map(|x| rename(x, map)).sum(),
        serde_json::Value::Object(o) => o.values_mut().map(|x| rename(x, map)).sum(),
        _ => 0,
    }
}

/// Rewrites every string in `v` that is a factory keycode name to its Keyglass name, at any
/// depth. Object keys are left alone. Returns how many strings changed.
pub fn to_keyglass(v: &mut serde_json::Value) -> usize {
    rename(v, keyglass_name)
}

/// Rewrites every string in `v` that is an Keyglass keycode name to its factory name, at any
/// depth. Object keys are left alone. Returns how many strings changed.
pub fn to_factory(v: &mut serde_json::Value) -> usize {
    rename(v, factory_name)
}

/// The factory keycode names used anywhere in `v`, each once, in table order.
pub fn factory_names_in(v: &serde_json::Value) -> Vec<&'static str> {
    fn walk(v: &serde_json::Value, seen: &mut [bool; KEYCODE_NAMES.len()]) {
        match v {
            serde_json::Value::String(s) => {
                if let Some(i) = KEYCODE_NAMES.iter().position(|(_, f)| f == s) {
                    seen[i] = true;
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, seen)),
            serde_json::Value::Object(o) => o.values().for_each(|x| walk(x, seen)),
            _ => {}
        }
    }
    let mut seen = [false; KEYCODE_NAMES.len()];
    walk(v, &mut seen);
    KEYCODE_NAMES
        .iter()
        .zip(seen)
        .filter(|(_, s)| *s)
        .map(|((_, f), _)| *f)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_translate_both_ways_at_any_depth_and_leave_keys_and_other_strings_alone() {
        let mut v = serde_json::json!({
            "KC_FF_BRIGHTNESS_UP": "KC_FF_BRIGHTNESS_UP",
            "layers": [{"keyActions": [{"keyCode": "KC_FF_THEME_CYCLE_NEXT"}, {"keyCode": "KC_A"}]}],
            "label": "KC_FF_THEME_CYCLE_NEXT is next theme",
        });
        assert_eq!(
            factory_names_in(&v),
            ["KC_FF_BRIGHTNESS_UP", "KC_FF_THEME_CYCLE_NEXT"]
        );
        assert_eq!(to_keyglass(&mut v), 2);
        assert_eq!(v["KC_FF_BRIGHTNESS_UP"], "KC_OF_BRIGHTNESS_UP");
        assert_eq!(
            v["layers"][0]["keyActions"][0]["keyCode"],
            "KC_OF_THEME_CYCLE_NEXT"
        );
        assert_eq!(v["layers"][0]["keyActions"][1]["keyCode"], "KC_A");
        assert_eq!(v["label"], "KC_FF_THEME_CYCLE_NEXT is next theme");
        assert!(factory_names_in(&v).is_empty());
        assert_eq!(to_keyglass(&mut v), 0, "translating twice changes nothing");
        assert_eq!(to_factory(&mut v), 2);
        assert_eq!(
            v["layers"][0]["keyActions"][0]["keyCode"],
            "KC_FF_THEME_CYCLE_NEXT"
        );
    }

    #[test]
    fn every_keyglass_name_is_one_the_keymap_codec_knows() {
        for (of, _) in KEYCODE_NAMES {
            let action = serde_json::json!({"type": 0, "keyCode": of});
            assert!(
                crate::codecs::keymap::qmk_action_to_firmware_word(&action).is_ok(),
                "{of}"
            );
        }
    }
}
