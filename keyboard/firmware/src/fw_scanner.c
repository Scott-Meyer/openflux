#include "fw_scanner.h"
#include "fw_scan_hot.h"
#include "fw_row_condition.h"
#include "fw_classic_row.h"
#include "fw_press_guard.h"

#include <string.h>

const uint8_t FW_BANK_PINS[FW_NUM_BANKS] = {18, 20, 21, 22, 24, 26, 27, 19, 23, 25};

/* Sensors per row; logical keys per row = row_len - 1. */
// clang-format off
const uint8_t FW_SCAN_TABLE(FW_ROW_LEN)[4][FW_MATRIX_ROWS] = {
    {17, 16, 16, 15, 15, 12}, /* 0: fallback ANSI (85 keys) */
    {17, 16, 16, 15, 15, 12}, /* 1: ANSI (85 keys) */
    {17, 16, 16, 16, 16, 12}, /* 2: ISO  (87 keys) */
    {17, 17, 16, 16, 16, 14}, /* 3: JIS  (90 keys) */
};
// clang-format on

/* Physical ADC slot 0..159 per [layout][row][col]. */
// clang-format off
const uint8_t FW_SCAN_TABLE(FW_SENSOR_MAP)[4][FW_MATRIX_ROWS][FW_MATRIX_MAX_COLS] = {
    /* Layout 0 */
    {
        {6, 13, 22, 16, 26, 36, 44, 54, 48, 58, 67, 77, 73, 80, 88, 98, 105},
        {4, 14,  9, 17, 27, 35, 45, 40, 49, 59, 69, 78, 87, 81, 97, 107,   0},
        {3, 11, 19, 31, 37, 46, 42, 52, 63, 57, 66, 74, 84, 95, 99, 108,   0},
        {2, 10, 18, 28, 34, 47, 41, 51, 62, 70, 65, 75, 83, 100, 109,  0,   0},
        {1, 20, 30, 24, 32, 43, 53, 50, 60, 64, 76, 85, 91, 101, 110,  0,   0},
        {0, 15, 21, 29, 25, 68, 79, 86, 82, 92, 102, 111, 0,  0,   0,  0,   0},
    },
    /* Layout 1 (ANSI) */
    {
        {6, 13, 22, 16, 26, 36, 44, 54, 48, 58, 67, 77, 73, 80, 88, 98, 105},
        {4, 14,  9, 17, 27, 35, 45, 40, 49, 59, 69, 78, 87, 81, 97, 107,   0},
        {3, 11, 19, 31, 37, 46, 42, 52, 63, 57, 66, 74, 84, 95, 99, 108,   0},
        {2, 10, 18, 28, 34, 47, 41, 51, 62, 70, 65, 75, 83, 100, 109,  0,   0},
        {1, 20, 30, 24, 32, 43, 53, 50, 60, 64, 76, 85, 91, 101, 110,  0,   0},
        {0, 15, 21, 29, 25, 68, 79, 86, 82, 92, 102, 111, 0,  0,   0,  0,   0},
    },
    /* Layout 2 (ISO) */
    {
        {6, 13, 22, 16, 26, 36, 44, 54, 48, 58, 67, 77, 73, 80, 88, 98, 105},
        {4, 14,  9, 17, 27, 35, 45, 40, 49, 59, 69, 78, 87, 81, 97, 107,   0},
        {3, 11, 19, 31, 37, 46, 42, 52, 63, 57, 66, 74, 84, 95, 99, 108,   0},
        {2, 10, 18, 28, 34, 47, 41, 51, 62, 70, 65, 75, 83, 94, 100, 109,  0},
        {1, 12, 20, 30, 24, 32, 43, 53, 50, 60, 64, 76, 85, 91, 101, 110,  0},
        {0, 15, 21, 29, 25, 68, 79, 86, 82, 92, 102, 111, 0,  0,   0,  0,   0},
    },
    /* Layout 3 (JIS) */
    {
        {6, 13, 22, 16, 26, 36, 44, 54, 48, 58, 67, 77, 73, 80, 88, 98, 105},
        {4, 14,  9, 17, 27, 35, 45, 40, 49, 59, 69, 78, 87, 81, 90, 97, 107},
        {3, 11, 19, 31, 37, 46, 42, 52, 63, 57, 66, 74, 84, 95, 99, 108,   0},
        {2, 10, 18, 28, 34, 47, 41, 51, 62, 70, 65, 75, 83, 94, 100, 109,  0},
        {1, 20, 30, 24, 32, 43, 53, 50, 60, 64, 76, 85, 93, 91, 101, 110,  0},
        {0, 15, 21, 29, 25, 33, 61, 68, 79, 86, 82, 92, 102, 111, 0,  0,   0},
    },
};
// clang-format on

static uint32_t rd_le32(const uint8_t *p) {
    return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}

static void wr_le32(uint8_t *p, uint32_t v) {
    p[0] = (uint8_t)(v & 0xFFU);
    p[1] = (uint8_t)((v >> 8) & 0xFFU);
    p[2] = (uint8_t)((v >> 16) & 0xFFU);
    p[3] = (uint8_t)((v >> 24) & 0xFFU);
}

static void wr_le16(uint8_t *p, uint16_t v) {
    p[0] = (uint8_t)(v & 0xFFU);
    p[1] = (uint8_t)((v >> 8) & 0xFFU);
}

uint32_t fw_mux_col_gpio_value(uint8_t col) {
    uint32_t c = col & 0x0FU;
    uint32_t nc = (~c) & 0x0FU;
    uint32_t val = 0;
    val |= ((c >> 0) & 1U) << 13;
    val |= ((c >> 1) & 1U) << 12;
    val |= ((c >> 2) & 1U) << 11;
    val |= ((c >> 3) & 1U) << 10;
    val |= ((nc >> 0) & 1U) << 17;
    val |= ((nc >> 1) & 1U) << 16;
    val |= ((nc >> 2) & 1U) << 15;
    val |= ((nc >> 3) & 1U) << 14;
    return val & FW_MUX_COL_MASK;
}

