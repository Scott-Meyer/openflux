/*
 * Platform shims connecting real vial-qmk to the Pico SDK / host test runtime.
 * Owns timer, RAM-backed EEPROM with debounced flush tracking, matrix rows,
 * QMK host_driver_t capture, Raw HID send, and dynamic XZ vial.json + UID cache busting.
 */

#include "fw_qmk.h"

#include "config.h"
#include "eeprom.h"
#include "host.h"
#include "host_driver.h"
#include "matrix.h"
#include "raw_hid.h"
#include "timer.h"
#include "usb_device_state.h"
#include "vial_generated_keyboard_definition.h"

#include <stdbool.h>
#include <stdint.h>
#include <string.h>

/* ---- Internal state shared with fw_qmk.c ---- */

fw_qmk_platform_t g_fw_qmk_plat;
uint32_t g_fw_qmk_now_ms = 0U;
uint32_t g_fw_qmk_keymap_rev = 1U;
fw_hid_report_t g_fw_qmk_hid;
uint32_t g_fw_qmk_of_func_mask = 0U;

/* FIFO preserving every HID report QMK emits within a single scan (critical for tap-hold
 * taps and long send_string/Vial macros that emit 100+ reports inside one keyboard_task() call). */
#define FW_QMK_HID_QUEUE_CAP 256U
static fw_hid_report_t g_hid_q[FW_QMK_HID_QUEUE_CAP];
static uint16_t g_hid_q_head = 0U;
static uint16_t g_hid_q_tail = 0U;
static fw_hid_report_t g_hid_q_last_pushed;
static bool g_hid_q_has_last = false;

static bool hid_kbd_consumer_equal(const fw_hid_report_t *a, const fw_hid_report_t *b) {
    return a->modifiers == b->modifiers && a->consumer_usage == b->consumer_usage &&
           memcmp(a->keycodes, b->keycodes, sizeof(a->keycodes)) == 0 &&
           memcmp(a->nkro_bitmap, b->nkro_bitmap, sizeof(a->nkro_bitmap)) == 0;
}

static void hid_q_push(const fw_hid_report_t *rep) {
    if (g_hid_q_has_last && hid_kbd_consumer_equal(rep, &g_hid_q_last_pushed))
        return;
    uint16_t next = (uint16_t)((g_hid_q_head + 1U) % FW_QMK_HID_QUEUE_CAP);
    if (next == g_hid_q_tail) {
        /* Try draining the oldest queued report to USB so giant macros (>120 chars) never lose keys. */
        if (g_fw_qmk_plat.hid_drain_one && g_fw_qmk_plat.hid_drain_one(&g_hid_q[g_hid_q_tail], g_fw_qmk_plat.ctx)) {
            g_hid_q_tail = (uint16_t)((g_hid_q_tail + 1U) % FW_QMK_HID_QUEUE_CAP);
        } else {
            /* Host stopped polling: drop oldest entry, keeping the most recent state. */
            g_hid_q_tail = (uint16_t)((g_hid_q_tail + 1U) % FW_QMK_HID_QUEUE_CAP);
        }
    }
    g_hid_q[g_hid_q_head] = *rep;
    g_hid_q_head = next;
    g_hid_q_last_pushed = *rep;
    g_hid_q_has_last = true;
}

bool fw_qmk_pop_hid_report(fw_hid_report_t *out_hid) {
    if (g_hid_q_tail == g_hid_q_head)
        return false;
    if (out_hid) {
        *out_hid = g_hid_q[g_hid_q_tail];
        out_hid->of_func_mask = g_fw_qmk_of_func_mask;
    }
    g_hid_q_tail = (uint16_t)((g_hid_q_tail + 1U) % FW_QMK_HID_QUEUE_CAP);
    return true;
}

void fw_qmk_clear_hid_queue(void) {
    g_hid_q_head = 0U;
    g_hid_q_tail = 0U;
    g_hid_q_has_last = false;
    memset(&g_hid_q_last_pushed, 0, sizeof(g_hid_q_last_pushed));
}

static uint8_t g_eeprom[FW_QMK_EEPROM_SIZE];
static bool g_eeprom_dirty = false;
static uint32_t g_eeprom_dirty_since_ms = 0U;
static bool g_eeprom_suppress_dirty = false;

