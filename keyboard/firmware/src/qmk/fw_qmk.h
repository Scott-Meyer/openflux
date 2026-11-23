#ifndef FW_QMK_H
#define FW_QMK_H

#include "fw_scanner.h"

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

/*
 * Public contract for the vendored vial-qmk typing/keymap module.
 *
 * Callers (fw_scanner.c, rp2040_main.c, fw_usb_cmd.c) talk only through this header and
 * never include QMK internals. QMK/Vial owns keymaps, layers, tap-hold, combos, tap dance,
 * key overrides, one-shot modifiers, Caps Word, macros, and Vial Raw HID.
 */

#define FW_QMK_VIAL_RAW_EPSIZE    32U
#define FW_QMK_EEPROM_SIZE        8192U
#define FW_QMK_MAX_VIAL_DEF_BYTES 10240U
#define FW_QMK_NUM_CUSTOM_SLOTS   64U
#define FW_QMK_BUILTIN_SLOTS      14U /* Slots 0..13 map to fw_of_func_code 0x02..0x0F */

typedef struct {
    /* Optional: load/save the flat per-layout QMK/Vial EEPROM image (layout_id 1=ANSI, 2=ISO, 3=JIS).
     * eeprom_load returns: 0 = loaded OK, 1 = absent (safe to seed defaults/migrate), <0 = I/O error
     * (run defaults in RAM ONLY and do NOT overwrite flash until an explicit user Vial edit). */
    int (*eeprom_load)(uint8_t layout_id, uint8_t *buf, size_t len, void *ctx);
    int (*eeprom_save)(uint8_t layout_id, const uint8_t *buf, size_t len, void *ctx);
    /* Optional: transmit a 32-byte Vial Raw HID response packet over USB IF 5. */
    void (*raw_hid_send)(const uint8_t data[FW_QMK_VIAL_RAW_EPSIZE], void *ctx);
    /* Optional: drain one queued HID report to USB when a giant macro fills the 256-entry FIFO. */
    bool (*hid_drain_one)(const fw_hid_report_t *rep, void *ctx);
    /* Optional: notify caller when a QK_KB_n Flux/module action is pressed or released (never sent to OS). */
    void (*on_flux_action)(uint16_t slot, bool pressed, uint8_t effective_mods, uint32_t active_layers, void *ctx);
    /* Optional: return current host keyboard LED mask (Caps/Num/Scroll Lock). */
    uint8_t (*get_host_leds)(void *ctx);
    void *ctx;
} fw_qmk_platform_t;

typedef struct {
    uint8_t effective_mods;
    uint8_t oneshot_mods;
    uint8_t oneshot_layer; /* 0xFF if none */
    uint8_t caps_word_active;
    uint32_t layer_state;
    uint32_t default_layer_state;
    uint32_t keymap_revision;
} fw_qmk_state_t;

/* Initialize QMK + Vial state. Safe to call repeatedly (e.g. between unit tests). */
void fw_qmk_init(const fw_qmk_platform_t *plat, uint8_t initial_layout_id, uint32_t now_ms);

/* One-time migration: if the current layout had no saved Vial EEPROM on flash and rt holds a
 * valid legacy 32-bit OpenFlux keymap section for layout_id, seed QMK's dynamic keymap from it. */
bool fw_qmk_seed_from_legacy_keymap(const fw_runtime_t *rt, uint8_t layout_id);

/* Switch active physical keyframe layout (1=ANSI, 2=ISO, 3=JIS). Flushes dirty EEPROM for the
 * previous layout (if any) and loads the target layout's EEPROM image into QMK/Vial. */
void fw_qmk_set_layout(uint8_t layout_id, uint32_t now_ms);

/* Update default layer (called when SET_ACTIVE_PROFILE switches profile). */
void fw_qmk_set_default_layer(uint8_t layer);

/* Step QMK with the latest Hall-effect boolean key states and timestamp, updating out_hid
 * with the current NKRO/6KRO keyboard report, consumer usage, and legacy of_func_mask.
 * Intermediate press/release transitions emitted inside a single scan (e.g. tap-hold taps,
 * buffered rolls, combos, macros) are preserved in a FIFO drained via fw_qmk_pop_hid_report(). */
void fw_qmk_process_scan(const uint8_t *key_pressed, uint16_t n_keys, uint32_t now_ms, fw_hid_report_t *out_hid);

/* Pop the next queued USB HID report emitted by QMK (returns false when the FIFO is empty). */
bool fw_qmk_pop_hid_report(fw_hid_report_t *out_hid);

/* Discard any queued HID reports (e.g. on USB disconnect/reconfiguration). */
void fw_qmk_clear_hid_queue(void);

/* Returns true if any reachable layer on linear key_idx resolves to a dual-role (MT/LT)
 * or Tap Dance keycode, so the Hall-effect scanner disables rapid trigger on that key. */
bool fw_qmk_key_forbids_rapid_trigger(uint16_t key_idx);

/* Feed an incoming 32-byte Vial Raw HID packet from USB IF 5. */
void fw_qmk_raw_hid_rx(const uint8_t data[FW_QMK_VIAL_RAW_EPSIZE], uint32_t now_ms);

/* Install a dynamic XZ-compressed vial.json definition (and optional explicit 8-byte UID;
 * if uid is NULL, the low 4 bytes of VIAL_KEYBOARD_UID are derived by hashing xz_data so
 * external Vial clients automatically invalidate cached definitions). */
int fw_qmk_set_vial_def(const uint8_t *xz_data, size_t len, const uint8_t *uid);

/* Read current honest QMK state for UART MSG_UPDATE / display legends. */
void fw_qmk_get_state(fw_qmk_state_t *out);

/* Resolve the effective 16-bit QMK keycode for linear key_idx under the current active
 * layer stack (handling KC_TRANSPARENT fallthrough and Key Overrides). */
uint16_t fw_qmk_resolve_key(uint16_t key_idx, uint8_t *out_source_layer);

/* Returns true if at least one Vial Key Override is enabled (so modifier changes can alter key faces). */
bool fw_qmk_has_enabled_key_overrides(void);

/* Flush pending RAM EEPROM edits to LittleFS once edits have settled for debounce_ms AND
 * keys_quiet is true (no keys held and no recent typing, so flash erase/prog never stalls typing).
 * Passing debounce_ms == 0 forces a flush regardless of debounce/quiet state (e.g. layout switch). */
bool fw_qmk_poll_eeprom_flush(uint32_t now_ms, uint32_t debounce_ms, bool keys_quiet);

/* Map linear key index (0..n_keys-1) for layout_id (1=ANSI, 2=ISO, 3=JIS) to (row, col). */
bool fw_qmk_key_to_rc(uint8_t layout_id, uint16_t key_idx, uint8_t *out_row, uint8_t *out_col);

#endif /* FW_QMK_H */
