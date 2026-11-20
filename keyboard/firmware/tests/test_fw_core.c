#include "fw_calibration.h"
#include "fw_hdlc.h"
#include "fw_protocol.h"
#include "fw_scanner.h"
#include "fw_usb_cmd.h"
#include "lfs_store.h"

#include <assert.h>
#include <stdio.h>
#include <string.h>

static uint8_t g_tx_buf[4096];
static size_t g_tx_len = 0;
static int g_tx_zlp_count = 0;
static int g_mcu_boot = 0;
static int g_soc_boot = 0;
static int g_fail_storage = 0;

static int test_save_keymap(const uint8_t *blob, size_t len, void *ctx) {
    (void)blob;
    (void)len;
    (void)ctx;
    return g_fail_storage ? -1 : 0;
}

static int test_save_profile(const uint8_t *xfer, size_t len, void *ctx) {
    (void)xfer;
    (void)len;
    (void)ctx;
    return g_fail_storage ? -1 : 0;
}

static void reset_tx(void) {
    g_tx_len = 0;
    g_tx_zlp_count = 0;
}

static int fake_usb_send(const uint8_t *pkt64, size_t len, void *ctx) {
    (void)ctx;
    if (len == 0U) {
        g_tx_zlp_count++;
        return 0;
    }
    assert(g_tx_len + len <= sizeof(g_tx_buf));
    memcpy(g_tx_buf + g_tx_len, pkt64, len);
    g_tx_len += len;
    return 0;
}

static void on_mcu_boot(void *ctx) {
    (void)ctx;
    g_mcu_boot++;
}
static void on_soc_boot(void *ctx) {
    (void)ctx;
    g_soc_boot++;
}
static void on_uart_ack_cb(const uint8_t *p, size_t len, void *ctx) {
    fw_runtime_handle_uart_response((fw_runtime_t *)ctx, p, len);
}

typedef struct {
    int ep_busy;
    int of_ep_busy;
    int kbd_sends;
    int cons_sends;
    int of_sends;
    uint8_t last_kbd_mod;
    uint8_t last_kbd_keys[6];
    uint8_t last_nkro_bitmap[32];
    uint16_t last_cons;
    uint32_t last_of;
} fake_hid_ep_t;

static int fake_send_kbd(uint8_t mods, const uint8_t keys[6], const uint8_t nkro_bitmap[32], void *ctx) {
    fake_hid_ep_t *ep = (fake_hid_ep_t *)ctx;
    if (ep->ep_busy)
        return -1;
    ep->ep_busy = 1;
    ep->kbd_sends++;
    ep->last_kbd_mod = mods;
    memcpy(ep->last_kbd_keys, keys, 6);
    memcpy(ep->last_nkro_bitmap, nkro_bitmap, 32);
    return 0;
}

static int fake_send_of(uint32_t func_mask, void *ctx) {
    fake_hid_ep_t *ep = (fake_hid_ep_t *)ctx;
    if (ep->of_ep_busy)
        return -1;
    ep->of_ep_busy = 1;
    ep->of_sends++;
    ep->last_of = func_mask;
    return 0;
}