static matrix_row_t g_matrix[MATRIX_ROWS];

/* Dynamic XZ vial.json override (when uploaded by openfluxd). */
static uint8_t g_vial_def_buf[FW_QMK_MAX_VIAL_DEF_BYTES];
static uint32_t g_vial_def_len = 0U;
static uint8_t g_vial_uid[8] = VIAL_KEYBOARD_UID;
static bool g_vial_uid_custom = false;

/* ---- Timer ---- */

void timer_init(void) {}
void timer_clear(void) { g_fw_qmk_now_ms = 0U; }

uint16_t timer_read(void) {
    return (uint16_t)(g_fw_qmk_now_ms & 0xFFFFU);
}

uint32_t timer_read32(void) {
    return g_fw_qmk_now_ms;
}

uint16_t timer_elapsed(uint16_t last) {
    return (uint16_t)(timer_read() - last);
}

uint32_t timer_elapsed32(uint32_t last) {
    return g_fw_qmk_now_ms - last;
}

void fw_qmk_wait_ms(uint32_t ms) {
    /* USB 1ms HID polling naturally paces queued FIFO reports without skewing QMK's scan clock. */
    (void)ms;
}

/* ---- EEPROM (RAM buffer + dirty tracking for debounced LittleFS flush) ---- */

uint8_t *fw_qmk_eeprom_buf(void) {
    return g_eeprom;
}

void fw_qmk_eeprom_set_suppress_dirty(bool suppress) {
    g_eeprom_suppress_dirty = suppress;
}

bool fw_qmk_eeprom_is_dirty(void) {
    return g_eeprom_dirty;
}

uint32_t fw_qmk_eeprom_dirty_since(void) {
    return g_eeprom_dirty_since_ms;
}

void fw_qmk_eeprom_clear_dirty(void) {
    g_eeprom_dirty = false;
}

static uint8_t ee_read_u8(uintptr_t off) {
    return (off < sizeof(g_eeprom)) ? g_eeprom[off] : 0xFFU;
}

static void ee_write_u8(uintptr_t off, uint8_t value) {
    if (off >= sizeof(g_eeprom))
        return;
    if (g_eeprom[off] != value) {
        g_eeprom[off] = value;
        if (!g_eeprom_suppress_dirty) {
            g_eeprom_dirty = true;
            g_eeprom_dirty_since_ms = g_fw_qmk_now_ms;
            g_fw_qmk_keymap_rev++;
        }
    }
}

uint8_t eeprom_read_byte(const uint8_t *addr) {
    return ee_read_u8((uintptr_t)addr);
}

void eeprom_write_byte(uint8_t *addr, uint8_t value) {
    ee_write_u8((uintptr_t)addr, value);
}

uint16_t eeprom_read_word(const uint16_t *addr) {
    uintptr_t off = (uintptr_t)addr;
    return (uint16_t)ee_read_u8(off) | ((uint16_t)ee_read_u8(off + 1U) << 8);
}

uint32_t eeprom_read_dword(const uint32_t *addr) {
    uintptr_t off = (uintptr_t)addr;
    return (uint32_t)ee_read_u8(off) | ((uint32_t)ee_read_u8(off + 1U) << 8) |
           ((uint32_t)ee_read_u8(off + 2U) << 16) | ((uint32_t)ee_read_u8(off + 3U) << 24);
}

void eeprom_read_block(void *buf, const void *addr, size_t len) {
    uintptr_t off = (uintptr_t)addr;
    uint8_t *dst = (uint8_t *)buf;
    for (size_t i = 0; i < len; i++) {
        dst[i] = ee_read_u8(off + i);
    }
}

void eeprom_write_word(uint16_t *addr, uint16_t value) {
    uintptr_t off = (uintptr_t)addr;
    ee_write_u8(off, (uint8_t)(value & 0xFFU));
    ee_write_u8(off + 1U, (uint8_t)(value >> 8));
}

void eeprom_write_dword(uint32_t *addr, uint32_t value) {
    uintptr_t off = (uintptr_t)addr;
    ee_write_u8(off, (uint8_t)(value & 0xFFU));
    ee_write_u8(off + 1U, (uint8_t)((value >> 8) & 0xFFU));
    ee_write_u8(off + 2U, (uint8_t)((value >> 16) & 0xFFU));
    ee_write_u8(off + 3U, (uint8_t)((value >> 24) & 0xFFU));
}

