#include "fw_gamepad.h"
#include "fw_protocol.h"
#include "fw_scanner.h"
#include "lfs_store.h"
#include "qmk/fw_qmk.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

typedef struct {
    uint8_t last_raw_tx[FW_QMK_VIAL_RAW_EPSIZE];
    uint32_t raw_tx_count;
    uint16_t last_flux_slot;
    bool last_flux_pressed;
    uint32_t flux_event_count;
} test_qmk_ctx_t;

static int test_ee_load(uint8_t layout_id, uint8_t *buf, size_t len, void *ctx) {
    (void)ctx;
    return fw_lfs_load_qmk_eeprom(layout_id, buf, len);
}

static int test_ee_save(uint8_t layout_id, const uint8_t *buf, size_t len, void *ctx) {
    (void)ctx;
    return fw_lfs_save_qmk_eeprom(layout_id, buf, len);
}

static void test_raw_hid_send(const uint8_t data[FW_QMK_VIAL_RAW_EPSIZE], void *ctx) {
    test_qmk_ctx_t *t = (test_qmk_ctx_t *)ctx;
    memcpy(t->last_raw_tx, data, FW_QMK_VIAL_RAW_EPSIZE);
    t->raw_tx_count++;
}

static void test_on_flux_action(uint16_t slot, bool pressed, uint8_t mods, uint32_t layers, void *ctx) {
    (void)mods;
    (void)layers;
    test_qmk_ctx_t *t = (test_qmk_ctx_t *)ctx;
    t->last_flux_slot = slot;
    t->last_flux_pressed = pressed;
    t->flux_event_count++;
}

static void vial_set_keycode(uint8_t layer, uint8_t row, uint8_t col, uint16_t kc, uint32_t now_ms) {
    uint8_t pkt[32] = {0};
    pkt[0] = 0x05U; /* id_dynamic_keymap_set_keycode */
    pkt[1] = layer;
    pkt[2] = row;
    pkt[3] = col;
    pkt[4] = (uint8_t)(kc >> 8);
    pkt[5] = (uint8_t)(kc & 0xFFU);
    fw_qmk_raw_hid_rx(pkt, now_ms);
}

static uint16_t vial_get_keycode(test_qmk_ctx_t *t, uint8_t layer, uint8_t row, uint8_t col, uint32_t now_ms) {
    uint8_t pkt[32] = {0};
    pkt[0] = 0x04U; /* id_dynamic_keymap_get_keycode */
    pkt[1] = layer;
    pkt[2] = row;
    pkt[3] = col;
    uint32_t before = t->raw_tx_count;
    fw_qmk_raw_hid_rx(pkt, now_ms);
    assert(t->raw_tx_count == before + 1U);
    return (uint16_t)(((uint16_t)t->last_raw_tx[4] << 8) | t->last_raw_tx[5]);
}

static void test_vial_id_and_dynamic_def_cache_bust(test_qmk_ctx_t *t) {
    /* 1. Query Vial keyboard ID & protocol version */
    uint8_t req[32] = {0};
    req[0] = 0xFEU; /* id_vial_prefix */
    req[1] = 0x00U; /* vial_get_keyboard_id */
    fw_qmk_raw_hid_rx(req, 10U);
    uint8_t uid_before[8];
    memcpy(uid_before, t->last_raw_tx + 4, 8U);

    /* 2. Query default XZ definition size & first 32 bytes (must begin with XZ magic FD 37 7A 58 5A 00) */
    memset(req, 0, sizeof(req));
    req[0] = 0xFEU;
    req[1] = 0x01U; /* vial_get_size */
    fw_qmk_raw_hid_rx(req, 11U);
    uint32_t def_sz = (uint32_t)t->last_raw_tx[0] | ((uint32_t)t->last_raw_tx[1] << 8) |
                      ((uint32_t)t->last_raw_tx[2] << 16) | ((uint32_t)t->last_raw_tx[3] << 24);
    assert(def_sz > 64U && def_sz < FW_QMK_MAX_VIAL_DEF_BYTES);

    memset(req, 0, sizeof(req));
    req[0] = 0xFEU;
    req[1] = 0x02U; /* vial_get_def, page 0 */
    fw_qmk_raw_hid_rx(req, 12U);
    static const uint8_t xz_magic[6] = {0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00};
    assert(memcmp(t->last_raw_tx, xz_magic, 6U) == 0);

    /* 3. Upload dynamic vial.json payload -> UID must change (cache busting) and new payload served */
    const uint8_t custom_def[16] = {0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10};
    assert(fw_qmk_set_vial_def(custom_def, sizeof(custom_def), NULL) == 0);

    memset(req, 0, sizeof(req));
    req[0] = 0xFEU;
    req[1] = 0x00U;
    fw_qmk_raw_hid_rx(req, 13U);
    assert(memcmp(uid_before, t->last_raw_tx + 4, 8U) != 0);

    memset(req, 0, sizeof(req));
    req[0] = 0xFEU;
    req[1] = 0x01U;
    fw_qmk_raw_hid_rx(req, 14U);
    assert(t->last_raw_tx[0] == sizeof(custom_def));
}