void fw_hid_scheduler_step(fw_hid_scheduler_t *sched, const fw_hid_report_t *desired, int ep_ready, int of_ep_ready,
                           fw_hid_send_kbd_fn_t send_kbd, fw_hid_send_consumer_fn_t send_consumer,
                           fw_hid_send_of_fn_t send_of_func, void *ctx) {
    if (!sched || !desired)
        return;

    /* Queue vendor-function mask transitions while EP 0x85 is busy, retaining the most recent 16 states. */
    uint8_t q_count = (uint8_t)(sched->of_q_tail - sched->of_q_head);
    uint32_t last_queued = (q_count > 0U) ? sched->of_queue[(sched->of_q_tail - 1U) & 15U] : sched->last_of_func_mask;
    if (!sched->of_initialized || last_queued != desired->of_func_mask) {
        if (q_count >= 16U) {
            sched->of_q_head++;
        }
        sched->of_queue[sched->of_q_tail & 15U] = desired->of_func_mask;
        sched->of_q_tail++;
        sched->of_initialized = 1U;
    }

    if (of_ep_ready && sched->of_q_head != sched->of_q_tail && send_of_func) {
        uint32_t next_mask = sched->of_queue[sched->of_q_head & 15U];
        if (send_of_func(next_mask, ctx) == 0) {
            sched->last_of_func_mask = next_mask;
            sched->of_q_head++;
        }
    }

    if (!ep_ready)
        return;

    int kbd_dirty = !sched->initialized || sched->last_modifiers != desired->modifiers ||
                    memcmp(sched->last_keycodes, desired->keycodes, sizeof(sched->last_keycodes)) != 0 ||
                    memcmp(sched->last_nkro_bitmap, desired->nkro_bitmap, sizeof(sched->last_nkro_bitmap)) != 0;
    int cons_dirty = !sched->initialized || sched->last_consumer_usage != desired->consumer_usage;

    if (kbd_dirty && send_kbd) {
        if (send_kbd(desired->modifiers, desired->keycodes, desired->nkro_bitmap, ctx) == 0) {
            sched->last_modifiers = desired->modifiers;
            memcpy(sched->last_keycodes, desired->keycodes, sizeof(sched->last_keycodes));
            memcpy(sched->last_nkro_bitmap, desired->nkro_bitmap, sizeof(sched->last_nkro_bitmap));
            sched->initialized = 1U;
        }
        /* EP 0x83 is busy with this report; defer consumer updates to the next ready tick. */
        return;
    }

    if (cons_dirty && send_consumer) {
        if (send_consumer(desired->consumer_usage, ctx) == 0) {
            sched->last_consumer_usage = desired->consumer_usage;
            sched->initialized = 1U;
        }
    }
}

void fw_runtime_init(fw_runtime_t *rt) {
    if (!rt)
        return;
    memset(rt, 0, sizeof(*rt));
    fw_calibration_init_defaults(&rt->cal);
    for (size_t i = 0; i < FW_NUM_SENSORS; i++) {
        rt->baseline_adc[i] = 1800U;
    }
    rt->active_layer_bitfield = 1U;
    rt->effective_layer_bitfield = 1U;
    rt->macro_hid_ready = 1U;
}

void fw_runtime_reset_tracking(fw_runtime_t *rt) {
#if FW_ENABLE_PRESS_REFINEMENT
    memset(rt->depth_rows, 0, sizeof(rt->depth_rows));
    memset(rt->refine_rows, 0, sizeof(rt->refine_rows));
#endif
    for (size_t k = 0; k < FW_MAX_KEYS_PER_LAYOUT; k++) {
        if (!rt->key_pressed[k]) {
            fw_rapid_rearm(&rt->rapid[k]);
            rt->fixed_transition_scans[k] = 0;
        }
    }
}

int fw_runtime_install_refinement(fw_runtime_t *rt, const uint8_t *payload, size_t len) {
#if FW_ENABLE_PRESS_REFINEMENT
    if (rt && fw_refine_decode(&rt->refinement, payload, len) == 0) {
        rt->refinement_active = true;
        fw_runtime_reset_tracking(rt);
        return 0;
    }
#else
    (void)rt;
    (void)payload;
    (void)len;
#endif
    return -1;
}

bool fw_runtime_refinement_active(const fw_runtime_t *rt) {
#if FW_ENABLE_PRESS_REFINEMENT
    return rt && rt->refinement_active;
#else
    (void)rt;
    return false;
#endif
}

void fw_runtime_set_keyframe(fw_runtime_t *rt, uint8_t attached, uint8_t layout, uint8_t feel) {
    if (!rt)
        return;
    if (!attached) {
        layout = feel = 0U;
    } else if (layout < 1U || layout > 3U) {
        return;
    }
    if (rt->keyframe_attached == !!attached && rt->keyframe_layout == layout && rt->switch_feel == feel)
        return;
    rt->keyframe_attached = !!attached;
    rt->keyframe_layout = layout;
    rt->lifecycle.attach_phase = attached ? 3U : 0U;
    rt->switch_feel = feel;
    rt->rest_ref_valid = 0U;
    rt->lifecycle.calibrated = false;
    rt->classic_setup = FW_CLASSIC_ATTACH;
    memset(rt->classic_keys, 0, sizeof(rt->classic_keys));
    memset(rt->classic_neighbors, 0, sizeof(rt->classic_neighbors));
    memset(rt->key_pressed, 0, sizeof(rt->key_pressed));
    memset(rt->latched_valid, 0, sizeof(rt->latched_valid));
    memset(rt->latched_action, 0, sizeof(rt->latched_action));
    memset(rt->rapid, 0, sizeof(rt->rapid));
    memset(rt->motion, 0, sizeof(rt->motion));
    memset(rt->neighbor_vetoes, 0, sizeof(rt->neighbor_vetoes));
    memset(rt->key_bitmap, 0, sizeof(rt->key_bitmap));
    memset(rt->flux, 0, sizeof(rt->flux));
    rt->effective_layer_bitfield =
        (rt->active_layer_bitfield ? rt->active_layer_bitfield : 1U) | rt->toggled_layer_bitfield;
    fw_runtime_reset_tracking(rt);
}

void fw_runtime_set_time(fw_runtime_t *rt, uint32_t now_ms) {
    if (!rt)
        return;
    rt->macro_clock_ms = now_ms;
    rt->macro_clock_updated = 1U;
}

void fw_runtime_set_hid_ready(fw_runtime_t *rt, uint8_t ready) {
    if (!rt)
        return;
    rt->macro_hid_ready = ready ? 1U : 0U;
}