void eeprom_write_block(const void *buf, void *addr, size_t len) {
    uintptr_t off = (uintptr_t)addr;
    const uint8_t *src = (const uint8_t *)buf;
    for (size_t i = 0; i < len; i++) {
        ee_write_u8(off + i, src[i]);
    }
}

void eeprom_update_byte(uint8_t *addr, uint8_t value) { eeprom_write_byte(addr, value); }
void eeprom_update_word(uint16_t *addr, uint16_t value) { eeprom_write_word(addr, value); }
void eeprom_update_dword(uint32_t *addr, uint32_t value) { eeprom_write_dword(addr, value); }
void eeprom_update_block(const void *buf, void *addr, size_t len) { eeprom_write_block(buf, addr, len); }

/* ---- Matrix ---- */

void fw_qmk_matrix_set(const matrix_row_t rows[MATRIX_ROWS]) {
    memcpy(g_matrix, rows, sizeof(g_matrix));
}

uint8_t matrix_rows(void) { return MATRIX_ROWS; }
uint8_t matrix_cols(void) { return MATRIX_COLS; }
void matrix_setup(void) {}
void matrix_init(void) { memset(g_matrix, 0, sizeof(g_matrix)); }
uint8_t matrix_scan(void) { return 1; }
bool matrix_can_read(void) { return true; }
matrix_row_t matrix_get_row(uint8_t row) { return (row < MATRIX_ROWS) ? g_matrix[row] : 0U; }
bool matrix_is_on(uint8_t row, uint8_t col) { return (matrix_get_row(row) & ((matrix_row_t)1U << col)) != 0U; }
void matrix_print(void) {}
void matrix_power_up(void) {}
void matrix_power_down(void) {}

/* ---- QMK host_driver_t -> fw_hid_report_t ---- */

static uint8_t drv_keyboard_leds(void) {
    if (g_fw_qmk_plat.get_host_leds) {
        return g_fw_qmk_plat.get_host_leds(g_fw_qmk_plat.ctx);
    }
    return 0U;
}

static void drv_send_keyboard(report_keyboard_t *report) {
    if (!report)
        return;
    memset(g_fw_qmk_hid.nkro_bitmap, 0, sizeof(g_fw_qmk_hid.nkro_bitmap));
    g_fw_qmk_hid.modifiers = report->mods;
    memcpy(g_fw_qmk_hid.keycodes, report->keys, 6U);
    for (uint8_t bit = 0U; bit < 8U; bit++) {
        if ((report->mods >> bit) & 1U) {
            uint8_t u = (uint8_t)(0xE0U + bit);
            g_fw_qmk_hid.nkro_bitmap[u >> 3] |= (uint8_t)(1U << (u & 7U));
        }
    }
    for (size_t i = 0; i < 6U; i++) {
        uint8_t u = report->keys[i];
        if (u > 0U) {
            g_fw_qmk_hid.nkro_bitmap[u >> 3] |= (uint8_t)(1U << (u & 7U));
        }
    }
    hid_q_push(&g_fw_qmk_hid);
}

static void drv_send_nkro(report_nkro_t *report) {
    if (!report)
        return;
    memset(g_fw_qmk_hid.nkro_bitmap, 0, sizeof(g_fw_qmk_hid.nkro_bitmap));
    memset(g_fw_qmk_hid.keycodes, 0, sizeof(g_fw_qmk_hid.keycodes));
    g_fw_qmk_hid.modifiers = report->mods;
    /* Copy usages 0x00..0xDF (28 bytes = 224 bits), then pack modifier bits 0xE0..0xE7 in byte 28. */
    memcpy(g_fw_qmk_hid.nkro_bitmap, report->bits, 28U);
    g_fw_qmk_hid.nkro_bitmap[28] = report->mods;
    /* Derive 6KRO boot fallback list from active NKRO keys. */
    size_t nk = 0;
    for (uint16_t u = 1U; u < 0xE0U && nk < 6U; u++) {
        if ((g_fw_qmk_hid.nkro_bitmap[u >> 3] >> (u & 7U)) & 1U) {
            g_fw_qmk_hid.keycodes[nk++] = (uint8_t)u;
        }
    }
    hid_q_push(&g_fw_qmk_hid);
}

