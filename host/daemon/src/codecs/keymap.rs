//! Binary keymap and PROFILE_SETTINGS_V1 codecs for the keyboard firmware.
//!
//! The firmware stores keymaps as little-endian 32-bit words; this module converts between that
//! layout and the keymap JSON document that the host and the display edit.

use serde::{Deserialize, Serialize};
use std::fmt;

pub const ACTUATION_TRADITIONAL: u32 = 1;
pub const ACTUATION_RAPID_TRIGGER: u32 = 2;
pub const NONE_KEYCODE: u32 = 0xF000_0000;
pub const MAX_PROFILES: usize = 64;
/// The most layers a layout section can have; the firmware refuses a keymap with more.
pub const MAX_LAYERS: u32 = 8;
/// The most actuation modes a layout section can have; the firmware refuses a keymap with more.
pub const MAX_ACTUATION_MODES: u32 = 8;
pub const FILE_TYPE_PROFILE_SETTINGS_V1: u32 = 0;

/// Key travel in micrometres (0.1 mm steps) to the magnetic flux density reading the firmware
/// compares against.
pub const DIST_UM_TO_FLUX_DENSITY: [(u32, u32); 29] = [
    (0, 0x000),
    (100, 0x0AB),
    (200, 0x0BE),
    (300, 0x0DE),
    (400, 0x0FE),
    (500, 0x126),
    (600, 0x150),
    (700, 0x17A),
    (800, 0x1A6),
    (900, 0x1D1),
    (1000, 0x1FD),
    (1100, 0x228),
    (1200, 0x253),
    (1300, 0x281),
    (1400, 0x2B3),
    (1500, 0x2E5),
    (1600, 0x318),
    (1700, 0x34C),
    (1800, 0x381),
    (1900, 0x3C0),
    (2000, 0x402),
    (2100, 0x444),
    (2200, 0x48A),
    (2300, 0x4D1),
    (2400, 0x518),
    (2500, 0x561),
    (2600, 0x5AB),
    (2700, 0x5D7),
    (2800, 0x613),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecError(pub String);

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for CodecError {}

pub fn dist_um_to_flux_density(dist_um: u32) -> u32 {
    let clamped = dist_um.min(2800);
    let stepped = (clamped / 100) * 100;
    DIST_UM_TO_FLUX_DENSITY
        .iter()
        .find(|&&(um, _)| um == stepped)
        .map(|&(_, d)| d)
        .unwrap_or(0xAB)
}

pub fn flux_density_to_dist_um(density: u32) -> Option<u32> {
    DIST_UM_TO_FLUX_DENSITY
        .iter()
        .find(|&&(_, d)| d == density)
        .map(|&(um, _)| um)
}

/// The 0.1 mm step whose reading is closest to `density`; the shallower step on a tie.
pub fn nearest_dist_um(density: u32) -> u32 {
    DIST_UM_TO_FLUX_DENSITY
        .iter()
        .min_by_key(|&&(um, d)| (d.abs_diff(density), um))
        .map(|&(um, _)| um)
        .unwrap_or(0)
}

/// The fixed actuation reading the firmware uses for a type 0 or 3 entry, before position gain.
pub const DEFAULT_FIXED_ACTUATION_DENSITY: u32 = 650;
/// The fixed release depth the firmware uses for a type 0 or 3 entry, before position gain.
pub const DEFAULT_FIXED_RELEASE_DEPTH: u32 = 216;
/// A fixed mode with no release point releases this far above its actuation point.
pub const DEFAULT_RELEASE_GAP_UM: u32 = 200;

/// Encodes one actuation word.
///
/// Fixed (`ACTUATION_TRADITIONAL`): `second_um` is the release point. The word stores the
/// release depth `R = density(actuation) - density(release)`, so a held key releases once its
/// reading falls to `A - R`. A release point as deep as the actuation point or deeper gives
/// `R = 0`, and `R` never exceeds `A`.
///
/// Rapid trigger: `second_um` is the sensitivity, stored as its reading.
pub fn encode_actuation_word(
    actuation_type: u32,
    actuation_um: u32,
    second_um: u32,
) -> Result<u32, CodecError> {
    let act_d = dist_um_to_flux_density(actuation_um) & 0xFFF;
    let second_d = dist_um_to_flux_density(second_um) & 0xFFF;
    let field = match actuation_type {
        ACTUATION_TRADITIONAL => act_d.saturating_sub(second_d),
        ACTUATION_RAPID_TRIGGER => second_d,
        _ => {
            return Err(CodecError(format!(
                "Unsupported actuation_type: {actuation_type}"
            )))
        }
    };
    Ok((actuation_type << 30) | (field << 12) | act_d)
}

/// One actuation word as the firmware applies it, with travel in 0.1 mm steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodedActuation {
    /// `ACTUATION_TRADITIONAL` or `ACTUATION_RAPID_TRIGGER`; type 0 and 3 decode as fixed.
    pub actuation_type: u32,
    /// The actuation reading, before position gain.
    pub actuation_density: u32,
    /// Fixed: the release depth `R`. Rapid trigger: the sensitivity reading.
    pub second_density: u32,
    /// The 0.1 mm step closest to the actuation reading.
    pub actuation_um: u32,
    /// Fixed: the step closest to the release reading `A - R`. Rapid trigger: the step closest
    /// to the sensitivity reading.
    pub second_um: u32,
    /// The word is a type 0 or 3 entry, which the firmware applies as its default fixed setting.
    pub firmware_default: bool,
    /// Encoding `actuation_um` and `second_um` gives back this word.
    pub exact: bool,
}

/// Decodes one actuation word the way the firmware applies it.
pub fn decode_actuation_word(word: u32) -> DecodedActuation {
    let raw_type = word >> 30;
    let firmware_default = raw_type != ACTUATION_TRADITIONAL && raw_type != ACTUATION_RAPID_TRIGGER;
    let (actuation_type, act_d, second_d) = if firmware_default {
        (
            ACTUATION_TRADITIONAL,
            DEFAULT_FIXED_ACTUATION_DENSITY,
            DEFAULT_FIXED_RELEASE_DEPTH,
        )
    } else {
        (raw_type, word & 0xFFF, (word >> 12) & 0xFFF)
    };
    let actuation_um = nearest_dist_um(act_d);
    let second_um = if actuation_type == ACTUATION_TRADITIONAL {
        nearest_dist_um(act_d.saturating_sub(second_d))
    } else {
        nearest_dist_um(second_d)
    };
    let exact = encode_actuation_word(actuation_type, actuation_um, second_um) == Ok(word);
    DecodedActuation {
        actuation_type,
        actuation_density: act_d,
        second_density: second_d,
        actuation_um,
        second_um,
        firmware_default,
        exact,
    }
}

fn um_to_mm(um: u32) -> f64 {
    (um as f64) / 1000.0
}

/// The editable JSON for one actuation word. A word the 0.1 mm values don't reproduce also
/// carries `firmwareWord`, which the encoder keeps while those values stay unchanged.
fn actuation_entry_json(word: u32) -> serde_json::Value {
    let d = decode_actuation_word(word);
    let rapid = d.actuation_type == ACTUATION_RAPID_TRIGGER;
    let mut entry = serde_json::json!({
        "actuationPointMm": um_to_mm(d.actuation_um),
        "rapidTrigger": rapid,
        "touched": false,
    });
    if rapid {
        entry["rapidTriggerSensitivityMm"] = serde_json::json!(um_to_mm(d.second_um));
    } else {
        entry["releasePointMm"] = serde_json::json!(um_to_mm(d.second_um));
    }
    if d.firmware_default {
        entry["firmwareDefault"] = serde_json::json!(true);
    }
    if !d.exact {
        entry["firmwareWord"] = serde_json::json!(word);
    }
    entry
}

fn mm_to_um(mm: f64, min_um: f64) -> u32 {
    (mm * 1000.0).round().clamp(min_um, 2800.0) as u32
}

/// Travel settings an actuation entry falls back to for fields it leaves out.
#[derive(Debug, Clone, Copy)]
struct ActuationDefaults {
    rapid: bool,
    act_mm: f64,
    sens_mm: f64,
}

/// Encodes one mode or per-key actuation entry, returning the word and the entry's settings for
/// per-key entries that inherit them.
fn encode_actuation_entry(
    entry: &serde_json::Value,
    defaults: ActuationDefaults,
) -> Result<(u32, ActuationDefaults), CodecError> {
    let rapid = entry
        .get("rapidTrigger")
        .and_then(|v| v.as_bool())
        .unwrap_or(defaults.rapid);
    let act_mm = entry
        .get("actuationPointMm")
        .and_then(|v| v.as_f64())
        .unwrap_or(defaults.act_mm);
    let sens_mm = entry
        .get("rapidTriggerSensitivityMm")
        .and_then(|v| v.as_f64())
        .unwrap_or(defaults.sens_mm);
    let act_um = mm_to_um(act_mm, 100.0);
    let (atype, second_mm, second_um) = if rapid {
        (ACTUATION_RAPID_TRIGGER, sens_mm, mm_to_um(sens_mm, 100.0))
    } else {
        match entry.get("releasePointMm").and_then(|v| v.as_f64()) {
            Some(mm) => (ACTUATION_TRADITIONAL, mm, mm_to_um(mm, 0.0)),
            None => {
                let um = act_um.saturating_sub(DEFAULT_RELEASE_GAP_UM);
                (ACTUATION_TRADITIONAL, um_to_mm(um), um)
            }
        }
    };
    let mut word = encode_actuation_word(atype, act_um, second_um)?;
    // A decoded word between 0.1 mm steps stays as it is while its values are unchanged.
    if let Some(source) = entry
        .get("firmwareWord")
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok())
    {
        let d = decode_actuation_word(source);
        if d.actuation_type == atype
            && d.actuation_um == dist_um_to_step(mm_to_um(act_mm, 0.0))
            && d.second_um == dist_um_to_step(mm_to_um(second_mm, 0.0))
        {
            word = source;
        }
    }
    let next = ActuationDefaults {
        rapid,
        act_mm,
        sens_mm,
    };
    Ok((word, next))
}

/// Travel rounded down to its 0.1 mm step, as the encoder applies it.
fn dist_um_to_step(dist_um: u32) -> u32 {
    (dist_um.min(2800) / 100) * 100
}

pub fn encode_macro_step(hid_usage: u32, delay_ms: u32, is_down: bool) -> Result<u32, CodecError> {
    if hid_usage > 0xFF {
        return Err(CodecError(format!("hid_usage out of range: {hid_usage}")));
    }
    if delay_ms > 0xFFFF {
        return Err(CodecError(format!("delay_ms out of range: {delay_ms}")));
    }
    let down_bit = if is_down { 1u32 } else { 0u32 };
    Ok((down_bit << 31) | (delay_ms << 8) | hid_usage)
}

/// One layout's section of the keymap blob. Its header words are, in order: the layer count,
/// the key count, the actuation mode count, the macro count (twice), and the offsets of the key
/// codes and the actuation tables.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeymapSection {
    pub num_actuation_modes: u32,
    pub num_keys: u32,
    pub num_layers: u32,
    pub reserved: [u32; 3],
    pub layers: Vec<Vec<u32>>,
    pub actuation_tables: Vec<Vec<u32>>,
    #[serde(default)]
    pub macros: Vec<Vec<u32>>,
}