static void test_homerow_mods_and_rapid_trigger_guard(test_qmk_ctx_t *t) {
    /* ANSI row 3: Caps(0), A(1), S(2), D(3), F(4), G(5), H(6), J(7)...
     * Linear key index of row 3 col 4 ('F') = 16 + 15 + 15 + 4 = 50.
     * Linear key index of row 3 col 7 ('J') = 16 + 15 + 15 + 7 = 53. */
    const uint16_t key_f = 50U;
    const uint16_t key_j = 53U;
    assert(!fw_qmk_key_forbids_rapid_trigger(key_f));
    assert(!fw_qmk_key_forbids_rapid_trigger(key_j));

    /* Bind MT(MOD_LSFT, KC_F) = 0x2000 | (0x02 << 8) | 0x09 = 0x2209 on Layer 0 (row 3, col 4) via Vial. */
    vial_set_keycode(0U, 3U, 4U, 0x2209U, 100U);
    assert(vial_get_keycode(t, 0U, 3U, 4U, 101U) == 0x2209U);

    /* Dual-role rapid-trigger guard must activate on 'F' and leave 'J' unguarded. */
    assert(fw_qmk_key_forbids_rapid_trigger(key_f));
    assert(!fw_qmk_key_forbids_rapid_trigger(key_j));

    uint8_t pressed[96] = {0};
    fw_hid_report_t hid;

    /* 1. Quick tap on F (press at 200ms, release at 250ms) -> FIFO MUST emit KC_F (0x09) DOWN then UP */
    fw_qmk_clear_hid_queue();
    pressed[key_f] = 1U;
    fw_qmk_process_scan(pressed, 85U, 200U, &hid);
    assert(hid.modifiers == 0U);
    pressed[key_f] = 0U;
    fw_qmk_process_scan(pressed, 85U, 250U, &hid);
    {
        fw_hid_report_t q;
        bool saw_f_down = false;
        bool saw_f_up = false;
        while (fw_qmk_pop_hid_report(&q)) {
            bool f_bit = (q.nkro_bitmap[0x09U >> 3] & (1U << (0x09U & 7U))) != 0U;
            assert(q.modifiers == 0U);
            if (f_bit) {
                saw_f_down = true;
            } else if (saw_f_down) {
                saw_f_up = true;
            }
        }
        assert(saw_f_down && saw_f_up);
    }

    /* 1b. Exact regression test: solo taps on LALT_T(KC_E) + fast overlapping roll across
     * four home-row layer-taps (A=LT4, S=LT3, D=LT2, F=LT1) MUST emit every single letter! */
    vial_set_keycode(0U, 2U, 3U, 0x2408U, 300U); /* E = LALT_T(KC_E) */
    vial_set_keycode(0U, 3U, 1U, 0x4404U, 301U); /* A = LT(4, KC_A) */
    vial_set_keycode(0U, 3U, 2U, 0x4316U, 302U); /* S = LT(3, KC_S) */
    vial_set_keycode(0U, 3U, 3U, 0x4207U, 303U); /* D = LT(2, KC_D) */
    vial_set_keycode(0U, 3U, 4U, 0x4109U, 304U); /* F = LT(1, KC_F) */
    const uint16_t key_e = 16U + 15U + 3U;       /* 34 */
    const uint16_t key_a = 47U, key_s = 48U, key_d = 49U;

    /* Solo tap E 3 times -> must observe 3 distinct KC_E (0x08) press edges in FIFO */
    fw_qmk_clear_hid_queue();
    uint32_t t_ms = 400U;
    int e_presses = 0;
    bool e_was_down = false;
    for (int rep = 0; rep < 3; rep++) {
        pressed[key_e] = 1U;
        fw_qmk_process_scan(pressed, 85U, t_ms, &hid);
        t_ms += 40U;
        pressed[key_e] = 0U;
        fw_qmk_process_scan(pressed, 85U, t_ms, &hid);
        t_ms += 60U;
        fw_hid_report_t q;
        while (fw_qmk_pop_hid_report(&q)) {
            bool down = (q.nkro_bitmap[0x08U >> 3] & (1U << (0x08U & 7U))) != 0U;
            if (down && !e_was_down)
                e_presses++;
            e_was_down = down;
        }
    }
    assert(e_presses == 3);

    /* Fast overlapping roll: a -> s -> d -> f across 4 LT keys -> must emit a, s, d, f in order! */
    fw_qmk_clear_hid_queue();
    uint8_t emitted[16] = {0};
    size_t n_emitted = 0;
    uint8_t prev_bm[32] = {0};
    const uint16_t roll_keys[4] = {key_a, key_s, key_d, key_f};
    const uint8_t expect_kc[4] = {0x04U, 0x16U, 0x07U, 0x09U};
    for (int step = 0; step < 5; step++) {
        if (step < 4)
            pressed[roll_keys[step]] = 1U;
        if (step > 0)
            pressed[roll_keys[step - 1]] = 0U;
        fw_qmk_process_scan(pressed, 85U, t_ms, &hid);
        t_ms += 45U;
        fw_hid_report_t q;
        while (fw_qmk_pop_hid_report(&q)) {
            for (uint16_t u = 1U; u < 0xE0U; u++) {
                bool was = (prev_bm[u >> 3] & (1U << (u & 7U))) != 0U;
                bool now = (q.nkro_bitmap[u >> 3] & (1U << (u & 7U))) != 0U;
                if (now && !was && n_emitted < sizeof(emitted)) {
                    emitted[n_emitted++] = (uint8_t)u;
                }
            }
            memcpy(prev_bm, q.nkro_bitmap, sizeof(prev_bm));
        }
    }
    assert(n_emitted == 4U);
    assert(memcmp(emitted, expect_kc, 4U) == 0);

    /* Restore MT(MOD_LSFT, KC_F) on row 3 col 4 for hold test & persistence test below */
    vial_set_keycode(0U, 3U, 4U, 0x2209U, 950U);

    /* 2. Hold F past tapping term (200ms) after idle > FLOW_TAP_TERM + tap opposite-hand J -> Shift + J */
    pressed[key_f] = 1U;
    fw_qmk_process_scan(pressed, 85U, 1500U, &hid);
    fw_qmk_process_scan(pressed, 85U, 1750U, &hid);
    assert((hid.modifiers & 0x02U) != 0U);

    /* Verify live QMK state reports effective_mods = LShift even before J is pressed */
    fw_qmk_state_t qs;
    fw_qmk_get_state(&qs);
    assert((qs.effective_mods & 0x02U) != 0U);

    pressed[key_j] = 1U;
    fw_qmk_process_scan(pressed, 85U, 1760U, &hid);
    assert((hid.modifiers & 0x02U) != 0U);
    assert((hid.nkro_bitmap[0x0DU >> 3] & (1U << (0x0DU & 7U))) != 0U);

    /* Release both */
    pressed[key_j] = 0U;
    pressed[key_f] = 0U;
    fw_qmk_process_scan(pressed, 85U, 1800U, &hid);
    assert(hid.modifiers == 0U && hid.keycodes[0] == 0U);
}