static void drv_send_mouse(report_mouse_t *report) {
    (void)report;
}

static void drv_send_extra(report_extra_t *report) {
    if (!report)
        return;
    if (report->report_id == REPORT_ID_CONSUMER) {
        g_fw_qmk_hid.consumer_usage = report->usage;
        hid_q_push(&g_fw_qmk_hid);
    }
}

static void drv_send_raw_hid(uint8_t *data, uint8_t length) {
    if (!data || length == 0U || !g_fw_qmk_plat.raw_hid_send)
        return;
    uint8_t pkt[FW_QMK_VIAL_RAW_EPSIZE];
    memset(pkt, 0, sizeof(pkt));
    size_t n = (length < sizeof(pkt)) ? (size_t)length : sizeof(pkt);
    memcpy(pkt, data, n);
    g_fw_qmk_plat.raw_hid_send(pkt, g_fw_qmk_plat.ctx);
}

static host_driver_t g_qmk_host_driver = {
    .keyboard_leds = drv_keyboard_leds,
    .send_keyboard = drv_send_keyboard,
    .send_nkro = drv_send_nkro,
    .send_mouse = drv_send_mouse,
    .send_extra = drv_send_extra,
    .send_raw_hid = drv_send_raw_hid,
};

host_driver_t *fw_qmk_get_host_driver(void) {
    return &g_qmk_host_driver;
}

/* ---- Dynamic XZ vial.json + UID cache busting (called by patched vial.c) ---- */

static uint32_t fnv1a_32(const uint8_t *data, size_t len) {
    uint32_t h = 2166136261UL;
    for (size_t i = 0; i < len; i++) {
        h ^= data[i];
        h *= 16777619UL;
    }
    return h;
}

int fw_qmk_set_vial_def(const uint8_t *xz_data, size_t len, const uint8_t *uid) {
    if (!xz_data || len == 0U || len > sizeof(g_vial_def_buf))
        return -1;
    memcpy(g_vial_def_buf, xz_data, len);
    g_vial_def_len = (uint32_t)len;
    if (uid) {
        memcpy(g_vial_uid, uid, 8U);
    } else {
        static const uint8_t base_uid[8] = VIAL_KEYBOARD_UID;
        memcpy(g_vial_uid, base_uid, 4U);
        uint32_t h = fnv1a_32(xz_data, len);
        g_vial_uid[4] = (uint8_t)(h & 0xFFU);
        g_vial_uid[5] = (uint8_t)((h >> 8) & 0xFFU);
        g_vial_uid[6] = (uint8_t)((h >> 16) & 0xFFU);
        g_vial_uid[7] = (uint8_t)((h >> 24) & 0xFFU);
    }
    g_vial_uid_custom = true;
    return 0;
}

void fw_qmk_reset_vial_def(void) {
    static const uint8_t base_uid[8] = VIAL_KEYBOARD_UID;
    g_vial_def_len = 0U;
    memcpy(g_vial_uid, base_uid, 8U);
    g_vial_uid_custom = false;
}

void fw_qmk_get_vial_uid(uint8_t out_uid[8]) {
    if (g_vial_uid_custom) {
        memcpy(out_uid, g_vial_uid, 8U);
    } else {
        static const uint8_t base_uid[8] = VIAL_KEYBOARD_UID;
        memcpy(out_uid, base_uid, 8U);
    }
}

const uint8_t *fw_qmk_get_vial_def_data(uint32_t *out_size) {
    if (g_vial_def_len > 0U) {
        if (out_size)
            *out_size = g_vial_def_len;
        return g_vial_def_buf;
    }
    if (out_size)
        *out_size = (uint32_t)sizeof(keyboard_definition);
    return keyboard_definition;
}

/* ---- Platform stubs required by QMK core ---- */

void bootloader_jump(void) {}
void mcu_reset(void) {}
int8_t sendchar(uint8_t c) {
    (void)c;
    return 0;
}
void platform_setup(void) {}
void protocol_pre_init(void) {}
void protocol_post_init(void) {}
void protocol_pre_task(void) {}
void protocol_post_task(void) {}
void suspend_power_down(void) {}
void suspend_wakeup_init(void) {}