static int fake_send_consumer(uint16_t usage, void *ctx) {
    fake_hid_ep_t *ep = (fake_hid_ep_t *)ctx;
    if (ep->ep_busy)
        return -1;
    ep->ep_busy = 1;
    ep->cons_sends++;
    ep->last_cons = usage;
    return 0;
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

typedef struct {
    uint32_t avail_bytes;
    uint32_t flushes;
    uint8_t out[FW_MAX_KEYMAP_BYTES + 256U];
    uint32_t out_len;
} fake_async_usb_t;

static uint32_t fake_async_avail(void *ctx) {
    return ((fake_async_usb_t *)ctx)->avail_bytes;
}

static uint32_t fake_async_write(const uint8_t *pkt, uint32_t len, void *ctx) {
    fake_async_usb_t *a = (fake_async_usb_t *)ctx;
    if (a->avail_bytes < len || a->out_len + len > sizeof(a->out))
        return 0U;
    memcpy(a->out + a->out_len, pkt, len);
    a->out_len += len;
    a->avail_bytes -= len;
    return len;
}

static void fake_async_flush(void *ctx) {
    ((fake_async_usb_t *)ctx)->flushes++;
}

/* Hold a position across the full confirmation window before checking the resulting action. */
static void scan_held_position(fw_runtime_t *rt, const uint16_t adc[FW_NUM_SENSORS], fw_hid_report_t *hid) {
    for (int i = 0; i < 24; i++)
        fw_runtime_process_scan(rt, adc, hid);
}

/* Exercise the full ANSI matrix, with a small spread in sensor gains and a nonzero action at every key. */
static void check_top_row_frame(fw_runtime_t *rt, int shift, int travel, int esc_down) {
    static const uint8_t top_row[] = {6, 13, 22, 16, 26, 36, 44, 54, 48, 58, 67, 77, 73, 80, 88, 98, 105};
    uint16_t adc[FW_NUM_SENSORS];
    for (size_t s = 0; s < FW_NUM_SENSORS; s++)
        adc[s] = 2048;
    for (size_t c = 0; c < sizeof(top_row); c++)
        adc[top_row[c]] = (uint16_t)(2048 + shift);
    adc[6] += travel;
    adc[13] -= travel;

    fw_hid_report_t hid;
    fw_runtime_process_scan(rt, adc, &hid);
    uint8_t expected_keys[6] = {esc_down ? 0x29 : 0};
    uint8_t expected_bitmap[32] = {esc_down ? 1 : 0};
    assert(hid.modifiers == 0 && hid.consumer_usage == 0);
    assert(memcmp(hid.keycodes, expected_keys, sizeof(expected_keys)) == 0);
    assert(memcmp(rt->key_bitmap, expected_bitmap, sizeof(expected_bitmap)) == 0);
}

static void test_hall_key_reports(void) {
    fw_runtime_t rt;
    fw_runtime_init(&rt);
    fw_runtime_set_keyframe(&rt, 1, 1, 1);
    uint16_t rest[FW_NUM_SENSORS];
    for (size_t s = 0; s < FW_NUM_SENSORS; s++) {
        rt.cal.factors[s] = (uint16_t)(184 + s % 9);
        rest[s] = 2048;
    }
    memcpy(rt.baseline_adc, rest, sizeof rest);
    enum { keys = 85 };
    uint8_t km[52 + 8 * keys] = {0};
    wr_le32(km, 1);      /* one layout */
    wr_le32(km + 4, 1);  /* ANSI */
    wr_le32(km + 24, 1); /* one mode and one layer */
    wr_le32(km + 28, keys);
    wr_le32(km + 32, 1);
    wr_le32(km + 48, keys);
    for (int k = 0; k < keys; k++) {
        wr_le32(km + 52 + 4 * k, k == 0 ? 0x29U : k == 15 ? 0x4CU : 0x04U);
        wr_le32(km + 52 + 4 * (keys + k), 0x400FE253U);
    }
    assert(fw_runtime_load_keymap(&rt, km, sizeof(km)) == 0);

    /* A coherent offset on a whole row must not press either edge, however long it lasts. */
    for (int sign = -1; sign <= 1; sign += 2)
        for (int pass = 0; pass < 24; pass++)
            check_top_row_frame(&rt, sign * 60, 0, 0);

    /* Isolated and interrupted attempts must not accumulate toward a press. */
    check_top_row_frame(&rt, 0, 900, 0);
    check_top_row_frame(&rt, 0, 0, 0);
    check_top_row_frame(&rt, 60, 900, 0);
    check_top_row_frame(&rt, 60, 900, 0);
    check_top_row_frame(&rt, 60, 0, 0);
    check_top_row_frame(&rt, 60, 900, 0);
    check_top_row_frame(&rt, 60, 900, 0);
    check_top_row_frame(&rt, 60, 900, 1);

    /* Esc still works on a shifted row, and one bad reading mid-hold cannot release it. */
    for (int pass = 0; pass < 24; pass++)
        check_top_row_frame(&rt, -60, 900, 1);
    check_top_row_frame(&rt, -60, 0, 1);
    check_top_row_frame(&rt, -60, 900, 1);
    check_top_row_frame(&rt, -60, 0, 1);
    check_top_row_frame(&rt, -60, 0, 1);
    check_top_row_frame(&rt, -60, 0, 0);

    for (size_t s = 0; s < FW_NUM_SENSORS; s++)
        rt.cal.factors[s] = 256;
    /* Unrefined detection: an isolated opposing pair loses 3 ADC counts at the
     * sensor and 50 more in the row. Sustained, nonzero travel just below the
     * release line must outlast held-key protection and release confirmation. */
    for (int pass = 0; pass < 3; pass++)
        check_top_row_frame(&rt, 0, 351, pass == 2); /* 596 > 595 */
    for (int pass = 0; pass < 3; pass++)
        check_top_row_frame(&rt, 0, 224, 1);
    for (int pass = 0; pass < 18; pass++)
        check_top_row_frame(&rt, 0, 223, pass < 17); /* 340 <= 595-254 */

    /* The same 590-count signal is below Esc's 595 but above W's position-adjusted 581. */
    for (int pass = 0; pass < 3; pass++)
        check_top_row_frame(&rt, 0, 348, 0);
    uint16_t w_adc[FW_NUM_SENSORS];
    memcpy(w_adc, rest, sizeof(w_adc));
    w_adc[19] += 348;
    w_adc[31] -= 348;
    fw_hid_report_t hid;
    scan_held_position(&rt, w_adc, &hid);
    assert(rt.key_bitmap[33 / 8] == (1U << (33 % 8)) && hid.keycodes[0] == 0x04);
    scan_held_position(&rt, rest, &hid);

    /* An unset entry still has a usable press threshold, not a key that oscillates at rest. */
    wr_le32(km + 52 + 4 * keys, 0);
    assert(fw_runtime_load_keymap(&rt, km, sizeof(km)) == 0);
    for (int pass = 0; pass < 6; pass++)
        check_top_row_frame(&rt, 0, 378, 0); /* exactly 650 */
    for (int pass = 0; pass < 3; pass++)
        check_top_row_frame(&rt, 0, 379, pass == 2);
}

/* One layer and two actuation modes: switching profile changes the threshold,
 * not the mapped action. The original serialized keymap remains unchanged. */
static void test_unequal_layer_and_mode_counts(void) {
    enum { keys = 85 };
    uint8_t km[52 + 3 * 4 * keys] = {0};
    wr_le32(km, 1);
    wr_le32(km + 4, 1);
    wr_le32(km + 28, keys);
    wr_le32(km + 48, keys);
    for (unsigned k = 0; k < keys; ++k) {
        wr_le32(km + 52 + k * 4, 0x29U);
        wr_le32(km + 52 + (keys + k) * 4, 0x400c8960U);     /* fixed: A=2400, R=200 */
        wr_le32(km + 52 + (2 * keys + k) * 4, 0x400c80faU); /* fixed: A=250, R=200 */
    }
    uint8_t profiles[48] = {0};
    wr_le32(profiles + 16, 2);
    wr_le32(profiles + 36, 1);
    wr_le32(profiles + 40, 1);
    wr_le32(profiles + 44, 1);
    uint16_t raw[FW_NUM_SENSORS];
    for (unsigned i = 0; i < FW_NUM_SENSORS; ++i)
        raw[i] = 1800;
    raw[6] += 600;
    raw[13] -= 600;
    wr_le32(km + 24, 1);
    wr_le32(km + 32, 2);
    fw_runtime_t rt;
    fw_runtime_init(&rt);
    assert(fw_runtime_load_keymap(&rt, km, sizeof(km)) == 0);
    assert(fw_runtime_load_profile_settings(&rt, profiles, sizeof(profiles)) == 0);
    fw_runtime_set_keyframe(&rt, 1, 1, 1);
    fw_hid_report_t hid;
    scan_held_position(&rt, raw, &hid);
    assert(hid.keycodes[0] == 0);
    assert(fw_runtime_set_active_profile(&rt, 1) == 0);
    scan_held_position(&rt, raw, &hid);
    assert(hid.keycodes[0] == 0x29);
    assert(rt.keymap_len == sizeof(km) && memcmp(rt.keymap_blob, km, sizeof(km)) == 0);
    wr_le32(km + 24, 2);
    wr_le32(km + 32, 1); /* Swapped counts cannot describe one layer of keycodes. */
    assert(fw_runtime_validate_keymap(km, sizeof(km)) != 0);
    wr_le32(km + 48, keys + 1); /* Keycode span matches neither layer count. */
    assert(fw_runtime_validate_keymap(km, sizeof(km)) != 0);
}

void test_fw_rapid(void);

void test_fw_rest(void);

void test_fw_refinement(void);
void test_fw_refine(void);
void test_fw_thermal(void);

int main(void) {
    test_fw_refinement();
    test_fw_refine();
    test_fw_thermal();
    test_fw_rest();
    test_fw_rapid();
    test_hall_key_reports();
    test_unequal_layer_and_mode_counts();

    /* 1. Verify differential column MUX GPIO 10..17 bit mapping */
    assert(fw_mux_col_gpio_value(0) == 0x0003C000U);
    assert(fw_mux_col_gpio_value(15) == 0x00003C00U);
    assert(fw_mux_col_gpio_value(1) == ((1U << 13) | (1U << 16) | (1U << 15) | (1U << 14)));

    /* 2. Verify ANSI/ISO/JIS logical key counts (85/87/90 keys) */
    uint16_t sum_ansi = 0, sum_iso = 0, sum_jis = 0;
    for (size_t r = 0; r < FW_MATRIX_ROWS; r++) {
        sum_ansi += (uint16_t)(FW_ROW_LEN[1][r] - 1U);
        sum_iso += (uint16_t)(FW_ROW_LEN[2][r] - 1U);
        sum_jis += (uint16_t)(FW_ROW_LEN[3][r] - 1U);
    }
    assert(sum_ansi == 85U);
    assert(sum_iso == 87U);
    assert(sum_jis == 90U);

    /* 3. Factory calibration v2 validation + checksum failure rejection */
    fw_calibration_t cal;
    fw_calibration_init_defaults(&cal);
    assert(cal.factors[0] == 0x0100U);

    uint8_t info[20] = {0};
    uint8_t factors_raw[FW_NUM_SENSORS * 2U];
    uint32_t sum = 0;
    for (size_t i = 0; i < FW_NUM_SENSORS; i++) {
        uint16_t v = (uint16_t)(300U + i);
        wr_le16(factors_raw + i * 2U, v);
        sum += v;
    }
    wr_le32(info + 0, FW_CALIB_MAGIC);
    wr_le32(info + 4, 2U);
    wr_le32(info + 8, FW_NUM_SENSORS);
    wr_le32(info + 16, sum);
    assert(fw_calibration_load(&cal, info, sizeof(info), factors_raw, sizeof(factors_raw)) == 0);
    assert(cal.loaded_from_flash == 1U);
    assert(cal.factors[0] == (uint16_t)((300U * 0xAAU) >> 8));

    wr_le32(info + 16, sum + 1U);
    fw_calibration_t bad_cal;
    fw_calibration_init_defaults(&bad_cal);
    assert(fw_calibration_load(&bad_cal, info, sizeof(info), factors_raw, sizeof(factors_raw)) == -1);
    assert(bad_cal.loaded_from_flash == 0U && bad_cal.factors[0] == 0x0100U);

    /* 4. Non-identity physical-sensor -> logical-key mapping & ID-sensor exclusion */
    fw_runtime_t rt;
    fw_runtime_init(&rt);
    fw_runtime_set_keyframe(&rt, 1, 1, 1);
    uint8_t test_rlen[FW_MATRIX_ROWS] = {5, 0, 0, 0, 0, 0};
    uint8_t test_smap[FW_MATRIX_ROWS][FW_MATRIX_MAX_COLS] = {0};
    test_smap[0][0] = 6;
    test_smap[0][1] = 13;
    test_smap[0][2] = 22;
    test_smap[0][3] = 16;
    test_smap[0][4] = 26;
    rt.test_row_len = test_rlen;
    rt.test_sensor_map = test_smap;

    uint8_t km[116];
    memset(km, 0, sizeof(km));
    wr_le32(km + 0, 1);  /* num_layouts */
    wr_le32(km + 4, 1);  /* bitmask=ANSI */
    wr_le32(km + 24, 2); /* n_layers */
    wr_le32(km + 28, 4); /* n_keys */
    wr_le32(km + 32, 2); /* n_act */
    wr_le32(km + 48, 8); /* actuations_off */
    uint8_t *kc = km + 52;
    /* Layer 0: [KC_A=0x04, KC_LCTRL=0xE0, LAYER_SW(1)=0x21000001, CONSUMER_VOL_UP=0x400000E9] */
    wr_le32(kc + 0, 0x00000004U);
    wr_le32(kc + 4, 0x000000E0U);
    wr_le32(kc + 8, 0x21000001U);
    wr_le32(kc + 12, 0x400000E9U);
    /* Layer 1: [NONE=0xF0000000, KC_B=0x05, NONE, NONE] */
    wr_le32(kc + 16, 0xF0000000U);
    wr_le32(kc + 20, 0x00000005U);
    wr_le32(kc + 24, 0xF0000000U);
    wr_le32(kc + 28, 0xF0000000U);
    uint8_t *act = km + 52 + 32;
    for (int i = 0; i < 4; i++) {
        wr_le32(act + i * 4, 0x4006412CU);      /* Traditional: act=300, rst=100 */
        wr_le32(act + 16 + i * 4, 0x800320C8U); /* Rapid trigger: act=200, rst=50 */
    }

    fw_usb_hooks_t hooks = {
        .on_mcu_bootloader = on_mcu_boot,
        .on_soc_bootloader = on_soc_boot,
        .on_keymap_saved = test_save_keymap,
        .on_profile_settings_saved = test_save_profile,
    };
    fw_usb_rx_t usb_rx;
    fw_usb_rx_init(&usb_rx, &rt, &hooks);

    /* USB CMD 0x01 REQUEST_VERSION */
    reset_tx();
    uint8_t hdr[64] = {0};
    hdr[0] = FW_CMD_REQUEST_VERSION;
    fw_usb_rx_feed(&usb_rx, hdr, sizeof(hdr), fake_usb_send, NULL);
    assert(g_tx_buf[0] == FW_RESP_ACK && g_tx_zlp_count == 1);
    assert(strcmp((const char *)(g_tx_buf + 64), FW_VERSION_STR) == 0);
    assert(strncmp((const char *)(g_tx_buf + 64), "openflux-firmware ", 18) == 0);

    /* USB CMD 0x64 SEND_KEYMAP rejects truncated & UINT32_MAX-wrapping offset attacks */
    assert(fw_runtime_load_keymap(&rt, km, sizeof(km) - 4U) == -1);
    uint8_t bad_km[116];
    memcpy(bad_km, km, sizeof(bad_km));
    wr_le32(bad_km + 48, 0xFFFFFFFFU);
    assert(fw_runtime_load_keymap(&rt, bad_km, sizeof(bad_km)) == -1);
    uint8_t bad_macro_km[124];
    memset(bad_macro_km, 0, sizeof(bad_macro_km));
    wr_le32(bad_macro_km + 0, 1);
    wr_le32(bad_macro_km + 4, 1);
    wr_le32(bad_macro_km + 24, 1);
    wr_le32(bad_macro_km + 28, 1);
    wr_le32(bad_macro_km + 32, 1);
    wr_le32(bad_macro_km + 36, 1);
    wr_le32(bad_macro_km + 44, 2);
    wr_le32(bad_macro_km + 48, 3);
    wr_le32(bad_macro_km + 52, 1);
    wr_le32(bad_macro_km + 56, 0xFFFFFFFFU);
    assert(fw_runtime_load_keymap(&rt, bad_macro_km, sizeof(bad_macro_km)) == -1);

    /* USB CMD 0x64 SEND_KEYMAP + 0x67 GET_KEYMAP roundtrip */
    reset_tx();
    memset(hdr, 0, sizeof(hdr));
    hdr[0] = FW_CMD_SEND_KEYMAP;
    wr_le32(hdr + 1, 2);
    wr_le32(hdr + 5, sizeof(km));
    fw_usb_rx_feed(&usb_rx, hdr, sizeof(hdr), fake_usb_send, NULL);
    uint8_t p1[64] = {0}, p2[64] = {0};
    memcpy(p1, km, 64);
    memcpy(p2, km + 64, sizeof(km) - 64);
    fw_usb_rx_feed(&usb_rx, p1, 64, fake_usb_send, NULL);
    fw_usb_rx_feed(&usb_rx, p2, 64, fake_usb_send, NULL);
    assert(g_tx_buf[0] == FW_RESP_ACK);

    reset_tx();
    memset(hdr, 0, sizeof(hdr));
    hdr[0] = FW_CMD_GET_KEYMAP;
    fw_usb_rx_feed(&usb_rx, hdr, sizeof(hdr), fake_usb_send, NULL);
    assert(g_tx_buf[0] == FW_RESP_ACK && memcmp(g_tx_buf + 64, km, sizeof(km)) == 0);

    /* Headers whose packet count doesn't match their byte count are refused at once, so the
     * next header is read as a command again. */
    {
        const uint32_t bad_counts[][2] = {
            {0U, (uint32_t)sizeof(km)}, /* no packets for a payload */
            {0xFFFFFFFFU, 64U},         /* far more packets than bytes */
            {3U, (uint32_t)sizeof(km)}, /* one packet too many */
        };
        for (size_t i = 0; i < sizeof(bad_counts) / sizeof(bad_counts[0]); i++) {
            reset_tx();
            memset(hdr, 0, sizeof(hdr));
            hdr[0] = FW_CMD_SEND_KEYMAP;
            wr_le32(hdr + 1, bad_counts[i][0]);
            wr_le32(hdr + 5, bad_counts[i][1]);
            fw_usb_rx_feed(&usb_rx, hdr, sizeof(hdr), fake_usb_send, NULL);
            assert(g_tx_buf[0] == FW_RESP_NAK && g_tx_zlp_count == 1);
            assert(!fw_usb_rx_busy(&usb_rx));
        }
        reset_tx();
        memset(hdr, 0, sizeof(hdr));
        hdr[0] = FW_CMD_REQUEST_VERSION;
        fw_usb_rx_feed(&usb_rx, hdr, sizeof(hdr), fake_usb_send, NULL);
        assert(g_tx_buf[0] == FW_RESP_ACK);
    }

    /* Short packets that leave the payload incomplete are refused, not dispatched. */
    {
        reset_tx();
        memset(hdr, 0, sizeof(hdr));
        hdr[0] = FW_CMD_SEND_KEYMAP;
        wr_le32(hdr + 1, 2);
        wr_le32(hdr + 5, sizeof(km));
        fw_usb_rx_feed(&usb_rx, hdr, sizeof(hdr), fake_usb_send, NULL);
        fw_usb_rx_feed(&usb_rx, km, 64, fake_usb_send, NULL);
        fw_usb_rx_feed(&usb_rx, km + 64, 8, fake_usb_send, NULL);
        assert(g_tx_len > 0U && g_tx_buf[0] == FW_RESP_NAK);
        assert(!fw_usb_rx_busy(&usb_rx));
    }

    /* An oversized request is discarded packet by packet; a reset ends the discard early. */
    {
        reset_tx();
        memset(hdr, 0, sizeof(hdr));
        hdr[0] = FW_CMD_SEND_KEYMAP;
        wr_le32(hdr + 1, 0x04000000U);
        wr_le32(hdr + 5, 0xFFFFFFFFU);
        fw_usb_rx_feed(&usb_rx, hdr, sizeof(hdr), fake_usb_send, NULL);
        assert(fw_usb_rx_busy(&usb_rx) && g_tx_len == 0U);
        fw_usb_rx_feed(&usb_rx, p1, 64, fake_usb_send, NULL);
        assert(fw_usb_rx_busy(&usb_rx) && g_tx_len == 0U);
        fw_usb_rx_reset(&usb_rx);
        assert(!fw_usb_rx_busy(&usb_rx));
        memset(hdr, 0, sizeof(hdr));
        hdr[0] = FW_CMD_REQUEST_VERSION;
        fw_usb_rx_feed(&usb_rx, hdr, sizeof(hdr), fake_usb_send, NULL);
        assert(g_tx_buf[0] == FW_RESP_ACK);
    }

    /* USB CMD 0x6B PROFILE_SETTINGS_V1 + 0x6C FILE_TO_HOST + 0x6A SET_ACTIVE_PROFILE */
    uint8_t ps_xfer[32 + 16];
    memset(ps_xfer, 0, sizeof(ps_xfer));
    wr_le32(ps_xfer + 16, 2);
    wr_le32(ps_xfer + 32, 0);
    wr_le32(ps_xfer + 36, 1);
    wr_le32(ps_xfer + 40, 1);
    wr_le32(ps_xfer + 44, 2);
    reset_tx();
    memset(hdr, 0, sizeof(hdr));
    hdr[0] = FW_CMD_FILE_FROM_HOST;
    wr_le32(hdr + 1, 1);
    wr_le32(hdr + 5, sizeof(ps_xfer));
    fw_usb_rx_feed(&usb_rx, hdr, sizeof(hdr), fake_usb_send, NULL);
    uint8_t pps[64] = {0};
    memcpy(pps, ps_xfer, sizeof(ps_xfer));
    fw_usb_rx_feed(&usb_rx, pps, 64, fake_usb_send, NULL);
    assert(g_tx_buf[0] == FW_RESP_ACK);

    reset_tx();
    memset(hdr, 0, sizeof(hdr));
    hdr[0] = FW_CMD_FILE_TO_HOST;
    fw_usb_rx_feed(&usb_rx, hdr, sizeof(hdr), fake_usb_send, NULL);
    assert(g_tx_buf[0] == FW_RESP_ACK && memcmp(g_tx_buf + 64, ps_xfer + 16, 32) == 0);

    /* Calibrate sensor 6 differently (2x factor) while leaving keyframe ID sensors 5 & 106 noisy */
    rt.cal.factors[6] = 0x0200U;
    uint16_t raw_adc[FW_NUM_SENSORS];
    for (size_t i = 0; i < FW_NUM_SENSORS; i++)
        raw_adc[i] = 1800U;
    raw_adc[5] = 3500U;
    raw_adc[106] = 3500U;

    fw_hid_report_t hid;
    scan_held_position(&rt, raw_adc, &hid);
    assert(rt.key_bitmap[0] == 0U && hid.keycodes[0] == 0U && hid.consumer_usage == 0U);

    raw_adc[6] = 2000U;
    raw_adc[13] = 1400U; /* balances the doubled gain on slot 6 */
    raw_adc[16] = 2200U;
    raw_adc[26] = 1400U;
    scan_held_position(&rt, raw_adc, &hid);
    assert((rt.key_bitmap[0] & 0x09U) == 0x09U);
    assert(hid.keycodes[0] == 0x04U);
    assert(hid.consumer_usage == 0x00E9U);

    /* 5. EP 0x83 (kbd/consumer) & dedicated EP 0x85 (vendor HID) schedulers */
    fw_hid_scheduler_t sched = {0};
    fake_hid_ep_t ep = {0};
    fw_hid_scheduler_step(&sched, &hid, 1, 1, fake_send_kbd, fake_send_consumer, fake_send_of, &ep);
    assert(ep.kbd_sends == 1 && ep.cons_sends == 0 && ep.of_sends == 1 && ep.last_of == 0U);
    ep.of_ep_busy = 0;
    fw_hid_scheduler_step(&sched, &hid, 0, 0, fake_send_kbd, fake_send_consumer, fake_send_of, &ep);
    assert(ep.cons_sends == 0);
    ep.ep_busy = 0;
    fw_hid_scheduler_step(&sched, &hid, 1, 1, fake_send_kbd, fake_send_consumer, fake_send_of, &ep);
    assert(ep.cons_sends == 1 && ep.last_cons == 0x00E9U);
    ep.ep_busy = 0;
    hid.consumer_usage = 0U;
    fw_hid_scheduler_step(&sched, &hid, 1, 1, fake_send_kbd, fake_send_consumer, fake_send_of, &ep);
    assert(ep.cons_sends == 2 && ep.last_cons == 0x0000U);
    ep.ep_busy = 0;
    hid.of_func_mask = (1UL << FW_OF_FUNC_AUTO_PROFILE_LOCK);
    fw_hid_scheduler_step(&sched, &hid, 1, 1, fake_send_kbd, fake_send_consumer, fake_send_of, &ep);
    assert(ep.of_sends == 2 && ep.last_of == (1UL << FW_OF_FUNC_AUTO_PROFILE_LOCK));
    ep.of_ep_busy = 0;
    hid.of_func_mask = 0U;
    fw_hid_scheduler_step(&sched, &hid, 1, 1, fake_send_kbd, fake_send_consumer, fake_send_of, &ep);
    assert(ep.of_sends == 3 && ep.last_of == 0U);

    /* Two fast taps (0 -> bit -> 0 -> bit -> 0) while EP 0x85 is busy must preserve all 4 edges,
     * and can drain while EP 0x83 is busy (independent of_ep_ready=1). */
    hid.of_func_mask = (1UL << FW_OF_FUNC_THEME_CYCLE_NEXT);
    fw_hid_scheduler_step(&sched, &hid, 0, 0, fake_send_kbd, fake_send_consumer, fake_send_of, &ep);
    hid.of_func_mask = 0U;
    fw_hid_scheduler_step(&sched, &hid, 0, 0, fake_send_kbd, fake_send_consumer, fake_send_of, &ep);
    hid.of_func_mask = (1UL << FW_OF_FUNC_AUTO_PROFILE_LOCK);
    fw_hid_scheduler_step(&sched, &hid, 0, 0, fake_send_kbd, fake_send_consumer, fake_send_of, &ep);
    hid.of_func_mask = 0U;
    fw_hid_scheduler_step(&sched, &hid, 0, 0, fake_send_kbd, fake_send_consumer, fake_send_of, &ep);
    assert(ep.of_sends == 3);
    ep.of_ep_busy = 0;
    fw_hid_scheduler_step(&sched, &hid, 0, 1, fake_send_kbd, fake_send_consumer, fake_send_of, &ep);
    assert(ep.of_sends == 4 && ep.last_of == (1UL << FW_OF_FUNC_THEME_CYCLE_NEXT));
    ep.of_ep_busy = 0;
    fw_hid_scheduler_step(&sched, &hid, 0, 1, fake_send_kbd, fake_send_consumer, fake_send_of, &ep);
    assert(ep.of_sends == 5 && ep.last_of == 0U);
    ep.of_ep_busy = 0;
    fw_hid_scheduler_step(&sched, &hid, 0, 1, fake_send_kbd, fake_send_consumer, fake_send_of, &ep);
    assert(ep.of_sends == 6 && ep.last_of == (1UL << FW_OF_FUNC_AUTO_PROFILE_LOCK));
    ep.of_ep_busy = 0;
    fw_hid_scheduler_step(&sched, &hid, 0, 1, fake_send_kbd, fake_send_consumer, fake_send_of, &ep);
    assert(ep.of_sends == 7 && ep.last_of == 0U);

    /* Canonical 139-byte UART MSG_UPDATE + response */
    uint8_t msg[FW_UART_MSG_UPDATE_LEN];
    fw_runtime_build_msg_update(&rt, 12345U, msg);
    assert(sizeof(msg) == 139U);
    assert(msg[0] == FW_UART_MSG_UPDATE && (msg[5] & 0x09U) == 0x09U);
    assert(msg[5 + 0x25U] == 1U && msg[5 + 0x6AU] == 3U);
    uint8_t uack[4] = {FW_UART_MSG_UPDATE, 0, 1, 0};
    uint8_t uwire[32];
    int uw = fw_hdlc_encode(uack, 4, uwire, sizeof(uwire));
    fw_hdlc_decoder_t udec;
    fw_hdlc_decoder_init(&udec);
    fw_hdlc_decoder_feed(&udec, uwire, (size_t)uw, on_uart_ack_cb, &rt);
    assert(rt.qc_mode == 1U);

    /* Failing LittleFS save returns NAK and preserves previous RAM config */
    g_fail_storage = 1;
    uint8_t km_alt[116];
    memcpy(km_alt, km, sizeof(km));
    wr_le32(km_alt + 52, 0x00000006U);
    reset_tx();
    memset(hdr, 0, sizeof(hdr));
    hdr[0] = FW_CMD_SEND_KEYMAP;
    wr_le32(hdr + 1, 2);
    wr_le32(hdr + 5, sizeof(km_alt));
    fw_usb_rx_feed(&usb_rx, hdr, sizeof(hdr), fake_usb_send, NULL);
    memcpy(p1, km_alt, 64);
    memcpy(p2, km_alt + 64, sizeof(km_alt) - 64);
    fw_usb_rx_feed(&usb_rx, p1, 64, fake_usb_send, NULL);
    fw_usb_rx_feed(&usb_rx, p2, 64, fake_usb_send, NULL);
    assert(g_tx_buf[0] == FW_RESP_NAK);
    reset_tx();
    memset(hdr, 0, sizeof(hdr));
    hdr[0] = FW_CMD_GET_KEYMAP;
    fw_usb_rx_feed(&usb_rx, hdr, sizeof(hdr), fake_usb_send, NULL);
    assert(g_tx_buf[0] == FW_RESP_ACK && memcmp(g_tx_buf + 64, km, sizeof(km)) == 0);
    g_fail_storage = 0;

    /* A key pressed before a layer switch keeps its press-time latched action until release */
    rt.active_layer_bitfield = 2U;
    fw_runtime_process_scan(&rt, raw_adc, &hid);
    assert(hid.keycodes[0] == 0x04U);

    /* Release and re-press on active layer 1: explicit NONE never falls back to base layer KC_A */
    uint16_t rel_adc[FW_NUM_SENSORS];
    for (size_t i = 0; i < FW_NUM_SENSORS; i++)
        rel_adc[i] = 1800U;
    scan_held_position(&rt, rel_adc, &hid);
    scan_held_position(&rt, raw_adc, &hid);
    assert(hid.keycodes[0] == 0U);

    /* Transparent (0x50000000) falls through to base layer, while layer-switch keys on higher layers chain cleanly */
    {
        uint8_t tr_km[132];
        memset(tr_km, 0, sizeof(tr_km));
        wr_le32(tr_km + 0, 1);
        wr_le32(tr_km + 4, 1);
        wr_le32(tr_km + 24, 3);  /* n_layers */
        wr_le32(tr_km + 28, 4);  /* n_keys */
        wr_le32(tr_km + 32, 1);  /* n_act */
        wr_le32(tr_km + 48, 12); /* act_off */
        uint8_t *tkc = tr_km + 52;
        /* Layer 0: [KC_A, MO(1), KC_D, KC_NO] */
        wr_le32(tkc + 0, 0x00000004U);
        wr_le32(tkc + 4, 0x21000001U);
        wr_le32(tkc + 8, 0x00000007U);
        wr_le32(tkc + 12, 0xF0000000U);
        /* Layer 1: [KC_TRANSPARENT, KC_TRANSPARENT, MO(2), KC_NO] */
        wr_le32(tkc + 16, 0x50000000U);
        wr_le32(tkc + 20, 0x50000000U);
        wr_le32(tkc + 24, 0x21000002U);
        wr_le32(tkc + 28, 0xF0000000U);
        /* Layer 2: [KC_B, KC_TRANSPARENT, KC_TRANSPARENT, KC_TRANSPARENT] */
        wr_le32(tkc + 32, 0x00000005U);
        wr_le32(tkc + 36, 0x50000000U);
        wr_le32(tkc + 40, 0x50000000U);
        wr_le32(tkc + 44, 0x50000000U);
        uint8_t *tact = tkc + 48;
        for (int i = 0; i < 4; i++)
            wr_le32(tact + i * 4, 0x4006412CU);

        fw_runtime_t rt_tr;
        fw_runtime_init(&rt_tr);
        fw_runtime_set_keyframe(&rt_tr, 1, 1, 1);
        rt_tr.test_row_len = test_rlen;
        rt_tr.test_sensor_map = test_smap;
        assert(fw_runtime_load_keymap(&rt_tr, tr_km, sizeof(tr_km)) == 0);

        uint16_t tr_adc[FW_NUM_SENSORS];
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            tr_adc[i] = 1800U;
        /* Row sensors are [6,13,22,16,26]; key i is pressed when the running sum of the deltas up to sensor i is
         * high. s6=+600, s13=-600 presses key 0 only. */
        tr_adc[6] = 2400U;
        tr_adc[13] = 1200U;
        rt_tr.active_layer_bitfield =
            2U; /* Layer 1 active; key 0 is KC_TRANSPARENT there and falls through to KC_A on layer 0. */
        scan_held_position(&rt_tr, tr_adc, &hid);
        assert(hid.keycodes[0] == 0x04U);

        /* Key 1 alone (MO(1) on L0, s13=+600, s22=-600): layers 0 and 1 are active, layer 2 isn't. */
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            tr_adc[i] = 1800U;
        tr_adc[13] = 2400U;
        tr_adc[22] = 1200U;
        rt_tr.active_layer_bitfield = 1U;
        scan_held_position(&rt_tr, tr_adc, &hid);
        assert((rt_tr.effective_layer_bitfield & 0x03U) == 0x03U);
        assert((rt_tr.effective_layer_bitfield & 0x04U) == 0U);

        /* Release every key. */
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            tr_adc[i] = 1800U;
        scan_held_position(&rt_tr, tr_adc, &hid);

        /* With L1[key 1] = MO(2) under L0[key 1] = MO(1), pressing key 1 activates layer 1 but not layer 2: a single
         * key never chains through its own layers. */
        wr_le32(tkc + 20, 0x21000002U);
        assert(fw_runtime_load_keymap(&rt_tr, tr_km, sizeof(tr_km)) == 0);
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            tr_adc[i] = 1800U;
        tr_adc[13] = 2400U;
        tr_adc[22] = 1200U;
        scan_held_position(&rt_tr, tr_adc, &hid);
        assert((rt_tr.effective_layer_bitfield & 0x03U) == 0x03U);
        assert((rt_tr.effective_layer_bitfield & 0x04U) == 0U);

        /* Key 1 stays held while keys 0 and 2 are added (s6=+600, s16=-600): MO(2) on key 2 chains to layer 2, where
         * key 0 is KC_B. */
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            tr_adc[i] = 1800U;
        tr_adc[6] = 2400U;
        tr_adc[16] = 1200U;
        scan_held_position(&rt_tr, tr_adc, &hid);
        assert((rt_tr.effective_layer_bitfield & 0x07U) == 0x07U);
        assert(hid.keycodes[0] == 0x05U);

        /* A simultaneous press with MO(2) at a lower index (key 0 on L1) than MO(1) (key 2 on L0), plus ordinary
         * key 3 (KC_C on L2, KC_A on L0/L1): the confirmed chord resolves to layer 2 and KC_C. */
        wr_le32(tkc + 0, 0x00000004U);  /* L0[0] = KC_A */
        wr_le32(tkc + 4, 0x00000004U);  /* L0[1] = KC_A */
        wr_le32(tkc + 8, 0x21000001U);  /* L0[2] = MO(1) (higher index than key 0) */
        wr_le32(tkc + 12, 0x00000004U); /* L0[3] = KC_A */
        wr_le32(tkc + 16, 0x21000002U); /* L1[0] = MO(2) (lower index than key 2) */
        wr_le32(tkc + 20, 0x50000000U); /* L1[1] = KC_TRANSPARENT */
        wr_le32(tkc + 24, 0x50000000U); /* L1[2] = KC_TRANSPARENT */
        wr_le32(tkc + 28, 0x00000004U); /* L1[3] = KC_A */
        wr_le32(tkc + 32, 0x50000000U); /* L2[0] = KC_TRANSPARENT */
        wr_le32(tkc + 36, 0x50000000U); /* L2[1] = KC_TRANSPARENT */
        wr_le32(tkc + 40, 0x50000000U); /* L2[2] = KC_TRANSPARENT */
        wr_le32(tkc + 44, 0x00000006U); /* L2[3] = KC_C */
        assert(fw_runtime_load_keymap(&rt_tr, tr_km, sizeof(tr_km)) == 0);
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            tr_adc[i] = 1800U;
        scan_held_position(&rt_tr, tr_adc, &hid); /* release all */
        /* A chord crossing actuation together, without a sudden neighbor movement that would
         * qualify its keys on different scans. Keys 0, 2, 3 sit just below threshold first. */
        tr_adc[6] = tr_adc[22] = 2103U;
        tr_adc[13] = tr_adc[26] = 1497U;
        scan_held_position(&rt_tr, tr_adc, &hid);
        assert(hid.keycodes[0] == 0);
        tr_adc[6] = tr_adc[22] = 2108U;
        tr_adc[13] = tr_adc[26] = 1492U;
        scan_held_position(&rt_tr, tr_adc, &hid);
        assert((rt_tr.effective_layer_bitfield & 0x07U) == 0x07U);
        assert(hid.keycodes[0] == 0x06U);
        /* The next scan resolves the same. */
        fw_hid_report_t hid2;
        fw_runtime_process_scan(&rt_tr, tr_adc, &hid2);
        assert((rt_tr.effective_layer_bitfield & 0x07U) == 0x07U);
        assert(hid2.keycodes[0] == 0x06U);
    }

    rt.host_state_status = 0x02U;
    uint8_t led_msg[FW_UART_MSG_UPDATE_LEN];
    fw_runtime_build_msg_update(&rt, 999U, led_msg);
    assert(led_msg[5U + 0x5CU] == 0x02U);
    uint8_t fn_msg[FW_UART_MSG_UPDATE_LEN];
    fw_runtime_build_msg_update(&rt, 1234U, fn_msg);
    assert((fn_msg[5U + 0x27U] & 0x03U) == 2U);
    rt.active_layer_bitfield = 1U;
    fw_runtime_process_scan(&rt, raw_adc, &hid);
    fw_runtime_build_msg_update(&rt, 1235U, fn_msg);
    assert(fn_msg[5U + 0x27U] == 1U);

    /* 6. Rapid-trigger trajectory via SET_ACTIVE_PROFILE(1) */
    uint8_t prof_payload[4];
    wr_le32(prof_payload, 1U);
    reset_tx();
    memset(hdr, 0, sizeof(hdr));
    hdr[0] = FW_CMD_SET_ACTIVE_PROFILE;
    wr_le32(hdr + 1, 1);
    wr_le32(hdr + 5, 4);
    fw_usb_rx_feed(&usb_rx, hdr, sizeof(hdr), fake_usb_send, NULL);
    uint8_t pp[64] = {0};
    memcpy(pp, prof_payload, 4);
    fw_usb_rx_feed(&usb_rx, pp, 64, fake_usb_send, NULL);
    assert(g_tx_buf[0] == FW_RESP_ACK && rt.active_actuation_mode == 1U);
    rt.active_layer_bitfield = 1U;
    rt.cal.factors[6] = FW_CALIB_DEFAULT_Q8;

    for (size_t i = 0; i < FW_NUM_SENSORS; i++)
        raw_adc[i] = 1800U;
    for (unsigned scan = 0; scan < 7; scan++)
        fw_runtime_process_scan(&rt, raw_adc, &hid);
    assert(rt.key_pressed[0] == 0U);

    raw_adc[6] = 2050U;
    raw_adc[13] = 1550U;
    fw_runtime_process_scan(&rt, raw_adc, &hid);
    assert(rt.key_bitmap[0] == 0x01U && rt.key_pressed[0] == 1U && hid.keycodes[0] == 0x04U);

    raw_adc[6] = 2010U;
    raw_adc[13] = 1590U;
    for (unsigned scan = 0; scan < 7; scan++)
        fw_runtime_process_scan(&rt, raw_adc, &hid);
    assert(rt.key_bitmap[0] == 0U && rt.key_pressed[0] == 0U && hid.keycodes[0] == 0U);

    raw_adc[6] = 2120U;
    raw_adc[13] = 1480U;
    for (unsigned scan = 0; scan < 7; scan++)
        fw_runtime_process_scan(&rt, raw_adc, &hid);
    assert(rt.key_bitmap[0] == 0x01U && rt.key_pressed[0] == 1U && hid.keycodes[0] == 0x04U);

    wr_le32(prof_payload, 0U);
    reset_tx();
    memset(hdr, 0, sizeof(hdr));
    hdr[0] = FW_CMD_SET_ACTIVE_PROFILE;
    wr_le32(hdr + 1, 1);
    wr_le32(hdr + 5, 4);
    fw_usb_rx_feed(&usb_rx, hdr, sizeof(hdr), fake_usb_send, NULL);
    memcpy(pp, prof_payload, 4);
    fw_usb_rx_feed(&usb_rx, pp, 64, fake_usb_send, NULL);
    assert(g_tx_buf[0] == FW_RESP_ACK && rt.active_actuation_mode == 0U);
    rt.active_layer_bitfield = 1U;
    raw_adc[6] = 2010U;
    raw_adc[13] = 1590U;
    fw_runtime_process_scan(&rt, raw_adc, &hid);
    assert(rt.key_bitmap[0] == 0x01U && rt.key_pressed[0] == 1U && hid.keycodes[0] == 0x04U);

    fw_runtime_set_keyframe(&rt, 0U, 1U, 1U);
    fw_runtime_process_scan(&rt, raw_adc, &hid);
    assert(rt.key_pressed[0] == 0U && hid.keycodes[0] == 0U);
    fw_runtime_set_keyframe(&rt, 1U, 2U, 1U);
    assert(rt.keyframe_layout == 2U && rt.key_pressed[0] == 0U);

    fw_runtime_set_keyframe(&rt, 1U, 1U, 1U);
    uint8_t jis_km[116];
    memcpy(jis_km, km, sizeof(km));
    wr_le32(jis_km + 4, 4U);
    wr_le32(jis_km + 52, 0x00000087U);
    fw_runtime_t rt_jis;
    fw_runtime_init(&rt_jis);
    fw_runtime_set_keyframe(&rt_jis, 1, 1, 1);
    rt_jis.test_row_len = test_rlen;
    rt_jis.test_sensor_map = test_smap;
    assert(rt_jis.keyframe_layout == 1U);
    assert(fw_runtime_load_keymap(&rt_jis, jis_km, sizeof(jis_km)) == 0);
    raw_adc[6] = 2200U;
    raw_adc[13] = 1400U;
    scan_held_position(&rt_jis, raw_adc, &hid);
    /* A JIS-only keymap cannot turn a physical ANSI frame into JIS. The
     * physical key still confirms with default thresholds; no action is mapped. */
    assert(rt_jis.key_bitmap[0] == 1U && hid.keycodes[0] == 0U);
    fw_runtime_set_keyframe(&rt_jis, 1, 3, 1);
    scan_held_position(&rt_jis, raw_adc, &hid);
    assert(rt_jis.key_pressed[0] == 1U && hid.keycodes[0] == 0x87U);

    /* 7. Save -> fresh runtime -> LittleFS boot restore applies persisted profile/mode/layer */
    assert(fw_lfs_host_format_and_mount() == 0);
    uint8_t boot_ps_xfer[32 + 8];
    memset(boot_ps_xfer, 0, sizeof(boot_ps_xfer));
    wr_le32(boot_ps_xfer + 16, 1U);
    wr_le32(boot_ps_xfer + 32, 1U);
    wr_le32(boot_ps_xfer + 36, 2U);
    assert(fw_lfs_save_keymap(km, sizeof(km)) == 0);
    assert(fw_lfs_save_profile_settings(boot_ps_xfer, sizeof(boot_ps_xfer)) == 0);
    assert(fw_lfs_save_u32("language_code", 471U) == 0);
    assert(fw_lfs_save_u32("platform_type_code", 2U) == 0);

    fw_runtime_t fresh_rt;
    fw_runtime_init(&fresh_rt);
    assert(fw_lfs_mount() == 0);
    fw_lfs_load_boot_state(&fresh_rt);
    assert(fresh_rt.num_profiles == 1U);
    assert(fresh_rt.active_actuation_mode == 1U);
    assert(fresh_rt.active_layer_bitfield == 2U);
    assert(fresh_rt.language_code == 471U && fresh_rt.platform_code == 2U);
    assert(fresh_rt.keymap_len == sizeof(km));

    /* 255-block LittleFS images must also mount via superblock block_count */
    assert(fw_lfs_host_format_blocks_and_mount(255U) == 0);
    assert(fw_lfs_save_u32("language_code", 1033U) == 0);
    assert(fw_lfs_host_unmount_and_remount() == 0);

    /* Bootloader hooks */
    reset_tx();
    memset(hdr, 0, sizeof(hdr));
    hdr[0] = FW_CMD_MCU_BOOTLOADER;
    fw_usb_rx_feed(&usb_rx, hdr, sizeof(hdr), fake_usb_send, NULL);
    assert(g_mcu_boot == 1 && g_tx_buf[0] == FW_RESP_ACK);

    reset_tx();
    memset(hdr, 0, sizeof(hdr));
    hdr[0] = FW_CMD_SOC_BOOTLOADER;
    fw_usb_rx_feed(&usb_rx, hdr, sizeof(hdr), fake_usb_send, NULL);
    assert(g_soc_boot == 1 && g_tx_buf[0] == FW_RESP_ACK);

    /* 8. OpenFlux function keys (0x5000_00xx): profile cycle, actuation mode cycle, and platform select */
    {
        /* 2-key layout, 3 actuation modes, 2 layers:
         * L0: key0 = PROFILE_CYCLE_NEXT (0x04), key1 = PROFILE_CYCLE_PREV (0x05)
         * L1: key0 = ACTUATION_MODE_NEXT (0x06), key1 = ACTUATION_MODE_PREV (0x07) */
        uint8_t of_km[24 + 28 + 2 * 2 * 4 + 3 * 2 * 4];
        memset(of_km, 0, sizeof(of_km));
        wr_le32(of_km + 0, 1U);
        wr_le32(of_km + 4, 1U);
        wr_le32(of_km + 24 + 0, 2U); /* num_layers */
        wr_le32(of_km + 24 + 4, 2U); /* num_keys */
        wr_le32(of_km + 24 + 8, 3U); /* num_act_modes */
        wr_le32(of_km + 24 + 20, 0U);
        wr_le32(of_km + 24 + 24, 4U);
        wr_le32(of_km + 52 + 0, 0x50000000U | FW_OF_FUNC_PROFILE_CYCLE_NEXT);
        wr_le32(of_km + 52 + 4, 0x50000000U | FW_OF_FUNC_PROFILE_CYCLE_PREV);
        wr_le32(of_km + 52 + 8, 0x50000000U | FW_OF_FUNC_ACTUATION_MODE_NEXT);
        wr_le32(of_km + 52 + 12, 0x50000000U | FW_OF_FUNC_ACTUATION_MODE_PREV);
        for (size_t m = 0; m < 3; m++) {
            for (size_t k = 0; k < 2; k++) {
                wr_le32(of_km + 52 + 16 + (m * 2 + k) * 4, 0x400FE253U);
            }
        }

        fw_runtime_t rt_of;
        fw_runtime_init(&rt_of);
        fw_runtime_set_keyframe(&rt_of, 1, 1, 1);
        static const uint8_t of_rlen[FW_MATRIX_ROWS] = {3, 0, 0, 0, 0, 0};
        static const uint8_t of_smap[FW_MATRIX_ROWS][FW_MATRIX_MAX_COLS] = {
            {6, 13, 20},
        };
        rt_of.test_row_len = of_rlen;
        rt_of.test_sensor_map = of_smap;
        assert(fw_runtime_load_keymap(&rt_of, of_km, sizeof(of_km)) == 0);

        /* 2 profiles: profile 0 (mode=0, layer=1), profile 1 (mode=2, layer=2) */
        uint8_t of_ps[32 + 16];
        memset(of_ps, 0, sizeof(of_ps));
        wr_le32(of_ps + 16, 2U);
        wr_le32(of_ps + 32, 0U);
        wr_le32(of_ps + 36, 1U);
        wr_le32(of_ps + 40, 2U);
        wr_le32(of_ps + 44, 2U);
        assert(fw_runtime_load_profile_settings(&rt_of, of_ps, sizeof(of_ps)) == 0);
        assert(fw_runtime_set_active_profile(&rt_of, 0U) == 0);

        uint16_t of_adc[FW_NUM_SENSORS];
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            of_adc[i] = 1800U;

        /* Press key 0 (PROFILE_CYCLE_NEXT): switches 0 -> 1 (mode=2, layer=2), no HID, no repeat while held */
        of_adc[6] = 2550U;
        of_adc[13] = 1050U;
        scan_held_position(&rt_of, of_adc, &hid);
        assert(rt_of.active_profile == 1U && rt_of.active_actuation_mode == 2U && rt_of.effective_layer_bitfield == 2U);
        assert(hid.modifiers == 0U && hid.keycodes[0] == 0U && hid.consumer_usage == 0U);
        assert(hid.of_func_mask == (1UL << FW_OF_FUNC_PROFILE_CYCLE_NEXT));
        /* Holding key 0 across another scan must not cycle again; the function bit stays set. */
        fw_runtime_process_scan(&rt_of, of_adc, &hid);
        assert(rt_of.active_profile == 1U && hid.of_func_mask == (1UL << FW_OF_FUNC_PROFILE_CYCLE_NEXT));

        /* Release key 0 -> clears of_func_mask */
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            of_adc[i] = 1800U;
        scan_held_position(&rt_of, of_adc, &hid);
        assert(hid.of_func_mask == 0U);

        /* On layer 2 (active_layer_bitfield=2), press key 0 (ACTUATION_MODE_NEXT): wraps 2 -> 0 */
        of_adc[6] = 2550U;
        of_adc[13] = 1050U;
        scan_held_position(&rt_of, of_adc, &hid);
        assert(rt_of.active_actuation_mode == 0U);
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            of_adc[i] = 1800U;
        scan_held_position(&rt_of, of_adc, &hid);

        /* Press key 1 (ACTUATION_MODE_PREV): wraps 0 -> 2 */
        of_adc[13] = 2550U;
        of_adc[20] = 1050U;
        scan_held_position(&rt_of, of_adc, &hid);
        assert(rt_of.active_actuation_mode == 2U);
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            of_adc[i] = 1800U;
        scan_held_position(&rt_of, of_adc, &hid);

        /* Set back to profile 0 (layer 1), press key 1 (PROFILE_CYCLE_PREV): wraps 0 -> 1 */
        assert(fw_runtime_set_active_profile(&rt_of, 0U) == 0);
        of_adc[13] = 2550U;
        of_adc[20] = 1050U;
        scan_held_position(&rt_of, of_adc, &hid);
        assert(rt_of.active_profile == 1U && rt_of.active_actuation_mode == 2U);
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            of_adc[i] = 1800U;
        scan_held_position(&rt_of, of_adc, &hid);

        /* Test PLATFORM_LINUX (0x08 -> 3), PLATFORM_MACOS (0x09 -> 2), PLATFORM_WINDOWS (0x0A -> 1) */
        wr_le32(of_km + 52 + 0, 0x50000000U | FW_OF_FUNC_PLATFORM_LINUX);
        wr_le32(of_km + 52 + 4, 0x50000000U | FW_OF_FUNC_PLATFORM_MACOS);
        wr_le32(of_km + 52 + 8, 0x50000000U | FW_OF_FUNC_PLATFORM_WINDOWS);
        assert(fw_runtime_load_keymap(&rt_of, of_km, sizeof(of_km)) == 0);
        assert(fw_runtime_set_active_profile(&rt_of, 0U) == 0);

        of_adc[6] = 2550U;
        of_adc[13] = 1050U;
        scan_held_position(&rt_of, of_adc, &hid);
        assert(rt_of.platform_code == 3U && rt_of.platform_dirty == 1U);
        rt_of.platform_dirty = 0U;
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            of_adc[i] = 1800U;
        scan_held_position(&rt_of, of_adc, &hid);

        of_adc[13] = 2550U;
        of_adc[20] = 1050U;
        scan_held_position(&rt_of, of_adc, &hid);
        assert(rt_of.platform_code == 2U && rt_of.platform_dirty == 1U);
        rt_of.platform_dirty = 0U;
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            of_adc[i] = 1800U;
        scan_held_position(&rt_of, of_adc, &hid);

        assert(fw_runtime_set_active_profile(&rt_of, 1U) == 0);
        of_adc[6] = 2550U;
        of_adc[13] = 1050U;
        scan_held_position(&rt_of, of_adc, &hid);
        assert(rt_of.platform_code == 1U && rt_of.platform_dirty == 1U);
        assert(hid.of_func_mask == (1UL << FW_OF_FUNC_PLATFORM_WINDOWS));
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            of_adc[i] = 1800U;
        scan_held_position(&rt_of, of_adc, &hid);
        assert(hid.of_func_mask == 0U);

        /* AUTO_PROFILE_LOCK on key 0 sets its function bit on press and clears it on release. */
        wr_le32(of_km + 52 + 0, 0x50000000U | FW_OF_FUNC_AUTO_PROFILE_LOCK);
        wr_le32(of_km + 52 + 4, 0x50000000U | FW_OF_FUNC_BRIGHTNESS_UP);
        wr_le32(of_km + 52 + 8, 0x50000000U | FW_OF_FUNC_THEME_CYCLE_NEXT);
        assert(fw_runtime_load_keymap(&rt_of, of_km, sizeof(of_km)) == 0);
        assert(fw_runtime_set_active_profile(&rt_of, 0U) == 0);
        of_adc[6] = 2550U;
        of_adc[13] = 1050U;
        scan_held_position(&rt_of, of_adc, &hid);
        assert(hid.of_func_mask == (1UL << FW_OF_FUNC_AUTO_PROFILE_LOCK));
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            of_adc[i] = 1800U;
        scan_held_position(&rt_of, of_adc, &hid);
        assert(hid.of_func_mask == 0U);

        /* Verify ordered of_event_seq/ring in MSG_UPDATE records firmware-resolved press events in order */
        uint8_t upd_msg[FW_UART_MSG_UPDATE_LEN];
        fw_runtime_build_msg_update(&rt_of, 999U, upd_msg);
        assert(upd_msg[5 + 0x2F] == 1U);
        uint16_t ev_seq = (uint16_t)upd_msg[5 + 0x30] | ((uint16_t)upd_msg[5 + 0x31] << 8);
        assert(ev_seq >= 2U);
        assert(upd_msg[5 + 0x32 + ((ev_seq - 1U) & 15U)] == FW_OF_FUNC_AUTO_PROFILE_LOCK);

        /* Verify FW_CMD_GET_RUNTIME_STATE (0x6D) returns 12B LE (profile, mode, platform) & NAKs non-empty */
        assert(fw_runtime_set_active_profile(&rt_of, 1U) == 0);
        rt_of.active_actuation_mode = 2U;
        rt_of.platform_code = 3U;
        fw_usb_rx_t state_rx;
        fw_usb_rx_init(&state_rx, &rt_of, NULL);
        reset_tx();
        uint8_t get_state_req[64] = {0};
        get_state_req[0] = FW_CMD_GET_RUNTIME_STATE;
        wr_le32(get_state_req + 1, 0U);
        wr_le32(get_state_req + 5, 0U);
        fw_usb_rx_feed(&state_rx, get_state_req, 64U, fake_usb_send, NULL);
        assert(g_tx_len >= 128U && g_tx_buf[0] == FW_RESP_ACK);
        uint32_t ack_packets = (uint32_t)g_tx_buf[1] | ((uint32_t)g_tx_buf[2] << 8);
        uint32_t ack_bytes = (uint32_t)g_tx_buf[5] | ((uint32_t)g_tx_buf[6] << 8);
        assert(ack_packets == 1U && ack_bytes == 12U);
        const uint8_t *st_p = g_tx_buf + FW_USB_PACKET_SIZE;
        uint32_t got_prof =
            (uint32_t)st_p[0] | ((uint32_t)st_p[1] << 8) | ((uint32_t)st_p[2] << 16) | ((uint32_t)st_p[3] << 24);
        uint32_t got_mode =
            (uint32_t)st_p[4] | ((uint32_t)st_p[5] << 8) | ((uint32_t)st_p[6] << 16) | ((uint32_t)st_p[7] << 24);
        uint32_t got_plat =
            (uint32_t)st_p[8] | ((uint32_t)st_p[9] << 8) | ((uint32_t)st_p[10] << 16) | ((uint32_t)st_p[11] << 24);
        assert(got_prof == 1U && got_mode == 2U && got_plat == 3U);

        reset_tx();
        get_state_req[5] = 4U;
        fw_usb_rx_feed(&state_rx, get_state_req, 64U, fake_usb_send, NULL);
        assert(g_tx_len >= 64U && g_tx_buf[0] == FW_RESP_NAK);
    }

    /* 9. Full NKRO: 10 simultaneous keys (HID 0x04..0x0D) + modifier (0xE0) across independent sensor pairs;
     *    all 10 usages must appear in nkro_bitmap while keycodes[6] retains the first 6 for boot fallback,
     *    and the scheduler must transmit updates when keys 7..10 are added or released. */
    {
        uint8_t nk_km[52 + 85 * 4 + 85 * 4];
        memset(nk_km, 0, sizeof(nk_km));
        wr_le32(nk_km + 0, 1);
        wr_le32(nk_km + 4, 1);
        wr_le32(nk_km + 24, 1);  /* n_layers */
        wr_le32(nk_km + 28, 85); /* n_keys */
        wr_le32(nk_km + 32, 1);  /* n_act */
        wr_le32(nk_km + 48, 85); /* act_off */
        static const uint16_t nk_keys[10] = {0, 2, 16, 18, 31, 33, 46, 48, 60, 74};
        for (size_t i = 0; i < 85; i++) {
            wr_le32(nk_km + 52 + i * 4U, 0xF0000000U);
            wr_le32(nk_km + 52 + 85 * 4U + i * 4U, 0x4006412CU);
        }
        for (size_t i = 0; i < 10; i++) {
            /* 9th key also includes Left Shift modifier bit (0x02) + usage 0x0C; 10th is 0x0D */
            uint32_t word = 0x00000004U + (uint32_t)i;
            if (i == 8)
                word |= (0x02U << 16);
            wr_le32(nk_km + 52 + nk_keys[i] * 4U, word);
        }

        fw_runtime_t rt_nk;
        fw_runtime_init(&rt_nk);
        fw_runtime_set_keyframe(&rt_nk, 1, 1, 1);
        assert(fw_runtime_load_keymap(&rt_nk, nk_km, sizeof(nk_km)) == 0);

        uint16_t nk_adc[FW_NUM_SENSORS];
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            nk_adc[i] = 1800U;

        static const uint8_t pos_s[10] = {6, 22, 4, 9, 3, 19, 2, 18, 1, 0};
        static const uint8_t neg_s[10] = {13, 16, 14, 17, 11, 31, 10, 28, 20, 15};

        /* First press keys 0..5 (6 keys) and flush scheduler */
        for (size_t i = 0; i < 6; i++) {
            nk_adc[pos_s[i]] = 2550U;
            nk_adc[neg_s[i]] = 1050U;
        }
        scan_held_position(&rt_nk, nk_adc, &hid);
        fw_hid_scheduler_t nk_sched;
        memset(&nk_sched, 0, sizeof(nk_sched));
        fake_hid_ep_t nk_ep;
        memset(&nk_ep, 0, sizeof(nk_ep));
        fw_hid_scheduler_step(&nk_sched, &hid, 1, 1, fake_send_kbd, fake_send_consumer, fake_send_of, &nk_ep);
        assert(nk_ep.kbd_sends == 1);

        /* Adding keys 6..9 changes the NKRO bitmap while the six boot keycodes remain unchanged. */
        for (size_t i = 6; i < 10; i++) {
            nk_adc[pos_s[i]] = 2550U;
            nk_adc[neg_s[i]] = 1050U;
        }
        scan_held_position(&rt_nk, nk_adc, &hid);
        assert(hid.modifiers == 0x02U);
        for (uint8_t code = 0x04U; code <= 0x0DU; code++) {
            assert((hid.nkro_bitmap[code >> 3] & (1U << (code & 7U))) != 0U);
        }
        /* Left Shift bit (0xE1) is also mirrored into nkro_bitmap */
        assert((hid.nkro_bitmap[0xE1U >> 3] & (1U << (0xE1U & 7U))) != 0U);

        nk_ep.ep_busy = 0;
        fw_hid_scheduler_step(&nk_sched, &hid, 1, 1, fake_send_kbd, fake_send_consumer, fake_send_of, &nk_ep);
        assert(nk_ep.kbd_sends == 2);
        for (uint8_t code = 0x04U; code <= 0x0DU; code++) {
            assert((nk_ep.last_nkro_bitmap[code >> 3] & (1U << (code & 7U))) != 0U);
        }

        /* Release all keys -> nkro_bitmap clears completely */
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            nk_adc[i] = 1800U;
        scan_held_position(&rt_nk, nk_adc, &hid);
        for (size_t b = 0; b < 32; b++)
            assert(hid.nkro_bitmap[b] == 0U);
    }

    /* 10. Layer control types (toggleLayer=2, switchToLayer=3) & macro pool playback (kind=0x1 with delays) */
    {
        /* 3 layers, 4 keys, 1 act mode, 1 macro [down LShift(0xE1,0ms), down A(0x04,0ms), up A(0x04,20ms), up
         * LShift(0xE1,0ms)] */
        uint8_t lm_km[24 + (7 + 6 + 3 * 4 + 4) * 4];
        memset(lm_km, 0, sizeof(lm_km));
        wr_le32(lm_km + 0, 1);
        wr_le32(lm_km + 4, 1);
        wr_le32(lm_km + 24 + 0, 3);       /* n_layers */
        wr_le32(lm_km + 24 + 4, 4);       /* n_keys */
        wr_le32(lm_km + 24 + 8, 1);       /* n_act */
        wr_le32(lm_km + 24 + 12, 1);      /* n_macros */
        wr_le32(lm_km + 24 + 16, 1);      /* n_macros again, ignored */
        wr_le32(lm_km + 24 + 20, 6);      /* kc_off (1 offset + 1 len + 4 steps) */
        wr_le32(lm_km + 24 + 24, 6 + 12); /* act_off */
        uint8_t *mp = lm_km + 24 + 28;
        wr_le32(mp + 0, 1);                                 /* macro 0 offset = 1 */
        wr_le32(mp + 4, 4);                                 /* macro 0 len = 4 steps */
        wr_le32(mp + 8, (1UL << 31) | (0U << 8) | 0xE1U);   /* down Left Shift, 0ms */
        wr_le32(mp + 12, (1UL << 31) | (0U << 8) | 0x04U);  /* down A, 0ms */
        wr_le32(mp + 16, (0UL << 31) | (20U << 8) | 0x04U); /* up A, 20ms */
        wr_le32(mp + 20, (0UL << 31) | (0U << 8) | 0xE1U);  /* up Left Shift, 0ms */

        uint8_t *lkc = mp + 24;
        /* Layer 0: [TOGGLE(1), SWITCH(2), MACRO(0), KC_A] */
        wr_le32(lkc + 0, 0x22000001U);
        wr_le32(lkc + 4, 0x23000002U);
        wr_le32(lkc + 8, 0x10000000U);
        wr_le32(lkc + 12, 0x00000004U);
        /* Layer 1: [KC_TRANSPARENT, KC_TRANSPARENT, KC_TRANSPARENT, KC_B] */
        wr_le32(lkc + 16, 0x50000000U);
        wr_le32(lkc + 20, 0x50000000U);
        wr_le32(lkc + 24, 0x50000000U);
        wr_le32(lkc + 28, 0x00000005U);
        /* Layer 2: [KC_TRANSPARENT, SWITCH(0), KC_TRANSPARENT, KC_C] */
        wr_le32(lkc + 32, 0x50000000U);
        wr_le32(lkc + 36, 0x23000000U);
        wr_le32(lkc + 40, 0x50000000U);
        wr_le32(lkc + 44, 0x00000006U);
        uint8_t *lact = lkc + 48;
        for (int i = 0; i < 4; i++)
            wr_le32(lact + i * 4, 0x4006412CU);

        static const uint8_t lm_rlen[FW_MATRIX_ROWS] = {5, 0, 0, 0, 0, 0};
        static const uint8_t lm_smap[FW_MATRIX_ROWS][FW_MATRIX_MAX_COLS] = {
            {6, 13, 22, 16, 26},
        };
        fw_runtime_t rt_lm;
        fw_runtime_init(&rt_lm);
        fw_runtime_set_keyframe(&rt_lm, 1, 1, 1);
        rt_lm.test_row_len = lm_rlen;
        rt_lm.test_sensor_map = lm_smap;
        assert(fw_runtime_load_keymap(&rt_lm, lm_km, sizeof(lm_km)) == 0);

        uint16_t lm_adc[FW_NUM_SENSORS];
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            lm_adc[i] = 1800U;

        /* Press key 0 (TOGGLE(1)): stays active after release */
        lm_adc[6] = 2550U;
        lm_adc[13] = 1050U;
        scan_held_position(&rt_lm, lm_adc, &hid);
        assert((rt_lm.effective_layer_bitfield & 0x02U) != 0U);
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            lm_adc[i] = 1800U;
        scan_held_position(&rt_lm, lm_adc, &hid);
        assert((rt_lm.effective_layer_bitfield & 0x02U) != 0U);
        /* With layer 1 toggled on, key 3 resolves to KC_B (0x05). */
        lm_adc[16] = 2550U;
        lm_adc[26] = 1050U;
        scan_held_position(&rt_lm, lm_adc, &hid);
        assert(hid.keycodes[0] == 0x05U);
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            lm_adc[i] = 1800U;
        scan_held_position(&rt_lm, lm_adc, &hid);
        /* Pressing key 0 again toggles layer 1 off. */
        lm_adc[6] = 2550U;
        lm_adc[13] = 1050U;
        scan_held_position(&rt_lm, lm_adc, &hid);
        assert((rt_lm.effective_layer_bitfield & 0x02U) == 0U);
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            lm_adc[i] = 1800U;
        scan_held_position(&rt_lm, lm_adc, &hid);

        /* Press key 1 (SWITCH(2)): switches base layer to 2, persists after release; key 3 -> KC_C (0x06) */
        lm_adc[13] = 2550U;
        lm_adc[22] = 1050U;
        scan_held_position(&rt_lm, lm_adc, &hid);
        assert(rt_lm.effective_layer_bitfield == 0x04U);
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            lm_adc[i] = 1800U;
        scan_held_position(&rt_lm, lm_adc, &hid);
        assert(rt_lm.effective_layer_bitfield == 0x04U);
        lm_adc[16] = 2550U;
        lm_adc[26] = 1050U;
        scan_held_position(&rt_lm, lm_adc, &hid);
        assert(hid.keycodes[0] == 0x06U);
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            lm_adc[i] = 1800U;
        scan_held_position(&rt_lm, lm_adc, &hid);
        /* Press key 1 on Layer 2 (SWITCH(0)) -> switches back to Layer 0 */
        lm_adc[13] = 2550U;
        lm_adc[22] = 1050U;
        scan_held_position(&rt_lm, lm_adc, &hid);
        assert(rt_lm.effective_layer_bitfield == 0x01U);
        for (size_t i = 0; i < FW_NUM_SENSORS; i++)
            lm_adc[i] = 1800U;
        scan_held_position(&rt_lm, lm_adc, &hid);

        /* Press key 2 (MACRO(0)) at t=1000: plays Shift+A respecting 20ms delay between press and release */
        uint32_t mt = 1000U;
        lm_adc[22] = 2550U;
        lm_adc[16] = 1050U;
        for (int s = 0; s < 24 && !rt_lm.macro_active; s++) {
            fw_runtime_set_time(&rt_lm, ++mt);
            fw_runtime_process_scan(&rt_lm, lm_adc, &hid);
        }
        assert(rt_lm.macro_active && hid.modifiers == 0x02U && hid.keycodes[0] == 0U);
        /* Next tick (+1ms): step 1 (down A, 0ms delay) */
        fw_runtime_set_time(&rt_lm, ++mt);
        fw_runtime_process_scan(&rt_lm, lm_adc, &hid);
        assert(hid.modifiers == 0x02U && hid.keycodes[0] == 0x04U);
        /* Before 20ms delay expires (+10ms): A must stay held */
        mt += 10U;
        fw_runtime_set_time(&rt_lm, mt);
        fw_runtime_process_scan(&rt_lm, lm_adc, &hid);
        assert(hid.modifiers == 0x02U && hid.keycodes[0] == 0x04U);
        /* After 20ms delay (+10ms more): step 2 (up A) */
        mt += 10U;
        fw_runtime_set_time(&rt_lm, mt);
        fw_runtime_process_scan(&rt_lm, lm_adc, &hid);
        assert(hid.modifiers == 0x02U && hid.keycodes[0] == 0U);
        /* Next tick (+1ms): step 3 (up LShift) */
        fw_runtime_set_time(&rt_lm, ++mt);
        fw_runtime_process_scan(&rt_lm, lm_adc, &hid);
        assert(hid.modifiers == 0U && hid.keycodes[0] == 0U);

        /* Re-trigger macro (key 2 on sensors 22 & 16) while USB HID endpoint is busy: steps must wait, never drop */
        fw_runtime_set_hid_ready(&rt_lm, 1U);
        lm_adc[22] = 1800U;
        lm_adc[16] = 1800U;
        for (int i = 0; i < 6; i++) {
            fw_runtime_set_time(&rt_lm, ++mt);
            fw_runtime_process_scan(&rt_lm, lm_adc, &hid);
        }
        /* Press key 2 (macro 0) while HID endpoint is busy */
        fw_runtime_set_hid_ready(&rt_lm, 0U);
        lm_adc[22] = 2550U;
        lm_adc[16] = 1050U;
        for (int i = 0; i < 12; i++) {
            fw_runtime_set_time(&rt_lm, ++mt);
            fw_runtime_process_scan(&rt_lm, lm_adc, &hid);
        }
        /* Endpoint was busy the whole time: step 0 must not have advanced yet */
        assert(hid.modifiers == 0U && hid.keycodes[0] == 0U);
        /* Endpoint becomes ready: step 0 (down LShift) emits */
        fw_runtime_set_hid_ready(&rt_lm, 1U);
        fw_runtime_set_time(&rt_lm, ++mt);
        fw_runtime_process_scan(&rt_lm, lm_adc, &hid);
        assert(hid.modifiers == 0x02U && hid.keycodes[0] == 0U);
        /* Endpoint busy again for several scans: step 1 (down A) waits */
        fw_runtime_set_hid_ready(&rt_lm, 0U);
        for (int i = 0; i < 4; i++) {
            fw_runtime_set_time(&rt_lm, ++mt);
            fw_runtime_process_scan(&rt_lm, lm_adc, &hid);
            assert(hid.modifiers == 0x02U && hid.keycodes[0] == 0U);
        }
        /* Endpoint ready: step 1 (down A) emits cleanly */
        fw_runtime_set_hid_ready(&rt_lm, 1U);
        fw_runtime_set_time(&rt_lm, ++mt);
        fw_runtime_process_scan(&rt_lm, lm_adc, &hid);
        assert(hid.modifiers == 0x02U && hid.keycodes[0] == 0x04U);
    }

    /* 11. Async bounded IF1 TX queue: respects backpressure (avail < 64), chunks large GET_KEYMAP, and times out
     * stalled host */
    {
        static fake_async_usb_t u;
        memset(&u, 0, sizeof(u));

        static fw_usb_rx_t arx;
        static fw_usb_tx_t atx;
        static fw_runtime_t art;
        fw_runtime_init(&art);
        /* Load a large valid 8-layer ANSI keymap (~3 KiB) to test multi-packet async GET_KEYMAP */
        uint8_t big_km[24 + (7 + 8 * 85 + 85) * 4];
        memset(big_km, 0, sizeof(big_km));
        wr_le32(big_km + 0, 1);
        wr_le32(big_km + 4, 1);
        wr_le32(big_km + 24 + 0, 8);
        wr_le32(big_km + 24 + 4, 85);
        wr_le32(big_km + 24 + 8, 1);
        wr_le32(big_km + 24 + 24, 8 * 85);
        for (size_t i = 0; i < 8 * 85; i++)
            wr_le32(big_km + 52 + i * 4U, 0x04U + (uint32_t)(i % 20U));
        for (size_t i = 0; i < 85; i++)
            wr_le32(big_km + 52 + 8 * 85 * 4U + i * 4U, 0x4006412CU);
        assert(fw_runtime_load_keymap(&art, big_km, sizeof(big_km)) == 0);

        fw_usb_rx_init(&arx, &art, NULL);
        fw_usb_tx_init(&atx);

        uint8_t req[64] = {0};
        req[0] = FW_CMD_GET_KEYMAP;
        fw_usb_rx_feed_async(&arx, &atx, req, sizeof(req));
        assert(fw_usb_tx_busy(&atx));

        /* Full backpressure (avail == 0): sends 0 packets, returns immediately without blocking */
        u.avail_bytes = 0U;
        assert(fw_usb_tx_step(&atx, 100U, 4U, fake_async_avail, fake_async_write, fake_async_flush, &u) == 0U);
        assert(u.out_len == 0U && fw_usb_tx_busy(&atx));

        /* Drain 256 B (4 packets) per step; each step sends between one and four packets. */
        uint32_t t_ms = 101U;
        uint32_t steps = 0U;
        while (fw_usb_tx_busy(&atx) && steps < 64U) {
            u.avail_bytes = 256U;
            uint32_t n = fw_usb_tx_step(&atx, t_ms++, 4U, fake_async_avail, fake_async_write, fake_async_flush, &u);
            assert(n >= 1U && n <= 4U);
            steps++;
        }
        assert(!fw_usb_tx_busy(&atx));
        uint32_t expected_pkts = (uint32_t)((sizeof(big_km) + 63U) / 64U);
        assert(u.out_len == (1U + expected_pkts) * 64U);
        assert(u.out[0] == FW_RESP_ACK);
        assert(memcmp(u.out + 64, big_km, sizeof(big_km)) == 0);

        /* Stall timeout recovery: if host stops reading for >= FW_USB_TX_STALL_TIMEOUT_MS, TX drops cleanly */
        fw_usb_rx_feed_async(&arx, &atx, req, sizeof(req));
        assert(fw_usb_tx_busy(&atx));
        u.avail_bytes = 0U;
        assert(fw_usb_tx_step(&atx, 5000U, 4U, fake_async_avail, fake_async_write, fake_async_flush, &u) == 0U);
        assert(fw_usb_tx_busy(&atx));
        assert(fw_usb_tx_step(&atx, 5000U + FW_USB_TX_STALL_TIMEOUT_MS, 4U, fake_async_avail, fake_async_write,
                              fake_async_flush, &u) == 0U);
        assert(!fw_usb_tx_busy(&atx));
    }

    printf("fw_core tests passed.\n");
    return 0;
}