static int parse_keymap_sections(const uint8_t *blob, size_t len, fw_layout_view_t out_parsed[4],
                                 uint32_t out_offsets[4]) {
    if (!blob || len < 24U || len > FW_MAX_KEYMAP_BYTES || (len % 4U) != 0U) {
        return -1;
    }
    size_t total_words = len / 4U;
    uint32_t num_layouts = rd_le32(blob + 0);
    uint32_t bitmask = rd_le32(blob + 4);
    if (num_layouts == 0U || num_layouts > 3U) {
        return -1;
    }
    out_offsets[0] = rd_le32(blob + 8);
    out_offsets[1] = rd_le32(blob + 12);
    out_offsets[2] = rd_le32(blob + 16);
    out_offsets[3] = rd_le32(blob + 20);

    memset(out_parsed, 0, sizeof(fw_layout_view_t) * 4U);
    uint32_t active_count = 0;
    uint32_t running_word_off = 0;
    for (uint32_t lid = 1U; lid <= 3U; lid++) {
        if (!((bitmask >> (lid - 1U)) & 1U)) {
            if (out_offsets[lid] != 0U)
                return -1;
            continue;
        }
        active_count++;
        if (out_offsets[lid] != running_word_off) {
            return -1;
        }
        size_t sec_word = 6U + running_word_off;
        if (sec_word + 7U > total_words) {
            return -1;
        }
        const uint8_t *sp = blob + sec_word * 4U;
        uint32_t n_layers = rd_le32(sp + 0);
        uint32_t n_keys = rd_le32(sp + 4);
        uint32_t n_act = rd_le32(sp + 8);
        uint32_t n_macros = rd_le32(sp + 12);
        uint32_t kc_off = rd_le32(sp + 20);
        uint32_t act_off = rd_le32(sp + 24);
        if (n_keys == 0U || n_keys > FW_MAX_KEYS_PER_LAYOUT || n_layers == 0U || n_layers > FW_MAX_LAYERS ||
            n_act == 0U || n_act > FW_MAX_ACT_MODES || n_macros > 512U) {
            return -1;
        }
        if (n_macros == 0U && kc_off != 0U)
            return -1;
        if (n_macros > 0U && kc_off < n_macros)
            return -1;
        size_t avail_after_hdr = total_words - sec_word - 7U;
        if ((uint64_t)kc_off > avail_after_hdr)
            return -1;
        uint64_t expected_act_off = (uint64_t)kc_off + (uint64_t)n_keys * (uint64_t)n_layers;
        if ((uint64_t)act_off != expected_act_off || expected_act_off > avail_after_hdr)
            return -1;
        uint64_t act_words = (uint64_t)n_act * (uint64_t)n_keys;
        if (act_words > avail_after_hdr - (uint64_t)act_off)
            return -1;
        uint32_t sec_len_words = (uint32_t)(7ULL + (uint64_t)act_off + act_words);
        if (kc_off > 0U) {
            const uint8_t *mp = sp + 7U * 4U;
            uint32_t m_pos = n_macros;
            for (uint32_t mi = 0; mi < n_macros; mi++) {
                if (rd_le32(mp + mi * 4U) != m_pos || m_pos >= kc_off) {
                    return -1;
                }
                uint32_t mlen = rd_le32(mp + m_pos * 4U);
                if (mlen > (kc_off - m_pos - 1U))
                    return -1;
                m_pos += 1U + mlen;
            }
            if (m_pos != kc_off)
                return -1;
        }
        out_parsed[lid].valid = 1U;
        out_parsed[lid].num_act_modes = (uint16_t)n_act;
        out_parsed[lid].num_keys = (uint16_t)n_keys;
        out_parsed[lid].num_layers = (uint16_t)n_layers;
        out_parsed[lid].num_macros = (uint16_t)n_macros;
        out_parsed[lid].macro_pool_words = kc_off;
        out_parsed[lid].keycodes_word_off = 7U + kc_off;
        out_parsed[lid].actuations_word_off = 7U + act_off;
        running_word_off += sec_len_words;
    }
    if (active_count != num_layouts || out_offsets[0] != 0U) {
        return -1;
    }
    return 0;
}

int fw_runtime_validate_keymap(const uint8_t *blob, size_t len) {
    fw_layout_view_t parsed[4];
    uint32_t offsets[4];
    return parse_keymap_sections(blob, len, parsed, offsets);
}

static void reload_actuation(fw_runtime_t *rt) {
    if (rt->classic_setup == FW_CLASSIC_READY)
        rt->classic_setup = FW_CLASSIC_RELOAD;
    memset(rt->fixed_transition_scans, 0, sizeof(rt->fixed_transition_scans));
    for (size_t k = 0; k < FW_MAX_KEYS_PER_LAYOUT; k++)
        fw_rapid_rearm(&rt->rapid[k]);
}

int fw_runtime_load_keymap(fw_runtime_t *rt, const uint8_t *blob, size_t len) {
    if (!rt)
        return -1;
    fw_layout_view_t parsed[4];
    uint32_t offsets[4];
    if (parse_keymap_sections(blob, len, parsed, offsets) != 0) {
        return -1;
    }
    memcpy(rt->keymap_blob, blob, len);
    rt->toggled_layer_bitfield = 0U;
    rt->macro_active = 0U;
    memset(rt->macro_nkro_bitmap, 0, sizeof(rt->macro_nkro_bitmap));
    reload_actuation(rt);
    if (rt->keyframe_attached)
        rt->classic_setup = FW_CLASSIC_REPLACED;
    rt->keymap_len = len;
    memcpy(rt->layouts, parsed, sizeof(parsed));
    for (uint32_t lid = 1U; lid <= 3U; lid++) {
        if (rt->layouts[lid].valid) {
            rt->layouts[lid].section_words =
                (const uint32_t *)(const void *)(rt->keymap_blob + (6U + offsets[lid]) * 4U);
        }
    }
    return 0;
}

int fw_runtime_validate_profile_settings(const uint8_t *xfer, size_t len) {
    if (!xfer || len < 32U) {
        return -1;
    }
    uint32_t file_type = rd_le32(xfer + 4U);
    if (file_type != 0U) {
        return -1;
    }
    uint32_t num_profiles = rd_le32(xfer + 16U);
    if (num_profiles > FW_MAX_PROFILES || len != 32U + num_profiles * 8U) {
        return -1;
    }
    return 0;
}

int fw_runtime_load_profile_settings(fw_runtime_t *rt, const uint8_t *xfer, size_t len) {
    if (!rt || fw_runtime_validate_profile_settings(xfer, len) != 0) {
        return -1;
    }
    uint32_t num_profiles = rd_le32(xfer + 16U);
    rt->num_profiles = num_profiles;
    for (uint32_t i = 0; i < num_profiles; i++) {
        rt->profiles[i].actuation_mode_index = rd_le32(xfer + 32U + i * 8U);
        rt->profiles[i].default_layer_bitfield = rd_le32(xfer + 32U + i * 8U + 4U);
    }
    (void)fw_runtime_set_active_profile(rt, rt->active_profile);
    return 0;
}