impl KeymapSection {
    pub fn to_words(&self) -> Result<Vec<u32>, CodecError> {
        if self.num_keys == 0 || self.num_keys > 512 {
            return Err(CodecError(format!(
                "num_keys out of range: {}",
                self.num_keys
            )));
        }
        if !(1..=MAX_LAYERS).contains(&self.num_layers) {
            return Err(CodecError(format!(
                "A layout has {} layers; the keyboard holds 1 to {MAX_LAYERS}",
                self.num_layers
            )));
        }
        if !(1..=MAX_ACTUATION_MODES).contains(&self.num_actuation_modes) {
            return Err(CodecError(format!(
                "A layout has {} actuation modes; the keyboard holds 1 to {MAX_ACTUATION_MODES}",
                self.num_actuation_modes
            )));
        }
        if self.macros.len() > 512 {
            return Err(CodecError("KeymapSection dimension exceeds limits".into()));
        }
        if self.layers.len() != self.num_layers as usize {
            return Err(CodecError(format!(
                "layers count {} != num_layers {}",
                self.layers.len(),
                self.num_layers
            )));
        }
        if self.actuation_tables.len() != self.num_actuation_modes as usize {
            return Err(CodecError(format!(
                "actuation_tables count {} != num_actuation_modes {}",
                self.actuation_tables.len(),
                self.num_actuation_modes
            )));
        }
        let mut macro_pool: Vec<u32> = Vec::new();
        let mut macro_offsets: Vec<u32> = Vec::with_capacity(self.macros.len());
        let mut running = u32::try_from(self.macros.len())
            .map_err(|_| CodecError("Macro count overflow".into()))?;
        for m in &self.macros {
            macro_offsets.push(running);
            let mlen =
                u32::try_from(m.len()).map_err(|_| CodecError("Macro length overflow".into()))?;
            macro_pool.push(mlen);
            macro_pool.extend_from_slice(m);
            running = running
                .checked_add(1)
                .and_then(|v| v.checked_add(mlen))
                .ok_or_else(|| CodecError("Macro pool offset overflow".into()))?;
        }
        let keycodes_offset = u32::try_from(macro_offsets.len())
            .ok()
            .and_then(|a| {
                u32::try_from(macro_pool.len())
                    .ok()
                    .and_then(|b| a.checked_add(b))
            })
            .ok_or_else(|| CodecError("keycodes_offset overflow".into()))?;
        let key_words = self
            .num_keys
            .checked_mul(self.num_layers)
            .ok_or_else(|| CodecError("key_words overflow".into()))?;
        let actuations_offset = keycodes_offset
            .checked_add(key_words)
            .ok_or_else(|| CodecError("actuations_offset overflow".into()))?;

        let mut words = Vec::new();
        words.extend_from_slice(&[
            self.num_layers,
            self.num_keys,
            self.num_actuation_modes,
            self.macros.len() as u32,
            self.macros.len() as u32,
            keycodes_offset,
            actuations_offset,
        ]);
        words.extend_from_slice(&macro_offsets);
        words.extend_from_slice(&macro_pool);
        for layer in &self.layers {
            if layer.len() != self.num_keys as usize {
                return Err(CodecError(format!(
                    "Layer length {} != num_keys {}",
                    layer.len(),
                    self.num_keys
                )));
            }
            for &code in layer {
                if code != NONE_KEYCODE && ((code >> 28) & 0xF) == 0x1 {
                    let macro_id = (code & 0xFFFF) as usize;
                    if macro_id >= self.macros.len() {
                        return Err(CodecError(format!(
                            "MacroKeyCode references macro_id={macro_id}, but only {} macro(s) exist",
                            self.macros.len()
                        )));
                    }
                }
            }
            words.extend_from_slice(layer);
        }
        for tbl in &self.actuation_tables {
            if tbl.len() != self.num_keys as usize {
                return Err(CodecError(format!(
                    "Actuation table length {} != num_keys {}",
                    tbl.len(),
                    self.num_keys
                )));
            }
            words.extend_from_slice(tbl);
        }
        Ok(words)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FirmwareKeymapBlob {
    pub num_keymaps: u32,
    /// Which layouts (bits 0..2 for layouts 1..3) have a keymap section.
    pub layout_bitmask: u32,
    /// The header's word offset of each layout's section, as read; [`Self::to_bytes`] recomputes
    /// them from the sections.
    pub layout_offset_0: u32,
    pub layout_offset_1: u32,
    #[serde(default)]
    pub layout_offset_2: u32,
    #[serde(default)]
    pub layout_offset_3: u32,
    pub keymaps: Vec<KeymapSection>,
    #[serde(default)]
    pub trailing_words: Vec<u32>,
}

impl FirmwareKeymapBlob {
    pub fn recomputed_offsets(&self) -> Result<[u32; 4], CodecError> {
        let active: Vec<usize> = (1..=3usize)
            .filter(|&lid| ((self.layout_bitmask >> (lid - 1)) & 1) != 0)
            .collect();
        if active.len() != self.keymaps.len() {
            return Err(CodecError(format!(
                "layout_bitmask 0x{:x} selects {} layout(s), but keymaps has {} section(s)",
                self.layout_bitmask,
                active.len(),
                self.keymaps.len()
            )));
        }
        let mut offsets = [0u32; 4];
        let mut running = 0u32;
        for (&lid, km) in active.iter().zip(self.keymaps.iter()) {
            offsets[lid] = running;
            running += km.to_words()?.len() as u32;
        }
        Ok(offsets)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, CodecError> {
        if self.keymaps.is_empty() {
            return Err(CodecError("At least one keymap section is required".into()));
        }
        if self.num_keymaps as usize != self.keymaps.len() {
            return Err(CodecError(format!(
                "num_keymaps {} != keymaps len {}",
                self.num_keymaps,
                self.keymaps.len()
            )));
        }
        let offs = self.recomputed_offsets()?;
        let mut words = Vec::with_capacity(6 + 256);
        words.extend_from_slice(&[
            self.num_keymaps,
            self.layout_bitmask,
            offs[0],
            offs[1],
            offs[2],
            offs[3],
        ]);
        for km in &self.keymaps {
            words.extend_from_slice(&km.to_words()?);
        }
        words.extend_from_slice(&self.trailing_words);
        let mut out = Vec::with_capacity(words.len() * 4);
        for w in words {
            out.extend_from_slice(&w.to_le_bytes());
        }
        Ok(out)
    }
}

pub fn decode_keymap_blob(data: &[u8]) -> Result<FirmwareKeymapBlob, CodecError> {
    if data.len() < 24 || !data.len().is_multiple_of(4) {
        return Err(CodecError(format!(
            "Invalid keymap blob size: {}",
            data.len()
        )));
    }
    let words: Vec<u32> = data
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let num_keymaps = words[0];
    let layout_bitmask = words[1];
    let off0 = words[2];
    let off1 = words[3];
    let off2 = words[4];
    let off3 = words[5];

    if num_keymaps == 0 || num_keymaps > 16 {
        return Err(CodecError(format!("Invalid num_keymaps: {num_keymaps}")));
    }

    let mut pos = 6usize;
    let mut keymaps = Vec::with_capacity(num_keymaps as usize);
    for idx in 0..num_keymaps as usize {
        if pos + 7 > words.len() {
            return Err(CodecError(format!("Truncated keymap[{idx}] header")));
        }
        let n_layers = words[pos];
        let n_keys = words[pos + 1];
        let n_act = words[pos + 2];
        let n_macros = words[pos + 3];
        let r1 = words[pos + 4];
        let keycodes_off = words[pos + 5];
        let actuations_off = words[pos + 6];
        pos += 7;

        if n_keys == 0 || n_keys > 512 || n_layers > 64 || n_act > 64 || n_macros > 512 {
            return Err(CodecError(format!("Invalid keymap[{idx}] dimensions")));
        }
        let key_words = n_keys
            .checked_mul(n_layers)
            .ok_or_else(|| CodecError(format!("Keymap[{idx}] key_words overflow")))?;
        let expected_act_off = keycodes_off
            .checked_add(key_words)
            .ok_or_else(|| CodecError(format!("Keymap[{idx}] keycodes_off overflow")))?;
        if actuations_off != expected_act_off {
            return Err(CodecError(format!(
                "Keymap[{idx}] actuations_off={actuations_off} != {expected_act_off}"
            )));
        }
        if n_macros == 0 && keycodes_off != 0 {
            return Err(CodecError(format!(
                "Keymap[{idx}] n_macros=0 with nonzero keycodes_off={keycodes_off}"
            )));
        }
        if n_macros > 0 && keycodes_off < n_macros {
            return Err(CodecError(format!(
                "Keymap[{idx}] keycodes_off={keycodes_off} < n_macros={n_macros}"
            )));
        }

        let mut macros = Vec::new();
        if keycodes_off > 0 {
            let ko = keycodes_off as usize;
            if pos + ko > words.len() {
                return Err(CodecError(format!("Truncated keymap[{idx}] macro pool")));
            }
            let pool = &words[pos..pos + ko];
            let nm = n_macros as usize;
            let offsets = &pool[..nm];
            let mut m_pos = nm;
            for (m_idx, &moff) in offsets.iter().enumerate() {
                if moff as usize != m_pos {
                    return Err(CodecError(format!(
                        "Keymap[{idx}] macro_offsets[{m_idx}]={moff} != expected {m_pos}"
                    )));
                }
                if m_pos >= pool.len() {
                    return Err(CodecError(format!("Truncated macro[{m_idx}] len")));
                }
                let m_len = pool[m_pos] as usize;
                m_pos += 1;
                if m_pos + m_len > pool.len() {
                    return Err(CodecError(format!("Truncated macro[{m_idx}] steps")));
                }
                macros.push(pool[m_pos..m_pos + m_len].to_vec());
                m_pos += m_len;
            }
            if m_pos != pool.len() {
                return Err(CodecError(format!(
                    "Trailing words in macro pool of keymap[{idx}]"
                )));
            }
            pos += ko;
        }

        let tail = (n_keys * n_layers + n_act * n_keys) as usize;
        if pos + tail > words.len() {
            return Err(CodecError(format!("Truncated keymap[{idx}] body")));
        }
        let mut layers = Vec::with_capacity(n_layers as usize);
        for l in 0..n_layers as usize {
            let slice = &words[pos + l * (n_keys as usize)..pos + (l + 1) * (n_keys as usize)];
            for &code in slice {
                if code != NONE_KEYCODE && ((code >> 28) & 0xF) == 0x1 {
                    let macro_id = code & 0xFFFF;
                    if macro_id >= n_macros {
                        return Err(CodecError(format!(
                            "Keymap[{idx}] layer[{l}] invalid macro_id={macro_id}"
                        )));
                    }
                }
            }
            layers.push(slice.to_vec());
        }
        pos += (n_keys * n_layers) as usize;

        let mut act_tables = Vec::with_capacity(n_act as usize);
        for m in 0..n_act as usize {
            let slice = &words[pos + m * (n_keys as usize)..pos + (m + 1) * (n_keys as usize)];
            act_tables.push(slice.to_vec());
        }
        pos += (n_act * n_keys) as usize;

        keymaps.push(KeymapSection {
            num_actuation_modes: n_act,
            num_keys: n_keys,
            num_layers: n_layers,
            reserved: [n_macros, r1, keycodes_off],
            layers,
            actuation_tables: act_tables,
            macros,
        });
    }

    let blob = FirmwareKeymapBlob {
        num_keymaps,
        layout_bitmask,
        layout_offset_0: off0,
        layout_offset_1: off1,
        layout_offset_2: off2,
        layout_offset_3: off3,
        keymaps,
        trailing_words: words[pos..].to_vec(),
    };
    let expected = blob.recomputed_offsets()?;
    if [off0, off1, off2, off3] != expected {
        return Err(CodecError(format!(
            "Keymap list offsets {:?} != expected {:?}",
            [off0, off1, off2, off3],
            expected
        )));
    }
    Ok(blob)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileSetting {
    pub actuation_mode_index: u32,
    pub default_layer_bitfield: u32,
}

impl ProfileSetting {
    pub fn from_layer(actuation_mode_index: u32, default_layer: u8) -> Result<Self, CodecError> {
        if default_layer > 31 {
            return Err(CodecError(format!(
                "default_layer must be in 0..=31, got {default_layer}"
            )));
        }
        Ok(Self {
            actuation_mode_index,
            default_layer_bitfield: 1u32 << default_layer,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileSettingsFile {
    pub settings: Vec<ProfileSetting>,
    #[serde(default)]
    pub file_version: u32,
    #[serde(default)]
    pub file_type: u32,
}

impl ProfileSettingsFile {
    pub fn to_data_bytes(&self) -> Result<Vec<u8>, CodecError> {
        if self.settings.len() > MAX_PROFILES {
            return Err(CodecError(format!(
                "Maximum {MAX_PROFILES} profiles allowed, got {}",
                self.settings.len()
            )));
        }
        let mut out = Vec::with_capacity(16 + self.settings.len() * 8);
        out.extend_from_slice(&(self.settings.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for s in &self.settings {
            out.extend_from_slice(&s.actuation_mode_index.to_le_bytes());
            out.extend_from_slice(&s.default_layer_bitfield.to_le_bytes());
        }
        Ok(out)
    }

    pub fn to_transfer_bytes(&self) -> Result<Vec<u8>, CodecError> {
        let data = self.to_data_bytes()?;
        let mut out = Vec::with_capacity(16 + data.len());
        out.extend_from_slice(&self.file_version.to_le_bytes());
        out.extend_from_slice(&self.file_type.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&data);
        Ok(out)
    }
}

pub fn decode_profile_settings(
    raw: &[u8],
    has_file_header: bool,
) -> Result<ProfileSettingsFile, CodecError> {
    let mut pos = 0usize;
    let mut file_version = 0u32;
    let mut file_type = FILE_TYPE_PROFILE_SETTINGS_V1;
    if has_file_header {
        if raw.len() < 32 {
            return Err(CodecError("Transfer payload too small".into()));
        }
        file_version = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
        file_type = u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]);
        if file_type != FILE_TYPE_PROFILE_SETTINGS_V1 {
            return Err(CodecError(format!("Unexpected file_type {file_type}")));
        }
        pos = 16;
    } else if raw.len() < 16 {
        return Err(CodecError("Data payload too small".into()));
    }

    let num_profiles =
        u32::from_le_bytes([raw[pos], raw[pos + 1], raw[pos + 2], raw[pos + 3]]) as usize;
    if num_profiles > MAX_PROFILES {
        return Err(CodecError(format!("Invalid num_profiles {num_profiles}")));
    }
    let expected = pos + 16 + num_profiles * 8;
    if raw.len() != expected {
        return Err(CodecError(format!(
            "Size mismatch: expected {expected}, got {}",
            raw.len()
        )));
    }
    let mut settings = Vec::with_capacity(num_profiles);
    for i in 0..num_profiles {
        let off = pos + 16 + i * 8;
        let act = u32::from_le_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]]);
        let lmask = u32::from_le_bytes([raw[off + 4], raw[off + 5], raw[off + 6], raw[off + 7]]);
        settings.push(ProfileSetting {
            actuation_mode_index: act,
            default_layer_bitfield: lmask,
        });
    }
    Ok(ProfileSettingsFile {
        settings,
        file_version,
        file_type,
    })
}

/// Top bits of a consumer-control key code.
pub const CONSUMER_KIND: u32 = 0x4000_0000;

/// The transparent key code: the key does what it does on the layer below.
pub const TRANSPARENT_KEYCODE: u32 = 0x5000_0000;

/// Keyglass's own keyboard functions and their key codes.
pub const KEYGLASS_KEYS: [(&str, u32); 14] = [
    ("KC_OF_BRIGHTNESS_UP", 0x5000_0002),
    ("KC_OF_BRIGHTNESS_DOWN", 0x5000_0003),
    ("KC_OF_PROFILE_CYCLE_NEXT", 0x5000_0004),
    ("KC_OF_PROFILE_CYCLE_PREV", 0x5000_0005),
    ("KC_OF_ACTUATION_MODE_NEXT", 0x5000_0006),
    ("KC_OF_ACTUATION_MODE_PREV", 0x5000_0007),
    ("KC_OF_PLATFORM_LINUX", 0x5000_0008),
    ("KC_OF_PLATFORM_MACOS", 0x5000_0009),
    ("KC_OF_PLATFORM_WINDOWS", 0x5000_000A),
    ("KC_OF_AUTO_PROFILE_LOCK", 0x5000_000B),
    ("KC_OF_START_PAUSE_POMODORO", 0x5000_000C),
    ("KC_OF_RESET_POMODORO", 0x5000_000D),
    ("KC_OF_THEME_CYCLE_NEXT", 0x5000_000E),
    ("KC_OF_THEME_CYCLE_PREV", 0x5000_000F),
];

pub use super::keycodes::CONSUMER_CONTROLS;

pub fn qmk_action_to_firmware_word(action: &serde_json::Value) -> Result<u32, CodecError> {
    let atype = action.get("type").and_then(|v| v.as_u64()).unwrap_or(0);
    if atype == 1 {
        let mid = action
            .get("macroId")
            .or_else(|| action.get("macro_id"))
            .and_then(|v| v.as_u64())
            .ok_or_else(|| CodecError("Macro action (type=1) missing macroId".into()))?;
        if mid > 0xFFFF {
            return Err(CodecError(format!("macroId out of range: {mid}")));
        }
        return Ok(0x1000_0000 | (mid as u32));
    }
    if atype == 2 {
        let lct = action
            .get("layerControlType")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let target = action
            .get("targetLayer")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if target > 0xFF {
            return Err(CodecError(format!("targetLayer out of range: {target}")));
        }
        let sw_type = match lct {
            "momentaryLayer" => 1u32,
            "toggleLayer" => 2u32,
            "switchToLayer" => 3u32,
            other => return Err(CodecError(format!("Unsupported layerControlType: {other}"))),
        };
        return Ok(0x2000_0000 | (sw_type << 24) | (target as u32));
    }
    if atype != 0 {
        return Err(CodecError(format!("Unsupported keyAction type: {atype}")));
    }

    // Encode explicit modifier bitmask if present (u64 bitmask, non-empty object/array rejected unless zero/null)
    let mut mod_bits = 0u32;
    if let Some(m) = action.get("modifiers") {
        if !m.is_null() {
            if let Some(n) = m.as_u64() {
                if n > 0xFFF {
                    return Err(CodecError(format!("modifiers out of range: {n}")));
                }
                mod_bits = n as u32;
            } else if let Some(arr) = m.as_array() {
                if !arr.is_empty() {
                    return Err(CodecError(
                        "Unsupported non-empty modifier array in keyAction".into(),
                    ));
                }
            } else if let Some(obj) = m.as_object() {
                if !obj.is_empty() {
                    return Err(CodecError(
                        "Unsupported modifier object in keyAction".into(),
                    ));
                }
            } else {
                return Err(CodecError(
                    "Unsupported modifiers field in keyAction".into(),
                ));
            }
        }
    }

    let Some(kc) = action.get("keyCode").and_then(|v| v.as_str()) else {
        return Err(CodecError("Missing keyCode in keyAction".into()));
    };
    let w = match crate::codecs::keycodes::canonical_name(kc) {
        "KC_TRANSPARENT" => TRANSPARENT_KEYCODE,
        "KC_NO" => NONE_KEYCODE,
        other => {
            if let Some(usage) = crate::codecs::keycodes::keyboard_usage(other) {
                usage
            } else if let Some((_, w)) = KEYGLASS_KEYS.iter().find(|(name, _)| *name == other) {
                *w
            } else if let Some(usage) = crate::codecs::keycodes::consumer_usage(other) {
                CONSUMER_KIND | usage
            } else if let Some(of) = crate::codecs::keycode_names::keyglass_name(other) {
                return Err(CodecError(format!(
                    "Unsupported keyCode for the firmware keymap: {other} (the factory software's name for {of})"
                )));
            } else {
                return Err(CodecError(format!(
                    "Unsupported keyCode for the firmware keymap: {other}"
                )));
            }
        }
    };
    if mod_bits != 0 {
        if (w >> 28) != 0 && (w >> 28) != 4 {
            return Err(CodecError(
                "Modifiers not supported on firmware/transparent keyCode".into(),
            ));
        }
        return Ok(w | ((mod_bits & 0xFFF) << 16));
    }
    Ok(w)
}

/// The Keyboard Page usage a macro step's key name stands for.
pub fn qmk_keycode_to_hid_usage(kc: &str) -> Result<u32, CodecError> {
    crate::codecs::keycodes::keyboard_usage(kc)
        .ok_or_else(|| CodecError(format!("Unsupported macro keyCode: {kc}")))
}

/// The key name a macro step uses for a Keyboard Page usage. Modifiers take their short names.
pub fn hid_usage_to_qmk_keycode(hid: u32) -> Option<&'static str> {
    match hid {
        0xE0 => Some("KC_LCTL"),
        0xE1 => Some("KC_LSHIFT"),
        0xE2 => Some("KC_LALT"),
        0xE3 => Some("KC_LGUI"),
        0xE4 => Some("KC_RCTL"),
        0xE5 => Some("KC_RSHIFT"),
        0xE6 => Some("KC_RALT"),
        0xE7 => Some("KC_RGUI"),
        u => crate::codecs::keycodes::keyboard_key_name(u),
    }
}

pub fn decode_macros_from_blob(blob_bytes: &[u8]) -> Result<serde_json::Value, CodecError> {
    let blob = decode_keymap_blob(blob_bytes)?;
    if blob.keymaps.is_empty() {
        return Ok(serde_json::Value::Array(Vec::new()));
    }
    let decode_pool = |pool: &[Vec<u32>]| -> Vec<serde_json::Value> {
        let mut out = Vec::with_capacity(pool.len());
        for (idx, mwords) in pool.iter().enumerate() {
            let mut steps = Vec::with_capacity(mwords.len());
            let mut all_mapped = true;
            for &w in mwords {
                // Words with reserved bits 24..30 set are kept raw.
                if (w & 0x7F00_0000) != 0 {
                    all_mapped = false;
                    break;
                }
                let hid = w & 0xFF;
                let delay_ms = (w >> 8) & 0xFFFF;
                let is_down = ((w >> 31) & 1) != 0;
                if let Some(kc) = hid_usage_to_qmk_keycode(hid) {
                    steps.push(serde_json::json!({
                        "keyCode": kc,
                        "delayMs": delay_ms,
                        "isDown": is_down,
                    }));
                } else {
                    all_mapped = false;
                    break;
                }
            }
            if all_mapped {
                out.push(serde_json::json!({
                    "name": format!("Macro {}", idx + 1),
                    "steps": steps,
                }));
            } else {
                out.push(serde_json::json!({
                    "name": format!("Macro {}", idx + 1),
                    "words": mwords,
                }));
            }
        }
        out
    };
    let all_equal = blob.keymaps.windows(2).all(|w| w[0].macros == w[1].macros);
    if all_equal {
        Ok(serde_json::Value::Array(decode_pool(
            &blob.keymaps[0].macros,
        )))
    } else {
        let mut scoped = serde_json::Map::new();
        for sec in &blob.keymaps {
            let lname = if sec.num_keys == 87 { "iso" } else { "ansi" };
            scoped.insert(
                lname.to_string(),
                serde_json::Value::Array(decode_pool(&sec.macros)),
            );
        }
        Ok(serde_json::Value::Object(scoped))
    }
}

fn dominant_table_word(tbl: &[u32]) -> u32 {
    let mut counts: std::collections::HashMap<u32, (usize, usize)> =
        std::collections::HashMap::new();
    for (idx, &w) in tbl.iter().enumerate() {
        counts
            .entry(w)
            .and_modify(|(c, _)| *c += 1)
            .or_insert((1, idx));
    }
    counts
        .into_iter()
        .max_by(|(wa, (ca, ia)), (wb, (cb, ib))| {
            ca.cmp(cb).then_with(|| ib.cmp(ia)).then_with(|| wa.cmp(wb))
        })
        .map(|(w, _)| w)
        .unwrap_or(0)
}

/// Decodes all actuation mode tables from a validated `FirmwareKeymapBlob` (primary layout `keymaps[0]`)
/// into editable JSON objects marked `touched: false`, so untouched modes (and any per-key
/// overrides inside them) re-encode to the same words.
pub fn decode_actuation_modes_from_blob(
    blob_bytes: &[u8],
) -> Result<serde_json::Value, CodecError> {
    let blob = decode_keymap_blob(blob_bytes)?;
    let Some(sec0) = blob.keymaps.first() else {
        return Err(CodecError(
            "FirmwareKeymapBlob has 0 keymap sections".into(),
        ));
    };
    let mut out = Vec::with_capacity(sec0.actuation_tables.len());
    for (idx, tbl0) in sec0.actuation_tables.iter().enumerate() {
        let first = dominant_table_word(tbl0);
        let mut mixed = false;
        let mut scoped_overrides = serde_json::Map::new();
        for sec in &blob.keymaps {
            let lname = if sec.num_keys == 87 { "iso" } else { "ansi" };
            let Some(tbl) = sec.actuation_tables.get(idx) else {
                continue;
            };
            let mut ov_map = serde_json::Map::new();
            for (kidx, &kw) in tbl.iter().enumerate() {
                if kw != first {
                    mixed = true;
                    ov_map.insert(kidx.to_string(), actuation_entry_json(kw));
                }
            }
            if !ov_map.is_empty() {
                scoped_overrides.insert(lname.to_string(), serde_json::Value::Object(ov_map));
            }
        }
        let name = match idx {
            0 => "Typing".to_string(),
            1 => "Gaming".to_string(),
            n => format!("Mode {}", n + 1),
        };
        let mut mode_obj = actuation_entry_json(first);
        let fields = [
            ("id", serde_json::json!(format!("mode_{idx}"))),
            ("name", serde_json::json!(name)),
            (
                "type",
                serde_json::json!(decode_actuation_word(first).actuation_type),
            ),
            ("mixedPerKey", serde_json::json!(mixed)),
            (
                "perKeyOverrides",
                serde_json::Value::Object(scoped_overrides),
            ),
        ];
        for (key, value) in fields {
            mode_obj[key] = value;
        }
        out.push(mode_obj);
    }
    Ok(serde_json::Value::Array(out))
}

/// The keymap document field holding the actuation modes' names by position. The firmware keymap
/// has no names, so they travel in `keymap.json`.
pub const MODE_NAMES_FIELD: &str = "actuationModeNames";

/// `modes` (as [`decode_actuation_modes_from_blob`] gives them) with the names `keymap_doc` keeps
/// for them. A mode without a kept name keeps the name it has.
pub fn name_actuation_modes(
    mut modes: serde_json::Value,
    keymap_doc: &serde_json::Value,
) -> serde_json::Value {
    let names = keymap_doc.get(MODE_NAMES_FIELD).and_then(|v| v.as_array());
    if let (Some(names), Some(arr)) = (names, modes.as_array_mut()) {
        for (mode, name) in arr.iter_mut().zip(names) {
            if let Some(n) = name.as_str().filter(|n| !n.trim().is_empty()) {
                mode["name"] = serde_json::json!(n);
            }
        }
    }
    modes
}

/// Records the names of `modes` in `keymap_doc` under [`MODE_NAMES_FIELD`].
pub fn keep_actuation_mode_names(keymap_doc: &mut serde_json::Value, modes: &serde_json::Value) {
    let (Some(doc), Some(arr)) = (keymap_doc.as_object_mut(), modes.as_array()) else {
        return;
    };
    let names: Vec<serde_json::Value> = arr
        .iter()
        .map(|m| m.get("name").cloned().unwrap_or(serde_json::Value::Null))
        .collect();
    doc.insert(MODE_NAMES_FIELD.into(), serde_json::Value::Array(names));
}

/// Verifies that `keymap_doc`'s layer actions match the decoded `FirmwareKeymapBlob` layer words
/// so a partially saved JSON/blob mismatch cannot be trusted as a paired snapshot.
pub fn verify_keymap_matches_blob(
    keymap_doc: &serde_json::Value,
    blob_bytes: &[u8],
) -> Result<(), CodecError> {
    let reencoded = encode_keymap_json_to_firmware_blob(keymap_doc, None, Some(blob_bytes))?;
    let a = decode_keymap_blob(blob_bytes)?;
    let b = decode_keymap_blob(&reencoded)?;
    if a.keymaps.len() != b.keymaps.len() {
        return Err(CodecError(
            "Mismatched keymap layout count between JSON and firmware blob".into(),
        ));
    }
    let has_explicit_macros = keymap_doc.get("macros").is_some();
    for (sa, sb) in a.keymaps.iter().zip(b.keymaps.iter()) {
        if sa.layers != sb.layers {
            return Err(CodecError(
                "Mismatched layer bindings between keymap.json and firmware blob".into(),
            ));
        }
        if has_explicit_macros && sa.macros != sb.macros {
            return Err(CodecError(
                "Mismatched macros pool between keymap.json and firmware blob".into(),
            ));
        }
    }
    Ok(())
}

/// The `keyActions` entry for a firmware key code: the inverse of
/// [`qmk_action_to_firmware_word`]. A key code the keymap document can't express is an error.
pub fn firmware_word_to_action(word: u32) -> Result<serde_json::Value, CodecError> {
    let unnamed = || {
        CodecError(format!(
            "The firmware keymap has a key code Keyglass can't name: 0x{word:08X}"
        ))
    };
    let key = |name: &str, mods: u32| {
        serde_json::json!({
            "keyCode": name,
            "modifiers": if mods == 0 { serde_json::Value::Null } else { serde_json::json!(mods) },
            "type": 0,
        })
    };
    let action = match word >> 28 {
        0xF if word == NONE_KEYCODE => key("KC_NO", 0),
        0x5 if word == TRANSPARENT_KEYCODE => key("KC_TRANSPARENT", 0),
        0x5 => {
            let (name, _) = KEYGLASS_KEYS
                .iter()
                .find(|(_, w)| *w == word)
                .ok_or_else(unnamed)?;
            key(name, 0)
        }
        0x1 => serde_json::json!({"type": 1, "macroId": word & 0xFFFF}),
        0x2 => {
            let lct = match (word >> 24) & 0xF {
                1 => "momentaryLayer",
                2 => "toggleLayer",
                3 => "switchToLayer",
                _ => return Err(unnamed()),
            };
            serde_json::json!({"type": 2, "layerControlType": lct, "targetLayer": word & 0xFF})
        }
        0x0 | 0x4 => {
            let usage = word & 0xFFFF;
            let mods = (word >> 16) & 0xFFF;
            let name = if word >> 28 == 0x4 {
                crate::codecs::keycodes::consumer_name(usage)
            } else {
                crate::codecs::keycodes::keyboard_key_name(usage)
            };
            key(name.ok_or_else(unnamed)?, mods)
        }
        _ => return Err(unnamed()),
    };
    // Bits the document can't hold (a macro or layer word with stray bits) make the round trip
    // fail; such a key is reported rather than silently changed.
    if qmk_action_to_firmware_word(&action).ok() != Some(word) {
        return Err(unnamed());
    }
    Ok(action)
}

/// Builds the keymap document that describes `blob`, the keymap the firmware stores.
///
/// Everything in `base` that doesn't describe key bindings is kept: layout and layer names, ids
/// and other fields. Each layout section of the blob replaces the bindings of the matching layout
/// (`ansi` for 85 keys, `iso` for 87), layer by layer; layers and layouts the blob doesn't have
/// are dropped. Macros are taken from the blob. The result is checked with
/// [`verify_keymap_matches_blob`], so it is always a document the blob agrees with.
pub fn keymap_json_from_blob(
    blob_bytes: &[u8],
    base: &serde_json::Value,
) -> Result<serde_json::Value, CodecError> {
    let blob = decode_keymap_blob(blob_bytes)?;
    let mut doc = base.as_object().cloned().unwrap_or_default();
    let old_layouts = doc
        .get("keymaps")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let mut layouts = serde_json::Map::new();
    for section in &blob.keymaps {
        let lname = match section.num_keys {
            85 => "ansi",
            87 => "iso",
            n => {
                return Err(CodecError(format!(
                    "The firmware keymap has a layout with {n} keys; Keyglass knows 85 (ANSI) and 87 (ISO)"
                )))
            }
        };
        let mut layout = old_layouts
            .get(lname)
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_else(|| {
                let mut m = serde_json::Map::new();
                m.insert("name".into(), serde_json::json!(lname.to_uppercase()));
                m.insert("defaultLayer".into(), serde_json::json!(0));
                m.insert("frameLayoutType".into(), serde_json::json!(lname));
                m
            });
        let old_layers = layout
            .get("layers")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut layers = Vec::with_capacity(section.layers.len());
        for (i, words) in section.layers.iter().enumerate() {
            let mut layer = old_layers
                .get(i)
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_else(|| {
                    let mut m = serde_json::Map::new();
                    m.insert("frameLayoutType".into(), serde_json::json!(lname));
                    m.insert("id".into(), serde_json::json!(format!("{lname}-layer-{i}")));
                    let name = if i == 0 {
                        "Base".to_string()
                    } else {
                        format!("Layer {i}")
                    };
                    m.insert("name".into(), serde_json::json!(name));
                    m
                });
            let actions = words
                .iter()
                .map(|w| firmware_word_to_action(*w))
                .collect::<Result<Vec<_>, _>>()?;
            layer.insert("keyActions".into(), serde_json::Value::Array(actions));
            layers.push(serde_json::Value::Object(layer));
        }
        layout.insert("layers".into(), serde_json::Value::Array(layers));
        layouts.insert(lname.into(), serde_json::Value::Object(layout));
    }
    let selected_ok = doc
        .get("selectedFrameLayoutType")
        .and_then(|v| v.as_str())
        .is_some_and(|s| layouts.contains_key(s));
    if !selected_ok {
        if let Some(first) = layouts.keys().next().cloned() {
            doc.insert("selectedFrameLayoutType".into(), serde_json::json!(first));
        }
    }
    doc.insert("keymaps".into(), serde_json::Value::Object(layouts));
    let macros = decode_macros_from_blob(blob_bytes)?;
    let has_macros = match &macros {
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(o) => o
            .values()
            .any(|v| v.as_array().is_some_and(|a| !a.is_empty())),
        _ => false,
    };
    doc.insert(
        "macros".into(),
        if has_macros {
            macros
        } else {
            serde_json::json!({})
        },
    );
    let doc = serde_json::Value::Object(doc);
    verify_keymap_matches_blob(&doc, blob_bytes)?;
    Ok(doc)
}

/// Encodes a `keymap.json` document into a validated `FirmwareKeymapBlob` while preserving
/// existing actuation tables / macros from `existing_blob` (when available) or building from `actuation_modes`.
///
/// Every layout gets one actuation table per entry of `actuation_modes`, in that order. An entry
/// that is untouched, or whose per-key entries are, reuses words of the existing table at its
/// position, or at `firmwareTable` when it carries one; `firmwareTable: null` marks a new mode
/// with nothing to reuse.
pub fn encode_keymap_json_to_firmware_blob(
    keymap_doc: &serde_json::Value,
    actuation_modes: Option<&serde_json::Value>,
    existing_blob: Option<&[u8]>,
) -> Result<Vec<u8>, CodecError> {
    let keymaps_obj = keymap_doc
        .get("keymaps")
        .and_then(|v| v.as_object())
        .ok_or_else(|| CodecError("Missing keymaps object".into()))?;
    let existing = existing_blob.and_then(|b| decode_keymap_blob(b).ok());

    let active_layout = keymap_doc
        .get("selectedFrameLayoutType")
        .and_then(|v| v.as_str())
        .unwrap_or("ansi");
    let mut sections = Vec::new();
    let mut bitmask = 0u32;

    for (lid, lname, expected_keys) in [(1u32, "ansi", 85usize), (2u32, "iso", 87usize)] {
        let Some(km_obj) = keymaps_obj.get(lname) else {
            continue;
        };
        let layers_arr = km_obj
            .get("layers")
            .and_then(|v| v.as_array())
            .ok_or_else(|| CodecError(format!("Missing layers in {lname}")))?;
        if layers_arr.is_empty() {
            return Err(CodecError(format!("Layout {lname} has 0 layers")));
        }

        let mut layers_words = Vec::with_capacity(layers_arr.len());
        for lobj in layers_arr {
            let actions = lobj
                .get("keyActions")
                .and_then(|v| v.as_array())
                .ok_or_else(|| CodecError(format!("Missing keyActions in {lname}")))?;
            if actions.len() != expected_keys {
                return Err(CodecError(format!(
                    "Layout {lname} layer has {} keys, expected {expected_keys}",
                    actions.len()
                )));
            }
            let mut words = Vec::with_capacity(expected_keys);
            for act in actions {
                words.push(qmk_action_to_firmware_word(act)?);
            }
            layers_words.push(words);
        }

        // Macros and actuation tables come from the existing blob for the same layout unless the
        // document provides them.
        let ex_sec = existing.as_ref().and_then(|eb| {
            eb.keymaps
                .iter()
                .find(|s| s.num_keys as usize == expected_keys)
        });

        let macro_arr_opt = keymap_doc.get("macros").and_then(|v| {
            v.as_array().or_else(|| {
                v.as_object()
                    .and_then(|o| o.get(lname))
                    .and_then(|x| x.as_array())
            })
        });
        let macros = if let Some(m_arr) = macro_arr_opt {
            let mut mvec = Vec::with_capacity(m_arr.len());
            for mobj in m_arr {
                if let Some(words) = mobj.get("words").and_then(|w| w.as_array()) {
                    let mut raw = Vec::with_capacity(words.len());
                    for x in words {
                        let w =
                            x.as_u64()
                                .and_then(|n| u32::try_from(n).ok())
                                .ok_or_else(|| {
                                    CodecError(format!(
                                        "Invalid macro word: {x} (expected 0..=4294967295)"
                                    ))
                                })?;
                        raw.push(w);
                    }
                    mvec.push(raw);
                } else if let Some(steps) = mobj.get("steps").and_then(|s| s.as_array()) {
                    let mut sw = Vec::with_capacity(steps.len());
                    for st in steps {
                        let kc = st.get("keyCode").and_then(|x| x.as_str()).unwrap_or("KC_A");
                        let hid = qmk_keycode_to_hid_usage(kc)?;
                        let delay_ms = match st.get("delayMs") {
                            None | Some(serde_json::Value::Null) => 10,
                            Some(v) => v
                                .as_u64()
                                .and_then(|n| u32::try_from(n).ok())
                                .ok_or_else(|| CodecError(format!("Invalid macro delayMs: {v}")))?,
                        };
                        let is_down = st.get("isDown").and_then(|x| x.as_bool()).unwrap_or(true);
                        sw.push(encode_macro_step(hid, delay_ms, is_down)?);
                    }
                    mvec.push(sw);
                } else {
                    mvec.push(Vec::new());
                }
            }
            mvec
        } else {
            ex_sec.map(|s| s.macros.clone()).unwrap_or_default()
        };
        let actuation_tables = if let Some(modes_arr) = actuation_modes.and_then(|v| v.as_array()) {
            if !modes_arr.is_empty() {
                let mut tbls = Vec::with_capacity(modes_arr.len());
                for (m_idx, m) in modes_arr.iter().enumerate() {
                    // The existing table this mode was read from: `firmwareTable` when the mode
                    // moved, none when it is new (`firmwareTable: null`), else its position.
                    let ex_tbl_of_mode = ex_sec.and_then(|s| {
                        let src = match m.get("firmwareTable") {
                            None => Some(m_idx),
                            Some(v) => v.as_u64().map(|n| n as usize),
                        };
                        src.and_then(|i| s.actuation_tables.get(i))
                    });
                    // An untouched mode keeps its existing per-key table unchanged.
                    let touched = m.get("touched").and_then(|v| v.as_bool()).unwrap_or(true);
                    if !touched {
                        if let Some(ex_tbl) = ex_tbl_of_mode {
                            tbls.push(ex_tbl.clone());
                            continue;
                        }
                    }
                    let global_touched = m
                        .get("globalTouched")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    let (w, mode_settings) = encode_actuation_entry(
                        m,
                        ActuationDefaults {
                            rapid: false,
                            act_mm: 2.0,
                            sens_mm: 0.3,
                        },
                    )?;
                    let mut row = if !global_touched {
                        if let Some(ex_tbl) = ex_tbl_of_mode {
                            ex_tbl.clone()
                        } else {
                            vec![w; expected_keys]
                        }
                    } else {
                        vec![w; expected_keys]
                    };
                    if let Some(ov_root) = m.get("perKeyOverrides").and_then(|v| v.as_object()) {
                        let ov_map = ov_root.get(lname).and_then(|v| v.as_object()).or_else(|| {
                            if lname == active_layout {
                                Some(ov_root)
                            } else {
                                None
                            }
                        });
                        if let Some(ov_map) = ov_map {
                            for (k_str, ov) in ov_map {
                                if let Ok(kidx) = k_str.parse::<usize>() {
                                    if kidx < expected_keys && ov.is_object() {
                                        let ov_touched = ov
                                            .get("touched")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(true);
                                        if !ov_touched {
                                            if let Some(ex_w) =
                                                ex_tbl_of_mode.and_then(|t| t.get(kidx)).copied()
                                            {
                                                row[kidx] = ex_w;
                                                continue;
                                            }
                                        }
                                        if ov
                                            .get("resetToDefault")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false)
                                        {
                                            row[kidx] = w;
                                            continue;
                                        }
                                        let (kw, _) = encode_actuation_entry(ov, mode_settings)?;
                                        row[kidx] = kw;
                                    }
                                }
                            }
                        }
                    }
                    tbls.push(row);
                }
                tbls
            } else if let Some(s) = ex_sec {
                s.actuation_tables.clone()
            } else {
                vec![
                    vec![encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 400)?; expected_keys],
                    vec![encode_actuation_word(ACTUATION_TRADITIONAL, 1000, 300)?; expected_keys],
                ]
            }
        } else if let Some(s) = ex_sec {
            s.actuation_tables.clone()
        } else {
            vec![
                vec![encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 400)?; expected_keys],
                vec![encode_actuation_word(ACTUATION_TRADITIONAL, 1000, 300)?; expected_keys],
            ]
        };

        bitmask |= 1 << (lid - 1);
        sections.push(KeymapSection {
            num_actuation_modes: actuation_tables.len() as u32,
            num_keys: expected_keys as u32,
            num_layers: layers_words.len() as u32,
            reserved: [0, 0, 0],
            layers: layers_words,
            actuation_tables,
            macros,
        });
    }

    if sections.is_empty() {
        return Err(CodecError("No ANSI or ISO layout found in keymap".into()));
    }

    let blob = FirmwareKeymapBlob {
        num_keymaps: sections.len() as u32,
        layout_bitmask: bitmask,
        layout_offset_0: 0,
        layout_offset_1: 0,
        layout_offset_2: 0,
        layout_offset_3: 0,
        keymaps: sections,
        trailing_words: Vec::new(),
    };
    blob.to_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keymap_actuation_lut_and_macro_roundtrip() {
        assert_eq!(dist_um_to_flux_density(1200), 0x253);
        assert_eq!(flux_density_to_dist_um(0x402), Some(2000));
        let w = encode_actuation_word(ACTUATION_RAPID_TRIGGER, 1200, 300).unwrap();
        assert_eq!(w, (2 << 30) | (0x0DE << 12) | 0x253);

        let macro_step = encode_macro_step(0x04, 15, true).unwrap();
        let blob = FirmwareKeymapBlob {
            num_keymaps: 1,
            layout_bitmask: 1,
            layout_offset_0: 0,
            layout_offset_1: 0,
            layout_offset_2: 0,
            layout_offset_3: 0,
            keymaps: vec![KeymapSection {
                num_actuation_modes: 1,
                num_keys: 2,
                num_layers: 1,
                reserved: [1, 1, 3],
                layers: vec![vec![0x1000_0000, 0x0000_0004]],
                actuation_tables: vec![vec![w, w]],
                macros: vec![vec![macro_step]],
            }],
            trailing_words: vec![],
        };
        let bytes = blob.to_bytes().unwrap();
        let dec = decode_keymap_blob(&bytes).unwrap();
        assert_eq!(dec.keymaps[0].macros, vec![vec![macro_step]]);

        // A corrupt macro pointer is rejected.
        let mut bad = bytes.clone();
        bad[6 * 4 + 7 * 4] = 99;
        assert!(decode_keymap_blob(&bad).is_err());

        // A keycodes offset of u32::MAX is rejected without overflow.
        let mut adv = [0u32; 13];
        adv[0] = 1; // num_keymaps
        adv[1] = 1; // layout_bitmask
        adv[6] = 1; // n_layers
        adv[7] = 1; // n_keys
        adv[8] = 1; // n_act
        adv[9] = 1; // n_macros
        adv[11] = u32::MAX; // keycodes_off
        adv[12] = 0; // actuations_off; u32::MAX + 1 would wrap to 0
        let adv_bytes: Vec<u8> = adv.iter().flat_map(|w| w.to_le_bytes()).collect();
        assert!(decode_keymap_blob(&adv_bytes).is_err());
    }

    const DEFAULT_KEYMAP_JSON: &str = include_str!("../../tests/fixtures/default_keymap.json");
    const DEFAULT_KEYMAP_BIN: &[u8] = include_bytes!("../../tests/fixtures/default_keymap.bin");

    /// The fixtures are Keyglass's default keymap and its firmware encoding. Regenerate them with
    /// `KEYGLASS_REGENERATE_FIXTURES=1 cargo test -p keyglassd default_keymap_fixtures`.
    #[test]
    fn default_keymap_fixtures_match_the_encoder() {
        let km = crate::defaults::default_keymap();
        let json = serde_json::to_string_pretty(&km).unwrap() + "\n";
        let bin = encode_keymap_json_to_firmware_blob(&km, None, None).unwrap();
        if std::env::var("KEYGLASS_REGENERATE_FIXTURES").as_deref() == Ok("1") {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
            std::fs::write(dir.join("default_keymap.json"), &json).unwrap();
            std::fs::write(dir.join("default_keymap.bin"), &bin).unwrap();
            return;
        }
        assert_eq!(
            DEFAULT_KEYMAP_JSON.replace("\r\n", "\n"),
            json,
            "tests/fixtures/default_keymap.json is stale"
        );
        assert_eq!(
            DEFAULT_KEYMAP_BIN,
            bin.as_slice(),
            "tests/fixtures/default_keymap.bin is stale"
        );
    }

    #[test]
    fn keymap_encoding_preserves_tables_macros_and_per_key_overrides() {
        let default_km: serde_json::Value = serde_json::from_str(DEFAULT_KEYMAP_JSON).unwrap();
        let default_bin = DEFAULT_KEYMAP_BIN;

        // 1. Encoding against the existing blob keeps its actuation tables and reproduces it exactly.
        let encoded_pres =
            encode_keymap_json_to_firmware_blob(&default_km, None, Some(default_bin)).unwrap();
        assert_eq!(encoded_pres.as_slice(), default_bin);

        // 2. A fresh encode produces the same ANSI (85) and ISO (87) layers.
        let encoded_fresh = encode_keymap_json_to_firmware_blob(&default_km, None, None).unwrap();
        let dec_fresh = decode_keymap_blob(&encoded_fresh).unwrap();
        let dec_default = decode_keymap_blob(default_bin).unwrap();
        assert_eq!(dec_fresh.keymaps[0].layers, dec_default.keymaps[0].layers);
        assert_eq!(dec_fresh.keymaps[1].layers, dec_default.keymaps[1].layers);

        // 3. Unknown keycodes and modifier objects are rejected.
        let mut bad_km = default_km.clone();
        bad_km["keymaps"]["ansi"]["layers"][0]["keyActions"][0]["keyCode"] =
            serde_json::json!("KC_NONEXISTENT_XYZ");
        assert!(encode_keymap_json_to_firmware_blob(&bad_km, None, None).is_err());

        let mut mod_km = default_km.clone();
        mod_km["keymaps"]["ansi"]["layers"][0]["keyActions"][0]["modifiers"] =
            serde_json::json!({"ctrl": true});
        assert!(encode_keymap_json_to_firmware_blob(&mod_km, None, None).is_err());

        // 4. A key-only edit keeps macros and per-key actuation; Fn and macro keys can be remapped to ordinary keys.
        let macro_step = encode_macro_step(0x04, 10, true).unwrap();
        let ex_blob = FirmwareKeymapBlob {
            num_keymaps: 2,
            layout_bitmask: 3,
            layout_offset_0: 0,
            layout_offset_1: 0,
            layout_offset_2: 0,
            layout_offset_3: 0,
            keymaps: vec![
                KeymapSection {
                    num_actuation_modes: 1,
                    num_keys: 85,
                    num_layers: 2,
                    reserved: [1, 1, 3],
                    layers: vec![vec![0x04; 85], vec![0x5000_0000; 85]],
                    actuation_tables: vec![(0..85)
                        .map(|i| if i == 33 { 0x800f_e150 } else { 0x400f_e253 })
                        .collect()],
                    macros: vec![vec![macro_step]],
                },
                KeymapSection {
                    num_actuation_modes: 1,
                    num_keys: 87,
                    num_layers: 2,
                    reserved: [0, 0, 0],
                    layers: vec![vec![0x04; 87], vec![0x5000_0000; 87]],
                    actuation_tables: vec![vec![0x400f_e253; 87]],
                    macros: Vec::new(),
                },
            ],
            trailing_words: Vec::new(),
        };
        let ex_bytes = ex_blob.to_bytes().unwrap();
        let mut macro_km = default_km.clone();
        macro_km["keymaps"]["ansi"]["layers"][0]["keyActions"][1] =
            serde_json::json!({"type": 1, "macroId": 0});
        macro_km["keymaps"]["ansi"]["layers"][0]["keyActions"][2]["modifiers"] =
            serde_json::json!(1); // LCtrl
        let re_bytes =
            encode_keymap_json_to_firmware_blob(&macro_km, None, Some(&ex_bytes)).unwrap();
        let re_dec = decode_keymap_blob(&re_bytes).unwrap();
        assert_eq!(re_dec.keymaps[0].macros, vec![vec![macro_step]]);
        assert_eq!(re_dec.keymaps[0].actuation_tables[0][33], 0x800f_e150);
        assert_eq!(re_dec.keymaps[0].layers[0][1], 0x1000_0000);
        assert_eq!(re_dec.keymaps[0].layers[0][2] >> 16, 1);

        macro_km["keymaps"]["ansi"]["layers"][0]["keyActions"][1] =
            serde_json::json!({"type": 0, "keyCode": "KC_A"});
        macro_km["keymaps"]["ansi"]["layers"][0]["keyActions"][64] =
            serde_json::json!({"type": 0, "keyCode": "KC_A"});
        let remap_bytes =
            encode_keymap_json_to_firmware_blob(&macro_km, None, Some(&re_bytes)).unwrap();
        let remap_dec = decode_keymap_blob(&remap_bytes).unwrap();
        assert_eq!(remap_dec.keymaps[0].layers[0][1], 0x04);
        assert_eq!(remap_dec.keymaps[0].layers[0][64], 0x04);

        // 5. Editing mode 0 of three leaves modes 1 and 2 unchanged.
        let mut blob3 = ex_blob.clone();
        let rt_wasd = encode_actuation_word(ACTUATION_RAPID_TRIGGER, 800, 200).unwrap();
        let mode2_word = encode_actuation_word(ACTUATION_TRADITIONAL, 1500, 500).unwrap();
        let mut gaming_tbl =
            vec![encode_actuation_word(ACTUATION_TRADITIONAL, 1000, 300).unwrap(); 85];
        gaming_tbl[33] = rt_wasd;
        blob3.keymaps[0].actuation_tables = vec![
            vec![encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 400).unwrap(); 85],
            gaming_tbl.clone(),
            vec![mode2_word; 85],
        ];
        blob3.keymaps[0].num_actuation_modes = 3;
        let b3_bytes = blob3.to_bytes().unwrap();
        let mut decoded_modes = decode_actuation_modes_from_blob(&b3_bytes).unwrap();
        assert_eq!(decoded_modes.as_array().unwrap().len(), 3);
        assert_eq!(decoded_modes[1]["mixedPerKey"], serde_json::json!(true));
        decoded_modes[0]["actuationPointMm"] = serde_json::json!(1.6);
        decoded_modes[0]["touched"] = serde_json::json!(true);
        let b3_next =
            encode_keymap_json_to_firmware_blob(&default_km, Some(&decoded_modes), Some(&b3_bytes))
                .unwrap();
        let dec3_next = decode_keymap_blob(&b3_next).unwrap();
        assert_eq!(dec3_next.keymaps[0].actuation_tables.len(), 3);
        assert_eq!(
            dec3_next.keymaps[0].actuation_tables[0][0],
            encode_actuation_word(ACTUATION_TRADITIONAL, 1600, 400).unwrap()
        );
        assert_eq!(dec3_next.keymaps[0].actuation_tables[1], gaming_tbl);
        assert_eq!(
            dec3_next.keymaps[0].actuation_tables[2],
            vec![mode2_word; 85]
        );

        // 6. Rapid trigger at 1.2 mm with 0.3 mm sensitivity decodes to the same values.
        let mut rt_blob = ex_blob.clone();
        let rt_w = encode_actuation_word(ACTUATION_RAPID_TRIGGER, 1200, 300).unwrap();
        rt_blob.keymaps[0].actuation_tables = vec![vec![rt_w; 85]];
        rt_blob.keymaps[0].num_actuation_modes = 1;
        let rt_dec = decode_actuation_modes_from_blob(&rt_blob.to_bytes().unwrap()).unwrap();
        assert_eq!(rt_dec[0]["rapidTrigger"], serde_json::json!(true));
        assert_eq!(
            rt_dec[0]["type"],
            serde_json::json!(ACTUATION_RAPID_TRIGGER)
        );
        assert_eq!(rt_dec[0]["actuationPointMm"], serde_json::json!(1.2));
        assert_eq!(
            rt_dec[0]["rapidTriggerSensitivityMm"],
            serde_json::json!(0.3)
        );

        // 7. A per-key override (globalTouched=false) keeps existing overrides such as key 10 and every
        //    other key, and an ANSI override doesn't touch the ISO layout.
        let mut seeded_dec = decode_keymap_blob(default_bin).unwrap();
        let pre_trad_10 = encode_actuation_word(ACTUATION_TRADITIONAL, 1000, 300).unwrap();
        let pre_iso_5 = encode_actuation_word(ACTUATION_TRADITIONAL, 1500, 500).unwrap();
        seeded_dec.keymaps[0].actuation_tables[0][10] = pre_trad_10;
        seeded_dec.keymaps[1].actuation_tables[0][5] = pre_iso_5;
        let seeded_bin = seeded_dec.to_bytes().unwrap();

        let mut pk_modes = decode_actuation_modes_from_blob(&seeded_bin).unwrap();
        pk_modes[0]["touched"] = serde_json::json!(true);
        pk_modes[0]["globalTouched"] = serde_json::json!(false);
        // Decoded overrides (key 10, touched=false) stay; key 34 is added with touched=true.
        pk_modes[0]["perKeyOverrides"]["ansi"]["34"] = serde_json::json!({
            "actuationPointMm": 0.4, "rapidTrigger": true, "rapidTriggerSensitivityMm": 0.2, "touched": true
        });
        let mut iso_selected_km = default_km.clone();
        iso_selected_km["selectedFrameLayoutType"] = serde_json::json!("iso");
        let pk_bytes = encode_keymap_json_to_firmware_blob(
            &iso_selected_km,
            Some(&pk_modes),
            Some(&seeded_bin),
        )
        .unwrap();
        let pk_dec = decode_keymap_blob(&pk_bytes).unwrap();
        assert_eq!(
            pk_dec.keymaps[1], seeded_dec.keymaps[1],
            "an ANSI per-key override changed the ISO layout"
        );
        for i in 0..85 {
            if i == 34 {
                assert_eq!(
                    pk_dec.keymaps[0].actuation_tables[0][i],
                    encode_actuation_word(ACTUATION_RAPID_TRIGGER, 400, 200).unwrap()
                );
            } else {
                assert_eq!(
                    pk_dec.keymaps[0].actuation_tables[0][i],
                    seeded_dec.keymaps[0].actuation_tables[0][i],
                    "untouched key {i} changed"
                );
            }
        }

        // 7b. resetToDefault on key 10 restores the mode default for that key only.
        let mut reset_modes = decode_actuation_modes_from_blob(&pk_bytes).unwrap();
        reset_modes[0]["touched"] = serde_json::json!(true);
        reset_modes[0]["globalTouched"] = serde_json::json!(false);
        reset_modes[0]["perKeyOverrides"]["ansi"]["10"] = serde_json::json!({
            "actuationPointMm": 1.2, "rapidTrigger": false, "resetToDefault": true, "touched": true
        });
        let rst_bytes =
            encode_keymap_json_to_firmware_blob(&default_km, Some(&reset_modes), Some(&pk_bytes))
                .unwrap();
        let rst_dec = decode_keymap_blob(&rst_bytes).unwrap();
        assert_eq!(rst_dec.keymaps[1], seeded_dec.keymaps[1]);
        assert_eq!(
            rst_dec.keymaps[0].actuation_tables[0][10],
            encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 400).unwrap()
        );
        assert_eq!(
            rst_dec.keymaps[0].actuation_tables[0][34],
            encode_actuation_word(ACTUATION_RAPID_TRIGGER, 400, 200).unwrap()
        );

