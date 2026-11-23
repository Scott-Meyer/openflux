/*
 * Public facade implementing fw_qmk.h on top of real vial-qmk.
 */

#include "fw_qmk.h"

#include "action.h"
#include "action_layer.h"
#include "action_tapping.h"
#include "action_util.h"
#include "caps_word.h"
#include "config.h"
#include "dynamic_keymap.h"
#include "eeconfig.h"
#include "host.h"
#include "host_driver.h"
#include "keyboard.h"
#include "keycodes.h"
#include "keymap_common.h"
#include "matrix.h"
#include "process_key_override.h"
#include "qmk_settings.h"
#include "quantum.h"
#include "raw_hid.h"
#include "via.h"
#include "vial.h"

#include <string.h>

/* Internals from fw_qmk_platform.c */
extern fw_qmk_platform_t g_fw_qmk_plat;
extern uint32_t g_fw_qmk_now_ms;
extern uint32_t g_fw_qmk_keymap_rev;
extern fw_hid_report_t g_fw_qmk_hid;
extern uint32_t g_fw_qmk_of_func_mask;

extern uint8_t *fw_qmk_eeprom_buf(void);
extern void fw_qmk_eeprom_set_suppress_dirty(bool suppress);
extern bool fw_qmk_eeprom_is_dirty(void);
extern uint32_t fw_qmk_eeprom_dirty_since(void);
extern void fw_qmk_eeprom_clear_dirty(void);
extern void fw_qmk_matrix_set(const matrix_row_t rows[MATRIX_ROWS]);
extern host_driver_t *fw_qmk_get_host_driver(void);
extern void fw_qmk_reset_vial_def(void);

static uint8_t g_active_layout_id = 1U;
static bool g_initialized = false;
/* Tri-state storage status: 0 = loaded OK, 1 = genuinely absent (safe to seed & persist), -1 = I/O error (RAM-only). */
static int g_storage_state = 1;

static uint8_t sanitize_layout_id(uint8_t layout_id) {
    return (layout_id >= 1U && layout_id <= 3U) ? layout_id : 1U;
}