int fw_runtime_export_profile_settings(const fw_runtime_t *rt, uint8_t *out, size_t cap, size_t *out_len) {
    if (!rt || !out)
        return -1;
    size_t need = 16U + rt->num_profiles * 8U;
    if (cap < need)
        return -1;
    memset(out, 0, 16U);
    wr_le32(out + 0, rt->num_profiles);
    for (uint32_t i = 0; i < rt->num_profiles; i++) {
        wr_le32(out + 16U + i * 8U, rt->profiles[i].actuation_mode_index);
        wr_le32(out + 16U + i * 8U + 4U, rt->profiles[i].default_layer_bitfield);
    }
    if (out_len)
        *out_len = need;
    return 0;
}

int fw_runtime_set_active_profile(fw_runtime_t *rt, uint32_t profile_idx) {
    if (!rt)
        return -1;
    rt->active_profile = profile_idx;
    rt->toggled_layer_bitfield = 0U;
    if (profile_idx < rt->num_profiles) {
        rt->active_actuation_mode = rt->profiles[profile_idx].actuation_mode_index;
        if (rt->profiles[profile_idx].default_layer_bitfield != 0U) {
            rt->active_layer_bitfield = rt->profiles[profile_idx].default_layer_bitfield;
        }
    }
    rt->effective_layer_bitfield = rt->active_layer_bitfield ? rt->active_layer_bitfield : 1U;
    return 0;
}

#if FW_ENABLE_PRESS_REFINEMENT
/* A marginal transition in a chord needs a longer uninterrupted run of evidence.
 * Use the held count from the start of the scan so key order cannot affect it. */
static void confirm_fixed_key(fw_runtime_t *rt, uint16_t k, int32_t flux, uint16_t act, int32_t release,
                              unsigned held_count, int32_t neighbor_delta) {
    int down = rt->key_pressed[k] != 0;
    int proposed =
        fw_fixed_candidate(down, flux, act, release, neighbor_delta, &rt->motion[k], &rt->neighbor_vetoes[k]);
    if (proposed == down) {
        rt->fixed_transition_scans[k] = 0;
        return;
    }
    int strong = down ? 100 * flux < 75 * release : 100 * flux > 175 * act;
    unsigned required = held_count > 2 && !strong ? 17U : 3U;
    if (++rt->fixed_transition_scans[k] >= required) {
        rt->key_pressed[k] = (uint8_t)proposed;
        rt->fixed_transition_scans[k] = 0;
    }
}

#endif

/* Position gains are Q8. The wider off-centre keys have a second gain on top of the row gain. */
static uint16_t position_gain(uint8_t layout, uint8_t row, uint8_t col) {
    static const uint16_t FW_SCAN_TABLE(row_gain)[FW_MATRIX_ROWS] = {256, 253, 250, 246, 242, 234};
    static const uint16_t FW_SCAN_TABLE(reduced)[2][FW_MATRIX_ROWS] = {
        {0, 1U << 13, 1U | (1U << 13), 1U | (1U << 12), 1U | (1U << 11), 1U},
        {0, 1U << 13, 1U | (1U << 13), 1U | (1U << 13), 1U | (1U << 12), 1U},
    };
    return (reduced[layout == 2U][row] & (1U << col)) ? (row_gain[row] * 224U) >> 8 : row_gain[row];
}

#if FW_ENABLE_PRESS_REFINEMENT
static void evaluate_key(fw_runtime_t *rt, uint16_t k, uint8_t layout, uint8_t row, uint8_t col, unsigned row_keys,
                         uint32_t word, unsigned held_count, uint8_t row_active) {
    unsigned atype = word >> 30;
    int has_setting = atype == 1U || atype == 2U;
    uint16_t gain = position_gain(layout, row, col);
    uint16_t act_thr = ((has_setting ? (word & 0xfffU) : 650U) * gain) >> 8;
    uint16_t reversal = has_setting ? ((word >> 12) & 0xfffU) : 216U;
    uint16_t reset_thr = (reversal * gain) >> 8;
    uint16_t flux =
        fw_runtime_guard_flux(rt, row, col, rt->flux[k], gain, rt->key_pressed[k] != 0, atype == 2U, rt->rapid[k].peak);
    int32_t neighbor_delta = fw_neighbor_delta(rt->motion + (k - col), col, row_keys);
    if (atype == 2U) {
        fw_rapid_input_t input = {
            .flux = flux,
            .actuation = act_thr,
            .hysteresis = reset_thr,
            .sensitivity = reversal,
            .weight_q8 = gain,
            .row_active = row_active,
            .motion = rt->motion + k,
            .neighbor_delta = neighbor_delta,
        };
        rt->key_pressed[k] = fw_rapid_down(rt->rapid + k, rt->key_pressed[k] != 0, &input);
    } else {
        confirm_fixed_key(rt, k, flux, act_thr, (int32_t)act_thr - reset_thr, held_count, neighbor_delta);
    }

    bool down = rt->key_pressed[k] != 0;
    bool proposed = (atype != 2U && rt->fixed_transition_scans[k]) ? !down : down;
    fw_runtime_observe_key(rt, row, col, rt->flux[k], down, proposed);
    if (down) {
        rt->key_bitmap[k >> 3] |= (uint8_t)(1U << (k & 7U));
    }
}
#endif