        // 7c. A global edit (globalTouched=true) moves ordinary keys to the new default and keeps
        //     untouched overrides.
        let mut glob_modes = decode_actuation_modes_from_blob(&pk_bytes).unwrap();
        glob_modes[0]["touched"] = serde_json::json!(true);
        glob_modes[0]["globalTouched"] = serde_json::json!(true);
        glob_modes[0]["actuationPointMm"] = serde_json::json!(1.8);
        let glob_bytes =
            encode_keymap_json_to_firmware_blob(&default_km, Some(&glob_modes), Some(&pk_bytes))
                .unwrap();
        let glob_dec = decode_keymap_blob(&glob_bytes).unwrap();
        assert_eq!(
            glob_dec.keymaps[0].actuation_tables[0][0],
            encode_actuation_word(ACTUATION_TRADITIONAL, 1800, 400).unwrap()
        );
        assert_eq!(
            glob_dec.keymaps[0].actuation_tables[0][34],
            encode_actuation_word(ACTUATION_RAPID_TRIGGER, 400, 200).unwrap()
        );
        assert_eq!(glob_dec.keymaps[0].actuation_tables[0][10], pre_trad_10);

        // 7d. Without an existing blob, perKeyOverrides apply even when touched=false.
        let json_only_modes = decode_actuation_modes_from_blob(&pk_bytes).unwrap();
        let json_only_bytes =
            encode_keymap_json_to_firmware_blob(&default_km, Some(&json_only_modes), None).unwrap();
        let json_only_dec = decode_keymap_blob(&json_only_bytes).unwrap();
        assert_eq!(
            json_only_dec.keymaps[0].actuation_tables[0][34],
            encode_actuation_word(ACTUATION_RAPID_TRIGGER, 400, 200).unwrap(),
            "perKeyOverrides with touched=false must still encode when no existing blob is present"
        );