static void test_flux_custom_action_no_os_keystroke(test_qmk_ctx_t *t) {
    /* Bind dynamic module slot 35 (USER35 = 0x7E23, within 14..38 dynamic block) on Layer 0 row 0 col 14. */
    vial_set_keycode(0U, 0U, 14U, 0x7E23U, 2000U);

    uint8_t pressed[96] = {0};
    fw_hid_report_t hid;
    uint32_t before = t->flux_event_count;

    pressed[14] = 1U;
    fw_qmk_process_scan(pressed, 85U, 2010U, &hid);
    assert(t->flux_event_count == before + 1U);
    assert(t->last_flux_slot == 35U && t->last_flux_pressed == true);
    /* Must NOT emit any OS keyboard keystroke or modifier */
    assert(hid.modifiers == 0U && hid.keycodes[0] == 0U && hid.consumer_usage == 0U);

    pressed[14] = 0U;
    fw_qmk_process_scan(pressed, 85U, 2040U, &hid);
    assert(t->flux_event_count == before + 2U);
    assert(t->last_flux_slot == 35U && t->last_flux_pressed == false);
}

static void test_analog_gamepad_math_and_curves(void) {
    fw_gamepad_config_t cfg;
    fw_gamepad_config_default(&cfg);
    cfg.mode = FW_GP_MODE_HID;
    cfg.deadzone_um = 200U;
    cfg.saturation_um = 2600U;
    cfg.curve = FW_GP_CURVE_LINEAR;
    cfg.radial_clamp = true;

    uint16_t flux[85] = {0};
    uint8_t pressed[85] = {0};
    uint16_t resolved[85] = {0};
    resolved[33] = FW_GP_LS_UP;    /* W */
    resolved[47] = FW_GP_LS_LEFT;  /* A */
    resolved[48] = FW_GP_LS_DOWN;  /* S */
    resolved[49] = FW_GP_LS_RIGHT; /* D */
    resolved[32] = FW_GP_LT;       /* Q */
    resolved[34] = FW_GP_RT;       /* E */
    resolved[78] = FW_GP_A;        /* Space */
    resolved[72] = FW_GP_DPAD_UP;
    resolved[84] = FW_GP_DPAD_RIGHT;

    fw_gamepad_report_t rep;

    /* 1. Inside deadzone -> zero stick, centered hat (8), zero buttons */
    flux[33] = 150U;
    fw_gamepad_evaluate(&cfg, flux, pressed, resolved, 85U, &rep);
    assert(rep.lx == 0 && rep.ly == 0 && rep.lt == 0 && rep.rt == 0 && rep.hat == 8U && rep.buttons == 0U);

    /* 2. Full diagonal W+D -> radial clamp normalizes magnitude to ~23170 each (0.707 * 32767) */
    flux[33] = 2800U;
    flux[49] = 2800U;
    flux[32] = 1400U; /* half travel -> ~50% trigger */
    pressed[78] = 1U; /* A button */
    pressed[72] = 1U; /* DPad Up */
    pressed[84] = 1U; /* DPad Right -> NE (1) */
    fw_gamepad_evaluate(&cfg, flux, pressed, resolved, 85U, &rep);
    assert(rep.lx > 23000 && rep.lx < 23300);
    assert(rep.ly < -23000 && rep.ly > -23300);
    assert(rep.lt > 30000 && rep.lt < 35000);
    assert(rep.hat == 1U); /* NE */
    assert((rep.buttons & 1U) == 1U);

    /* 3. Precision curve (t^2) at half travel must be significantly lower than Snappy (1-(1-t)^2) */
    memset(flux, 0, sizeof(flux));
    memset(pressed, 0, sizeof(pressed));
    flux[49] = 1400U; /* exact midpoint between 200 and 2600 */
    cfg.curve = FW_GP_CURVE_PRECISION;
    fw_gamepad_evaluate(&cfg, flux, pressed, resolved, 85U, &rep);
    int16_t prec_x = rep.lx;
    cfg.curve = FW_GP_CURVE_SNAPPY;
    fw_gamepad_evaluate(&cfg, flux, pressed, resolved, 85U, &rep);
    int16_t snap_x = rep.lx;
    assert(prec_x > 7500 && prec_x < 9000);   /* ~0.25 * 32767 = 8192 */
    assert(snap_x > 24000 && snap_x < 25000); /* ~0.75 * 32767 = 24575 */

    /* 4. Mode Off -> always zeroed */
    cfg.mode = FW_GP_MODE_OFF;
    fw_gamepad_evaluate(&cfg, flux, pressed, resolved, 85U, &rep);
    assert(rep.lx == 0 && rep.ly == 0 && rep.hat == 8U);
}

