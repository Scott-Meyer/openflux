//! Keymaps written by keyboard software other than Keyglass.
//!
//! Such keymaps store a consumer control as the index of a bit in a consumer bitmap. Keyglass
//! stores the HID Consumer Page usage itself. A keyboard's firmware keymap survives the firmware
//! flash in both directions, so conversion translates its consumer controls to usages before
//! Keyglass uses it, and a reset to factory translates them back before the factory firmware does.

use crate::codecs::keymap::{
    decode_keymap_blob, encode_keymap_json_to_firmware_blob, CodecError, CONSUMER_CONTROLS,
    NONE_KEYCODE,
};

/// Consumer-control bit index used by other keyboard software, and the HID Consumer Page usage
/// Keyglass stores for it. The factory firmware's consumer bitmap has these 35 bits, in this order.
pub const CONSUMER_TRANSLATION: [(u32, u32); 35] = [
    (0x0000, 0x0032), // sleep
    (0x0001, 0x006F), // brightness up
    (0x0002, 0x0070), // brightness down
    (0x0003, 0x00B5), // next track
    (0x0004, 0x00B6), // previous track
    (0x0005, 0x00B7), // stop
    (0x0006, 0x00B8), // eject
    (0x0007, 0x00CD), // play/pause
    (0x0008, 0x00E2), // mute
    (0x0009, 0x00E5), // bass boost
    (0x000A, 0x00E7), // loudness
    (0x000B, 0x00E9), // volume up
    (0x000C, 0x00EA), // volume down
    (0x000D, 0x0152), // bass up
    (0x000E, 0x0153), // bass down
    (0x000F, 0x0154), // treble up
    (0x0010, 0x0155), // treble down
    (0x0011, 0x0183), // media select
    (0x0012, 0x018A), // mail
    (0x0013, 0x0192), // calculator
    (0x0014, 0x0194), // my computer
    (0x0015, 0x021F), // find
    (0x0016, 0x0221), // browser search
    (0x0017, 0x0223), // browser home
    (0x0018, 0x0224), // browser back
    (0x0019, 0x0225), // browser forward
    (0x001A, 0x0226), // browser stop
    (0x001B, 0x0227), // browser refresh
    (0x001C, 0x022A), // browser favorites
    (0x001D, 0x0233), // scroll up
    (0x001E, 0x0234), // scroll down
    (0x001F, 0x023D), // edit
    (0x0020, 0x029D), // globe
    (0x0021, 0x029F), // mission control
    (0x0022, 0x02A0), // launchpad
];

/// What translating a keymap did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Translation {
    pub translated: usize,
    /// Consumer controls with no Keyglass equivalent; they are left as they are.
    pub unknown: Vec<u32>,
}

/// Rewrites every consumer control in a firmware keymap blob to its HID usage. Modifier bits are
/// kept. A blob that is already in Keyglass's encoding comes back unchanged.
pub fn translate_consumer_controls(blob: &[u8]) -> Result<(Vec<u8>, Translation), CodecError> {
    let mut km = decode_keymap_blob(blob)?;
    let mut t = Translation::default();
    let standard: Vec<u32> = CONSUMER_TRANSLATION.iter().map(|(_, new)| *new).collect();
    for section in &mut km.keymaps {
        for layer in &mut section.layers {
            for word in layer.iter_mut() {
                if (*word >> 28) != 0x4 {
                    continue;
                }
                let usage = *word & 0xFFFF;
                match CONSUMER_TRANSLATION.iter().find(|(old, _)| *old == usage) {
                    Some((_, new)) => {
                        *word = (*word & !0xFFFF) | new;
                        t.translated += 1;
                    }
                    None if standard.contains(&usage) => {}
                    None => t.unknown.push(*word),
                }
            }
        }
    }
    Ok((km.to_bytes()?, t))
}

/// A key whose consumer control has no factory encoding and was given its default binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fallback {
    /// Number of keys of the layout (85 ANSI, 87 ISO).
    pub layout_keys: u32,
    pub layer: usize,
    pub key: usize,
    /// The word that couldn't be written in the factory encoding.
    pub word: u32,
    /// What the key holds instead.
    pub replacement: u32,
}