static uint32_t rd_le32_qmk(const uint8_t *p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

/* Adjust compiled-in ANSI Layer 0 defaults when seeding a fresh ISO (2) or JIS (3) EEPROM,
 * matching FW_ROW_LEN[2]/[3] and host/daemon/src/defaults.rs. */
static void apply_layout_default_overrides(uint8_t layout_id) {
    if (layout_id == 2U) {
        /* ISO (87 keys): row 2 col 13 = Enter; row 3 inserts # at col 12; row 4 inserts NUBS at col 1. */
        static const uint16_t iso_r3[15] = {
            KC_CAPS, KC_A, KC_S, KC_D, KC_F, KC_G, KC_H, KC_J, KC_K, KC_L, KC_SCLN, KC_QUOT, KC_NUHS, KC_ENT, KC_PGUP,
        };
        static const uint16_t iso_r4[15] = {
            KC_LSFT, KC_NUBS, KC_Z, KC_X, KC_C, KC_V, KC_B, KC_N, KC_M, KC_COMM, KC_DOT, KC_SLSH, KC_RSFT, KC_UP,
            KC_PGDN,
        };
        dynamic_keymap_set_keycode(0, 2, 13, KC_ENT);
        for (uint8_t c = 0U; c < 15U; c++) {
            dynamic_keymap_set_keycode(0, 3, c, iso_r3[c]);
            dynamic_keymap_set_keycode(0, 4, c, iso_r4[c]);
        }
    } else if (layout_id == 3U) {
        /* JIS (90 keys): row 1 inserts INT3 (Yen) before Backspace; row 3 inserts NUHS; row 4 inserts INT1 (Ro);
         * row 5 has 13 keys (Muhenkan left of Space, Henkan + Kana right of Space). */
        static const uint16_t jis_r1[16] = {
            KC_GRV, KC_1, KC_2, KC_3, KC_4, KC_5, KC_6, KC_7, KC_8, KC_9, KC_0, KC_MINS, KC_EQL, KC_INT3, KC_BSPC,
            KC_HOME,
        };
        static const uint16_t jis_r3[15] = {
            KC_CAPS, KC_A, KC_S, KC_D, KC_F, KC_G, KC_H, KC_J, KC_K, KC_L, KC_SCLN, KC_QUOT, KC_NUHS, KC_ENT, KC_PGUP,
        };
        static const uint16_t jis_r4[15] = {
            KC_LSFT, KC_Z, KC_X, KC_C, KC_V, KC_B, KC_N, KC_M, KC_COMM, KC_DOT, KC_SLSH, KC_INT1, KC_RSFT, KC_UP,
            KC_PGDN,
        };
        static const uint16_t jis_r5[13] = {
            KC_LCTL, KC_LGUI, KC_LALT, QK_USER_31, KC_INT5, KC_SPC, KC_INT4, KC_INT2, KC_RALT, MO(1), KC_LEFT, KC_DOWN,
            KC_RGHT,
        };
        dynamic_keymap_set_keycode(0, 2, 13, KC_ENT);
        for (uint8_t c = 0U; c < 16U; c++)
            dynamic_keymap_set_keycode(0, 1, c, jis_r1[c]);
        for (uint8_t c = 0U; c < 15U; c++) {
            dynamic_keymap_set_keycode(0, 3, c, jis_r3[c]);
            dynamic_keymap_set_keycode(0, 4, c, jis_r4[c]);
        }
        for (uint8_t c = 0U; c < 13U; c++)
            dynamic_keymap_set_keycode(0, 5, c, jis_r5[c]);
    }
}

static uint16_t translate_legacy_word(uint32_t w) {
    if (w == 0U || w == 0xF0000000U)
        return KC_NO;
    if (w == 0x50000000U)
        return KC_TRNS;
    uint8_t kind = (uint8_t)((w >> 28) & 0xFU);
    if (kind == 0x0U) {
        uint16_t hid = (uint16_t)(w & 0xFFU);
        uint8_t mods = (uint8_t)((w >> 16) & 0xFFU);
        if (hid == 0U)
            return KC_NO;
        /* Translate 8-bit HID modifier mask (low nibble=left, high nibble=right) into QMK QK_MODS. */
        uint16_t qmods = 0U;
        if (mods & 0x0FU) {
            qmods = (uint16_t)(mods & 0x0FU);
        } else if (mods & 0xF0U) {
            qmods = (uint16_t)(0x10U | ((mods >> 4) & 0x0FU));
        }
        return (uint16_t)((qmods << 8) | hid);
    }
    if (kind == 0x2U) {
        uint8_t sw_type = (uint8_t)((w >> 24) & 0xFU);
        uint8_t target = (uint8_t)(w & 0x1FU);
        if (target >= DYNAMIC_KEYMAP_LAYER_COUNT)
            return KC_NO;
        if (sw_type == 2U || sw_type == 4U)
            return (uint16_t)(QK_TOGGLE_LAYER | target);
        if (sw_type == 3U)
            return (uint16_t)(QK_TO | target);
        return (uint16_t)(QK_MOMENTARY | target);
    }
    if (kind == 0x4U) {
        uint16_t usage = (uint16_t)(w & 0xFFFFU);
        switch (usage) {
            case 0x006FU: return KC_BRIU;
            case 0x0070U: return KC_BRID;
            case 0x00B5U: return KC_MNXT;
            case 0x00B6U: return KC_MPRV;
            case 0x00B7U: return KC_MSTP;
            case 0x00B8U: return KC_EJCT;
            case 0x00CDU: return KC_MPLY;
            case 0x00E2U: return KC_MUTE;
            case 0x00E9U: return KC_VOLU;
            case 0x00EAU: return KC_VOLD;
            case 0x0183U: return KC_MSEL;
            case 0x018AU: return KC_MAIL;
            case 0x0192U: return KC_CALC;
            case 0x0194U: return KC_MYCM;
            case 0x0221U: return KC_WSCH;
            case 0x0223U: return KC_WHOM;
            case 0x0224U: return KC_WBAK;
            case 0x0225U: return KC_WFWD;
            case 0x0226U: return KC_WSTP;
            case 0x0227U: return KC_WREF;
            case 0x022AU: return KC_WFAV;
            case 0x029DU: return QK_USER_31; /* OF_GLOBE */
            case 0x029FU: return KC_MCTL;
            case 0x02A0U: return KC_LPAD;
            default: return KC_NO;
        }
    }
    if (kind == 0x5U) {
        uint8_t fcode = (uint8_t)(w & 0xFFU);
        if (fcode >= 0x02U && fcode <= 0x0FU) {
            return (uint16_t)(0x7E00U + (fcode - 0x02U));
        }
    }
    return KC_NO;
}

bool fw_qmk_key_to_rc(uint8_t layout_id, uint16_t key_idx, uint8_t *out_row, uint8_t *out_col) {
    uint8_t lid = sanitize_layout_id(layout_id);
    uint16_t off = 0U;
    for (uint8_t r = 0U; r < MATRIX_ROWS; r++) {
        /* FW_ROW_LEN stores Hall sensors per row; logical keys per row = sensors - 1. */
        uint8_t sensors = FW_ROW_LEN[lid][r];
        uint8_t rlen = (sensors > 1U) ? (uint8_t)(sensors - 1U) : 0U;
        if (key_idx < (uint16_t)(off + rlen)) {
            uint8_t c = (uint8_t)(key_idx - off);
            if (c >= MATRIX_COLS)
                return false;
            if (out_row)
                *out_row = r;
            if (out_col)
                *out_col = c;
            return true;
        }
        off = (uint16_t)(off + rlen);
    }
    return false;
}

/* Called by fw_qmk_keymap.c's process_record_user() for QK_KB_0..31 / QK_USER_0..31. */
void fw_qmk_emit_custom_action(uint16_t slot, bool pressed) {
    /* Maintain legacy of_func_mask for built-in slots 0..13 (fw_of_func_code 0x02..0x0F). */
    if (slot < FW_QMK_BUILTIN_SLOTS) {
        uint8_t fcode = (uint8_t)(0x02U + slot);
        if (pressed) {
            g_fw_qmk_of_func_mask |= (1UL << fcode);
        } else {
            g_fw_qmk_of_func_mask &= ~(1UL << fcode);
        }
    }
    if (g_fw_qmk_plat.on_flux_action) {
        uint8_t mods = (uint8_t)(get_mods() | get_weak_mods() | get_oneshot_mods());
        uint32_t layers = (uint32_t)(layer_state | default_layer_state | 1U);
        g_fw_qmk_plat.on_flux_action(slot, pressed, mods, layers, g_fw_qmk_plat.ctx);
    }
}

static void load_layout_eeprom_and_reinit(uint8_t layout_id) {
    uint8_t *ee = fw_qmk_eeprom_buf();
    memset(ee, 0xFF, FW_QMK_EEPROM_SIZE);

    int load_rc = 1; /* Default: absent (no storage callback configured) */
    if (g_fw_qmk_plat.eeprom_load) {
        load_rc = g_fw_qmk_plat.eeprom_load(layout_id, ee, FW_QMK_EEPROM_SIZE, g_fw_qmk_plat.ctx);
    }
    g_storage_state = load_rc;

    /* Suppress dirty on load_rc == 0 (just loaded) AND load_rc < 0 (I/O/corrupt error: NEVER overwrite
     * the user's flash file with defaults automatically — only explicit future Vial edits may write). */
    bool suppress = (load_rc != 1);
    fw_qmk_eeprom_set_suppress_dirty(suppress);
    keyboard_setup();
    keyboard_init();
    vial_init();
    if (load_rc != 0) {
        apply_layout_default_overrides(layout_id);
    }
    keymap_config.nkro = 1;
    fw_qmk_eeprom_set_suppress_dirty(false);
    if (suppress) {
        fw_qmk_eeprom_clear_dirty();
    }
    g_fw_qmk_keymap_rev++;
}

bool fw_qmk_seed_from_legacy_keymap(const fw_runtime_t *rt, uint8_t layout_id) {
    uint8_t lid = sanitize_layout_id(layout_id);
    /* Only run one-time legacy import when vial_ee_<lid>.bin was genuinely ABSENT (never on load error!). */
    if (!g_initialized || g_storage_state != 1 || lid != g_active_layout_id || !rt)
        return false;
    const fw_layout_view_t *lv = &rt->layouts[lid];
    if (!lv->valid || !lv->section_words || lv->num_layers == 0U || lv->num_keys == 0U)
        return false;

    uint16_t n_layers = lv->num_layers;
    if (n_layers > DYNAMIC_KEYMAP_LAYER_COUNT)
        n_layers = DYNAMIC_KEYMAP_LAYER_COUNT;

    /* Clear layers 0..n_layers-1 to KC_TRNS (layer 0 -> KC_NO) before importing legacy bindings. */
    for (uint8_t l = 0U; l < DYNAMIC_KEYMAP_LAYER_COUNT; l++) {
        uint16_t fill = (l == 0U) ? KC_NO : KC_TRNS;
        for (uint8_t r = 0U; r < MATRIX_ROWS; r++) {
            for (uint8_t c = 0U; c < MATRIX_COLS; c++) {
                dynamic_keymap_set_keycode(l, r, c, fill);
            }
        }
    }

    for (uint16_t l = 0U; l < n_layers; l++) {
        for (uint16_t k = 0U; k < lv->num_keys; k++) {
            uint8_t r = 0U, c = 0U;
            if (!fw_qmk_key_to_rc(lid, k, &r, &c))
                continue;
            const uint8_t *kp =
                (const uint8_t *)lv->section_words + (lv->keycodes_word_off + (uint32_t)l * lv->num_keys + k) * 4U;
            uint16_t qkc = translate_legacy_word(rd_le32_qmk(kp));
            dynamic_keymap_set_keycode((uint8_t)l, r, c, qkc);
        }
    }
    g_fw_qmk_keymap_rev++;
    return true;
}

void fw_qmk_init(const fw_qmk_platform_t *plat, uint8_t initial_layout_id, uint32_t now_ms) {
    if (plat) {
        g_fw_qmk_plat = *plat;
    } else {
        memset(&g_fw_qmk_plat, 0, sizeof(g_fw_qmk_plat));
    }
    g_fw_qmk_now_ms = now_ms;
    g_fw_qmk_keymap_rev = 1U;
    memset(&g_fw_qmk_hid, 0, sizeof(g_fw_qmk_hid));
    g_fw_qmk_of_func_mask = 0U;
    fw_qmk_clear_hid_queue();
    fw_qmk_reset_vial_def();
    fw_qmk_eeprom_clear_dirty();

    host_set_driver(fw_qmk_get_host_driver());
    g_active_layout_id = sanitize_layout_id(initial_layout_id);
    load_layout_eeprom_and_reinit(g_active_layout_id);
    g_initialized = true;
}

void fw_qmk_set_layout(uint8_t layout_id, uint32_t now_ms) {
    uint8_t lid = sanitize_layout_id(layout_id);
    g_fw_qmk_now_ms = now_ms;
    if (!g_initialized) {
        fw_qmk_init(&g_fw_qmk_plat, lid, now_ms);
        return;
    }
    if (lid == g_active_layout_id)
        return;

    /* Flush any unsaved edits for the departing layout before loading the new layout. */
    (void)fw_qmk_poll_eeprom_flush(now_ms, 0U, true);
    memset(&g_fw_qmk_hid, 0, sizeof(g_fw_qmk_hid));
    g_fw_qmk_of_func_mask = 0U;
    clear_keyboard();
    g_active_layout_id = lid;
    load_layout_eeprom_and_reinit(lid);
}

void fw_qmk_set_default_layer(uint8_t layer) {
    if (!g_initialized || layer >= DYNAMIC_KEYMAP_LAYER_COUNT)
        return;
    default_layer_set((layer_state_t)1U << layer);
}

void fw_qmk_process_scan(const uint8_t *key_pressed, uint16_t n_keys, uint32_t now_ms, fw_hid_report_t *out_hid) {
    if (!g_initialized) {
        if (out_hid)
            memset(out_hid, 0, sizeof(*out_hid));
        return;
    }

    g_fw_qmk_now_ms = now_ms;

    matrix_row_t rows[MATRIX_ROWS];
    memset(rows, 0, sizeof(rows));
    if (key_pressed) {
        for (uint16_t k = 0U; k < n_keys; k++) {
            if (!key_pressed[k])
                continue;
            uint8_t r = 0U, c = 0U;
            if (fw_qmk_key_to_rc(g_active_layout_id, k, &r, &c)) {
                rows[r] |= ((matrix_row_t)1U << c);
            }
        }
    }
    fw_qmk_matrix_set(rows);
    keyboard_task();

    if (out_hid) {
        *out_hid = g_fw_qmk_hid;
        out_hid->of_func_mask = g_fw_qmk_of_func_mask;
    }
}

static bool is_dual_role_keycode(uint16_t kc) {
    return IS_QK_MOD_TAP(kc) || IS_QK_LAYER_TAP(kc) || IS_QK_TAP_DANCE(kc) || IS_QK_LAYER_TAP_TOGGLE(kc);
}

bool fw_qmk_key_forbids_rapid_trigger(uint16_t key_idx) {
    if (!g_initialized)
        return false;
    uint8_t r = 0U, c = 0U;
    if (!fw_qmk_key_to_rc(g_active_layout_id, key_idx, &r, &c))
        return false;
    /* Guard across all layers so activating a layer mid-press never flips rapid trigger unexpectedly. */
    for (uint8_t layer = 0U; layer < DYNAMIC_KEYMAP_LAYER_COUNT; layer++) {
        uint16_t kc = dynamic_keymap_get_keycode(layer, r, c);
        if (is_dual_role_keycode(kc))
            return true;
    }
    return false;
}

void fw_qmk_raw_hid_rx(const uint8_t data[FW_QMK_VIAL_RAW_EPSIZE], uint32_t now_ms) {
    if (!g_initialized || !data)
        return;
    g_fw_qmk_now_ms = now_ms;
    uint8_t buf[FW_QMK_VIAL_RAW_EPSIZE];
    memcpy(buf, data, sizeof(buf));
    raw_hid_receive(buf, (uint8_t)sizeof(buf));
}

void fw_qmk_get_state(fw_qmk_state_t *out) {
    if (!out)
        return;
    memset(out, 0, sizeof(*out));
    out->oneshot_layer = 0xFFU;
    if (!g_initialized)
        return;

    out->effective_mods = (uint8_t)(get_mods() | get_weak_mods());
    out->oneshot_mods = get_oneshot_mods();
#ifndef NO_ACTION_ONESHOT
    uint8_t osl = get_oneshot_layer();
    out->oneshot_layer = (osl > 0U && get_oneshot_layer_state() != ONESHOT_OTHER_KEY_PRESSED) ? osl : 0xFFU;
#endif
    out->caps_word_active = is_caps_word_on() ? 1U : 0U;
    out->layer_state = (uint32_t)(layer_state | default_layer_state | 1U);
    out->default_layer_state = (uint32_t)(default_layer_state ? default_layer_state : 1U);
    out->keymap_revision = g_fw_qmk_keymap_rev;
}

uint16_t fw_qmk_resolve_key(uint16_t key_idx, uint8_t *out_source_layer) {
    if (out_source_layer)
        *out_source_layer = 0U;
    if (!g_initialized)
        return KC_NO;

    uint8_t r = 0U, c = 0U;
    if (!fw_qmk_key_to_rc(g_active_layout_id, key_idx, &r, &c))
        return KC_NO;

    uint32_t eff = (uint32_t)(layer_state | default_layer_state | 1U);
    uint16_t resolved = KC_NO;
    uint8_t src_layer = 0U;

    for (int l = (int)DYNAMIC_KEYMAP_LAYER_COUNT - 1; l >= 0; l--) {
        if (!((eff >> (uint32_t)l) & 1U))
            continue;
        uint16_t cand = dynamic_keymap_get_keycode((uint8_t)l, r, c);
        if (cand == KC_TRNS)
            continue;
        resolved = cand;
        src_layer = (uint8_t)l;
        break;
    }

    if (out_source_layer)
        *out_source_layer = src_layer;

    /* Apply active Key Overrides when modifiers match (e.g. Shift+Bksp -> Del). */
#ifdef KEY_OVERRIDE_ENABLE
    uint8_t active_mods = (uint8_t)(get_mods() | get_oneshot_mods());
    uint8_t eff_layer = get_highest_layer(layer_state | default_layer_state);
    for (uint8_t i = 0U; i < VIAL_KEY_OVERRIDE_ENTRIES; i++) {
        vial_key_override_entry_t entry;
        if (dynamic_keymap_get_key_override(i, &entry) != 0)
            continue;
        if (!(entry.options & vial_ko_enabled))
            continue;
        if (entry.trigger != resolved)
            continue;
        if (!(entry.layers & (1U << eff_layer)))
            continue;
        if (entry.trigger_mods != 0U && (active_mods & entry.trigger_mods) == 0U)
            continue;
        if ((active_mods & entry.negative_mod_mask) != 0U)
            continue;
        resolved = entry.replacement;
        break;
    }
#endif

    return resolved;
}

bool fw_qmk_has_enabled_key_overrides(void) {
#ifdef KEY_OVERRIDE_ENABLE
    if (!g_initialized)
        return false;
    for (uint8_t i = 0U; i < VIAL_KEY_OVERRIDE_ENTRIES; i++) {
        vial_key_override_entry_t entry;
        if (dynamic_keymap_get_key_override(i, &entry) == 0 && (entry.options & vial_ko_enabled))
            return true;
    }
#endif
    return false;
}

bool fw_qmk_poll_eeprom_flush(uint32_t now_ms, uint32_t debounce_ms, bool keys_quiet) {
    if (!g_initialized || !fw_qmk_eeprom_is_dirty())
        return false;
    if (debounce_ms > 0U) {
        if (!keys_quiet)
            return false;
        if ((uint32_t)(now_ms - fw_qmk_eeprom_dirty_since()) < debounce_ms)
            return false;
    }
    if (g_fw_qmk_plat.eeprom_save) {
        if (g_fw_qmk_plat.eeprom_save(g_active_layout_id, fw_qmk_eeprom_buf(), FW_QMK_EEPROM_SIZE, g_fw_qmk_plat.ctx) !=
            0) {
            return false;
        }
    }
    fw_qmk_eeprom_clear_dirty();
    return true;
}