        // 7e. With 84 keys at 2.0 mm and Esc at 0.4 mm rapid trigger, 2.0 mm is the mode baseline and only
        //     key 0 is an override.
        let mut esc_blob = ex_blob.clone();
        let base_20 = encode_actuation_word(ACTUATION_TRADITIONAL, 2000, 1800).unwrap();
        let esc_04 = encode_actuation_word(ACTUATION_RAPID_TRIGGER, 400, 200).unwrap();
        esc_blob.keymaps[0].actuation_tables[0] = vec![base_20; 85];
        esc_blob.keymaps[0].actuation_tables[0][0] = esc_04;
        let esc_bytes = esc_blob.to_bytes().unwrap();
        let mut esc_dec = decode_actuation_modes_from_blob(&esc_bytes).unwrap();
        assert_eq!(esc_dec[0]["actuationPointMm"], serde_json::json!(2.0));
        assert_eq!(esc_dec[0]["rapidTrigger"], serde_json::json!(false));
        assert!(esc_dec[0]["perKeyOverrides"]["ansi"]["0"].is_object());
        assert!(esc_dec[0]["perKeyOverrides"]["ansi"]["1"].is_null());
        esc_dec[0]["touched"] = serde_json::json!(true);
        esc_dec[0]["globalTouched"] = serde_json::json!(false);
        esc_dec[0]["perKeyOverrides"]["ansi"]["0"]["resetToDefault"] = serde_json::json!(true);
        esc_dec[0]["perKeyOverrides"]["ansi"]["0"]["touched"] = serde_json::json!(true);
        let esc_rst =
            encode_keymap_json_to_firmware_blob(&default_km, Some(&esc_dec), Some(&esc_bytes))
                .unwrap();
        let esc_rst_dec = decode_keymap_blob(&esc_rst).unwrap();
        assert_eq!(esc_rst_dec.keymaps[0].actuation_tables[0][0], base_20);