static int fw_handle_of_function(fw_runtime_t *rt, const fw_layout_view_t *lv, uint8_t func_code) {
    switch (func_code) {
        case FW_OF_FUNC_PROFILE_CYCLE_NEXT:
            if (rt->num_profiles > 0U) {
                uint32_t next_p = (rt->active_profile + 1U) % rt->num_profiles;
                (void)fw_runtime_set_active_profile(rt, next_p);
                return 1;
            }
            return 0;
        case FW_OF_FUNC_PROFILE_CYCLE_PREV:
            if (rt->num_profiles > 0U) {
                uint32_t prev_p = (rt->active_profile + rt->num_profiles - 1U) % rt->num_profiles;
                (void)fw_runtime_set_active_profile(rt, prev_p);
                return 1;
            }
            return 0;
        case FW_OF_FUNC_ACTUATION_MODE_NEXT:
            if (lv && lv->valid && lv->num_act_modes > 0U) {
                rt->active_actuation_mode = (rt->active_actuation_mode + 1U) % (uint32_t)lv->num_act_modes;
            }
            return 0;
        case FW_OF_FUNC_ACTUATION_MODE_PREV:
            if (lv && lv->valid && lv->num_act_modes > 0U) {
                uint32_t nm = (uint32_t)lv->num_act_modes;
                rt->active_actuation_mode = (rt->active_actuation_mode + nm - 1U) % nm;
            }
            return 0;
        case FW_OF_FUNC_PLATFORM_LINUX:
            if (rt->platform_code != 3U) {
                rt->platform_code = 3U;
                rt->platform_dirty = 1U;
            }
            return 0;
        case FW_OF_FUNC_PLATFORM_MACOS:
            if (rt->platform_code != 2U) {
                rt->platform_code = 2U;
                rt->platform_dirty = 1U;
            }
            return 0;
        case FW_OF_FUNC_PLATFORM_WINDOWS:
            if (rt->platform_code != 1U) {
                rt->platform_code = 1U;
                rt->platform_dirty = 1U;
            }
            return 0;
        default:
            return 0;
    }
}

static uint16_t report_flux(int32_t value) {
    return value <= 0 ? 0 : value > UINT16_MAX ? UINT16_MAX : (uint16_t)value;
}

static int32_t apply_feel(int32_t flux, bool stronger) {
    return stronger ? (int32_t)((uint32_t)flux * 307U) >> 8 : flux;
}

static uint32_t setting_word(const fw_layout_view_t *lv, uint32_t mode, unsigned key) {
    if (!lv || key >= lv->num_keys)
        return 0;
    size_t i = lv->actuations_word_off + mode * lv->num_keys + key;
    return rd_le32((const uint8_t *)lv->section_words + 4 * i);
}

/* Both trackers consume the same calibrated sensor counts. The model-free tracker
 * conditions each row before inferring key travel, correcting adjacent switches,
 * then committing new key states against the previous scan's occupancy. */
static uint16_t FW_SCAN_CODE(scan_classic)(fw_runtime_t *rt, const fw_layout_view_t *lv, uint32_t mode,
                                           const uint8_t *widths, const uint8_t map[][FW_MATRIX_MAX_COLS],
                                           bool stronger) {
    /* An attached replacement resets the active table to zero for one decision
     * scan. The current profile's mode is applied at the next scan boundary. */
    bool replaced = rt->classic_setup == FW_CLASSIC_REPLACED;
    uint32_t table_mode = replaced ? 0 : mode;
    uint16_t base = 0;
    unsigned held = 0;
    for (unsigned i = 0; i < FW_MAX_KEYS_PER_LAYOUT; ++i)
        held += rt->key_pressed[i] != 0;
    for (unsigned r = 0; r < FW_MATRIX_ROWS; ++r) {
        unsigned width = widths[r];
        if (width < 2 || width > FW_MATRIX_MAX_COLS)
            continue;
        unsigned n = width - 1;
        fw_classic_settings_t *cfg = &rt->classic_settings[r];
        fw_classic_keys_t *keys = &rt->classic_keys[r];
        int32_t field[FW_MATRIX_MAX_COLS], travel[FW_MATRIX_MAX_COLS - 1];
        for (unsigned c = 0; c < width; ++c)
            field[c] = map[r][c] < FW_NUM_SENSORS ? rt->sensor_scaled[map[r][c]] : 0;
        if (rt->classic_setup != FW_CLASSIC_READY) {
            for (unsigned c = 0; c < n; ++c) {
                unsigned k = base + c;
                fw_classic_key_load(cfg, keys, rt->keyframe_layout, r, c, lv && k < lv->num_keys,
                                    setting_word(lv, table_mode, k));
                /* Changing the active table while attached reapplies its position gain. */
                if (rt->classic_setup != FW_CLASSIC_ATTACH)
                    fw_classic_key_weight(cfg, rt->keyframe_layout, r, c);
            }
        }
        fw_classic_row(r, width, field, field, travel);
        for (unsigned c = 0; c < n; ++c)
            travel[c] = apply_feel(travel[c], stronger);
        fw_classic_neighbors(&rt->classic_neighbors[r], rt->keyframe_layout, r, n, field, travel, keys->down,
                             cfg->actuation, cfg->hysteresis);
        for (unsigned c = 0; c < n; ++c) {
            uint16_t f = rt->flux[base + c] = report_flux(travel[c]);
            uint16_t guarded = fw_runtime_guard_flux(rt, r, c, f, position_gain(rt->keyframe_layout, r, c),
                                                     keys->down[c], cfg->rapid[c], report_flux(keys->reversal[c].peak));
            if (guarded != f)
                travel[c] = guarded;
        }
        fw_classic_keys_step(keys, cfg, r, n, held, travel);
        for (unsigned c = 0; c < n; ++c) {
            unsigned k = base + c;
            rt->key_pressed[k] = keys->down[c];
            if (keys->down[c])
                rt->key_bitmap[k >> 3] |= (uint8_t)(1U << (k & 7U));
            fw_runtime_observe_key(rt, r, c, rt->flux[k], keys->down[c], keys->pending[c]);
        }
        base += n;
    }
    rt->classic_setup = replaced && mode != 0 ? FW_CLASSIC_RELOAD : FW_CLASSIC_READY;
    return base;
}