static void test_per_layout_littlefs_persistence_and_uart_state(test_qmk_ctx_t *t) {
    /* Quiet-keys guard: refuse to flush to flash while keys are active, flush once quiet */
    assert(!fw_qmk_poll_eeprom_flush(2100U, 500U, true));  /* not past 500ms debounce */
    assert(!fw_qmk_poll_eeprom_flush(2600U, 500U, false)); /* keys still active */
    assert(fw_qmk_poll_eeprom_flush(2600U, 500U, true));   /* quiet + past debounce -> flushes */

    /* Switch to ISO (layout 2): must type real ISO keys (NOT shifted ANSI neighbors!) */
    fw_qmk_set_layout(2U, 2700U);
    assert(vial_get_keycode(t, 0U, 3U, 4U, 2710U) == 0x0009U);  /* KC_F */
    assert(vial_get_keycode(t, 0U, 3U, 12U, 2711U) == 0x0032U); /* KC_NUHS (#) */
    assert(vial_get_keycode(t, 0U, 4U, 1U, 2712U) == 0x0064U);  /* KC_NUBS (\) */
    assert(vial_get_keycode(t, 0U, 4U, 2U, 2713U) == 0x001DU);  /* KC_Z */
    {
        uint8_t pressed[96] = {0};
        fw_hid_report_t hid;
        /* ISO linear index of row 4 col 1 (NUBS) = 16+15+15+15+1 = 62; col 2 (Z) = 63 */
        pressed[62] = 1U;
        fw_qmk_process_scan(pressed, 87U, 2720U, &hid);
        assert((hid.nkro_bitmap[0x64U >> 3] & (1U << (0x64U & 7U))) != 0U);
        pressed[62] = 0U;
        pressed[63] = 1U;
        fw_qmk_process_scan(pressed, 87U, 2740U, &hid);
        assert((hid.nkro_bitmap[0x1DU >> 3] & (1U << (0x1DU & 7U))) != 0U);
        pressed[63] = 0U;
        fw_qmk_process_scan(pressed, 87U, 2760U, &hid);
    }

    /* Switch to JIS (layout 3): verify JIS row 1 INT3 (Yen), row 4 INT1 (Ro), row 5 13 keys */
    fw_qmk_set_layout(3U, 2800U);
    assert(vial_get_keycode(t, 0U, 1U, 13U, 2810U) == 0x0089U); /* KC_INT3 */
    assert(vial_get_keycode(t, 0U, 1U, 14U, 2811U) == 0x002AU); /* KC_BSPC */
    assert(vial_get_keycode(t, 0U, 4U, 11U, 2812U) == 0x0087U); /* KC_INT1 */
    {
        uint8_t pressed[96] = {0};
        fw_hid_report_t hid;
        /* JIS linear index of row 1 col 13 (INT3) = 16 + 13 = 29 */
        pressed[29] = 1U;
        fw_qmk_process_scan(pressed, 90U, 2820U, &hid);
        assert((hid.nkro_bitmap[0x89U >> 3] & (1U << (0x89U & 7U))) != 0U);
        pressed[29] = 0U;
        fw_qmk_process_scan(pressed, 90U, 2840U, &hid);
    }

    /* Simulate power-cycle remount and re-init ANSI (layout 1): 0x2209 must survive! */
    assert(fw_lfs_host_unmount_and_remount() == 0);
    fw_qmk_platform_t plat = {
        .eeprom_load = test_ee_load,
        .eeprom_save = test_ee_save,
        .raw_hid_send = test_raw_hid_send,
        .on_flux_action = test_on_flux_action,
        .ctx = t,
    };
    fw_qmk_init(&plat, 1U, 3000U);
    assert(vial_get_keycode(t, 0U, 3U, 4U, 3010U) == 0x2209U);

    /* Verify macOS Globe key at ANSI (5,3) (linear key 77) emits Consumer Usage 0x029D */
    {
        uint8_t pressed[96] = {0};
        fw_hid_report_t hid;
        pressed[77] = 1U;
        fw_qmk_process_scan(pressed, 85U, 3015U, &hid);
        assert(hid.consumer_usage == 0x029DU);
        pressed[77] = 0U;
        fw_qmk_process_scan(pressed, 85U, 3025U, &hid);
        assert(hid.consumer_usage == 0U);
    }

    /* Verify Fn (MO(1) at ANSI (5,6) = linear 80) + F12 (KC_VOLU=0xA9 at linear 12) emits
     * Consumer Usage 0x00E9 on press and 0x0000 on release! */
    {
        uint8_t pressed[96] = {0};
        fw_hid_report_t hid;
        fw_qmk_clear_hid_queue();
        pressed[80] = 1U; /* MO(1) */
        fw_qmk_process_scan(pressed, 85U, 3028U, &hid);
        pressed[12] = 1U; /* F12 -> KC_VOLU (0x00A9) on Layer 1 */
        fw_qmk_process_scan(pressed, 85U, 3032U, &hid);
        assert(hid.consumer_usage == 0x00E9U);
        pressed[12] = 0U;
        pressed[80] = 0U;
        fw_qmk_process_scan(pressed, 85U, 3036U, &hid);
        assert(hid.consumer_usage == 0U);
    }

    /* Verify MSG_UPDATE QMK state header & FW_UART_MSG_RESOLVED_KEYS */
    fw_runtime_t rt;
    fw_runtime_init(&rt);
    fw_runtime_set_keyframe(&rt, 1U, 1U, 1U);
    rt.qmk_enabled = 1U;
    uint8_t update[FW_UART_MSG_UPDATE_LEN];
    fw_runtime_build_msg_update(&rt, 3030U, update);
    assert(update[5U + 0x52U] == 1U); /* qmk_state_ver == 1 */

    uint8_t resolved[320];
    size_t rlen = fw_runtime_build_msg_resolved_keys(&rt, resolved, sizeof(resolved));
    assert(rlen == 10U + 85U * 3U);
    assert(resolved[0] == FW_UART_MSG_RESOLVED_KEYS);
    assert(resolved[9] == 85U);
    /* Key 50 (row 3 col 4) must resolve to 0x2209 */
    uint16_t k50 = (uint16_t)resolved[10U + 50U * 3U] | ((uint16_t)resolved[10U + 50U * 3U + 1U] << 8);
    assert(k50 == 0x2209U);
}