        // 8. Fixed words whose release reading lies between 0.1 mm steps decode to the nearest steps
        //    and re-encode to the same words, even without the original blob.
        let between_uni = 0x400f_e253; // A 0x253 (1.2 mm), R 254: releases at 341, near 0.6 mm
        let between_2000 = (1 << 30) | (100 << 12) | 0x402; // releases at 926, near 1.8 mm
        let mut fl_blob = ex_blob.clone();
        fl_blob.keymaps[0].actuation_tables[0] = vec![between_uni; 85];
        fl_blob.keymaps[0].actuation_tables[0][12] = between_2000;
        let fl_bytes = fl_blob.to_bytes().unwrap();
        let fl_modes = decode_actuation_modes_from_blob(&fl_bytes).unwrap();
        assert_eq!(fl_modes[0]["actuationPointMm"], serde_json::json!(1.2));
        assert_eq!(fl_modes[0]["releasePointMm"], serde_json::json!(0.6));
        assert_eq!(fl_modes[0]["firmwareWord"], serde_json::json!(between_uni));
        let fl_key = &fl_modes[0]["perKeyOverrides"]["ansi"]["12"];
        assert_eq!(fl_key["actuationPointMm"], serde_json::json!(2.0));
        assert_eq!(fl_key["releasePointMm"], serde_json::json!(1.8));
        let fl_reenc =
            encode_keymap_json_to_firmware_blob(&default_km, Some(&fl_modes), None).unwrap();
        let fl_redec = decode_keymap_blob(&fl_reenc).unwrap();
        assert_eq!(fl_redec.keymaps[0].actuation_tables[0][0], between_uni);
        assert_eq!(fl_redec.keymaps[0].actuation_tables[0][12], between_2000);