#if FW_ENABLE_PRESS_REFINEMENT
static uint16_t FW_SCAN_CODE(scan_refined)(fw_runtime_t *rt, const fw_layout_view_t *lv, uint32_t act_mode,
                                           const uint8_t *row_lens, const uint8_t smap[][FW_MATRIX_MAX_COLS],
                                           bool stronger) {
    uint8_t lid = rt->keyframe_layout;
    uint16_t logical_k = 0;
    for (uint8_t row = 0; row < FW_MATRIX_ROWS; row++) {
        uint8_t rlen = row_lens[row];
        if (rlen <= 1U || rlen > FW_MATRIX_MAX_COLS)
            continue;

        int32_t rvals[FW_MATRIX_MAX_COLS];
        for (uint8_t c = 0; c < rlen; c++) {
            uint8_t phys = smap[row][c];
            rvals[c] = (phys < FW_NUM_SENSORS) ? rt->sensor_scaled[phys] : 0;
        }
        fw_condition_row_q4(rvals, rlen);
        int32_t travel[FW_MATRIX_MAX_COLS - 1];
        fw_depth_row_update(&rt->depth_rows[row], rlen - 1U, rvals, travel);
        for (uint8_t c = 0; c < rlen; ++c)
            rvals[c] /= 16;
        fw_refine_step(&rt->refinement, &rt->refine_rows[row], rvals, travel, rlen, travel);
        for (uint8_t c = 0; c + 1U < rlen; ++c)
            rt->flux[logical_k++] = report_flux(apply_feel(travel[c], stronger));
    }
    uint16_t n_keys = logical_k;
    unsigned held_count = 0;
    for (uint16_t k = 0; k < n_keys; k++)
        held_count += rt->key_pressed[k] != 0;

    uint16_t k = 0;
    for (uint8_t row = 0; row < FW_MATRIX_ROWS; row++) {
        uint8_t len = row_lens[row];
        if (len <= 1U || len > FW_MATRIX_MAX_COLS || k >= n_keys)
            continue;
        unsigned row_keys = len - 1U;
        if (row_keys > (unsigned)(n_keys - k))
            row_keys = n_keys - k;
        fw_update_row_motion(rt->motion + k, rt->flux + k, row_keys);
        uint8_t row_active = 0;
        for (unsigned c = 0; c < row_keys; c++)
            row_active += rt->key_pressed[k + c] || rt->fixed_transition_scans[k + c];
        for (uint8_t col = 0; col < row_keys; col++, k++) {
            uint32_t word = setting_word(lv, act_mode, k);
            evaluate_key(rt, k, lid, row, col, row_keys, word, held_count, row_active);
        }
    }

    return n_keys;
}
#endif