static void wr32_le(uint8_t *p, uint32_t v) {
    p[0] = (uint8_t)(v & 0xFFU);
    p[1] = (uint8_t)((v >> 8) & 0xFFU);
    p[2] = (uint8_t)((v >> 16) & 0xFFU);
    p[3] = (uint8_t)((v >> 24) & 0xFFU);
}

static void test_one_time_legacy_keymap_upgrade_migration(test_qmk_ctx_t *t) {
    /* Fresh flash with NO vial_ee_1.bin, simulating an upgrade from legacy OpenFlux firmware. */
    fw_lfs_host_reset_flash();
    assert(fw_lfs_host_format_and_mount() == 0);

    /* Build a minimal valid 1-layout (ANSI, 85 keys, 2 layers, 1 act mode, 0 macros) legacy blob. */
    const uint32_t L = 2U, K = 85U, A = 1U;
    const uint32_t sec_words = 7U + L * K + A * K;
    uint8_t blob[(6U + sec_words) * 4U];
    memset(blob, 0, sizeof(blob));
    wr32_le(blob + 0 * 4, 1U); /* num_layouts = 1 */
    wr32_le(blob + 1 * 4, 1U); /* layout_mask = ANSI */
    /* section header starts at word 6 */
    uint8_t *sec = blob + 6 * 4;
    wr32_le(sec + 0 * 4, L);
    wr32_le(sec + 1 * 4, K);
    wr32_le(sec + 2 * 4, A);
    wr32_le(sec + 3 * 4, 0U);
    wr32_le(sec + 4 * 4, 0U);
    wr32_le(sec + 5 * 4, 0U);      /* kc off = 0 */
    wr32_le(sec + 6 * 4, L * K);   /* act off */
    uint8_t *kcs = sec + 7 * 4;
    for (uint32_t i = 0; i < L * K; i++)
        wr32_le(kcs + i * 4, (i < K) ? 0xF0000000U : 0x50000000U);
    /* Custom legacy keys: key 0 -> KC_B (0x05), key 77 -> KC_GLOBE (0x4000029D), key 79 -> MO(1),
     * key 80 -> KC_OF_START_PAUSE_POMODORO (0x5000000C -> QK_KB_10 = 0x7E0A), Layer 1 key 0 -> KC_MUTE (0x400000E2) */
    wr32_le(kcs + 0 * 4, 0x00000005U);
    wr32_le(kcs + 77 * 4, 0x4000029DU);
    wr32_le(kcs + 79 * 4, 0x21000001U);
    wr32_le(kcs + 80 * 4, 0x5000000CU);
    wr32_le(kcs + (K + 0) * 4, 0x400000E2U);

    fw_runtime_t rt;
    fw_runtime_init(&rt);
    assert(fw_runtime_load_keymap(&rt, blob, sizeof(blob)) == 0);

    fw_qmk_platform_t plat = {
        .eeprom_load = test_ee_load,
        .eeprom_save = test_ee_save,
        .raw_hid_send = test_raw_hid_send,
        .on_flux_action = test_on_flux_action,
        .ctx = t,
    };
    fw_qmk_init(&plat, 1U, 0U);
    assert(fw_qmk_seed_from_legacy_keymap(&rt, 1U));

    assert(vial_get_keycode(t, 0U, 0U, 0U, 10U) == 0x0005U); /* KC_B */
    assert(vial_get_keycode(t, 0U, 5U, 3U, 11U) == 0x7E5FU); /* OF_GLOBE */
    assert(vial_get_keycode(t, 0U, 5U, 5U, 12U) == 0x5221U); /* MO(1) */
    assert(vial_get_keycode(t, 0U, 5U, 6U, 13U) == 0x7E0AU); /* QK_KB_10 (Pomo) */
    assert(vial_get_keycode(t, 1U, 0U, 0U, 14U) == 0x00A8U); /* KC_MUTE */

    /* Once saved to LittleFS, subsequent boots MUST NOT overwrite Vial EEPROM with legacy blob! */
    assert(fw_qmk_poll_eeprom_flush(1000U, 0U, true));
    fw_qmk_init(&plat, 1U, 1100U);
    assert(!fw_qmk_seed_from_legacy_keymap(&rt, 1U));
}