        // 7f. ANSI at 2.0 mm and ISO at 1.0 mm: ISO is recorded as overrides against the 2.0 mm baseline,
        //     so encoding without an existing blob keeps ISO at 1.0 mm.
        let mut div_act_blob = ex_blob.clone();
        let w_20 = encode_actuation_word(ACTUATION_TRADITIONAL, 2000, 1800).unwrap();
        let w_10 = encode_actuation_word(ACTUATION_TRADITIONAL, 1000, 800).unwrap();
        div_act_blob.keymaps[0].actuation_tables[0] = vec![w_20; 85];
        div_act_blob.keymaps[1].actuation_tables[0] = vec![w_10; 87];
        let div_act_modes =
            decode_actuation_modes_from_blob(&div_act_blob.to_bytes().unwrap()).unwrap();
        let div_json_only =
            encode_keymap_json_to_firmware_blob(&default_km, Some(&div_act_modes), None).unwrap();
        let div_json_dec = decode_keymap_blob(&div_json_only).unwrap();
        assert_eq!(div_json_dec.keymaps[0].actuation_tables[0], vec![w_20; 85]);
        assert_eq!(div_json_dec.keymaps[1].actuation_tables[0], vec![w_10; 87]);

        // 8b. Different ANSI and ISO macro pools, and reserved bits 24..30, survive decode and re-encode.
        let mut div_blob = ex_blob.clone();
        div_blob.keymaps[0].macros = vec![vec![
            encode_macro_step(0x06, 10, true).unwrap() | (0x05 << 24),
        ]];
        div_blob.keymaps[1].macros = vec![vec![encode_macro_step(0x07, 15, true).unwrap()]];
        let div_bytes = div_blob.to_bytes().unwrap();
        let decoded_div = decode_macros_from_blob(&div_bytes).unwrap();
        assert!(
            decoded_div.is_object(),
            "divergent pools must decode layout-scoped, not global"
        );
        assert!(
            decoded_div["ansi"][0]["words"].is_array(),
            "reserved bits 24..30 must stay raw words"
        );
        let mut div_km = default_km.clone();
        div_km["macros"] = decoded_div;
        div_km["keymaps"]["ansi"]["layers"][0]["keyActions"][0]["keyCode"] =
            serde_json::json!("KC_B");
        let re_div_bytes =
            encode_keymap_json_to_firmware_blob(&div_km, None, Some(&div_bytes)).unwrap();
        let re_div_dec = decode_keymap_blob(&re_div_bytes).unwrap();
        assert_eq!(re_div_dec.keymaps[0].macros, div_blob.keymaps[0].macros);
        assert_eq!(re_div_dec.keymaps[1].macros, div_blob.keymaps[1].macros);