void FW_SCAN_CODE(fw_runtime_process_scan)(fw_runtime_t *rt, const uint16_t raw_adc[FW_NUM_SENSORS],
                                           fw_hid_report_t *out_hid) {
    if (!rt || !raw_adc || !out_hid)
        return;
    memcpy(rt->raw_adc, raw_adc, sizeof(rt->raw_adc));
    memset(out_hid, 0, sizeof(*out_hid));
    memset(rt->key_bitmap, 0, sizeof(rt->key_bitmap));
    memset(rt->flux, 0, sizeof(rt->flux));

    if (!rt->keyframe_attached) {
        memset(rt->fixed_transition_scans, 0, sizeof(rt->fixed_transition_scans));
        memset(rt->key_pressed, 0, sizeof(rt->key_pressed));
        memset(rt->latched_valid, 0, sizeof(rt->latched_valid));
        memset(rt->latched_action, 0, sizeof(rt->latched_action));
        memset(rt->rapid, 0, sizeof(rt->rapid));
        memset(rt->motion, 0, sizeof(rt->motion));
        memset(rt->neighbor_vetoes, 0, sizeof(rt->neighbor_vetoes));
        rt->macro_active = 0U;
        memset(rt->macro_nkro_bitmap, 0, sizeof(rt->macro_nkro_bitmap));
        rt->effective_layer_bitfield =
            (rt->active_layer_bitfield ? rt->active_layer_bitfield : 1U) | rt->toggled_layer_bitfield;
        return;
    }

    if (!rt->macro_clock_updated) {
        rt->macro_clock_ms++;
    }
    rt->macro_clock_updated = 0U;

    /* Stage 1 & 2: signed calibrated delta per physical ADC sensor (0..159). */
    for (size_t s = 0; s < FW_NUM_SENSORS; s++) {
        int32_t d = (int32_t)raw_adc[s] - (int32_t)rt->baseline_adc[s];
        int32_t scaled = (d * (int32_t)rt->cal.factors[s]) >> 8;
        if (scaled >= -2 && scaled <= 2) {
            scaled = 0;
        } else if (scaled > 2) {
            scaled -= 3;
        } else {
            scaled += 3;
        }
        rt->sensor_scaled[s] = scaled;
    }

    /* The frame sets the physical sensor coordinates. A missing matching keymap
     * section uses default actuation and emits no mapped actions. */
    const uint8_t lid = rt->keyframe_layout;
    if (lid < 1U || lid > 3U)
        return;
    const fw_layout_view_t *lv = rt->layouts[lid].valid ? &rt->layouts[lid] : NULL;
    const uint8_t *row_lens = FW_ROW_LEN[lid];
    const uint8_t(*smap)[FW_MATRIX_MAX_COLS] = FW_SENSOR_MAP[lid];
#ifdef HOST_TEST
    if (rt->test_row_len)
        row_lens = rt->test_row_len;
    if (rt->test_sensor_map)
        smap = rt->test_sensor_map;
#endif

    uint32_t act_mode = rt->active_actuation_mode;
    if (lv && act_mode >= lv->num_act_modes)
        act_mode = 0;
    if (rt->last_scan_mode != act_mode) {
        reload_actuation(rt);
        rt->last_scan_mode = (uint16_t)act_mode;
    }
    bool stronger = rt->switch_feel == 2 && rt->cal.loaded_from_flash && rt->cal.version == 2U;
    uint16_t n_keys;
#if FW_ENABLE_PRESS_REFINEMENT
    if (rt->refinement_active)
        n_keys = scan_refined(rt, lv, act_mode, row_lens, smap, stronger);
    else
#endif
        n_keys = scan_classic(rt, lv, act_mode, row_lens, smap, stronger);

    if (!lv || !lv->section_words) {
        memset(rt->latched_valid, 0, sizeof(rt->latched_valid));
        memset(rt->latched_action, 0, sizeof(rt->latched_action));
        rt->macro_active = 0U;
        memset(rt->macro_nkro_bitmap, 0, sizeof(rt->macro_nkro_bitmap));
        rt->effective_layer_bitfield =
            (rt->active_layer_bitfield ? rt->active_layer_bitfield : 1U) | rt->toggled_layer_bitfield;
        return;
    }

    if (n_keys > lv->num_keys)
        n_keys = lv->num_keys;

    /* Clear released keys' latched actions first, and collect active layers from already-held momentary layer keys. */
    uint32_t eff_mask = (rt->active_layer_bitfield ? rt->active_layer_bitfield : 1U) | rt->toggled_layer_bitfield;
    uint32_t held_layer_mask = 0U;
    for (uint16_t k = 0; k < n_keys; k++) {
        if (!rt->key_pressed[k]) {
            rt->latched_valid[k] = 0U;
            rt->latched_action[k] = 0U;
        } else if (rt->latched_valid[k]) {
            uint32_t kc = rt->latched_action[k];
            if (((kc >> 28) & 0xFU) == 0x2U) {
                uint8_t sw_type = (uint8_t)((kc >> 24) & 0xFU);
                uint8_t target = (uint8_t)(kc & 0xFFU);
                if (sw_type <= 1U && target < lv->num_layers) {
                    held_layer_mask |= (1U << target);
                    eff_mask |= (1U << target);
                }
            }
        }
    }

    /* Latch newly pressed layer-switch keys to fixed point (press-time only, never self-chaining at one key),
     * then latch remaining newly pressed ordinary keys against the complete effective layer mask. */
    uint32_t lookup_mask = eff_mask | 1U;
    for (uint16_t pass = 0; pass < lv->num_layers; pass++) {
        int changed = 0;
        for (uint16_t k = 0; k < n_keys; k++) {
            if (!rt->key_pressed[k] || rt->latched_valid[k])
                continue;
            uint32_t kc = 0U;
            for (int l = (int)lv->num_layers - 1; l >= 0; l--) {
                if (!((lookup_mask >> (uint32_t)l) & 1U))
                    continue;
                const uint8_t *kp =
                    (const uint8_t *)lv->section_words + (lv->keycodes_word_off + (uint32_t)l * lv->num_keys + k) * 4U;
                uint32_t cand = rd_le32(kp);
                if (cand == 0x50000000U)
                    continue; /* KC_TRANSPARENT falls through */
                kc = cand;
                break;
            }
            if (((kc >> 28) & 0xFU) == 0x2U) {
                rt->latched_action[k] = kc;
                rt->latched_valid[k] = 1U;
                uint8_t sw_type = (uint8_t)((kc >> 24) & 0xFU);
                uint8_t target = (uint8_t)(kc & 0xFFU);
                if (target < lv->num_layers) {
                    if (sw_type == 2U) {
                        rt->toggled_layer_bitfield ^= (1U << target);
                    } else if (sw_type == 3U) {
                        rt->active_layer_bitfield = (1U << target);
                        rt->toggled_layer_bitfield = 0U;
                    } else if (sw_type == 4U) {
                        if (rt->toggled_layer_bitfield & (1U << target)) {
                            rt->toggled_layer_bitfield &= ~(1U << target);
                        } else {
                            rt->toggled_layer_bitfield = (1U << target);
                        }
                    } else {
                        held_layer_mask |= (1U << target);
                    }
                    eff_mask = (rt->active_layer_bitfield ? rt->active_layer_bitfield : 1U) |
                               rt->toggled_layer_bitfield | held_layer_mask;
                    lookup_mask = eff_mask | 1U;
                }
                changed = 1;
            }
        }
        if (!changed)
            break;
    }
    for (uint16_t k = 0; k < n_keys; k++) {
        if (!rt->key_pressed[k] || rt->latched_valid[k])
            continue;
        uint32_t kc = 0U;
        for (int l = (int)lv->num_layers - 1; l >= 0; l--) {
            if (!((lookup_mask >> (uint32_t)l) & 1U))
                continue;
            const uint8_t *kp =
                (const uint8_t *)lv->section_words + (lv->keycodes_word_off + (uint32_t)l * lv->num_keys + k) * 4U;
            uint32_t cand = rd_le32(kp);
            if (cand == 0x50000000U)
                continue;
            kc = cand;
            break;
        }
        rt->latched_action[k] = kc;
        rt->latched_valid[k] = 1U;
        uint8_t kind = (uint8_t)((kc >> 28) & 0xFU);
        if (kind == 0x1U) {
            uint16_t mid = (uint16_t)(kc & 0xFFFFU);
            if (mid < lv->num_macros) {
                uint32_t m_off = rd_le32((const uint8_t *)lv->section_words + (7U + (uint32_t)mid) * 4U);
                uint32_t m_len = rd_le32((const uint8_t *)lv->section_words + (7U + m_off) * 4U);
                memset(rt->macro_nkro_bitmap, 0, sizeof(rt->macro_nkro_bitmap));
                if (m_len > 0U) {
                    uint32_t step0 = rd_le32((const uint8_t *)lv->section_words + (7U + m_off + 1U) * 4U);
                    uint32_t d0 = (step0 >> 8) & 0xFFFFU;
                    rt->macro_active = 1U;
                    rt->macro_layout = lid;
                    rt->macro_id = mid;
                    rt->macro_step_idx = 0U;
                    rt->macro_next_ms = rt->macro_clock_ms + d0;
                } else {
                    rt->macro_active = 0U;
                }
            }
        } else if (kind == 0x5U) {
            uint8_t fcode = (uint8_t)(kc & 0xFFU);
            if (fw_handle_of_function(rt, lv, fcode)) {
                uint32_t base_l = rt->active_layer_bitfield ? rt->active_layer_bitfield : 1U;
                eff_mask = base_l | rt->toggled_layer_bitfield | held_layer_mask;
            }
            if (fcode > 0U && fcode < 32U) {
                uint8_t slot = (uint8_t)(rt->of_event_seq & 15U);
                rt->of_event_ring[slot] = fcode;
                rt->of_event_profiles[slot] = (uint8_t)rt->active_profile;
                rt->of_event_seq++;
            }
        }
    }
    rt->effective_layer_bitfield = eff_mask;

    /* Advance active macro playback at most one step per scan and only when the keyboard HID
     * endpoint can transmit this scan's report, so no intermediate press/release is overwritten. */
    if (rt->macro_active) {
        if (rt->macro_layout != lid || rt->macro_id >= lv->num_macros) {
            rt->macro_active = 0U;
            memset(rt->macro_nkro_bitmap, 0, sizeof(rt->macro_nkro_bitmap));
        } else if (rt->macro_hid_ready) {
            uint32_t m_off = rd_le32((const uint8_t *)lv->section_words + (7U + (uint32_t)rt->macro_id) * 4U);
            uint32_t m_len = rd_le32((const uint8_t *)lv->section_words + (7U + m_off) * 4U);
            if (rt->macro_step_idx >= m_len) {
                rt->macro_active = 0U;
                memset(rt->macro_nkro_bitmap, 0, sizeof(rt->macro_nkro_bitmap));
            } else if ((int32_t)(rt->macro_clock_ms - rt->macro_next_ms) >= 0) {
                uint32_t step =
                    rd_le32((const uint8_t *)lv->section_words + (7U + m_off + 1U + rt->macro_step_idx) * 4U);
                uint8_t is_down = (uint8_t)((step >> 31) & 1U);
                uint8_t usage = (uint8_t)(step & 0xFFU);
                if (usage > 0U) {
                    if (is_down) {
                        rt->macro_nkro_bitmap[usage >> 3] |= (uint8_t)(1U << (usage & 7U));
                    } else {
                        rt->macro_nkro_bitmap[usage >> 3] &= (uint8_t)(~(1U << (usage & 7U)));
                    }
                }
                rt->macro_step_idx++;
                if (rt->macro_step_idx < m_len) {
                    uint32_t nxt =
                        rd_le32((const uint8_t *)lv->section_words + (7U + m_off + 1U + rt->macro_step_idx) * 4U);
                    rt->macro_next_ms = rt->macro_clock_ms + ((nxt >> 8) & 0xFFFFU);
                }
            }
        }
    }

    size_t nk = 0;
    for (uint16_t k = 0; k < n_keys; k++) {
        if (!rt->key_pressed[k] || !rt->latched_valid[k])
            continue;
        uint32_t kc = rt->latched_action[k];
        /* 0xF0000000 is explicit disabled key (KC_NO); never fall back further. */
        if (kc == 0xF0000000U || kc == 0U) {
            continue;
        }
        uint8_t kind = (uint8_t)((kc >> 28) & 0xFU);
        if (kind == 0x0U) {
            uint16_t hid = (uint16_t)(kc & 0xFFFFU);
            uint8_t mods = (uint8_t)((kc >> 16) & 0xFFU);
            out_hid->modifiers |= mods;
            for (uint8_t mbit = 0; mbit < 8U; mbit++) {
                if ((mods >> mbit) & 1U) {
                    uint8_t mcode = (uint8_t)(0xE0U + mbit);
                    out_hid->nkro_bitmap[mcode >> 3] |= (uint8_t)(1U << (mcode & 7U));
                }
            }
            if (hid >= 0xE0U && hid <= 0xE7U) {
                out_hid->modifiers |= (uint8_t)(1U << (hid - 0xE0U));
                out_hid->nkro_bitmap[hid >> 3] |= (uint8_t)(1U << (hid & 7U));
            } else if (hid > 0U && hid <= 0xFFU) {
                out_hid->nkro_bitmap[hid >> 3] |= (uint8_t)(1U << (hid & 7U));
                if (hid < 0xE0U && nk < 6U) {
                    out_hid->keycodes[nk++] = (uint8_t)hid;
                }
            }
        } else if (kind == 0x4U && out_hid->consumer_usage == 0U) {
            out_hid->consumer_usage = (uint16_t)(kc & 0xFFFFU);
        } else if (kind == 0x5U) {
            uint8_t sub = (uint8_t)(kc & 0xFFU);
            if (sub > 0U && sub < 32U) {
                out_hid->of_func_mask |= (1UL << sub);
            }
        }
    }
    if (rt->macro_active) {
        for (uint16_t u = 1U; u <= 0xFFU; u++) {
            if ((rt->macro_nkro_bitmap[u >> 3] & (1U << (u & 7U))) != 0U) {
                out_hid->nkro_bitmap[u >> 3] |= (uint8_t)(1U << (u & 7U));
                if (u >= 0xE0U && u <= 0xE7U) {
                    out_hid->modifiers |= (uint8_t)(1U << (u - 0xE0U));
                } else if (u < 0xE0U && nk < 6U) {
                    int dup = 0;
                    for (size_t i = 0; i < nk; i++) {
                        if (out_hid->keycodes[i] == (uint8_t)u) {
                            dup = 1;
                            break;
                        }
                    }
                    if (!dup) {
                        out_hid->keycodes[nk++] = (uint8_t)u;
                    }
                }
            }
        }
    }
}