/// What translating a keymap back to the factory encoding did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FactoryTranslation {
    pub translated: usize,
    pub fallbacks: Vec<Fallback>,
}

/// A consumer word in the factory encoding, if it has one.
///
/// A usage Keyglass names is translated, and one with no factory equivalent has none. Any other
/// consumer value can't have come from Keyglass's encoder: it is either a value the conversion left
/// unchanged or a word already translated back, so it is kept, and translating twice changes
/// nothing.
fn factory_word(word: u32, named: &[u32]) -> Option<u32> {
    if (word >> 28) != 0x4 {
        return Some(word);
    }
    let usage = word & 0xFFFF;
    if let Some((old, _)) = CONSUMER_TRANSLATION.iter().find(|(_, new)| *new == usage) {
        return Some((word & !0xFFFF) | old);
    }
    (!named.contains(&usage)).then_some(word)
}

/// Rewrites every consumer control of a firmware keymap blob in the factory encoding.
///
/// A named consumer control with no factory encoding takes the binding the key has in the
/// default keymap for its layout, layer and position (itself written in the factory encoding), or
/// no binding when the default has none it can use. Every such key is listed. Nothing outside the
/// layers changes: actuation tables, macros and the blob's header are kept as they are.
pub fn to_factory_encoding(blob: &[u8]) -> Result<(Vec<u8>, FactoryTranslation), CodecError> {
    let named: Vec<u32> = CONSUMER_CONTROLS.iter().map(|(_, u)| *u).collect();
    to_factory_encoding_with(blob, &named)
}