        // 9. The pairing check rejects different macro steps when the document provides macros.
        assert!(verify_keymap_matches_blob(&macro_km, &remap_bytes).is_ok());
        let mut macro_mismatch = macro_km.clone();
        macro_mismatch["macros"] = serde_json::json!([{
            "name": "Wrong",
            "steps": [{"keyCode": "KC_B", "delayMs": 10, "isDown": true}]
        }]);
        assert!(verify_keymap_matches_blob(&macro_mismatch, &remap_bytes).is_err());
    }

    #[test]
    fn fixed_words_store_the_release_depth_below_actuation() {
        // 1.2 mm actuation releasing at 1.0 mm: R = 0x253 - 0x1FD.
        let w = encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 1000).unwrap();
        assert_eq!(w, (1 << 30) | ((0x253 - 0x1FD) << 12) | 0x253);
        // Every actuation and release step round trips exactly.
        for act_um in (100..=2800).step_by(100) {
            for release_um in (0..act_um).step_by(100) {
                let w = encode_actuation_word(ACTUATION_TRADITIONAL, act_um, release_um).unwrap();
                let a = w & 0xFFF;
                let r = (w >> 12) & 0xFFF;
                assert_eq!(a, dist_um_to_flux_density(act_um));
                assert_eq!(a - r, dist_um_to_flux_density(release_um));
                let d = decode_actuation_word(w);
                assert_eq!(
                    (d.actuation_type, d.actuation_um, d.second_um),
                    (ACTUATION_TRADITIONAL, act_um, release_um),
                    "word 0x{w:08x}"
                );
                assert!(d.exact && !d.firmware_default);
            }
        }
    }

    #[test]
    fn fixed_release_depth_is_clamped_to_zero_through_the_actuation_reading() {
        let act_d = dist_um_to_flux_density(1200);
        // A release point at or below the actuation point gives zero hysteresis.
        for release_um in [1200, 1500, 2800, 9000] {
            let w = encode_actuation_word(ACTUATION_TRADITIONAL, 1200, release_um).unwrap();
            assert_eq!(w, (1 << 30) | act_d, "release {release_um}");
        }
        // The top of travel releases at a zero reading: R = A.
        let w = encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 0).unwrap();
        assert_eq!((w >> 12) & 0xFFF, act_d);

        let km: serde_json::Value = serde_json::from_str(DEFAULT_KEYMAP_JSON).unwrap();
        let word_for = |mode: serde_json::Value| {
            let blob =
                encode_keymap_json_to_firmware_blob(&km, Some(&serde_json::json!([mode])), None)
                    .unwrap();
            decode_keymap_blob(&blob).unwrap().keymaps[0].actuation_tables[0][0]
        };
        assert_eq!(
            word_for(serde_json::json!({"actuationPointMm": 1.2, "releasePointMm": 1.6})),
            (1 << 30) | act_d
        );
        assert_eq!(
            word_for(serde_json::json!({"actuationPointMm": 1.2, "releasePointMm": -0.5})),
            encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 0).unwrap()
        );
        assert_eq!(
            word_for(serde_json::json!({"actuationPointMm": 1.2, "releasePointMm": 0.9})),
            encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 900).unwrap()
        );
        // Without a release point, a fixed mode releases 0.2 mm above actuation.
        assert_eq!(
            word_for(serde_json::json!({"actuationPointMm": 2.0, "rapidTrigger": false})),
            encode_actuation_word(ACTUATION_TRADITIONAL, 2000, 1800).unwrap()
        );
        assert_eq!(
            word_for(serde_json::json!({"actuationPointMm": 0.1})),
            encode_actuation_word(ACTUATION_TRADITIONAL, 100, 0).unwrap()
        );
        // The default configuration's fixed mode.
        let defaults: serde_json::Value =
            serde_json::from_str(crate::defaults::DEFAULT_CONFIG_JSON).unwrap();
        assert_eq!(
            word_for(defaults["actuationModes"][0].clone()),
            encode_actuation_word(ACTUATION_TRADITIONAL, 2000, 1800).unwrap()
        );
    }

    #[test]
    fn type_0_and_3_words_decode_as_the_firmware_default() {
        for word in [0u32, 0x3000_0000, 0xC000_0000, 0xC012_3456, 0x0012_3456] {
            let d = decode_actuation_word(word);
            assert_eq!(d.actuation_type, ACTUATION_TRADITIONAL, "word 0x{word:08x}");
            assert_eq!(d.actuation_density, DEFAULT_FIXED_ACTUATION_DENSITY);
            assert_eq!(d.second_density, DEFAULT_FIXED_RELEASE_DEPTH);
            // 650 is nearest 1.3 mm; the release reading 434 is nearest 0.8 mm.
            assert_eq!((d.actuation_um, d.second_um), (1300, 800));
            assert!(d.firmware_default && !d.exact);
        }

        let km: serde_json::Value = serde_json::from_str(DEFAULT_KEYMAP_JSON).unwrap();
        let mut blob = decode_keymap_blob(DEFAULT_KEYMAP_BIN).unwrap();
        for sec in &mut blob.keymaps {
            sec.actuation_tables = vec![vec![0; sec.num_keys as usize]];
            sec.actuation_tables[0][3] = 0xC000_0000;
            sec.num_actuation_modes = 1;
        }
        let bytes = blob.to_bytes().unwrap();
        let mut modes = decode_actuation_modes_from_blob(&bytes).unwrap();
        let mode = &modes[0];
        assert_eq!(mode["type"], serde_json::json!(ACTUATION_TRADITIONAL));
        assert_eq!(mode["rapidTrigger"], serde_json::json!(false));
        assert_eq!(mode["actuationPointMm"], serde_json::json!(1.3));
        assert_eq!(mode["releasePointMm"], serde_json::json!(0.8));
        assert_eq!(mode["firmwareDefault"], serde_json::json!(true));
        assert_eq!(mode["firmwareWord"], serde_json::json!(0));
        assert_eq!(
            mode["perKeyOverrides"]["ansi"]["3"]["firmwareWord"],
            serde_json::json!(0xC000_0000u32)
        );

        // Re-encoding the unchanged modes, even as edited ones and without the blob, keeps the words.
        modes[0]["touched"] = serde_json::json!(true);
        modes[0]["perKeyOverrides"]["ansi"]["3"]["touched"] = serde_json::json!(true);
        let again = encode_keymap_json_to_firmware_blob(&km, Some(&modes), None).unwrap();
        let again = decode_keymap_blob(&again).unwrap();
        assert_eq!(
            again.keymaps[0].actuation_tables,
            blob.keymaps[0].actuation_tables
        );

        // A changed value encodes from the travel settings.
        modes[0]["actuationPointMm"] = serde_json::json!(1.5);
        let edited = encode_keymap_json_to_firmware_blob(&km, Some(&modes), Some(&bytes)).unwrap();
        let edited = decode_keymap_blob(&edited).unwrap();
        assert_eq!(
            edited.keymaps[0].actuation_tables[0][0],
            encode_actuation_word(ACTUATION_TRADITIONAL, 1500, 800).unwrap()
        );
        assert_eq!(edited.keymaps[0].actuation_tables[0][3], 0xC000_0000);
    }

    #[test]
    fn re_encoding_unedited_actuation_entries_keeps_their_words() {
        let km: serde_json::Value = serde_json::from_str(DEFAULT_KEYMAP_JSON).unwrap();
        let mut blob = decode_keymap_blob(DEFAULT_KEYMAP_BIN).unwrap();
        let words = [
            0x400f_e253, // fixed, release between steps
            encode_actuation_word(ACTUATION_TRADITIONAL, 2000, 1800).unwrap(),
            encode_actuation_word(ACTUATION_RAPID_TRIGGER, 1200, 300).unwrap(),
            (2 << 30) | (0x0C0 << 12) | 0x250, // rapid trigger between steps
            (1 << 30) | 0x253,                 // zero hysteresis
        ];
        for sec in &mut blob.keymaps {
            let n = sec.num_keys as usize;
            sec.actuation_tables = words
                .iter()
                .map(|&w| {
                    let mut t = vec![w; n];
                    t[7] = words[(words.iter().position(|&x| x == w).unwrap() + 1) % words.len()];
                    t
                })
                .collect();
            sec.num_actuation_modes = words.len() as u32;
        }
        let bytes = blob.to_bytes().unwrap();
        let decoded = decode_actuation_modes_from_blob(&bytes).unwrap();
        assert_eq!(decoded[4]["releasePointMm"], serde_json::json!(1.2));

        // Untouched, touched-but-unchanged and blob-less encodes all give the same tables.
        let mut touched = decoded.clone();
        for m in touched.as_array_mut().unwrap() {
            m["touched"] = serde_json::json!(true);
            m["globalTouched"] = serde_json::json!(true);
            for ov in m["perKeyOverrides"]["ansi"]
                .as_object_mut()
                .unwrap()
                .values_mut()
            {
                ov["touched"] = serde_json::json!(true);
            }
        }
        for (modes, existing) in [
            (&decoded, Some(bytes.as_slice())),
            (&touched, Some(bytes.as_slice())),
            (&decoded, None),
            (&touched, None),
        ] {
            let out = encode_keymap_json_to_firmware_blob(&km, Some(modes), existing).unwrap();
            let out = decode_keymap_blob(&out).unwrap();
            for (sec, orig) in out.keymaps.iter().zip(&blob.keymaps) {
                assert_eq!(sec.actuation_tables, orig.actuation_tables);
            }
        }

        // A carried word that no longer matches its travel settings is replaced.
        let mut moved = touched.clone();
        moved[0]["releasePointMm"] = serde_json::json!(0.9);
        let out = encode_keymap_json_to_firmware_blob(&km, Some(&moved), Some(&bytes)).unwrap();
        assert_eq!(
            decode_keymap_blob(&out).unwrap().keymaps[0].actuation_tables[0][0],
            encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 900).unwrap()
        );
    }

    #[test]
    fn decoded_traditional_modes_keep_their_release_points_without_the_original_blob() {
        let default_km: serde_json::Value = serde_json::from_str(DEFAULT_KEYMAP_JSON).unwrap();
        let mut blob = decode_keymap_blob(DEFAULT_KEYMAP_BIN).unwrap();
        let mode = encode_actuation_word(ACTUATION_TRADITIONAL, 1200, 400).unwrap();
        let key = encode_actuation_word(ACTUATION_TRADITIONAL, 1500, 500).unwrap();
        for sec in &mut blob.keymaps {
            let n = sec.num_keys as usize;
            let mut tbl = vec![mode; n];
            tbl[5] = key;
            sec.actuation_tables = vec![tbl];
            sec.num_actuation_modes = 1;
        }
        let bytes = blob.to_bytes().unwrap();
        let modes = decode_actuation_modes_from_blob(&bytes).unwrap();
        let fresh = encode_keymap_json_to_firmware_blob(&default_km, Some(&modes), None).unwrap();
        let dec = decode_keymap_blob(&fresh).unwrap();
        for (sec, orig) in dec.keymaps.iter().zip(&blob.keymaps) {
            assert_eq!(sec.actuation_tables, orig.actuation_tables);
        }
    }

    #[test]
    fn invalid_macro_values_are_refused_rather_than_truncated() {
        let mut km: serde_json::Value = serde_json::from_str(DEFAULT_KEYMAP_JSON).unwrap();
        km["macros"] = serde_json::json!([{"words": [4_294_967_306u64]}]);
        assert!(encode_keymap_json_to_firmware_blob(&km, None, None).is_err());
        km["macros"] = serde_json::json!([{"words": ["4"]}]);
        assert!(encode_keymap_json_to_firmware_blob(&km, None, None).is_err());
        km["macros"] = serde_json::json!([{
            "steps": [{"keyCode": "KC_A", "delayMs": 4_294_967_306u64, "isDown": true}]
        }]);
        assert!(encode_keymap_json_to_firmware_blob(&km, None, None).is_err());
        km["macros"] = serde_json::json!([{
            "steps": [{"keyCode": "KC_A", "delayMs": 10, "isDown": true}]
        }]);
        assert!(encode_keymap_json_to_firmware_blob(&km, None, None).is_ok());
    }

    #[test]
    fn profile_settings_roundtrip_and_limit() {
        let pf = ProfileSettingsFile {
            settings: vec![
                ProfileSetting::from_layer(0, 0).unwrap(),
                ProfileSetting::from_layer(1, 2).unwrap(),
            ],
            file_version: 0,
            file_type: 0,
        };
        let t = pf.to_transfer_bytes().unwrap();
        let dec = decode_profile_settings(&t, true).unwrap();
        assert_eq!(dec, pf);

        let too_many = ProfileSettingsFile {
            settings: vec![ProfileSetting::from_layer(0, 0).unwrap(); MAX_PROFILES + 1],
            file_version: 0,
            file_type: 0,
        };
        assert!(too_many.to_data_bytes().is_err());
    }

    #[test]
    fn the_default_keymap_is_rebuilt_from_its_blob_key_for_key() {
        let default = crate::defaults::default_keymap();
        let blob = encode_keymap_json_to_firmware_blob(&default, None, None).unwrap();
        let rebuilt = keymap_json_from_blob(&blob, &serde_json::json!({})).unwrap();
        for layout in ["ansi", "iso"] {
            let want = default["keymaps"][layout]["layers"].as_array().unwrap();
            let got = rebuilt["keymaps"][layout]["layers"].as_array().unwrap();
            assert_eq!(got.len(), want.len(), "{layout}");
            for (g, w) in got.iter().zip(want) {
                assert_eq!(g["keyActions"], w["keyActions"], "{layout}");
            }
        }
        assert_eq!(rebuilt["selectedFrameLayoutType"], "ansi");
        assert!(verify_keymap_matches_blob(&rebuilt, &blob).is_ok());
    }

    #[test]
    fn a_rebuilt_keymap_keeps_the_documents_names_and_drops_what_the_blob_lacks() {
        let default = crate::defaults::default_keymap();
        let mut ansi_only = default.clone();
        ansi_only["keymaps"].as_object_mut().unwrap().remove("iso");
        // Shift + A on the first key, a macro on the second, toggle layer 1 on the third.
        ansi_only["keymaps"]["ansi"]["layers"][0]["keyActions"][0] =
            serde_json::json!({"keyCode": "KC_A", "modifiers": 2, "type": 0});
        ansi_only["keymaps"]["ansi"]["layers"][0]["keyActions"][1] =
            serde_json::json!({"type": 1, "macroId": 0});
        ansi_only["keymaps"]["ansi"]["layers"][0]["keyActions"][2] =
            serde_json::json!({"type": 2, "layerControlType": "toggleLayer", "targetLayer": 1});
        ansi_only["macros"] = serde_json::json!([{"name": "Macro 1", "steps": [
            {"keyCode": "KC_B", "delayMs": 5, "isDown": true},
            {"keyCode": "KC_B", "delayMs": 5, "isDown": false},
        ]}]);
        let blob = encode_keymap_json_to_firmware_blob(&ansi_only, None, None).unwrap();

        let mut base = default.clone();
        base["keymaps"]["ansi"]["layers"][0]["name"] = serde_json::json!("Mine");
        base["selectedFrameLayoutType"] = serde_json::json!("iso");
        let rebuilt = keymap_json_from_blob(&blob, &base).unwrap();
        let layer = &rebuilt["keymaps"]["ansi"]["layers"][0];
        assert_eq!(layer["name"], "Mine");
        assert_eq!(layer["id"], default["keymaps"]["ansi"]["layers"][0]["id"]);
        assert_eq!(
            layer["keyActions"][0],
            serde_json::json!({"keyCode": "KC_A", "modifiers": 2, "type": 0})
        );
        assert_eq!(
            layer["keyActions"][1],
            serde_json::json!({"type": 1, "macroId": 0})
        );
        assert_eq!(layer["keyActions"][2]["layerControlType"], "toggleLayer");
        assert!(rebuilt["keymaps"].get("iso").is_none());
        assert_eq!(rebuilt["selectedFrameLayoutType"], "ansi");
        assert_eq!(rebuilt["macros"][0]["steps"][0]["keyCode"], "KC_B");
        assert!(verify_keymap_matches_blob(&rebuilt, &blob).is_ok());
    }

    #[test]
    fn a_key_code_a_document_cant_express_is_refused() {
        assert!(firmware_word_to_action(0x5000_0001).is_err());
        assert!(firmware_word_to_action(0x2400_0001).is_err());
        assert!(firmware_word_to_action(0x2100_0101).is_err(), "stray bits");
        assert!(
            firmware_word_to_action(0x4000_0001).is_err(),
            "factory consumer index"
        );
        assert_eq!(
            firmware_word_to_action(0x4000_0000 | (0x8 << 16) | 0xE9).unwrap(),
            serde_json::json!({"keyCode": "KC_AUDIO_VOL_UP", "modifiers": 8, "type": 0})
        );
        let factory = serde_json::json!({"type": 0, "keyCode": "KC_FF_THEME_CYCLE_NEXT"});
        let e = qmk_action_to_firmware_word(&factory).unwrap_err().0;
        assert!(e.contains("KC_OF_THEME_CYCLE_NEXT"), "{e}");
    }

    const ACTUATION_WORDS_JSON: &str = include_str!("../../tests/fixtures/actuation_words.json");

    /// Actuation words covering the encoding rules: the default keymap's two modes, rapid trigger,
    /// readings between 0.1 mm steps, a release depth of 0 and one beyond the actuation reading,
    /// and type 0 and 3 entries.
    const FIXTURE_WORDS: [u32; 9] = [
        0x4015_5253,
        0x4011_F1FD,
        0x800F_E150,
        (ACTUATION_TRADITIONAL << 30) | (100 << 12) | 600,
        (ACTUATION_TRADITIONAL << 30) | 0x253,
        (ACTUATION_TRADITIONAL << 30) | (0x300 << 12) | 0x100,
        (ACTUATION_RAPID_TRIGGER << 30) | (200 << 12) | 0x150,
        0,
        0xC012_3456,
    ];

    /// `(type, actuation µm, second µm)` encoded by the fixture.
    const FIXTURE_ENCODES: [(u32, u32, u32); 7] = [
        (ACTUATION_TRADITIONAL, 1200, 400),
        (ACTUATION_TRADITIONAL, 1000, 300),
        (ACTUATION_TRADITIONAL, 1200, 1500),
        (ACTUATION_TRADITIONAL, 2000, 1800),
        (ACTUATION_TRADITIONAL, 2800, 0),
        (ACTUATION_RAPID_TRIGGER, 600, 400),
        (ACTUATION_RAPID_TRIGGER, 100, 2800),
    ];

    fn actuation_words_fixture() -> serde_json::Value {
        let words: Vec<_> = FIXTURE_WORDS
            .iter()
            .map(|&word| {
                let d = decode_actuation_word(word);
                serde_json::json!({
                    "word": word,
                    "actuationType": d.actuation_type,
                    "actuationDensity": d.actuation_density,
                    "secondDensity": d.second_density,
                    "actuationUm": d.actuation_um,
                    "secondUm": d.second_um,
                    "firmwareDefault": d.firmware_default,
                    "exact": d.exact,
                    "entry": actuation_entry_json(word),
                })
            })
            .collect();
        let encodes: Vec<_> = FIXTURE_ENCODES
            .iter()
            .map(|&(t, act, second)| {
                serde_json::json!({
                    "actuationType": t,
                    "actuationUm": act,
                    "secondUm": second,
                    "word": encode_actuation_word(t, act, second).unwrap(),
                })
            })
            .collect();
        serde_json::json!({
            "words": words,
            "encodes": encodes,
            "defaultKeymapModes": decode_actuation_modes_from_blob(DEFAULT_KEYMAP_BIN).unwrap(),
        })
    }

    /// The default keymap with `layers` layers in every layout; layer `n` beyond the default
    /// ones maps every key to F`n`.
    fn keymap_with_layers(layers: usize) -> serde_json::Value {
        let mut km = crate::defaults::default_keymap();
        for layout in km["keymaps"].as_object_mut().unwrap().values_mut() {
            let arr = layout["layers"].as_array_mut().unwrap();
            let keys = arr[0]["keyActions"].as_array().unwrap().len();
            arr.truncate(layers);
            while arr.len() < layers {
                let n = arr.len();
                arr.push(serde_json::json!({
                    "name": format!("Layer {n}"),
                    "keyActions": vec![serde_json::json!({"type": 0, "keyCode": format!("KC_F{n}")}); keys],
                }));
            }
        }
        km
    }

    fn fixed_mode(act_mm: f64) -> serde_json::Value {
        serde_json::json!({"name": format!("{act_mm} mm"), "actuationPointMm": act_mm, "releasePointMm": act_mm - 0.2, "rapidTrigger": false})
    }

    #[test]
    fn unequal_layer_and_mode_counts_follow_the_section_header() {
        let km = keymap_with_layers(3);
        let modes = serde_json::json!([fixed_mode(1.2), fixed_mode(2.0)]);
        let bytes = encode_keymap_json_to_firmware_blob(&km, Some(&modes), None).unwrap();
        let words: Vec<u32> = bytes
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        // ANSI section: 3 layers, 85 keys, 2 modes, no macros, key codes at 0, tables at 3 * 85.
        assert_eq!(&words[6..13], &[3, 85, 2, 0, 0, 0, 255]);
        let iso = 6 + words[4] as usize;
        assert_eq!(&words[iso..iso + 7], &[3, 87, 2, 0, 0, 0, 261]);
        assert_eq!(words.len(), 6 + 2 * 7 + (3 + 2) * (85 + 87));

        let blob = decode_keymap_blob(&bytes).unwrap();
        for sec in &blob.keymaps {
            assert_eq!((sec.num_layers, sec.num_actuation_modes), (3, 2));
            assert!(sec.layers[2].iter().all(|&w| w == 0x3A + 1)); // KC_F2
            assert_eq!(
                sec.actuation_tables[0][0],
                encode_actuation_word(1, 1200, 1000).unwrap()
            );
            assert_eq!(
                sec.actuation_tables[1][0],
                encode_actuation_word(1, 2000, 1800).unwrap()
            );
        }
        let doc = keymap_json_from_blob(&bytes, &km).unwrap();
        assert_eq!(
            doc["keymaps"]["ansi"]["layers"].as_array().unwrap().len(),
            3
        );
        assert_eq!(
            decode_actuation_modes_from_blob(&bytes)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );

        // The counts the other way round don't describe this blob: the key codes span 3 * 85
        // words, not 2 * 85, so the reader refuses it as the firmware does.
        let mut swapped = words.clone();
        swapped.swap(6, 8);
        swapped.swap(iso, iso + 2);
        let swapped_bytes: Vec<u8> = swapped.iter().flat_map(|w| w.to_le_bytes()).collect();
        assert!(decode_keymap_blob(&swapped_bytes).is_err());

        // Two modes and three layers the other way round as well.
        let km2 = keymap_with_layers(2);
        let modes3 = serde_json::json!([fixed_mode(1.0), fixed_mode(1.5), fixed_mode(2.5)]);
        let b2 = encode_keymap_json_to_firmware_blob(&km2, Some(&modes3), None).unwrap();
        let sec = &decode_keymap_blob(&b2).unwrap().keymaps[0];
        assert_eq!((sec.num_layers, sec.num_actuation_modes), (2, 3));
        assert_eq!(u32::from_le_bytes(b2[24..28].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(b2[32..36].try_into().unwrap()), 3);
    }

    #[test]
    fn layer_and_mode_counts_stay_within_what_the_firmware_holds() {
        let one_mode = serde_json::json!([fixed_mode(1.2)]);
        assert!(
            encode_keymap_json_to_firmware_blob(&keymap_with_layers(8), Some(&one_mode), None)
                .is_ok()
        );
        let err =
            encode_keymap_json_to_firmware_blob(&keymap_with_layers(9), Some(&one_mode), None)
                .unwrap_err();
        assert!(err.0.contains("9 layers"), "{err}");
        let eight: Vec<_> = (0..8).map(|i| fixed_mode(0.5 + 0.2 * i as f64)).collect();
        let km = keymap_with_layers(2);
        assert!(
            encode_keymap_json_to_firmware_blob(&km, Some(&serde_json::json!(eight)), None).is_ok()
        );
        let mut nine = eight.clone();
        nine.push(fixed_mode(2.6));
        let err = encode_keymap_json_to_firmware_blob(&km, Some(&serde_json::json!(nine)), None)
            .unwrap_err();
        assert!(err.0.contains("9 actuation modes"), "{err}");
    }

    #[test]
    fn deleting_or_moving_a_mode_keeps_the_other_modes_tables() {
        let km = keymap_with_layers(2);
        let mut modes = serde_json::json!([fixed_mode(1.0), fixed_mode(1.5), fixed_mode(2.5)]);
        // Mode 2 has a per-key override on ANSI key 5.
        modes[2]["perKeyOverrides"] = serde_json::json!({"ansi": {"5": {"actuationPointMm": 0.4, "rapidTrigger": true, "rapidTriggerSensitivityMm": 0.2}}});
        let before = encode_keymap_json_to_firmware_blob(&km, Some(&modes), None).unwrap();
        let old = decode_keymap_blob(&before).unwrap();
        let read = decode_actuation_modes_from_blob(&before).unwrap();

        // Delete mode 0: the two others, untouched, move up with their tables.
        let mut kept: Vec<serde_json::Value> = read.as_array().unwrap()[1..].to_vec();
        kept[0]["firmwareTable"] = serde_json::json!(1);
        kept[1]["firmwareTable"] = serde_json::json!(2);
        let after =
            encode_keymap_json_to_firmware_blob(&km, Some(&serde_json::json!(kept)), Some(&before))
                .unwrap();
        for (sec_old, sec_new) in old
            .keymaps
            .iter()
            .zip(decode_keymap_blob(&after).unwrap().keymaps)
        {
            assert_eq!(sec_new.num_actuation_modes, 2);
            assert_eq!(
                sec_new.actuation_tables,
                sec_old.actuation_tables[1..].to_vec()
            );
        }

        // A shorter list is the whole list: no table of the removed mode stays behind.
        let first_two: Vec<serde_json::Value> = read.as_array().unwrap()[..2].to_vec();
        let shorter = encode_keymap_json_to_firmware_blob(
            &km,
            Some(&serde_json::json!(first_two)),
            Some(&before),
        )
        .unwrap();
        assert_eq!(
            decode_keymap_blob(&shorter).unwrap().keymaps[0].num_actuation_modes,
            2
        );

        // A new untouched mode (`firmwareTable: null`) is encoded from its values, not copied
        // from the table at its position.
        let mut added = read.as_array().unwrap().clone();
        added.insert(0, fixed_mode(0.8));
        added[0]["touched"] = serde_json::json!(false);
        added[0]["firmwareTable"] = serde_json::Value::Null;
        for (i, m) in added.iter_mut().enumerate().skip(1) {
            m["firmwareTable"] = serde_json::json!(i - 1);
        }
        let grown = encode_keymap_json_to_firmware_blob(
            &km,
            Some(&serde_json::json!(added)),
            Some(&before),
        )
        .unwrap();
        let sec = &decode_keymap_blob(&grown).unwrap().keymaps[0];
        assert_eq!(sec.num_actuation_modes, 4);
        assert_eq!(
            sec.actuation_tables[0],
            vec![encode_actuation_word(1, 800, 600).unwrap(); 85]
        );
        assert_eq!(
            sec.actuation_tables[1..],
            old.keymaps[0].actuation_tables[..]
        );
    }

    #[test]
    fn mode_names_travel_in_the_keymap_document() {
        let mut km = keymap_with_layers(2);
        let modes = serde_json::json!([{"name": "Writing"}, {"name": "Shooter"}, {"name": ""}]);
        keep_actuation_mode_names(&mut km, &modes);
        assert_eq!(
            km[MODE_NAMES_FIELD],
            serde_json::json!(["Writing", "Shooter", ""])
        );
        let decoded = serde_json::json!([{"name": "Typing"}, {"name": "Gaming"}, {"name": "Mode 3"}, {"name": "Mode 4"}]);
        let named = name_actuation_modes(decoded, &km);
        let names: Vec<&str> = named
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Writing", "Shooter", "Mode 3", "Mode 4"]);
    }

    /// `tests/fixtures/actuation_words.json` is what this codec makes of its words and of
    /// `default_keymap.bin`; `tools/keyglass-dev/tests/test_codecs.py` checks the Python codec
    /// against the same file. Regenerate it with
    /// `KEYGLASS_REGENERATE_FIXTURES=1 cargo test -p keyglassd actuation_words_fixture`.
    #[test]
    fn actuation_words_fixture_matches_the_codec() {
        let fixture = actuation_words_fixture();
        if std::env::var("KEYGLASS_REGENERATE_FIXTURES").as_deref() == Ok("1") {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/actuation_words.json");
            std::fs::write(path, serde_json::to_string_pretty(&fixture).unwrap() + "\n").unwrap();
            return;
        }
        let stored: serde_json::Value = serde_json::from_str(ACTUATION_WORDS_JSON).unwrap();
        assert_eq!(
            stored, fixture,
            "tests/fixtures/actuation_words.json is stale"
        );
        // Every decoded entry, untouched or edited without changes, encodes back to its word.
        let mode_defaults = ActuationDefaults {
            rapid: false,
            act_mm: 2.0,
            sens_mm: 0.3,
        };
        for case in fixture["words"].as_array().unwrap() {
            let (word, _) = encode_actuation_entry(&case["entry"], mode_defaults).unwrap();
            assert_eq!(serde_json::json!(word), case["word"], "{case}");
        }
    }
}