void fw_runtime_build_msg_update(fw_runtime_t *rt, uint32_t timestamp_ms, uint8_t out_buf[FW_UART_MSG_UPDATE_LEN]) {
    if (!rt || !out_buf)
        return;
    memset(out_buf, 0, FW_UART_MSG_UPDATE_LEN);
    out_buf[0] = FW_UART_MSG_UPDATE;
    wr_le32(out_buf + 1U, ++rt->uart_tx_seq);

    uint8_t *st = out_buf + 5U;
    memcpy(st + 0x00U, rt->key_bitmap, 32U);
    wr_le32(st + 0x20U, timestamp_ms);
    st[0x24U] = rt->keyframe_layout;
    st[0x25U] = rt->keyframe_attached;
    st[0x26U] = rt->switch_feel;
    uint32_t pub_layer = rt->effective_layer_bitfield ? rt->effective_layer_bitfield
                                                      : (rt->active_layer_bitfield ? rt->active_layer_bitfield : 1U);
    wr_le32(st + 0x27U, pub_layer);
    wr_le32(st + 0x2BU, rt->active_actuation_mode);
    st[0x2FU] = 1U; /* ordered of_event ring capability version */
    wr_le16(st + 0x30U, rt->of_event_seq);
    memcpy(st + 0x32U, rt->of_event_ring, 16U);
    memcpy(st + 0x42U, rt->of_event_profiles, 16U);
    st[0x5CU] = rt->host_state_status;
    wr_le32(st + 0x5DU, rt->language_code);
    wr_le32(st + 0x61U, rt->platform_code);
    wr_le32(st + 0x65U, rt->active_profile);
    st[0x6AU] = rt->lifecycle.attach_phase;
}

void fw_runtime_build_msg_qc(fw_runtime_t *rt, uint8_t out_buf[FW_UART_MSG_QC_LEN]) {
    if (!rt || !out_buf)
        return;
    memset(out_buf, 0, FW_UART_MSG_QC_LEN);
    out_buf[0] = FW_UART_MSG_QC_SENSORS;
    wr_le32(out_buf + 1U, ++rt->uart_tx_seq);
    for (size_t i = 0; i < FW_NUM_SENSORS; i++) {
        wr_le16(out_buf + 5U + i * 2U, rt->raw_adc[i]);
    }
}

void fw_runtime_handle_uart_response(fw_runtime_t *rt, const uint8_t *payload, size_t len) {
    if (!rt || !payload || len < 4U)
        return;
    uint8_t mtype = payload[0];
    if (mtype == FW_UART_MSG_UPDATE || mtype == FW_UART_MSG_QC_SENSORS) {
        rt->qc_mode = payload[2] ? 1U : 0U;
        rt->stress_test_enabled = payload[3] ? 1U : 0U;
    }
}