fn to_factory_encoding_with(
    blob: &[u8],
    named: &[u32],
) -> Result<(Vec<u8>, FactoryTranslation), CodecError> {
    let mut km = decode_keymap_blob(blob)?;
    let defaults = decode_keymap_blob(&encode_keymap_json_to_firmware_blob(
        &crate::defaults::default_keymap(),
        None,
        None,
    )?)?;
    let mut t = FactoryTranslation::default();
    for section in &mut km.keymaps {
        let default_section = defaults
            .keymaps
            .iter()
            .find(|d| d.num_keys == section.num_keys);
        let layout_keys = section.num_keys;
        for (layer_idx, layer) in section.layers.iter_mut().enumerate() {
            for (key_idx, word) in layer.iter_mut().enumerate() {
                match factory_word(*word, named) {
                    Some(w) if w == *word => {}
                    Some(w) => {
                        *word = w;
                        t.translated += 1;
                    }
                    None => {
                        let replacement = default_section
                            .and_then(|d| d.layers.get(layer_idx))
                            .and_then(|l| l.get(key_idx))
                            .and_then(|d| factory_word(*d, named))
                            .unwrap_or(NONE_KEYCODE);
                        t.fallbacks.push(Fallback {
                            layout_keys,
                            layer: layer_idx,
                            key: key_idx,
                            word: *word,
                            replacement,
                        });
                        *word = replacement;
                    }
                }
            }
        }
    }
    Ok((km.to_bytes()?, t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codecs::keymap::{qmk_action_to_firmware_word, FirmwareKeymapBlob, KeymapSection};

    fn blob(words: Vec<u32>) -> Vec<u8> {
        let n = words.len() as u32;
        FirmwareKeymapBlob {
            num_keymaps: 1,
            layout_bitmask: 1,
            layout_offset_0: 0,
            layout_offset_1: 0,
            layout_offset_2: 0,
            layout_offset_3: 0,
            keymaps: vec![KeymapSection {
                num_actuation_modes: 1,
                num_keys: n,
                num_layers: 1,
                reserved: [0; 3],
                layers: vec![words],
                actuation_tables: vec![vec![0; n as usize]],
                macros: Vec::new(),
            }],
            trailing_words: Vec::new(),
        }
        .to_bytes()
        .unwrap()
    }

    fn layer(bytes: &[u8]) -> Vec<u32> {
        decode_keymap_blob(bytes).unwrap().keymaps[0].layers[0].clone()
    }

    #[test]
    fn every_consumer_control_is_translated_to_the_usage_keyglass_encodes() {
        let old: Vec<u32> = CONSUMER_TRANSLATION
            .iter()
            .map(|(o, _)| 0x4000_0000 | o)
            .collect();
        let (out, t) = translate_consumer_controls(&blob(old)).unwrap();
        assert_eq!(t.translated, CONSUMER_CONTROLS.len());
        let expected: Vec<u32> = CONSUMER_CONTROLS
            .iter()
            .map(|(n, _)| {
                qmk_action_to_firmware_word(&serde_json::json!({"type": 0, "keyCode": n})).unwrap()
            })
            .collect();
        assert_eq!(layer(&out), expected);
    }

    #[test]
    fn other_keys_and_modifiers_are_kept_and_translation_is_idempotent() {
        let with_mods = 0x4000_0000 | (0x8 << 16) | 0x000B;
        let words = vec![0x04, with_mods, 0x5000_0002, 0x4000_0023, 0x2100_0001];
        let (out, t) = translate_consumer_controls(&blob(words)).unwrap();
        assert_eq!(
            layer(&out),
            vec![
                0x04,
                0x4000_0000 | (0x8 << 16) | 0x00E9,
                0x5000_0002,
                0x4000_0023,
                0x2100_0001
            ]
        );
        assert_eq!(t.unknown, vec![0x4000_0023]);
        let (again, t2) = translate_consumer_controls(&out).unwrap();
        assert_eq!(again, out);
        assert_eq!(t2.translated, 0);
    }

    fn default_blob() -> Vec<u8> {
        encode_keymap_json_to_firmware_blob(&crate::defaults::default_keymap(), None, None).unwrap()
    }

    #[test]
    fn consumer_controls_go_back_to_the_factory_encoding_and_the_rest_is_kept() {
        let with_mods = 0x4000_0000 | (0x8 << 16) | 0x00E9;
        let (out, t) = translate_back(vec![0x04, with_mods, 0x4000_006F, 0x5000_0002, 0x4000_000C]);
        assert_eq!(
            out,
            vec![
                0x04,
                0x4000_0000 | (0x8 << 16) | 0x000B,
                0x4000_0001,
                0x5000_0002,
                0x4000_000C
            ]
        );
        assert_eq!(t.translated, 2);
        assert!(t.fallbacks.is_empty());
    }

    fn translate_back(words: Vec<u32>) -> (Vec<u32>, FactoryTranslation) {
        let (out, t) = to_factory_encoding(&blob(words)).unwrap();
        (layer(&out), t)
    }

    #[test]
    fn every_named_consumer_control_has_a_factory_encoding() {
        for (name, usage) in CONSUMER_CONTROLS {
            assert!(
                CONSUMER_TRANSLATION.iter().any(|(_, n)| *n == usage),
                "{name} has no factory encoding"
            );
        }
    }

    #[test]
    fn a_named_consumer_control_without_a_factory_encoding_takes_the_keys_default() {
        // As if Keyglass named usage 0x0030 (power), which the factory encoding lacks.
        let named: Vec<u32> = CONSUMER_CONTROLS
            .iter()
            .map(|(_, u)| *u)
            .chain([0x0030])
            .collect();
        let mut km = decode_keymap_blob(&default_blob()).unwrap();
        let section = &mut km.keymaps[0];
        let default_word = section.layers[0][1];
        section.layers[0][1] = 0x4000_0030;
        let (out, t) = to_factory_encoding_with(&km.to_bytes().unwrap(), &named).unwrap();
        let out = decode_keymap_blob(&out).unwrap();
        assert_eq!(
            out.keymaps[0].layers[0][1],
            factory_word(default_word, &named).unwrap()
        );
        assert_eq!(t.fallbacks.len(), 1);
        assert_eq!(
            (
                t.fallbacks[0].layer,
                t.fallbacks[0].key,
                t.fallbacks[0].word
            ),
            (0, 1, 0x4000_0030)
        );
        // A keymap of a size no default covers falls back to no binding.
        let (out, t) = to_factory_encoding_with(&blob(vec![0x04, 0x4000_0030]), &named).unwrap();
        assert_eq!(layer(&out), vec![0x04, NONE_KEYCODE]);
        assert_eq!(t.fallbacks[0].replacement, NONE_KEYCODE);
    }

    #[test]
    fn a_value_conversion_left_as_it_was_goes_back_unchanged() {
        let (out, t) = translate_back(vec![0x4000_0023, 0x4000_00E9]);
        assert_eq!(out, vec![0x4000_0023, 0x4000_000B]);
        assert!(t.fallbacks.is_empty());
    }

    #[test]
    fn conversion_and_reset_round_trip_and_reset_is_idempotent() {
        let factory: Vec<u32> = CONSUMER_TRANSLATION
            .iter()
            .map(|(o, _)| 0x4000_0000 | o)
            .chain([0x04, 0x2100_0001])
            .collect();
        let (keyglass, _) = translate_consumer_controls(&blob(factory.clone())).unwrap();
        let (back, t) = to_factory_encoding(&keyglass).unwrap();
        assert_eq!(layer(&back), factory);
        assert_eq!(t.translated, CONSUMER_TRANSLATION.len());
        let (again, t2) = to_factory_encoding(&back).unwrap();
        assert_eq!(again, back);
        assert_eq!((t2.translated, t2.fallbacks.len()), (0, 0));
    }

    #[test]
    fn a_factory_keymap_with_every_named_key_converts_to_a_document_with_their_names() {
        use crate::codecs::keycodes::KEYBOARD_KEYS;
        use crate::codecs::keymap::{keymap_json_from_blob, KEYGLASS_KEYS};
        // Every keyboard key, every factory consumer bit and every Keyglass function, with Ctrl
        // on one key and one consumer control, laid out over as many 85-key layers as it takes.
        let mut factory: Vec<u32> = KEYBOARD_KEYS.iter().map(|(_, u)| *u).collect();
        factory.extend(CONSUMER_TRANSLATION.iter().map(|(o, _)| 0x4000_0000 | o));
        factory.extend(KEYGLASS_KEYS.iter().map(|(_, w)| *w));
        factory.extend([0x0001_0049, 0x4001_0005]);
        let mut expected: Vec<(&str, Option<u32>)> =
            KEYBOARD_KEYS.iter().map(|(n, _)| (*n, None)).collect();
        expected.extend(CONSUMER_CONTROLS.iter().map(|(n, _)| (*n, None)));
        expected.extend(KEYGLASS_KEYS.iter().map(|(n, _)| (*n, None)));
        expected.extend([("KC_INSERT", Some(1)), ("KC_MEDIA_STOP", Some(1))]);
        let layers: Vec<Vec<u32>> = factory
            .chunks(85)
            .map(|c| {
                let mut l = c.to_vec();
                l.resize(85, NONE_KEYCODE);
                l
            })
            .collect();
        let factory_blob = FirmwareKeymapBlob {
            num_keymaps: 1,
            layout_bitmask: 1,
            layout_offset_0: 0,
            layout_offset_1: 0,
            layout_offset_2: 0,
            layout_offset_3: 0,
            keymaps: vec![KeymapSection {
                num_actuation_modes: 1,
                num_keys: 85,
                num_layers: layers.len() as u32,
                reserved: [0; 3],
                layers,
                actuation_tables: vec![vec![0; 85]],
                macros: Vec::new(),
            }],
            trailing_words: Vec::new(),
        }
        .to_bytes()
        .unwrap();

        let (keyglass, t) = translate_consumer_controls(&factory_blob).unwrap();
        assert_eq!(t.translated, CONSUMER_TRANSLATION.len() + 1);
        assert!(t.unknown.is_empty(), "{:?}", t.unknown);
        let doc = keymap_json_from_blob(&keyglass, &serde_json::json!({})).unwrap();
        let actions: Vec<(String, Option<u32>)> = doc["keymaps"]["ansi"]["layers"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|l| l["keyActions"].as_array().unwrap().clone())
            .map(|a| {
                let mods = a["modifiers"].as_u64().map(|m| m as u32);
                (a["keyCode"].as_str().unwrap().to_string(), mods)
            })
            .collect();
        for (i, (name, mods)) in expected.iter().enumerate() {
            assert_eq!(actions[i], (name.to_string(), *mods), "key {i}");
        }
        assert!(actions[expected.len()..].iter().all(|(n, _)| n == "KC_NO"));

        let (back, _) = to_factory_encoding(&keyglass).unwrap();
        assert_eq!(back, factory_blob, "reset gives back the factory keymap");
    }
}