static int test_ee_load_fail(uint8_t layout_id, uint8_t *buf, size_t len, void *ctx) {
    (void)layout_id;
    (void)buf;
    (void)len;
    (void)ctx;
    return -1; /* Simulate LittleFS I/O or corrupt read failure */
}

static void test_long_macro_zero_drops_and_load_error_safety(test_qmk_ctx_t *t) {
    /* 1. Program a 54-char Vial macro ("the quick brown fox jumps over the lazy dog 0123456789")
     *    via VIA id_dynamic_keymap_macro_set_buffer (0x0E) and bind QK_MACRO_0 (0x7700) to key 0. */
    const char *msg = "the quick brown fox jumps over the lazy dog 0123456789";
    size_t msg_len = strlen(msg);
    assert(msg_len == 54U);
    uint8_t buf[64];
    memset(buf, 0, sizeof(buf));
    memcpy(buf, msg, msg_len);
    buf[msg_len] = 0x00U; /* macro 0 NUL terminator */

    /* Write in 24-byte chunks via id_dynamic_keymap_macro_set_buffer (cmd 0x0F, off_hi, off_lo, size, data...) */
    for (size_t off = 0; off < msg_len + 1U; off += 24U) {
        size_t chunk = (msg_len + 1U - off > 24U) ? 24U : (msg_len + 1U - off);
        uint8_t pkt[32] = {0};
        pkt[0] = 0x0FU;
        pkt[1] = (uint8_t)(off >> 8);
        pkt[2] = (uint8_t)(off & 0xFFU);
        pkt[3] = (uint8_t)chunk;
        memcpy(pkt + 4, buf + off, chunk);
        fw_qmk_raw_hid_rx(pkt, 4000U);
    }

    vial_set_keycode(0U, 0U, 0U, 0x7700U, 4010U); /* QK_MACRO_0 */
    fw_qmk_clear_hid_queue();

    uint8_t pressed[96] = {0};
    fw_hid_report_t hid;
    pressed[0] = 1U;
    fw_qmk_process_scan(pressed, 85U, 4020U, &hid);
    pressed[0] = 0U;
    fw_qmk_process_scan(pressed, 85U, 4040U, &hid);

    /* Count distinct key-down edges emitted by the FIFO: MUST equal all 54 characters! */
    uint8_t prev_bm[32] = {0};
    size_t emitted_chars = 0;
    fw_hid_report_t q;
    while (fw_qmk_pop_hid_report(&q)) {
        for (uint16_t u = 1U; u < 0xE0U; u++) {
            bool was = (prev_bm[u >> 3] & (1U << (u & 7U))) != 0U;
            bool now = (q.nkro_bitmap[u >> 3] & (1U << (u & 7U))) != 0U;
            if (now && !was)
                emitted_chars++;
        }
        memcpy(prev_bm, q.nkro_bitmap, sizeof(prev_bm));
    }
    assert(emitted_chars == msg_len);

    /* 2. Failed EEPROM load (-1) MUST NOT mark EEPROM dirty or overwrite user's flash file! */
    uint32_t save_calls = 0;
    (void)save_calls;
    fw_qmk_platform_t fail_plat = {
        .eeprom_load = test_ee_load_fail,
        .eeprom_save = test_ee_save,
        .raw_hid_send = test_raw_hid_send,
        .on_flux_action = test_on_flux_action,
        .ctx = t,
    };
    fw_qmk_init(&fail_plat, 1U, 5000U);
    /* Must NOT flush defaults over flash, and must NOT run legacy migration over an I/O error! */
    assert(!fw_qmk_poll_eeprom_flush(6000U, 500U, true));
    fw_runtime_t rt;
    fw_runtime_init(&rt);
    assert(!fw_qmk_seed_from_legacy_keymap(&rt, 1U));
}

void test_fw_qmk(void) {
    fw_lfs_host_reset_flash();
    assert(fw_lfs_host_format_and_mount() == 0);

    test_qmk_ctx_t ctx;
    memset(&ctx, 0, sizeof(ctx));
    fw_qmk_platform_t plat = {
        .eeprom_load = test_ee_load,
        .eeprom_save = test_ee_save,
        .raw_hid_send = test_raw_hid_send,
        .on_flux_action = test_on_flux_action,
        .ctx = &ctx,
    };
    fw_qmk_init(&plat, 1U, 0U);

    test_vial_id_and_dynamic_def_cache_bust(&ctx);
    test_homerow_mods_and_rapid_trigger_guard(&ctx);
    test_flux_custom_action_no_os_keystroke(&ctx);
    test_per_layout_littlefs_persistence_and_uart_state(&ctx);
    test_one_time_legacy_keymap_upgrade_migration(&ctx);
    test_long_macro_zero_drops_and_load_error_safety(&ctx);
    test_analog_gamepad_math_and_curves();

    printf("fw_qmk tests passed.\n");
}
